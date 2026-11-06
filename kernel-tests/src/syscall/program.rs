// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
static MODE: AtomicU64 = AtomicU64::new(0);
static ARCHIVE: Once<&'static [u8]> = Once::new();
static SERVICES: Mutex<Vec<Task>> = Mutex::new(Vec::new());
static SUPERVISING: AtomicBool = AtomicBool::new(false);
static INIT_TASK: AtomicU64 = AtomicU64::new(0);
static TRACK_BINDINGS: AtomicBool = AtomicBool::new(false);
static BINDING: Mutex<Option<(u32, abi::Handle)>> = Mutex::new(None);
static PCI_RESULTS: AtomicU64 = AtomicU64::new(0);

pub fn run_device(image: &'static [u8], devices: DeviceSet, resident: bool) -> u64 {
    DEVICES.call_once(|| devices);
    PCI_RESULTS.store(0, Ordering::Relaxed);
    run_cases(image, if resident { &[8, 6] } else { &[5] }, true, false);
    PCI_RESULTS.load(Ordering::Relaxed)
}

pub fn run_init(image: &'static [u8], archive: &'static [u8], devices: DeviceSet) {
    DEVICES.call_once(|| devices);
    ARCHIVE.call_once(|| archive);
    run_cases(image, &[9, 10, 11, 12, 13], true, true);
}

pub fn handle(trap: Trap) -> TrapOutcome {
    let binding = if TRACK_BINDINGS.load(Ordering::Relaxed)
        && let TrapEvent::Syscall {
            handle, arguments, ..
        } = trap.event
        && arguments[0] == abi::method::device::BIND as usize
        && DEVICES.get().is_some_and(|devices| {
            devices
                .describe(arguments[1] as u32)
                .is_ok_and(|device| device.console)
        })
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
    let caller = thread::current().and_then(|thread| thread.task());
    let supervising = if ARCHIVE.get().is_some()
        && let TrapEvent::Syscall { arguments, .. } = trap.event
        && arguments[0] == abi::method::thread::WAIT_MANY as usize
        && arguments[2] == 3
    {
        caller.as_ref().is_some_and(|caller| {
            caller
                .get(
                    abi::Handle::from_raw(INIT_TASK.load(Ordering::Relaxed)),
                    ObjectType::Task,
                    Rights::CREATE_TASK,
                )
                .is_ok()
        })
    } else {
        false
    };
    let method = if ARCHIVE.get().is_some()
        && let TrapEvent::Syscall {
            handle, arguments, ..
        } = trap.event
        && arguments[0] == abi::method::task::CREATE as usize
        && caller.as_ref().is_some_and(|caller| {
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
    if supervising {
        SUPERVISING.store(true, Ordering::Relaxed);
    }
    drop(caller);
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
            .and_then(|thread| thread.task())
            .as_ref()
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
        SERVICES.lock().push(task);
    }
    outcome
}

pub fn run(image: &'static [u8]) {
    kernel::logger::init(kernel::log::LevelFilter::Info).unwrap();
    kernel::log::info!(target: "kernel::boot", "Early kernel log");
    kernel::logger::LOGGER.enable_timestamps(|| hardware::timer().now_ns().ok());
    kernel::log::warn!(target: "kernel::test", "Timestamped kernel log");
    run_cases(image, &[0, 1, 2, 3, 4, 0], false, false);
}

pub fn run_process(image: &'static [u8]) {
    run_cases(image, &[7], false, false);
}

fn run_cases(
    image: &'static [u8],
    modes: &[u64],
    reset_baseline_after_first: bool,
    repeat_first: bool,
) {
    IMAGE.call_once(|| image);
    let manager = memory::manager();
    manager.reclaim_heap();
    let mut before = manager.stats();
    let mut heap = manager.heap_stats();
    let channels = kernel::ipc::active_channels();
    let messages = kernel::ipc::queued_messages();
    let selections = kernel::ipc::active_selections();
    let tasks = kernel::task::registered_tasks();
    let threads = kernel::thread::allocated_threads();
    let events = kernel::ipc::active_events();
    let timers = kernel::ipc::active_timers();
    let replies = kernel::ipc::active_replies();
    let grants = kernel::device::active_grant_tokens();
    let records = kernel::task::allocated_task_records();
    let task_resources = kernel::task::allocated_task_resources();
    // 常驻映射的索引页可在节点迁移后继续回收。
    let repeat = repeat_first.then(|| modes[0]);
    super::ACTIVE.store(true, Ordering::Relaxed);
    for (index, mode) in modes.iter().copied().chain(repeat).enumerate() {
        MODE.store(mode, Ordering::Relaxed);
        SUPERVISING.store(false, Ordering::Relaxed);
        crate::run_thread(scenario);
        if mode == 6 {
            break;
        }
        let deadline = hardware::timer().now_ns().unwrap() + 10_000_000_000;
        loop {
            manager.reclaim_heap();
            let resources_reclaimed = kernel::ipc::active_selections() == selections
                && kernel::task::registered_tasks() == tasks
                && kernel::thread::allocated_threads() == threads
                && kernel::ipc::active_events() == events
                && kernel::ipc::active_timers() == timers
                && kernel::ipc::active_replies() == replies
                && kernel::device::active_grant_tokens() == grants
                && kernel::task::allocated_task_records() == records
                && kernel::task::allocated_task_resources() == task_resources
                && kernel::ipc::active_channels() == channels
                && kernel::ipc::queued_messages() == messages;
            let memory = manager.stats();
            let mut expected = before;
            expected.kernel.free_frames = memory.kernel.free_frames;
            if resources_reclaimed
                && (reset_baseline_after_first && index == 0
                    || memory == expected
                        && memory.kernel.free_frames >= before.kernel.free_frames
                        && manager.heap_stats() == heap)
            {
                if reset_baseline_after_first && index == 0 {
                    before = memory;
                    heap = manager.heap_stats();
                }
                break;
            }
            assert!(
                hardware::timer().now_ns().unwrap() < deadline,
                "reclaim failed scenario={mode} resources={resources_reclaimed} memory={memory:?}/{before:?} heap={:?}/{heap:?}",
                manager.heap_stats(),
            );
            kernel::thread::scheduler::run_ready().unwrap();
            core::hint::spin_loop();
        }
    }
    super::ACTIVE.store(false, Ordering::Relaxed);
}

fn scenario(_: usize) -> usize {
    let mode = MODE.load(Ordering::Relaxed);
    let task = Task::new().unwrap();
    let held = (mode == 11).then(|| DEVICES.get().unwrap().claim_group(console_group()).unwrap());
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
    let stack = task.allocate_frames(64).unwrap();
    let range = task
        .map(
            &stack,
            0..64,
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
    if mode >= 9 {
        INIT_TASK.store(task_handle.raw(), Ordering::Relaxed);
    }
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
        return 0;
    }
    if mode == 8 {
        managed_service(&task, &user, receiver);
        TRACK_BINDINGS.store(false, Ordering::Relaxed);
        *BINDING.lock() = None;
        return 0;
    }
    if mode >= 9 {
        drop(receiver);
        init_lifecycle(&task, &user, mode);
        *SERVICES.lock() = Vec::new();
        drop(held);
        drop(DEVICES.get().unwrap().claim_group(console_group()).unwrap());
        return 0;
    }
    let status = usize::from(matches!(mode, 1..=3));
    assert_eq!(user.wait().unwrap(), status, "runtime test scenario {mode}");
    assert_eq!(task.state(), TaskState::Dead(status));
    if status == 0 {
        let report = receiver.try_receive().unwrap();
        assert_eq!(report.tag, 73);
        if mode == 5 {
            let counts = match *report.words() {
                [2] => 0,
                [2, mapped, wide, interrupts] => (mapped << 32) | (wide << 16) | interrupts,
                _ => panic!("invalid device test report"),
            };
            PCI_RESULTS.store(counts, Ordering::Relaxed);
        } else {
            assert_eq!(report.words(), [37, 41]);
        }
    } else {
        assert_eq!(
            receiver.try_receive(),
            Err(kernel::ipc::ChannelError::EndOfStream)
        );
    }
    0
}

fn resident(task: &Task, user: &Thread) {
    wait_service_idle(task, user);
    let devices = DEVICES.get().unwrap();
    let group = console_group();
    assert!(matches!(devices.claim_group(group), Err(DeviceError::Busy)));
    assert_eq!(
        task.wait_until(hardware::timer().now_ns().unwrap() + 20_000_000)
            .unwrap(),
        None
    );
    assert_eq!(task.state(), TaskState::Active);
    wait_service_idle(task, user);
    assert!(matches!(devices.claim_group(group), Err(DeviceError::Busy)));
}

fn wait_service_idle(task: &Task, user: &Thread) {
    let deadline = hardware::timer().now_ns().unwrap() + 1_000_000_000;
    while user.state() != ThreadState::Blocked {
        assert_eq!(task.state(), TaskState::Active);
        assert!(
            hardware::timer().now_ns().unwrap() < deadline,
            "service did not become idle"
        );
        thread::yield_now().unwrap();
    }
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
    discover_rng(&client, control);
    exercise_rtc(&client, control);
    let (status, unused) = open_session(&client, control, u32::MAX);
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
        let (status, channel) = open_session(&client, control, id);
        assert_eq!(status, 0, "UART {id} did not start");
        assert_eq!(service_call(&client, channel, 7, &[1]).tag, 0);
        let (status, unused) = open_session(&client, control, id);
        assert_eq!(status, 0);
        assert_eq!(service_call(&client, unused, 7, &[1]).tag, 2);
        assert_eq!(service_call(&client, unused, 7, &[2]).tag, 0);
        assert_eq!(service_call(&client, unused, 3, &[1]).tag, 7);
        assert_eq!(service_call(&client, unused, 7, &[1]).tag, 7);
        assert_eq!(service_call(&client, unused, 5, &[]).tag, 0);
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

fn discover_rng(client: &Task, control: abi::Handle) {
    let response = service_call(client, control, 2, &[3, 0]);
    let [rng] = *response.words() else {
        panic!("invalid RNG discovery response")
    };
    assert_ne!(rng, 0, "RNG device was not discovered");
    assert_eq!(service_call(client, control, 2, &[3, rng]).words(), [0]);
}

fn exercise_rtc(client: &Task, control: abi::Handle) {
    let response = service_call(client, control, 2, &[4, 0]);
    let [rtc] = *response.words() else {
        panic!("invalid RTC discovery response")
    };
    assert_ne!(rtc, 0, "RTC device was not discovered");
    assert_eq!(service_call(client, control, 2, &[4, rtc]).words(), [0]);
    let (status, channel) = open_session(client, control, rtc as u32);
    assert_eq!(status, 0);
    let response = service_call(client, channel, 2, &[]);
    assert_eq!(response.tag, 0);
    let [seconds, nanoseconds] = *response.words() else {
        panic!("invalid RTC time response")
    };
    assert!(seconds > 0 && nanoseconds < 1_000_000_000);
    client.close(channel).unwrap();
}

fn open_session(client: &Task, control: abi::Handle, id: u32) -> (u64, abi::Handle) {
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
        let (mut status, channel) = open_session(client, control, id);
        if status == 0 {
            status = service_call(client, channel, 7, &[1]).tag;
        }
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
    if matches!(mode, 11 | 12) {
        assert_eq!(user.wait().unwrap(), 1);
        assert_eq!(task.state(), TaskState::Dead(1));
        if mode == 11 {
            let children = SERVICES.lock().clone();
            assert_eq!(children.len(), 2);
            for child in children {
                assert_eq!(child.wait().unwrap(), 1);
                assert_eq!(child.state(), TaskState::Dead(1));
            }
        } else {
            assert!(SERVICES.lock().is_empty());
        }
        return;
    }
    let deadline = hardware::timer().now_ns().unwrap() + 30_000_000_000;
    let devices = DEVICES.get().unwrap();
    let group = console_group();
    loop {
        let busy = matches!(devices.claim_group(group), Err(DeviceError::Busy));
        let children = SERVICES.lock().clone();
        let active = children.len() == 3
            && children
                .iter()
                .all(|child| child.state() == TaskState::Active);
        if SUPERVISING.load(Ordering::Relaxed)
            && user.state() == ThreadState::Blocked
            && busy
            && active
        {
            break;
        }
        assert_eq!(task.state(), TaskState::Active);
        assert!(
            hardware::timer().now_ns().unwrap() < deadline,
            "init did not reach the service wait: supervising={}, init={:?}",
            SUPERVISING.load(Ordering::Relaxed),
            user.state(),
        );
        thread::yield_now().unwrap();
    }
    let children = SERVICES.lock().clone();
    assert_eq!(children.len(), 3);
    for (index, child) in children.iter().enumerate() {
        assert_eq!(child.state(), TaskState::Active, "service {index}");
    }
    assert!(matches!(devices.claim_group(group), Err(DeviceError::Busy)));
    assert_eq!(user.state(), ThreadState::Blocked);
    let status = if mode == 9 {
        children[2].terminate(73);
        73
    } else if mode == 13 {
        children[0].terminate(74);
        74
    } else {
        task.terminate(0);
        0
    };
    assert_eq!(user.wait().unwrap(), status);
    assert_eq!(task.state(), TaskState::Dead(status));
    for (index, child) in children.iter().enumerate() {
        let expected = if mode == 10 {
            0
        } else if (mode == 9 && index == 2) || (mode == 13 && index == 0) {
            status
        } else {
            1
        };
        assert_eq!(child.wait().unwrap(), expected);
        assert_eq!(child.state(), TaskState::Dead(expected));
    }
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
    if let Some(devices) = DEVICES.get().filter(|_| matches!(mode, 5 | 6 | 9..=13)) {
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
