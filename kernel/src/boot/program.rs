// SPDX-License-Identifier: MPL-2.0

//! 启动程序的段描述及首任务创建。

use abi::process::ProcessStart;
use alloc::vec;

use crate::{
    device::DeviceSet,
    hardware::{
        self,
        cpu::InterruptGuard,
        execution::UserEntry,
        mmu::{PAGE_SIZE, PagePermissions},
    },
    ipc::{Channel, Message, wait::WaitMode},
    memory::{self, FrameAllocation, MappingOptions, MemoryBuffer, PhysRange, VirtRange},
    task::{HandleEntry, Object, ObjectType, Rights, Task},
};

const STACK_FRAMES: u64 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootData {
    pub physical: PhysRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProgramSegment {
    pub memory: VirtRange,
    pub data: Option<BootData>,
    pub permissions: PagePermissions,
}

pub trait BootProgram: core::fmt::Debug {
    fn entry(&self) -> usize;
    fn segment(&self, index: usize) -> Result<Option<ProgramSegment>, &'static str>;
}

impl BootData {
    pub fn len(self) -> Result<usize, &'static str> {
        self.physical
            .end
            .checked_sub(self.physical.start)
            .and_then(|size| usize::try_from(size).ok())
            .ok_or("invalid boot data range")
    }

    pub fn is_empty(self) -> bool {
        self.physical.start == self.physical.end
    }

    pub fn read(self, offset: usize, bytes: &mut [u8]) -> Result<(), &'static str> {
        let length = self.len()?;
        if offset
            .checked_add(bytes.len())
            .is_none_or(|end| end > length)
        {
            return Err("boot data read is out of bounds");
        }
        let _interrupts = InterruptGuard::new();
        let mut done = 0;
        let mut words = [0; PAGE_SIZE / 8];
        while done < bytes.len() {
            let address = self.physical.start + (offset + done) as u64;
            let inside = address as usize % PAGE_SIZE;
            let count = (PAGE_SIZE - inside).min(bytes.len() - done);
            let word_count = (inside % 8 + count).div_ceil(8);
            hardware::mmu()
                .read_frame(
                    address & !(PAGE_SIZE as u64 - 1),
                    inside / 8,
                    &mut words[..word_count],
                )
                .map_err(|_| "failed to read boot data")?;
            for (index, byte) in bytes[done..done + count].iter_mut().enumerate() {
                let position = inside % 8 + index;
                *byte = words[position / 8].to_le_bytes()[position % 8];
            }
            done += count;
        }
        Ok(())
    }
}

pub(super) fn start(
    program: &dyn BootProgram,
    archive: BootData,
    devices: DeviceSet,
) -> Result<(), &'static str> {
    let _interrupts = InterruptGuard::new();
    let task = Task::new_root().map_err(|_| "failed to create initial task")?;
    if let Err(error) = prepare(&task, program, archive, devices) {
        task.terminate(1);
        return Err(error);
    }
    Ok(())
}

fn prepare(
    task: &Task,
    program: &dyn BootProgram,
    archive: BootData,
    devices: DeviceSet,
) -> Result<(), &'static str> {
    let mut index = 0;
    while let Some(segment) = program.segment(index)? {
        let start = segment.memory.start & !(PAGE_SIZE - 1);
        let length = segment
            .memory
            .end
            .checked_sub(start)
            .ok_or("invalid program segment")?;
        let frames = length.div_ceil(PAGE_SIZE) as u64;
        let allocation = task
            .allocate_frames(frames)
            .map_err(|_| "failed to allocate program memory")?;
        if let Some(data) = segment.data {
            copy(data, &allocation, segment.memory.start - start)?;
        }
        task.map_at(
            start,
            &allocation,
            0..frames,
            MappingOptions::new(segment.permissions),
        )
        .map_err(|_| "failed to map program segment")?;
        index += 1;
    }
    let bootstrap = bootstrap(task, archive, devices)?;
    let task_handle = task
        .insert(
            Object::Task(task.clone()),
            ObjectType::Task.supported_rights(),
        )
        .map_err(|_| "failed to install initial task handle")?;
    let stack = task
        .allocate_frames(STACK_FRAMES)
        .map_err(|_| "failed to allocate initial stack")?;
    let reservation = task
        .reserve((STACK_FRAMES as usize + 1) * PAGE_SIZE, PAGE_SIZE, None)
        .map_err(|_| "failed to reserve initial stack")?;
    task.unreserve(reservation)
        .map_err(|_| "failed to prepare initial stack")?;
    task.reserve(PAGE_SIZE, PAGE_SIZE, Some(reservation.start))
        .map_err(|_| "failed to reserve stack guard")?;
    let range = task
        .map_at(
            reservation.start + PAGE_SIZE,
            &stack,
            0..STACK_FRAMES,
            MappingOptions::new(PagePermissions::ReadWrite),
        )
        .map_err(|_| "failed to map initial stack")?;
    let argument = range.end - ProcessStart::SIZE;
    MemoryBuffer::allocation(&stack, argument - range.start, ProcessStart::SIZE, true)
        .and_then(|buffer| {
            buffer.write(
                0,
                &ProcessStart {
                    task: task_handle,
                    bootstrap: Some(bootstrap),
                }
                .encode(),
            )
        })
        .map_err(|_| "failed to write process start record")?;
    let thread = task
        .create_thread(UserEntry {
            pc: program.entry(),
            stack_pointer: argument & !15,
            thread_pointer: 0,
            argument,
        })
        .map_err(|_| "failed to create initial thread")?;
    thread
        .start()
        .map_err(|_| "failed to start initial thread")?;
    log::info!("Started init");
    Ok(())
}

fn bootstrap(
    task: &Task,
    archive: BootData,
    devices: DeviceSet,
) -> Result<abi::Handle, &'static str> {
    // TODO: 释放启动数据引用后回收外部归档的保留页框。
    let length = archive.len()?;
    let frames = task
        .allocate_frames(length.div_ceil(PAGE_SIZE) as u64)
        .map_err(|_| "failed to allocate initial archive")?;
    copy(archive, &frames, 0)?;
    let frames = memory::manager()
        .allocate_arc(frames)
        .map_err(|_| "failed to retain initial archive")?;
    let (sender, receiver) = Channel::pair().map_err(|_| "failed to create bootstrap channel")?;
    sender
        .send_with::<crate::ipc::ChannelError>(
            Message::new(0, &[length as u64]).map_err(|_| "invalid bootstrap message")?,
            WaitMode::NonBlocking,
            false,
            || {
                Ok(vec![
                    HandleEntry {
                        object: Object::FrameAllocation(frames.clone()),
                        rights: Rights::READ | Rights::MAP | Rights::TRANSFER,
                    },
                    HandleEntry {
                        object: Object::DeviceSet(devices.clone()),
                        rights: ObjectType::DeviceSet.supported_rights(),
                    },
                ])
            },
        )
        .map_err(|_| "failed to send bootstrap archive")?;
    task.insert(Object::Channel(receiver), Rights::READ)
        .map_err(|_| "failed to install bootstrap channel")
}

fn copy(data: BootData, allocation: &FrameAllocation, offset: usize) -> Result<(), &'static str> {
    let length = data.len()?;
    let output = MemoryBuffer::allocation(allocation, offset, length, true)
        .map_err(|_| "failed to access program frames")?;
    let mut buffer = [0; PAGE_SIZE];
    for offset in (0..length).step_by(buffer.len()) {
        let count = buffer.len().min(length - offset);
        data.read(offset, &mut buffer[..count])?;
        output
            .write(offset, &buffer[..count])
            .map_err(|_| "failed to write program frames")?;
    }
    Ok(())
}
