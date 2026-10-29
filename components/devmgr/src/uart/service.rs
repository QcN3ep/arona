// SPDX-License-Identifier: MPL-2.0

//! UART 会话、收发缓冲与中断驱动的请求处理。

use alloc::collections::VecDeque;
use runtime::{
    Channel, Event, Handle, KernelError, Reply,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
    thread,
};

use super::{
    DataBits, Parity, Settings, StopBits, Uart,
    session::{
        self, BUSY, CLOCK_UNAVAILABLE, INVALID_CONFIGURATION, INVALID_REQUEST, MAX_BYTES, SUCCESS,
        UNSUPPORTED,
    },
};
use crate::Error;

const BUFFER_SIZE: usize = 4096;
const TRANSMIT_POLL_NS: u64 = 1_000_000;

struct Buffer {
    bytes: [u8; BUFFER_SIZE],
    start: usize,
    length: usize,
}

struct Connection {
    channel: Handle<Channel>,
    reads: VecDeque<(Handle<Reply>, usize)>,
    writes: VecDeque<PendingWrite>,
}

enum PendingWrite {
    Write {
        reply: Handle<Reply>,
        bytes: [u8; MAX_BYTES],
        length: usize,
    },
    Flush(Handle<Reply>),
}

struct State {
    receive: Buffer,
    transmit: Buffer,
    errors: u64,
    connection: Option<Connection>,
    interrupt_pending: bool,
}

pub fn run(
    uart: &mut Uart<'_>,
    event: &Handle<Event>,
    control: Option<&Handle<Channel>>,
    device: u32,
) -> Result<(), Error> {
    let mut state = State {
        receive: Buffer::new(),
        transmit: Buffer::new(),
        errors: 0,
        connection: None,
        interrupt_pending: false,
    };
    loop {
        let mut again = state.pump(uart)?;
        state.receive_request(uart)?;
        if let Some(control) = control {
            match control.receive(WaitMode::Poll) {
                Ok(request) => {
                    state.open(request, device)?;
                    again = true;
                }
                Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
                Err(runtime::Error::Kernel(KernelError::EndOfStream)) => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
        again |= state.complete(uart)?;
        state.transmit(uart)?;
        if again {
            continue;
        }
        let mode = if state.connection.as_ref().is_some_and(|connection| {
            matches!(connection.writes.front(), Some(PendingWrite::Flush(_)))
        }) && state.transmit.length == 0
        {
            WaitMode::Until(
                thread::now_ns()?
                    .checked_add(TRANSMIT_POLL_NS)
                    .ok_or(runtime::Error::Protocol)?,
            )
        } else {
            WaitMode::Forever
        };
        let mut items = [
            WaitItem::new(event, 1),
            WaitItem::new(event, 1),
            WaitItem::new(event, 1),
        ];
        let mut count = 1;
        if let Some(control) = control {
            items[count] = WaitItem::new(control, ipc::READABLE | ipc::PEER_CLOSED);
            count += 1;
        }
        if let Some(connection) = &state.connection {
            items[count] = WaitItem::new(&connection.channel, ipc::READABLE | ipc::PEER_CLOSED);
            count += 1;
        }
        match ipc::wait_many(&items[..count], mode) {
            Ok(ready) => state.interrupt_pending |= ready.index == 0,
            Err(runtime::Error::Kernel(KernelError::TimedOut)) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

impl State {
    fn pump(&mut self, uart: &mut Uart<'_>) -> Result<bool, Error> {
        let mut bytes = [0; MAX_BYTES];
        let read = uart.read(&mut bytes)?;
        self.errors |= session::error_bits(read.errors);
        for byte in &bytes[..read.bytes] {
            if self.receive.length == BUFFER_SIZE {
                self.errors |= 1;
            } else {
                self.receive.push(*byte);
            }
        }
        self.transmit(uart)?;
        if !self.interrupt_pending {
            return Ok(false);
        }
        match uart.acknowledge_interrupt() {
            Ok(()) => {
                self.interrupt_pending = false;
                Ok(false)
            }
            Err(Error::Busy) => Ok(true),
            Err(error) => Err(error),
        }
    }

    fn transmit(&mut self, uart: &mut Uart<'_>) -> Result<(), Error> {
        while self.transmit.length != 0 {
            let written = uart.write(self.transmit.contiguous())?;
            self.transmit.consume(written);
            if written == 0 {
                break;
            }
        }
        Ok(())
    }

    fn open(&mut self, mut request: ReceivedMessage, device: u32) -> Result<(), Error> {
        let Some(reply) = request.take_reply() else {
            return Ok(());
        };
        if request.tag != session::OPEN
            || request.words().len() != 1
            || request.handles().len() != 1
        {
            return reject(&reply, INVALID_REQUEST);
        }
        if request.words() != [u64::from(device)] {
            return reject(&reply, UNSUPPORTED);
        }
        if self.connection.is_some() {
            return reject(&reply, BUSY);
        }
        let Ok(channel) = request.take_handle(0).unwrap().downcast::<Channel>() else {
            return reject(&reply, INVALID_REQUEST);
        };
        if !channel.info()?.rights.contains(session::CHANNEL_RIGHTS) {
            return reject(&reply, INVALID_REQUEST);
        }
        if deliver(&reply, &mut Message::new(SUCCESS, &[])?)? {
            self.connection = Some(Connection {
                channel,
                reads: VecDeque::new(),
                writes: VecDeque::new(),
            });
        }
        Ok(())
    }

    fn receive_request(&mut self, uart: &mut Uart<'_>) -> Result<(), Error> {
        let Some(connection) = &mut self.connection else {
            return Ok(());
        };
        let mut request = match connection.channel.receive(WaitMode::Poll) {
            Ok(request) => request,
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => return Ok(()),
            Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                self.connection = None;
                self.receive = Buffer::new();
                self.errors = 0;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        let Some(reply) = request.take_reply() else {
            return Ok(());
        };
        if !request.handles().is_empty() {
            return reject(&reply, INVALID_REQUEST);
        }
        match (request.tag, request.words()) {
            (session::READ | session::TRY_READ, [length]) if *length <= MAX_BYTES as u64 => {
                if *length == 0 || request.tag == session::TRY_READ && !connection.reads.is_empty()
                {
                    deliver(&reply, &mut Message::new(SUCCESS, &[0, 0])?)?;
                } else if request.tag == session::TRY_READ {
                    self.read(&reply, *length as usize)?;
                } else {
                    connection.reads.push_back((reply, *length as usize));
                }
            }
            (session::WRITE, [length, data @ ..])
                if *length <= MAX_BYTES as u64 && data.len() == (*length as usize).div_ceil(8) =>
            {
                let mut bytes = [0; MAX_BYTES];
                for (index, byte) in bytes[..*length as usize].iter_mut().enumerate() {
                    *byte = (data[index / 8] >> ((index % 8) * 8)) as u8;
                }
                connection.writes.push_back(PendingWrite::Write {
                    reply,
                    bytes,
                    length: *length as usize,
                });
            }
            (session::FLUSH, []) => connection.writes.push_back(PendingWrite::Flush(reply)),
            (session::CONFIGURE, [baud, bits, parity, stop]) => {
                self.configure(uart, &reply, [*baud, *bits, *parity, *stop])?;
            }
            _ => reject(&reply, INVALID_REQUEST)?,
        }
        Ok(())
    }

    fn configure(
        &mut self,
        uart: &mut Uart<'_>,
        reply: &Handle<Reply>,
        words: [u64; 4],
    ) -> Result<(), Error> {
        let settings = match words {
            [baud, bits @ 5..=8, parity @ 0..=2, stop @ 0..=2] if baud <= u32::MAX as u64 => {
                Settings {
                    baud_rate: (baud != 0).then_some(baud as u32),
                    data_bits: match bits {
                        5 => DataBits::Five,
                        6 => DataBits::Six,
                        7 => DataBits::Seven,
                        _ => DataBits::Eight,
                    },
                    parity: match parity {
                        0 => Parity::None,
                        1 => Parity::Odd,
                        _ => Parity::Even,
                    },
                    stop_bits: match stop {
                        0 => StopBits::One,
                        1 => StopBits::OneAndHalf,
                        _ => StopBits::Two,
                    },
                }
            }
            _ => return reject(reply, INVALID_CONFIGURATION),
        };
        let result =
            if self.transmit.length == 0 && self.connection.as_ref().unwrap().writes.is_empty() {
                uart.configure(settings)
            } else {
                Err(Error::Busy)
            };
        match result {
            Ok(()) => {
                deliver(reply, &mut Message::new(SUCCESS, &[])?)?;
                Ok(())
            }
            Err(Error::Busy) => reject(reply, BUSY),
            Err(Error::Unsupported) => reject(reply, UNSUPPORTED),
            Err(Error::InvalidConfiguration) => reject(reply, INVALID_CONFIGURATION),
            Err(Error::ClockUnavailable) => reject(reply, CLOCK_UNAVAILABLE),
            Err(error) => Err(error),
        }
    }

    fn complete(&mut self, uart: &mut Uart<'_>) -> Result<bool, Error> {
        let Some(connection) = &mut self.connection else {
            return Ok(false);
        };
        let read = if self.receive.length != 0 || self.errors != 0 {
            connection.reads.pop_front()
        } else {
            None
        };
        let write = match connection.writes.front() {
            Some(PendingWrite::Write { length, .. })
                if BUFFER_SIZE - self.transmit.length >= *length =>
            {
                connection.writes.pop_front()
            }
            Some(PendingWrite::Flush(_))
                if self.transmit.length == 0 && uart.is_transmit_empty()? =>
            {
                connection.writes.pop_front()
            }
            _ => None,
        };
        let progressed = read.is_some() || write.is_some();
        if let Some((reply, length)) = read {
            self.read(&reply, length)?;
        }
        match write {
            Some(PendingWrite::Write {
                reply,
                bytes,
                length,
            }) => {
                if deliver(&reply, &mut Message::new(SUCCESS, &[length as u64])?)? {
                    for byte in &bytes[..length] {
                        self.transmit.push(*byte);
                    }
                }
            }
            Some(PendingWrite::Flush(reply)) => {
                deliver(&reply, &mut Message::new(SUCCESS, &[])?)?;
            }
            None => {}
        }
        Ok(progressed)
    }

    fn read(&mut self, reply: &Handle<Reply>, length: usize) -> Result<(), Error> {
        let count = length.min(self.receive.length);
        let mut bytes = [0; MAX_BYTES];
        for (index, byte) in bytes[..count].iter_mut().enumerate() {
            *byte = self.receive.get(index);
        }
        let encoded = session::encode(&bytes[..count]);
        let mut words = [0; runtime::ipc::MESSAGE_WORDS];
        words[0] = count as u64;
        words[1] = self.errors;
        words[2..2 + count.div_ceil(8)].copy_from_slice(&encoded[1..1 + count.div_ceil(8)]);
        if deliver(
            reply,
            &mut Message::new(SUCCESS, &words[..2 + count.div_ceil(8)])?,
        )? {
            self.receive.consume(count);
            self.errors = 0;
        }
        Ok(())
    }
}

impl Buffer {
    const fn new() -> Self {
        Self {
            bytes: [0; BUFFER_SIZE],
            start: 0,
            length: 0,
        }
    }
    fn get(&self, index: usize) -> u8 {
        self.bytes[(self.start + index) % BUFFER_SIZE]
    }
    fn push(&mut self, byte: u8) {
        self.bytes[(self.start + self.length) % BUFFER_SIZE] = byte;
        self.length += 1;
    }
    fn consume(&mut self, length: usize) {
        self.start = (self.start + length) % BUFFER_SIZE;
        self.length -= length;
    }
    fn contiguous(&self) -> &[u8] {
        &self.bytes[self.start..self.start + self.length.min(BUFFER_SIZE - self.start)]
    }
}

fn reject(reply: &Handle<Reply>, status: u64) -> Result<(), Error> {
    deliver(reply, &mut Message::new(status, &[])?)?;
    Ok(())
}

fn deliver(reply: &Handle<Reply>, message: &mut Message) -> Result<bool, Error> {
    match reply.send(message) {
        Ok(()) => Ok(true),
        Err(runtime::Error::Kernel(KernelError::InvalidState)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
