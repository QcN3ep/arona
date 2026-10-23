// SPDX-License-Identifier: MPL-2.0

use kernel::{
    hardware::{self, cpu::InterruptState},
    ipc::{Channel, ChannelError, Message},
    memory::{self, FrameAllocation, FrameOwner, FramePurpose, HeapArc, MemoryError},
    task::{
        Handle, HandleError, HandleTable, Object, ObjectReference, ObjectType, Rights,
        TransferRequest,
    },
    thread::{Thread, ThreadState, scheduler},
};
use spin::Mutex;

static THREAD_HANDLE: Mutex<Option<(HeapArc<HandleTable>, Handle)>> = Mutex::new(None);

pub fn run() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let heap = manager.heap_stats();
    let frames = manager.stats().kernel.free_frames;
    thread_lifecycle();
    transfer_lifecycle();
    channel_lifecycle();
    allocation_failure();
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats().kernel.free_frames, frames);
}

fn thread_lifecycle() {
    let table = memory::manager().allocate_arc(HandleTable::new()).unwrap();
    let other = HandleTable::new();
    let rights = Rights::DUPLICATE | Rights::TRANSFER | Rights::WAIT | Rights::START;
    let handle = table
        .insert(
            Object::Thread(Thread::new(|value| value, 73).unwrap()),
            rights,
        )
        .unwrap();
    assert_eq!(handle.raw(), (1u64 << 32) | 1);
    assert!(Rights::from_bits(1 << 15).is_none());
    assert!(Rights::from_bits(1 << 26).is_none());
    assert_eq!(Rights::from_bits(rights.bits()), Some(rights));
    for invalid in [0, 1, 1u64 << 32, u64::MAX] {
        assert!(matches!(
            table.get(Handle::from_raw(invalid), ObjectType::Thread, Rights::WAIT),
            Err(HandleError::BadHandle)
        ));
    }
    assert!(matches!(
        other.get(handle, ObjectType::Thread, Rights::WAIT),
        Err(HandleError::BadHandle)
    ));
    assert!(matches!(
        table.get(handle, ObjectType::FrameAllocation, Rights::WRITE),
        Err(HandleError::WrongType)
    ));
    assert_eq!(
        table.duplicate(handle, Rights::WRITE),
        Err(HandleError::Denied)
    );
    let wait = table.duplicate(handle, Rights::WAIT).unwrap();
    assert_eq!(
        table.duplicate(wait, Rights::WAIT),
        Err(HandleError::Denied)
    );
    assert!(matches!(
        table.get(wait, ObjectType::Thread, Rights::START),
        Err(HandleError::Denied)
    ));
    let ObjectReference::Thread(thread) = table
        .get(handle, ObjectType::Thread, Rights::START)
        .unwrap()
    else {
        panic!("expected thread");
    };
    thread.start().unwrap();
    table.close(handle).unwrap();
    assert_eq!(table.close(handle), Err(HandleError::BadHandle));
    *THREAD_HANDLE.lock() = Some((table.clone(), wait));
    let waiter = Thread::new(wait_from_handle, 0).unwrap();
    waiter.start().unwrap();
    scheduler::run_ready().unwrap();
    assert_eq!(thread.state(), ThreadState::Dead(73));
    assert_eq!(waiter.state(), ThreadState::Dead(73));
    let ObjectReference::Thread(result) =
        table.get(wait, ObjectType::Thread, Rights::WAIT).unwrap()
    else {
        panic!("expected thread");
    };
    assert_eq!(result.state(), ThreadState::Dead(73));
    table.close(wait).unwrap();
    let reused = table.insert(Object::Thread(result), Rights::NONE).unwrap();
    assert_eq!(reused.raw() as u32, wait.raw() as u32);
    assert_eq!(reused.raw() >> 32, (wait.raw() >> 32) + 1);
    assert!(matches!(
        table.get(wait, ObjectType::Thread, Rights::NONE),
        Err(HandleError::BadHandle)
    ));
    hardware::cpu().restore_interrupts(InterruptState::Enabled);
    table.close(reused).unwrap();
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Enabled);
    hardware::cpu().disable_interrupts();
}

fn wait_from_handle(_: usize) -> usize {
    let (table, handle) = THREAD_HANDLE.lock().take().unwrap();
    let ObjectReference::Thread(target) =
        table.get(handle, ObjectType::Thread, Rights::WAIT).unwrap()
    else {
        panic!("expected thread");
    };
    target.wait().unwrap()
}

fn transfer_lifecycle() {
    let source = memory::manager().allocate_arc(HandleTable::new()).unwrap();
    let target = HandleTable::new();
    let rights = Rights::DUPLICATE | Rights::TRANSFER | Rights::WAIT;
    let first = source
        .insert(Object::Thread(Thread::new(|_| 0, 0).unwrap()), rights)
        .unwrap();
    let second = source.duplicate(first, rights).unwrap();
    let requests = [
        TransferRequest {
            handle: first,
            rights: Rights::WAIT,
        },
        TransferRequest {
            handle: second,
            rights: Rights::WAIT,
        },
    ];
    assert!(matches!(
        source.prepare_transfer(&target, &[requests[0], requests[0]]),
        Err(HandleError::InvalidArgument)
    ));
    assert!(matches!(
        source.prepare_transfer(
            &target,
            &[TransferRequest {
                handle: first,
                rights: Rights::WRITE
            }]
        ),
        Err(HandleError::Denied)
    ));
    let cancelled = source.prepare_transfer(&target, &requests).unwrap();
    let unpublished = cancelled.handles().next().unwrap();
    assert!(matches!(
        target.get(unpublished, ObjectType::Thread, Rights::NONE),
        Err(HandleError::BadHandle)
    ));
    assert_eq!(target.close(unpublished), Err(HandleError::BadHandle));
    drop(cancelled);
    let pending = source.prepare_transfer(&target, &requests).unwrap();
    let destination = pending.handles().next().unwrap();
    assert_ne!(destination, unpublished);
    *THREAD_HANDLE.lock() = Some((source.clone(), second));
    let closer = Thread::new(
        |_| {
            let (table, handle) = THREAD_HANDLE.lock().take().unwrap();
            table.close(handle).unwrap();
            0
        },
        0,
    )
    .unwrap();
    closer.start().unwrap();
    scheduler::run_ready().unwrap();
    let replacement = source.duplicate(first, rights).unwrap();
    assert_eq!(replacement.raw() as u32, second.raw() as u32);
    assert_eq!(pending.commit(), Err(HandleError::BadHandle));
    assert!(source.get(first, ObjectType::Thread, Rights::WAIT).is_ok());
    assert!(
        source
            .get(replacement, ObjectType::Thread, Rights::WAIT)
            .is_ok()
    );
    assert!(matches!(
        target.get(destination, ObjectType::Thread, Rights::NONE),
        Err(HandleError::BadHandle)
    ));
    successful_transfers(&source, &target, first, replacement);
}

fn successful_transfers(source: &HandleTable, target: &HandleTable, first: Handle, second: Handle) {
    let pending = source
        .prepare_transfer(
            target,
            &[
                TransferRequest {
                    handle: first,
                    rights: Rights::WAIT,
                },
                TransferRequest {
                    handle: second,
                    rights: Rights::WAIT | Rights::TRANSFER,
                },
            ],
        )
        .unwrap();
    let mut handles = pending.handles();
    let first_target = handles.next().unwrap();
    let second_target = handles.next().unwrap();
    drop(handles);
    pending.commit().unwrap();
    assert_eq!(source.close(first), Err(HandleError::BadHandle));
    assert_eq!(source.close(second), Err(HandleError::BadHandle));
    assert_eq!(
        target.duplicate(first_target, Rights::WAIT),
        Err(HandleError::Denied)
    );
    let same = target
        .prepare_transfer(
            target,
            &[TransferRequest {
                handle: second_target,
                rights: Rights::WAIT,
            }],
        )
        .unwrap();
    let moved = same.handles().next().unwrap();
    same.commit().unwrap();
    assert_eq!(target.close(second_target), Err(HandleError::BadHandle));
    assert!(target.get(moved, ObjectType::Thread, Rights::WAIT).is_ok());
    target.close(first_target).unwrap();
    target.close(moved).unwrap();
}

fn channel_lifecycle() {
    let source = HandleTable::new();
    let target = HandleTable::new();
    let (channel, peer) = Channel::pair().unwrap();
    let rights = Rights::DUPLICATE | Rights::TRANSFER | Rights::WAIT | Rights::READ | Rights::WRITE;
    let handle = source.insert(Object::Channel(channel), rights).unwrap();
    assert!(matches!(
        source.get(handle, ObjectType::Thread, Rights::WAIT),
        Err(HandleError::WrongType)
    ));
    assert_eq!(
        source.duplicate(handle, Rights::EXECUTE),
        Err(HandleError::Denied)
    );
    let duplicate = source.duplicate(handle, Rights::READ).unwrap();
    assert!(matches!(
        source.get(duplicate, ObjectType::Channel, Rights::WRITE),
        Err(HandleError::Denied)
    ));
    assert_eq!(
        source.duplicate(duplicate, Rights::READ),
        Err(HandleError::Denied)
    );
    let ObjectReference::Channel(reference) = source
        .get(handle, ObjectType::Channel, Rights::WRITE)
        .unwrap()
    else {
        panic!("expected channel");
    };
    let requests = [TransferRequest {
        handle,
        rights: Rights::READ | Rights::WRITE,
    }];
    drop(source.prepare_transfer(&target, &requests).unwrap());
    assert_eq!(peer.try_receive(), Err(ChannelError::WouldBlock));
    let transfer = source.prepare_transfer(&target, &requests).unwrap();
    let moved = transfer.handles().next().unwrap();
    assert!(matches!(
        target.get(moved, ObjectType::Channel, Rights::READ),
        Err(HandleError::BadHandle)
    ));
    transfer.commit().unwrap();
    assert_eq!(source.close(handle), Err(HandleError::BadHandle));
    source.close(duplicate).unwrap();
    let ObjectReference::Channel(transferred) = target
        .get(moved, ObjectType::Channel, Rights::WRITE)
        .unwrap()
    else {
        panic!("expected channel");
    };
    let message = Message::new(73, &[19, 42]).unwrap();
    transferred.try_send(message).unwrap();
    target.close(moved).unwrap();
    assert_eq!(reference.try_send(message), Err(ChannelError::Closed));
    assert_eq!(transferred.try_receive(), Err(ChannelError::Closed));
    assert_eq!(peer.try_receive(), Ok(message));
    assert_eq!(peer.try_receive(), Err(ChannelError::EndOfStream));
    assert_eq!(peer.try_send(message), Err(ChannelError::PeerClosed));

    let (channel, peer) = Channel::pair().unwrap();
    assert_eq!(
        source.insert(Object::Channel(channel), Rights::EXECUTE),
        Err(HandleError::InvalidArgument)
    );
    assert_eq!(peer.try_receive(), Err(ChannelError::EndOfStream));
}

fn allocation_failure() {
    let manager = memory::manager();
    let source = HandleTable::new();
    let target = HandleTable::new();
    let allocation = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, 2)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    let object = Object::FrameAllocation(manager.allocate_arc(allocation).unwrap());
    let handle = source
        .insert(
            object.clone(),
            Rights::DUPLICATE | Rights::TRANSFER | Rights::READ,
        )
        .unwrap();
    assert_eq!(
        source.insert(object.clone(), Rights::START),
        Err(HandleError::InvalidArgument)
    );
    let pending = source
        .prepare_transfer(
            &target,
            &[TransferRequest {
                handle,
                rights: Rights::READ,
            }],
        )
        .unwrap();
    let destination = pending.handles().next().unwrap();
    let mut requests = [TransferRequest {
        handle,
        rights: Rights::READ,
    }; 128];
    for request in &mut requests {
        request.handle = source
            .duplicate(handle, Rights::READ | Rights::TRANSFER)
            .unwrap();
    }
    let pressure = HandleTable::new();
    let exhausted = exhaust_memory(&pressure, object.clone());
    let heap = manager.heap_stats();
    assert!(matches!(
        Channel::pair(),
        Err(ChannelError::Memory(MemoryError::NoMemory))
    ));
    assert_eq!(manager.heap_stats(), heap);
    assert!(matches!(
        source.prepare_transfer(
            &pressure,
            &[TransferRequest {
                handle,
                rights: Rights::READ
            }]
        ),
        Err(HandleError::NoMemory)
    ));
    let unavailable = HandleTable::new();
    assert!(matches!(
        source.prepare_transfer(&unavailable, &requests),
        Err(HandleError::NoMemory)
    ));
    for request in requests {
        assert!(
            source
                .get(request.handle, ObjectType::FrameAllocation, Rights::READ)
                .is_ok()
        );
    }
    pending.commit().unwrap();
    assert_eq!(source.close(handle), Err(HandleError::BadHandle));
    let ObjectReference::FrameAllocation(reference) = target
        .get(destination, ObjectType::FrameAllocation, Rights::READ)
        .unwrap()
    else {
        panic!("expected frame");
    };
    let Object::FrameAllocation(original) = object else {
        panic!("expected frame");
    };
    assert!(HeapArc::ptr_eq(&reference, &original));
    assert!(reference.contains(frame));
    drop(exhausted);
    target.close(destination).unwrap();
    assert_eq!(reference.info().frames, 2);
}

fn exhaust_memory(pressure: &HandleTable, object: Object) -> FrameAllocation {
    let manager = memory::manager();
    let mut last = pressure
        .insert(object, Rights::DUPLICATE | Rights::READ)
        .unwrap();
    manager.reclaim_heap();
    let exhausted = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames,
        )
        .unwrap();
    loop {
        match pressure.duplicate(last, Rights::DUPLICATE | Rights::READ) {
            Ok(handle) => last = handle,
            Err(error) => {
                assert_eq!(error, HandleError::NoMemory);
                break;
            }
        }
    }
    assert!(
        pressure
            .get(last, ObjectType::FrameAllocation, Rights::READ)
            .is_ok()
    );
    exhausted
}
