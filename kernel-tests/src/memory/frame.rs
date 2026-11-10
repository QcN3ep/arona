// SPDX-License-Identifier: MPL-2.0

use alloc::collections::BTreeSet;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use kernel::{
    memory::{FrameAccess, FrameAllocation, FrameOwner, FramePurpose, HeapArc, ReferenceKind},
    thread::{self, Thread, ThreadState, scheduler},
};

use super::*;

static LIVE: Mutex<BTreeSet<Frame>> = Mutex::new(BTreeSet::new());
static SHARED: Mutex<Option<HeapArc<FrameAllocation>>> = Mutex::new(None);
static WRITERS: AtomicUsize = AtomicUsize::new(0);
static EXECUTORS: AtomicUsize = AtomicUsize::new(0);

pub(super) fn run() {
    zeroed();
    references();
    access();
}

fn zeroed() {
    let manager = memory::manager();
    let initial = manager.stats();
    assert_eq!(Frame::new(1), None);
    for owner in [FrameOwner::User(7), FrameOwner::User(8)] {
        let allocation = manager.allocate(owner, FramePurpose::Data, 3).unwrap();
        assert_eq!(allocation.info().owner, owner);
        assert_eq!(allocation.info().purpose, FramePurpose::Data);
        assert_eq!(allocation.info().frames, 3);
        let mut frames = allocation.frames();
        let identifiers: [_; 3] = core::array::from_fn(|_| frames.next().unwrap());
        assert!(frames.next().is_none());
        for (index, &frame) in identifiers.iter().enumerate() {
            assert!(!identifiers[..index].contains(&frame));
            let access = allocation
                .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
                .unwrap();
            let mut words = [u64::MAX; PAGE_SIZE / 8];
            access.read(0, &mut words).unwrap();
            assert_eq!(words, [0; PAGE_SIZE / 8]);
            access.write(0, &[0xdead_beef; PAGE_SIZE / 8]).unwrap();
        }
        drop(allocation);
        assert_eq!(manager.stats(), initial);
    }
}

fn references() {
    let manager = memory::manager();
    let initial = manager.stats();
    let allocation = manager
        .allocate(FrameOwner::User(9), FramePurpose::Data, 2)
        .unwrap();
    let mapping = allocation.reference(ReferenceKind::Mapping).unwrap();
    let dma = allocation.reference(ReferenceKind::Dma).unwrap();
    let temporary = allocation.reference(ReferenceKind::Temporary).unwrap();
    let info = allocation.info();
    assert_eq!((info.mappings, info.dma, info.temporary), (1, 1, 1));
    let mut frames = allocation.frames();
    let identifiers: [_; 2] = core::array::from_fn(|_| frames.next().unwrap());
    drop(allocation);
    let shared = mapping.reference(ReferenceKind::Mapping).unwrap();
    drop((mapping, dma, temporary));
    assert_eq!(
        manager.stats().user.free_frames,
        initial.user.free_frames - 2
    );
    assert_eq!(shared.info().mappings, 1);
    assert!(shared.frames().all(|frame| identifiers.contains(&frame)));
    drop(shared);
    assert_eq!(manager.stats(), initial);
}

fn access() {
    let manager = memory::manager();
    let initial = manager.stats();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let other = manager
        .allocate(FrameOwner::User(2), FramePurpose::Data, 1)
        .unwrap();
    let frame = first_frame(&allocation);
    assert!(matches!(
        allocation.access(
            first_frame(&other),
            ReferenceKind::Temporary,
            FrameAccess::Write
        ),
        Err(MemoryError::InvalidFrame)
    ));
    assert_eq!(allocation.info().temporary, 0);
    let writer = allocation
        .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
        .unwrap();
    let reader = allocation
        .access(frame, ReferenceKind::Temporary, FrameAccess::Read)
        .unwrap();
    writer.write(0, &[42]).unwrap();
    assert_eq!(reader.write(0, &[0]), Err(MemoryError::AccessConflict));
    let mut word = [0];
    reader.read(0, &mut word).unwrap();
    assert_eq!(word, [42]);
    assert_eq!(reader.frame(), frame);
    assert!(reader.read(PAGE_SIZE / 8, &mut word).is_err());
    drop((allocation, other, writer));
    assert_eq!(
        manager.stats().user.free_frames,
        initial.user.free_frames - 2
    );
    reader.read(0, &mut word).unwrap();
    assert_eq!(word, [42]);
    drop(reader);
    assert_eq!(manager.stats(), initial);
}

pub(super) fn concurrent() {
    let manager = memory::manager();
    manager.reclaim_heap();
    let initial = manager.stats();
    let heap = manager.heap_stats();
    *SHARED.lock() = Some(HeapArc::new(
        manager
            .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
            .unwrap(),
    ));
    let workers: [_; 4] = core::array::from_fn(|index| Thread::new(worker, index).unwrap());
    for worker in &workers {
        worker.start().unwrap();
    }
    scheduler::run_ready().unwrap();
    for worker in &workers {
        assert_eq!(worker.state(), ThreadState::Dead(0));
    }
    assert!(LIVE.lock().is_empty());
    drop(core::mem::take(&mut *LIVE.lock()));
    assert_eq!(WRITERS.load(Ordering::Relaxed), 0);
    assert_eq!(EXECUTORS.load(Ordering::Relaxed), 0);
    *SHARED.lock() = None;
    drop(workers);
    manager.reclaim_heap();
    assert_eq!(manager.heap_stats(), heap);
    assert_eq!(manager.stats(), initial);
}

fn worker(index: usize) -> usize {
    let shared = SHARED.lock().as_ref().unwrap().clone();
    let frame = first_frame(&shared);
    let (kind, access) = match index {
        0 => (ReferenceKind::Mapping, FrameAccess::Execute),
        1 => (ReferenceKind::Mapping, FrameAccess::Write),
        2 => (ReferenceKind::Dma, FrameAccess::Write),
        _ => (ReferenceKind::Temporary, FrameAccess::Write),
    };
    for _ in 0..16 {
        let allocation = memory::manager()
            .allocate(FrameOwner::User(index as u64 + 1), FramePurpose::Data, 2)
            .unwrap();
        for frame in allocation.frames() {
            assert!(LIVE.lock().insert(frame));
        }
        thread::yield_now().unwrap();
        match shared.access(frame, kind, access) {
            Ok(reference) => {
                let (active, conflicting) = if access == FrameAccess::Execute {
                    (&EXECUTORS, &WRITERS)
                } else {
                    (&WRITERS, &EXECUTORS)
                };
                active.fetch_add(1, Ordering::Relaxed);
                assert_eq!(conflicting.load(Ordering::Relaxed), 0);
                thread::yield_now().unwrap();
                assert_eq!(conflicting.load(Ordering::Relaxed), 0);
                active.fetch_sub(1, Ordering::Relaxed);
                drop(reference);
            }
            Err(MemoryError::AccessConflict) => thread::yield_now().unwrap(),
            Err(error) => panic!("unexpected frame access failure: {error:?}"),
        }
        for frame in allocation.frames() {
            assert!(LIVE.lock().remove(&frame));
        }
    }
    0
}
