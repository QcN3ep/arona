// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::Ordering;
use kernel::{
    hardware::{
        execution::UserEntry,
        mmu::{PAGE_SIZE, PagePermissions},
    },
    ipc::{Channel, Message},
    memory::{self, FrameAccess, FrameAllocation, MappingOptions, ReferenceKind},
    task::{Object, ObjectType, Rights, Task, TaskState},
};
use runtime::process::ProcessStart;
use spin::Once;

static IMAGE: Once<&'static [u8]> = Once::new();

pub fn run(image: &'static [u8]) {
    IMAGE.call_once(|| image);
    let manager = memory::manager();
    manager.reclaim_heap();
    let before = manager.stats();
    let heap = manager.heap_stats();
    super::ACTIVE.store(true, Ordering::Relaxed);
    crate::run_thread(scenario);
    super::ACTIVE.store(false, Ordering::Relaxed);
    manager.reclaim_heap();
    assert_eq!(manager.stats(), before);
    assert_eq!(manager.heap_stats(), heap);
}

fn scenario(_: usize) -> usize {
    for mode in [0, 1, 2, 0] {
        let task = Task::new().unwrap();
        let image = IMAGE.get().unwrap();
        assert_eq!(&image[..7], b"\x7fELF\x02\x01\x01");
        let headers = word(image, 32) as usize;
        let header_size = u16::from_le_bytes(image[54..56].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(image[56..58].try_into().unwrap()) as usize;
        for index in 0..count {
            let header = &image[headers + index * header_size..];
            if u32::from_le_bytes(header[..4].try_into().unwrap()) != 1 {
                continue;
            }
            load_segment(&task, image, header);
        }
        let stack = task.allocate_frames(16).unwrap();
        let range = task
            .map(
                &stack,
                0..16,
                MappingOptions::new(PagePermissions::ReadWrite),
            )
            .unwrap();
        let (sender, receiver) = Channel::pair().unwrap();
        receiver.send(Message::new(mode, &[]).unwrap()).unwrap();
        let task_handle = task
            .insert(
                Object::Task(task.clone()),
                ObjectType::Task.supported_rights(),
            )
            .unwrap();
        let bootstrap = task
            .insert(Object::Channel(sender), Rights::READ_WRITE)
            .unwrap();
        write(
            &stack,
            0,
            &ProcessStart {
                task: task_handle,
                bootstrap: Some(bootstrap),
            }
            .encode(),
        );
        let user = task
            .create_thread(UserEntry {
                pc: word(image, 24) as usize,
                stack_pointer: range.end,
                thread_pointer: 0,
                argument: range.start,
            })
            .unwrap();
        user.start().unwrap();
        let status = usize::from(mode != 0);
        assert_eq!(user.wait().unwrap(), status, "runtime test scenario {mode}");
        assert_eq!(task.state(), TaskState::Dead(status));
        if mode == 0 {
            let report = receiver.try_receive().unwrap();
            assert_eq!(report.tag, 73);
            assert_eq!(report.words(), &[37, 41]);
        } else {
            assert_eq!(
                receiver.try_receive(),
                Err(kernel::ipc::ChannelError::EndOfStream)
            );
        }
    }
    0
}

fn load_segment(task: &Task, image: &[u8], header: &[u8]) {
    let flags = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let offset = word(header, 8) as usize;
    let address = word(header, 16) as usize;
    let file_size = word(header, 32) as usize;
    let memory_size = word(header, 40) as usize;
    if memory_size == 0 {
        return;
    }
    assert!(file_size <= memory_size);
    let start = address & !(PAGE_SIZE - 1);
    let count = (address - start + memory_size).div_ceil(PAGE_SIZE) as u64;
    let allocation = task.allocate_frames(count).unwrap();
    write(
        &allocation,
        address - start,
        &image[offset..offset + file_size],
    );
    let permissions = match flags {
        4 => PagePermissions::ReadOnly,
        5 => PagePermissions::ReadExecute,
        6 => PagePermissions::ReadWrite,
        _ => panic!("unsupported test image permissions"),
    };
    task.map_at(
        start,
        &allocation,
        0..count,
        MappingOptions::new(permissions),
    )
    .unwrap();
}

fn write(allocation: &FrameAllocation, offset: usize, bytes: &[u8]) {
    let mut done = 0;
    for (index, frame) in allocation.frames().enumerate() {
        if index < offset / PAGE_SIZE {
            continue;
        }
        let inside = if done == 0 { offset % PAGE_SIZE } else { 0 };
        let size = (PAGE_SIZE - inside).min(bytes.len() - done);
        let mut page = [0; PAGE_SIZE / 8];
        for (position, byte) in bytes[done..done + size].iter().enumerate() {
            let position = inside + position;
            page[position / 8] |= (*byte as u64) << ((position % 8) * 8);
        }
        allocation
            .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
            .unwrap()
            .write(0, &page)
            .unwrap();
        done += size;
        if done == bytes.len() {
            break;
        }
    }
    assert_eq!(done, bytes.len());
}

fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
