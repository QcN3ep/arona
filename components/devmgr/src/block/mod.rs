// SPDX-License-Identifier: MPL-2.0

//! VirtIO 块设备的有界请求池、完成确认与停止流程。

pub mod service;
mod session;

pub use session::Session;

use alloc::vec::Vec;
use runtime::{
    Device, DmaDomain, Event, FrameAllocation, Handle, Task,
    device::InterruptBinding,
    dma::{DmaBuffer, DmaDirection},
    thread,
};

use crate::{
    Error,
    device::DeviceDescription,
    virtio::{
        self, Transport,
        queue::{Descriptor, Queue},
    },
};

// REFERENCE: https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html
pub const MAX_REQUESTS: usize = 16;
pub const MAX_TRANSFER: usize = 4096;
const READ_ONLY: u64 = 1 << 5;
const BLOCK_SIZE: u64 = 1 << 6;
const FLUSH: u64 = 1 << 9;
const SUPPORTED_FEATURES: u64 = READ_ONLY | BLOCK_SIZE | FLUSH;
const STOP_TIMEOUT_NS: u64 = 2_000_000_000;
const HEADER_STRIDE: usize = 32;
const STATUS_OFFSET: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct Configuration {
    pub(crate) id: u32,
    transport: virtio::Configuration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Info {
    pub capacity: u64,
    pub block_size: u32,
    pub read_only: bool,
    pub flush: bool,
    pub max_transfer: usize,
    pub max_requests: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Status {
    Success = 0,
    Io = 1,
    Unsupported = 2,
    Cancelled = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Completion {
    pub status: Status,
    pub bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelResult {
    Cancelled,
    Submitted,
    Completed,
}

pub struct Block<'a> {
    transport: Transport<'a>,
    interrupt: InterruptBinding<'a>,
    queue: Queue<'a>,
    headers: DmaBuffer<'a>,
    buffers: Vec<DmaBuffer<'a>>,
    requests: Vec<Option<Pending>>,
    domain: &'a Handle<DmaDomain>,
    info: Info,
    state: DriverState,
    next_order: u64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DriverState {
    Running,
    Stopping,
    Stopped,
    Failed,
}

struct Pending {
    id: u64,
    order: u64,
    kind: Kind,
    length: usize,
    state: State,
    cancelled: bool,
}

enum Kind {
    Read {
        output: Handle<FrameAllocation>,
        offset: usize,
    },
    Write,
    Flush,
}

enum State {
    Queued,
    Submitted,
    Completed(Completion),
}

pub fn probe(device: &DeviceDescription<'_>) -> Result<Option<Configuration>, Error> {
    Ok(virtio::probe(device)?.map(|transport| Configuration {
        id: device.info().id,
        transport,
    }))
}

impl<'a> Block<'a> {
    pub fn new(
        configuration: Configuration,
        grant: &'a Handle<Device>,
        domain: &'a Handle<DmaDomain>,
        event: &'a Handle<Event>,
        task: &Handle<Task>,
    ) -> Result<Self, Error> {
        let mut transport = Transport::open(configuration.transport, grant)?;
        let features = transport.negotiate(SUPPORTED_FEATURES)?;
        let queue = Queue::new(domain, task, &mut transport)?;
        let count = MAX_REQUESTS.min(queue.size() / 3);
        let info = configuration_info(&mut transport, features, count)?;
        let headers = domain.allocate(task, 1, DmaDirection::Bidirectional)?;
        let mut buffers = Vec::with_capacity(count);
        for _ in 0..count {
            buffers.push(domain.allocate(task, 1, DmaDirection::Bidirectional)?);
        }
        let interrupt = grant.bind_interrupt(configuration.id, 0, event, 1)?;
        let mut block = Self {
            transport,
            interrupt,
            queue,
            headers,
            buffers,
            requests: (0..count).map(|_| None).collect(),
            domain,
            info,
            state: DriverState::Stopped,
            next_order: 0,
        };
        domain.resume()?;
        block.state = DriverState::Running;
        block.transport.start()?;
        Ok(block)
    }

    pub fn info(&self) -> Info {
        self.info
    }

    pub(crate) fn idle(&self) -> bool {
        self.requests.iter().all(Option::is_none)
    }

    pub fn read(
        &mut self,
        id: u64,
        offset: u64,
        output: Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
    ) -> Result<(), Error> {
        self.validate(offset, &output, buffer_offset, length, false)?;
        self.insert(
            id,
            offset,
            length,
            Kind::Read {
                output,
                offset: buffer_offset,
            },
        )
    }

    pub fn write(
        &mut self,
        id: u64,
        offset: u64,
        input: &Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
    ) -> Result<(), Error> {
        self.validate(offset, input, buffer_offset, length, true)?;
        let index = self.free(id)?;
        let mut bytes = [0; MAX_TRANSFER];
        input.read(buffer_offset, &mut bytes[..length])?;
        self.buffers[index].write(0, &bytes[..length])?;
        self.insert(id, offset, length, Kind::Write)
    }

    pub fn flush(&mut self, id: u64) -> Result<(), Error> {
        if !self.info.flush {
            return Err(Error::Unsupported);
        }
        self.insert(id, 0, 0, Kind::Flush)
    }

    pub fn submit(&mut self) -> Result<(), Error> {
        if self.state != DriverState::Running {
            return Err(Error::Unavailable);
        }
        let result = self.publish();
        self.check_result(result)
    }

    fn publish(&mut self) -> Result<(), Error> {
        if self.requests.iter().flatten().any(|request| {
            matches!(request.kind, Kind::Flush) && matches!(request.state, State::Submitted)
        }) {
            return Ok(());
        }
        let mut notify = false;
        while let Some(index) = self
            .requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| {
                request
                    .as_ref()
                    .filter(|request| matches!(request.state, State::Queued))
                    .map(|request| (index, request.order))
            })
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
        {
            let request = self.requests[index].as_mut().unwrap();
            // FLUSH 须等先前提交的请求完成。
            if matches!(request.kind, Kind::Flush) && self.queue.in_flight() != 0 {
                break;
            }
            let header = self.headers.bus_address().unwrap() + (index * HEADER_STRIDE) as u64;
            let entries = [
                Descriptor {
                    address: header,
                    length: 16,
                    writable: false,
                },
                Descriptor {
                    address: self.buffers[index].bus_address().unwrap(),
                    length: request.length as u32,
                    writable: matches!(request.kind, Kind::Read { .. }),
                },
                Descriptor {
                    address: header + STATUS_OFFSET as u64,
                    length: 1,
                    writable: true,
                },
            ];
            if matches!(request.kind, Kind::Flush) {
                self.queue
                    .publish((index * 3) as u16, &[entries[0], entries[2]])?;
            } else {
                self.queue.publish((index * 3) as u16, &entries)?;
            }
            request.state = State::Submitted;
            notify = true;
            if matches!(request.kind, Kind::Flush) {
                break;
            }
        }
        if notify {
            self.transport.notify()?;
        }
        Ok(())
    }

    pub fn poll(&mut self) -> Result<(), Error> {
        if !matches!(self.state, DriverState::Running | DriverState::Stopping) {
            return Err(Error::Unavailable);
        }
        let result = self.consume();
        self.check_result(result)
    }

    fn consume(&mut self) -> Result<(), Error> {
        self.transport.check()?;
        while let Some((head, length)) = self.queue.complete()? {
            let index = usize::from(head) / 3;
            if !head.is_multiple_of(3) {
                return Err(Error::DeviceFault);
            }
            let request = self
                .requests
                .get_mut(index)
                .and_then(Option::as_mut)
                .ok_or(Error::DeviceFault)?;
            if !matches!(request.state, State::Submitted) {
                return Err(Error::DeviceFault);
            }
            let mut status = [0];
            self.headers
                .read((index * HEADER_STRIDE + STATUS_OFFSET) as u64, &mut status)?;
            let mut completion = Completion {
                status: match status[0] {
                    0 => Status::Success,
                    1 => Status::Io,
                    2 => Status::Unsupported,
                    _ => return Err(Error::DeviceFault),
                },
                bytes: 0,
            };
            if request.cancelled {
                completion.status = Status::Cancelled;
            } else if completion.status == Status::Success {
                if let Kind::Read { output, offset } = &mut request.kind {
                    let bytes = (length - 1) as usize;
                    if !bytes.is_multiple_of(self.info.block_size as usize) {
                        return Err(Error::DeviceFault);
                    }
                    let mut data = [0; MAX_TRANSFER];
                    self.buffers[index].read(0, &mut data[..bytes])?;
                    if output.write(*offset, &data[..bytes]).is_err() {
                        completion.status = Status::Io;
                    } else {
                        completion.bytes = bytes;
                    }
                    if bytes != request.length && completion.status == Status::Success {
                        completion.status = Status::Io;
                    }
                } else {
                    completion.bytes = request.length;
                }
            }
            request.state = State::Completed(completion);
        }
        Ok(())
    }

    pub fn cancel(&mut self, id: u64) -> Result<CancelResult, Error> {
        let request = self.request_mut(id)?;
        Ok(match request.state {
            State::Queued => {
                request.state = State::Completed(Completion {
                    status: Status::Cancelled,
                    bytes: 0,
                });
                CancelResult::Cancelled
            }
            State::Submitted => {
                request.cancelled = true;
                CancelResult::Submitted
            }
            State::Completed(_) => CancelResult::Completed,
        })
    }

    pub fn completion(&self, id: u64) -> Result<Option<Completion>, Error> {
        let request = self
            .requests
            .iter()
            .flatten()
            .find(|request| request.id == id)
            .ok_or(Error::NotFound)?;
        Ok(match request.state {
            State::Completed(value) => Some(value),
            _ => None,
        })
    }

    pub fn acknowledge(&mut self, id: u64) -> Result<Completion, Error> {
        let index = self
            .requests
            .iter()
            .position(|request| request.as_ref().is_some_and(|request| request.id == id))
            .ok_or(Error::NotFound)?;
        let State::Completed(completion) = self.requests[index].as_ref().unwrap().state else {
            return Err(Error::Busy);
        };
        self.requests[index] = None;
        Ok(completion)
    }

    pub fn acknowledge_interrupt(&mut self) -> Result<(), Error> {
        if self.transport.interrupt_status()? & 2 != 0 {
            // 容量变化后由监督者重新建立实例。
            return Err(Error::DeviceFault);
        }
        self.interrupt.acknowledge()?;
        Ok(())
    }

    pub fn discard(&mut self) {
        for request in self.requests.iter_mut().flatten() {
            request.cancelled = true;
            if matches!(request.state, State::Queued) {
                request.state = State::Completed(Completion {
                    status: Status::Cancelled,
                    bytes: 0,
                });
            }
        }
        for request in &mut self.requests {
            if request
                .as_ref()
                .is_some_and(|request| matches!(request.state, State::Completed(_)))
            {
                *request = None;
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        match self.state {
            DriverState::Stopped => return Ok(()),
            DriverState::Failed => return Err(Error::DeviceFault),
            _ => self.state = DriverState::Stopping,
        }
        self.discard();
        let deadline = thread::now_ns()?.saturating_add(STOP_TIMEOUT_NS);
        while self.queue.in_flight() != 0 {
            self.poll()?;
            if thread::now_ns()? >= deadline {
                return self.check_result(Err(Error::DeviceFault));
            }
            thread::sleep_until(thread::now_ns()?.saturating_add(100_000))?;
        }
        let reset = self.transport.reset();
        self.check_result(reset)?;
        // SAFETY: 队列须已停止，且所有在途 DMA 均已完成。
        self.domain.quiesce()?;
        self.state = DriverState::Stopped;
        self.interrupt.unbind()?;
        Ok(())
    }

    fn check_result(&mut self, result: Result<(), Error>) -> Result<(), Error> {
        if result.is_err() {
            self.state = DriverState::Failed;
            // 无法确认排空时，停止设备并保留活动域中的 DMA 页框。
            let _ = self.transport.reset();
        }
        result
    }

    fn validate(
        &self,
        offset: u64,
        frames: &Handle<FrameAllocation>,
        buffer_offset: usize,
        length: usize,
        write: bool,
    ) -> Result<(), Error> {
        let rights = if write {
            runtime::Rights::READ
        } else {
            runtime::Rights::WRITE
        };
        if !frames.info()?.rights.contains(rights) {
            return Err(Error::InvalidRequest);
        }
        let buffer_length = frames
            .frames()?
            .checked_mul(4096)
            .ok_or(Error::InvalidRequest)?;
        if length == 0
            || length > MAX_TRANSFER
            || !offset.is_multiple_of(self.info.block_size.into())
            || !length.is_multiple_of(self.info.block_size as usize)
            || offset
                .checked_add(length as u64)
                .is_none_or(|end| end > self.info.capacity)
            || buffer_offset
                .checked_add(length)
                .is_none_or(|end| end as u64 > buffer_length)
        {
            return Err(Error::InvalidRequest);
        }
        if write && self.info.read_only {
            return Err(Error::ReadOnly);
        }
        Ok(())
    }

    fn free(&self, id: u64) -> Result<usize, Error> {
        if self.state != DriverState::Running {
            return Err(Error::Unavailable);
        }
        if self
            .requests
            .iter()
            .flatten()
            .any(|request| request.id == id)
        {
            return Err(Error::InvalidRequest);
        }
        self.requests
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Busy)
    }

    fn insert(&mut self, id: u64, offset: u64, length: usize, kind: Kind) -> Result<(), Error> {
        let index = self.free(id)?;
        let next_order = self.next_order.checked_add(1).ok_or(Error::Unavailable)?;
        let operation = match kind {
            Kind::Read { .. } => 0u32,
            Kind::Write => 1,
            Kind::Flush => 4,
        };
        let mut header = [0; HEADER_STRIDE];
        header[..4].copy_from_slice(&operation.to_le_bytes());
        header[8..16].copy_from_slice(&(offset / 512).to_le_bytes());
        header[STATUS_OFFSET] = 0xff;
        self.headers
            .write((index * HEADER_STRIDE) as u64, &header)?;
        self.requests[index] = Some(Pending {
            id,
            order: self.next_order,
            kind,
            length,
            state: State::Queued,
            cancelled: false,
        });
        self.next_order = next_order;
        Ok(())
    }

    fn request_mut(&mut self, id: u64) -> Result<&mut Pending, Error> {
        self.requests
            .iter_mut()
            .flatten()
            .find(|request| request.id == id)
            .ok_or(Error::NotFound)
    }
}

impl Drop for Block<'_> {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn configuration_info(
    transport: &mut Transport<'_>,
    features: u64,
    count: usize,
) -> Result<Info, Error> {
    let deadline = thread::now_ns()?.saturating_add(1_000_000_000);
    loop {
        let generation = transport.generation()?;
        let sectors =
            u64::from(transport.config_word(0)?) | u64::from(transport.config_word(4)?) << 32;
        let block_size = if features & BLOCK_SIZE != 0 {
            transport.config_word(20)?
        } else {
            512
        };
        if generation == transport.generation()? {
            let capacity = sectors
                .checked_mul(512)
                .ok_or(Error::InvalidConfiguration)?;
            if !(512..=MAX_TRANSFER as u32).contains(&block_size)
                || !block_size.is_power_of_two()
                || capacity == 0
                || !capacity.is_multiple_of(block_size.into())
            {
                return Err(Error::InvalidConfiguration);
            }
            return Ok(Info {
                capacity,
                block_size,
                read_only: features & READ_ONLY != 0,
                flush: features & FLUSH != 0,
                max_transfer: MAX_TRANSFER,
                max_requests: count,
            });
        }
        if thread::now_ns()? >= deadline {
            return Err(Error::DeviceFault);
        }
        thread::yield_now()?;
    }
}
