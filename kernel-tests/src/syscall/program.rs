// SPDX-License-Identifier: MPL-2.0

use core::sync::atomic::{AtomicBool, Ordering};
use kernel::{
    device::{DeviceError, DeviceSet},
    hardware::{
        self,
        execution::UserEntry,
        mmu::{PAGE_SIZE, PagePermissions},
    },
    ipc::{Channel, Message},
    memory::{self, FrameAccess, FrameAllocation, MappingOptions, ReferenceKind},
    task::{Object, ObjectReference, ObjectType, Rights, Task, TaskState, TransferRequest},
    thread::{self, Thread, ThreadState},
    trap::{Trap, TrapEvent, TrapOutcome},
};
use runtime::process::ProcessStart;
use spin::{Mutex, Once};

static IMAGE: Once<&'static [u8]> = Once::new();
static DEVICES: Once<DeviceSet> = Once::new();
static MODES: Once<&'static [u64]> = Once::new();
static ARCHIVE: Once<&'static [u8]> = Once::new();
static SERVICE: Mutex<Option<Task>> = Mutex::new(None);
static SUPERVISING: AtomicBool = AtomicBool::new(false);

pub fn run_device(image: &'static [u8], devices: DeviceSet, resident: bool) {
    DEVICES.call_once(|| devices);
    run_cases(image, if resident { &[6] } else { &[5] });
}

pub fn run_init(image: &'static [u8], archive: &'static [u8], devices: DeviceSet) {
    DEVICES.call_once(|| devices);
    ARCHIVE.call_once(|| archive);
    run_cases(image, &[9, 10, 11, 12]);
}

pub fn handle(trap: Trap) -> TrapOutcome {
    let method = if ARCHIVE.get().is_some()
        && let TrapEvent::Syscall {
            handle, arguments, ..
        } = trap.event
        && thread::current()
            .and_then(|thread| thread.task())
            .as_ref()
            .is_some_and(|caller| {
                caller
                    .get(
                        abi::Handle::from_raw(handle as u64),
                        ObjectType::Task,
                        Rights::NONE,
                    )
                    .is_ok()
            }) {
        Some(arguments[0] as u64)
    } else {
        None
    };
    if method == Some(abi::method::task::WAIT) {
        SUPERVISING.store(true, Ordering::Relaxed);
    }
    let outcome = kernel::trap::dispatch(trap);
    if method == Some(abi::method::task::CREATE)
        && let TrapOutcome::Return { status: 0, values } = outcome
    {
        let ObjectReference::Task(task) = thread::current()
            .unwrap()
            .task()
            .unwrap()
            .get(
                abi::Handle::from_raw(values[0] as u64),
                ObjectType::Task,
                Rights::NONE,
            )
            .unwrap()
        else {
            panic!("created object is not a Task")
        };
        assert!(SERVICE.lock().replace(task).is_none());
    }
    outcome
}

pub fn run(image: &'static [u8]) {
    run_cases(image, &[0, 1, 2, 3, 4, 0]);
}

pub fn run_process(image: &'static [u8]) {
    run_cases(image, &[7]);
}

fn run_cases(image: &'static [u8], modes: &'static [u64]) {
    IMAGE.call_once(|| image);
    MODES.call_once(|| modes);
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
    for &mode in *MODES.get().unwrap() {
        let task = Task::new().unwrap();
        let held =
            (mode == 11).then(|| DEVICES.get().unwrap().claim_group(console_group()).unwrap());
        if mode >= 9 {
            SUPERVISING.store(false, Ordering::Relaxed);
        }
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
        bootstrap(&receiver, mode);
        let mut rights = ObjectType::Task.supported_rights();
        if mode == 4 {
            rights = Rights::from_bits(rights.bits() & !Rights::CREATE_THREAD.bits()).unwrap();
        }
        let task_handle = task.insert(Object::Task(task.clone()), rights).unwrap();
        let bootstrap = task
            .insert(
                Object::Channel(sender),
                if mode == 6 || mode >= 9 {
                    Rights::READ
                } else {
                    Rights::READ_WRITE
                },
            )
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
        if mode == 6 {
            drop(receiver);
            resident(&task, &user);
            continue;
        }
        if mode >= 9 {
            drop(receiver);
            init_lifecycle(&task, &user, mode);
            *SERVICE.lock() = None;
            drop(held);
            drop(DEVICES.get().unwrap().claim_group(console_group()).unwrap());
            continue;
        }
        let status = usize::from(matches!(mode, 1..=3));
        assert_eq!(user.wait().unwrap(), status, "runtime test scenario {mode}");
        assert_eq!(task.state(), TaskState::Dead(status));
        if status == 0 {
            let report = receiver.try_receive().unwrap();
            assert_eq!(report.tag, 73);
            assert_eq!(
                report.words(),
                if mode == 5 {
                    [2].as_slice()
                } else {
                    [37, 41].as_slice()
                }
            );
        } else {
            assert_eq!(
                receiver.try_receive(),
                Err(kernel::ipc::ChannelError::EndOfStream)
            );
        }
    }
    0
}

fn resident(task: &Task, user: &Thread) {
    let deadline = hardware::timer().now_ns().unwrap() + 1_000_000_000;
    while user.state() != ThreadState::Blocked {
        assert_eq!(task.state(), TaskState::Active);
        assert!(
            hardware::timer().now_ns().unwrap() < deadline,
            "service did not become idle"
        );
        thread::yield_now().unwrap();
    }
    let devices = DEVICES.get().unwrap();
    let group = console_group();
    assert!(matches!(devices.claim_group(group), Err(DeviceError::Busy)));
    assert_eq!(
        task.wait_until(hardware::timer().now_ns().unwrap() + 20_000_000)
            .unwrap(),
        None
    );
    assert_eq!(task.state(), TaskState::Active);
    assert_eq!(user.state(), ThreadState::Blocked);
    assert!(matches!(devices.claim_group(group), Err(DeviceError::Busy)));
    task.terminate(0);
    assert_eq!(user.wait().unwrap(), 0);
    assert_eq!(task.state(), TaskState::Dead(0));
    drop(devices.claim_group(group).unwrap());
}

fn init_lifecycle(task: &Task, user: &Thread, mode: u64) {
    if mode >= 11 {
        assert_eq!(user.wait().unwrap(), 1);
        assert_eq!(task.state(), TaskState::Dead(1));
        assert!(!SUPERVISING.load(Ordering::Relaxed));
        if mode == 11 {
            assert_eq!(SERVICE.lock().as_ref().unwrap().state(), TaskState::Dead(1));
        } else {
            assert!(SERVICE.lock().is_none());
        }
        return;
    }
    let deadline = hardware::timer().now_ns().unwrap() + 5_000_000_000;
    while !SUPERVISING.load(Ordering::Relaxed) || user.state() != ThreadState::Blocked {
        assert_eq!(task.state(), TaskState::Active);
        assert!(
            hardware::timer().now_ns().unwrap() < deadline,
            "init did not supervise devmgr: waiting={}, init={:?}, child={:?}",
            SUPERVISING.load(Ordering::Relaxed),
            user.state(),
            SERVICE.lock().as_ref().map(|task| task.state()),
        );
        thread::yield_now().unwrap();
    }
    let child = SERVICE.lock().as_ref().unwrap().clone();
    assert_eq!(child.state(), TaskState::Active);
    let devices = DEVICES.get().unwrap();
    assert!(matches!(
        devices.claim_group(console_group()),
        Err(DeviceError::Busy)
    ));
    assert_eq!(
        task.wait_until(hardware::timer().now_ns().unwrap() + 20_000_000)
            .unwrap(),
        None
    );
    assert_eq!(child.state(), TaskState::Active);
    assert_eq!(user.state(), ThreadState::Blocked);
    let status = if mode == 9 {
        child.terminate(73);
        73
    } else {
        task.terminate(0);
        0
    };
    assert_eq!(user.wait().unwrap(), status);
    assert_eq!(task.state(), TaskState::Dead(status));
    assert_eq!(child.wait().unwrap(), status);
    assert_eq!(child.state(), TaskState::Dead(status));
}

fn console_group() -> u32 {
    let devices = DEVICES.get().unwrap();
    let mut cursor = 0;
    loop {
        let id = devices.next(cursor).unwrap().unwrap();
        let info = devices.describe(id).unwrap();
        if info.console {
            return info.group.unwrap();
        }
        cursor = id;
    }
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

fn bootstrap(receiver: &Channel, mode: u64) {
    if let Some(devices) = DEVICES.get().filter(|_| matches!(mode, 5 | 6 | 9..=12)) {
        let source = Task::new().unwrap();
        let channel = source
            .insert(Object::Channel(receiver.clone()), Rights::WRITE)
            .unwrap();
        let handle = source
            .insert(
                Object::DeviceSet(devices.clone()),
                ObjectType::DeviceSet.supported_rights(),
            )
            .unwrap();
        let device = TransferRequest {
            handle,
            rights: ObjectType::DeviceSet.supported_rights(),
        };
        if mode == 6 || mode >= 9 {
            let bytes = ARCHIVE.get().copied().unwrap_or(&[]);
            let length = if mode == 12 {
                bytes.len() - 124
            } else {
                bytes.len().max(PAGE_SIZE)
            };
            let archive = source
                .allocate_frames(length.div_ceil(PAGE_SIZE) as u64)
                .unwrap();
            write(&archive, 0, &bytes[..bytes.len().min(length)]);
            let archive = memory::manager().allocate_arc(archive).unwrap();
            let rights = Rights::READ | Rights::MAP | Rights::TRANSFER;
            let handle = source
                .insert(Object::FrameAllocation(archive), rights)
                .unwrap();
            source
                .try_send(
                    channel,
                    Message::new(0, &[length as u64]).unwrap(),
                    &[TransferRequest { handle, rights }, device],
                )
                .unwrap();
        } else {
            source
                .try_send(channel, Message::new(mode, &[]).unwrap(), &[device])
                .unwrap();
        }
        source.terminate(0);
    } else if mode == 7 {
        let source = Task::new().unwrap();
        let channel = source
            .insert(Object::Channel(receiver.clone()), Rights::WRITE)
            .unwrap();
        let image = IMAGE.get().unwrap();
        let frames = source
            .allocate_frames(image.len().div_ceil(PAGE_SIZE) as u64)
            .unwrap();
        write(&frames, 0, image);
        let rights = Rights::READ | Rights::TRANSFER;
        let handle = source
            .insert(
                Object::FrameAllocation(memory::manager().allocate_arc(frames).unwrap()),
                rights,
            )
            .unwrap();
        source
            .try_send(
                channel,
                Message::new(mode, &[image.len() as u64]).unwrap(),
                &[TransferRequest { handle, rights }],
            )
            .unwrap();
        source.terminate(0);
    } else {
        receiver.send(Message::new(mode, &[]).unwrap()).unwrap();
    }
}
