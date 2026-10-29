// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
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
static TRACK_BINDINGS: AtomicBool = AtomicBool::new(false);
static BINDING: Mutex<Option<(u32, abi::Handle)>> = Mutex::new(None);

pub fn run_device(image: &'static [u8], devices: DeviceSet, resident: bool) {
    DEVICES.call_once(|| devices);
    run_cases(image, if resident { &[6, 8] } else { &[5] });
}

pub fn run_init(image: &'static [u8], archive: &'static [u8], devices: DeviceSet) {
    DEVICES.call_once(|| devices);
    ARCHIVE.call_once(|| archive);
    run_cases(image, &[9, 10, 11, 12]);
}

pub fn handle(trap: Trap) -> TrapOutcome {
    let binding = if TRACK_BINDINGS.load(Ordering::Relaxed)
        && let TrapEvent::Syscall {
            handle, arguments, ..
        } = trap.event
        && arguments[0] == abi::method::device::BIND as usize
        && thread::current()
            .and_then(|thread| thread.task())
            .is_some_and(|task| {
                task.get(
                    abi::Handle::from_raw(handle as u64),
                    ObjectType::Device,
                    Rights::IRQ_ACK,
                )
                .is_ok()
            }) {
        Some((
            arguments[1] as u32,
            abi::Handle::from_raw(arguments[3] as u64),
        ))
    } else {
        None
    };
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
    if let Some(binding) = binding
        && matches!(outcome, TrapOutcome::Return { status: 0, .. })
    {
        BINDING.lock().get_or_insert(binding);
    }
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
    kernel::logger::init(kernel::log::LevelFilter::Info).unwrap();
    kernel::log::info!(target: "kernel::boot", "Early kernel log");
    kernel::logger::LOGGER.enable_timestamps(|| hardware::timer().now_ns().ok());
    kernel::log::warn!(target: "kernel::test", "Timestamped kernel log");
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
        if mode != 8 {
            bootstrap(&receiver, mode);
        }
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
                } else if mode == 8 {
                    Rights::READ_WRITE | Rights::WAIT
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
        TRACK_BINDINGS.store(mode == 8, Ordering::Relaxed);
        user.start().unwrap();
        if mode == 6 {
            drop(receiver);
            resident(&task, &user);
            continue;
        }
        if mode == 8 {
            managed_service(&task, &user, receiver);
            TRACK_BINDINGS.store(false, Ordering::Relaxed);
            *BINDING.lock() = None;
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

fn managed_service(task: &Task, user: &Thread, control: Channel) {
    let client = Task::new().unwrap();
    let control = client
        .insert(Object::Channel(control), Rights::READ_WRITE)
        .unwrap();
    let devices = DEVICES.get().unwrap();
    let rights = ObjectType::DeviceSet.supported_rights();
    let catalog = client
        .insert(Object::DeviceSet(devices.clone()), rights)
        .unwrap();
    assert_eq!(
        service_transfer(
            &client,
            control,
            0,
            &[],
            &[TransferRequest {
                handle: catalog,
                rights
            }]
        ),
        Message::new(0, &[]).unwrap(),
    );
    assert_eq!(service_call(&client, control, u64::MAX, &[]).tag, 1);
    let (status, unused) = open_uart(&client, control, u32::MAX);
    assert_eq!(status, 3);
    client.close(unused).unwrap();

    let mut sessions = Vec::new();
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let info = devices.describe(id).unwrap();
        if !(0..info.identities).any(|index| {
            devices
                .read(id, abi::device::DeviceField::IdentityNamespace, index)
                .unwrap()
                == b"devicetree"
                && matches!(
                    devices
                        .read(id, abi::device::DeviceField::Identity, index)
                        .unwrap(),
                    b"ns16550" | b"ns16550a"
                )
        }) {
            continue;
        }
        let (status, channel) = open_uart(&client, control, id);
        assert_eq!(status, 0, "UART {id} did not start");
        let (status, unused) = open_uart(&client, control, id);
        assert_eq!(status, 2);
        client.close(unused).unwrap();
        assert_eq!(
            service_call(&client, channel, 3, &[1]),
            Message::new(0, &[0, 0]).unwrap()
        );
        sessions.push((id, channel, info.group.unwrap()));
    }
    assert!(!sessions.is_empty());
    let (id, channel, group) = sessions.pop().unwrap();
    client.close(channel).unwrap();
    let channel = reconnect_uart(&client, control, id);
    sessions.push((id, channel, group));
    let (id, event) = BINDING.lock().unwrap();
    let position = sessions
        .iter()
        .position(|(device, _, _)| *device == id)
        .unwrap();
    let (_, channel, group) = sessions.remove(position);
    task.close(event).unwrap();
    assert!(matches!(
        client.receive_until(channel, 0, service_deadline()),
        Err(kernel::task::TaskError::Channel(
            kernel::ipc::ChannelError::EndOfStream
        ))
    ));
    client.close(channel).unwrap();
    let channel = reconnect_uart(&client, control, id);
    sessions.push((id, channel, group));
    for (_, channel, _) in &sessions {
        assert_eq!(service_call(&client, *channel, 6, &[0, 5, 0, 2]).tag, 4);
        assert_eq!(
            service_call(&client, *channel, 3, &[1]),
            Message::new(0, &[0, 0]).unwrap()
        );
        assert_eq!(
            service_call(&client, *channel, 5, &[]),
            Message::new(0, &[]).unwrap()
        );
    }
    client.close(control).unwrap();
    assert_eq!(user.wait().unwrap(), 0);
    assert_eq!(task.state(), TaskState::Dead(0));
    for (_, channel, group) in sessions {
        assert!(matches!(
            client.try_receive(channel, 0),
            Err(kernel::task::TaskError::Channel(
                kernel::ipc::ChannelError::EndOfStream
            ))
        ));
        client.close(channel).unwrap();
        drop(devices.claim_group(group).unwrap());
    }
    client.terminate(0);
}

fn open_uart(client: &Task, control: abi::Handle, id: u32) -> (u64, abi::Handle) {
    let (channel, server) = Channel::pair().unwrap();
    let channel = client
        .insert(Object::Channel(channel), Rights::READ_WRITE)
        .unwrap();
    let rights = Rights::READ_WRITE | Rights::WAIT | Rights::TRANSFER;
    let server = client.insert(Object::Channel(server), rights).unwrap();
    let response = service_transfer(
        client,
        control,
        1,
        &[u64::from(id)],
        &[TransferRequest {
            handle: server,
            rights,
        }],
    );
    assert!(response.words().is_empty());
    (response.tag, channel)
}

fn reconnect_uart(client: &Task, control: abi::Handle, id: u32) -> abi::Handle {
    let deadline = service_deadline();
    loop {
        let (status, channel) = open_uart(client, control, id);
        if status == 0 {
            return channel;
        }
        assert!(matches!(status, 2 | 6));
        client.close(channel).unwrap();
        assert!(hardware::timer().now_ns().unwrap() < deadline);
        thread::yield_now().unwrap();
    }
}

fn service_call(client: &Task, channel: abi::Handle, tag: u64, words: &[u64]) -> Message {
    service_transfer(client, channel, tag, words, &[])
}

fn service_transfer(
    client: &Task,
    channel: abi::Handle,
    tag: u64,
    words: &[u64],
    transfers: &[TransferRequest],
) -> Message {
    let response = client
        .call_until(
            channel,
            Message::new(tag, words).unwrap(),
            transfers,
            service_deadline(),
        )
        .unwrap();
    assert!(response.handles().is_empty());
    response.message
}

fn service_deadline() -> u64 {
    hardware::timer().now_ns().unwrap() + 5_000_000_000
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
            let mut transfers = alloc::vec![TransferRequest { handle, rights }, device];
            if mode >= 9 {
                let rights = ObjectType::KernelLog.supported_rights();
                let handle = source
                    .insert(
                        Object::KernelLog(kernel::logger::LOGGER.kernel_log()),
                        rights,
                    )
                    .unwrap();
                transfers.push(TransferRequest { handle, rights });
            }
            source
                .try_send(
                    channel,
                    Message::new(0, &[length as u64]).unwrap(),
                    &transfers,
                )
                .unwrap();
        } else {
            let rights = ObjectType::KernelLog.supported_rights();
            let log = source
                .insert(
                    Object::KernelLog(kernel::logger::LOGGER.kernel_log()),
                    rights,
                )
                .unwrap();
            source
                .try_send(
                    channel,
                    Message::new(mode, &[]).unwrap(),
                    &[
                        device,
                        TransferRequest {
                            handle: log,
                            rights,
                        },
                    ],
                )
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
    } else if mode == 0 {
        log_bootstrap(receiver, mode);
    } else {
        receiver.send(Message::new(mode, &[]).unwrap()).unwrap();
    }
}

fn log_bootstrap(receiver: &Channel, mode: u64) {
    let source = Task::new().unwrap();
    let channel = source
        .insert(Object::Channel(receiver.clone()), Rights::WRITE)
        .unwrap();
    let rights = ObjectType::KernelLog.supported_rights();
    let handle = source
        .insert(
            Object::KernelLog(kernel::logger::LOGGER.kernel_log()),
            rights,
        )
        .unwrap();
    source
        .try_send(
            channel,
            Message::new(mode, &[]).unwrap(),
            &[TransferRequest { handle, rights }],
        )
        .unwrap();
    source.terminate(0);
}
