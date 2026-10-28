// SPDX-License-Identifier: MPL-2.0

use super::{ThreadState, spawn::Resources};
use crate::{
    Error, Event, Handle, Rights, Thread,
    ipc::{WaitItem, WaitMode, wait_many},
    process,
};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use arrayvec::ArrayVec;
use spin::{Mutex, Once};

static REAPER: Once<Reaper> = Once::new();

struct Reaper {
    state: Arc<State>,
    _worker: Mutex<Box<Resources<()>>>,
}

struct State {
    wake: Handle<Event>,
    pending: Mutex<Vec<Box<dyn Retired>>>,
}

pub(super) trait Retired: Send {
    fn thread(&self) -> &Handle<Thread>;
}

impl<T: Send + 'static> Retired for Resources<T> {
    fn thread(&self) -> &Handle<Thread> {
        self.thread()
    }
}

pub(super) fn initialize() -> Result<(), Error> {
    REAPER.try_call_once(|| {
        let state = Arc::new(State {
            wake: process::task().event(Rights::WAIT | Rights::SIGNAL)?,
            pending: Mutex::new(Vec::new()),
        });
        let worker_state = state.clone();
        let worker = Resources::start(64 * 1024, move || worker_state.run())?;
        Ok(Reaper {
            state,
            _worker: Mutex::new(worker),
        })
    })?;
    Ok(())
}

pub(super) fn retire(resources: Box<dyn Retired>) {
    let state = &REAPER.get().unwrap().state;
    state.pending.lock().push(resources);
    state.wake.signal(1).expect("failed to wake thread reaper");
}

impl State {
    fn run(&self) {
        let mut pending: Vec<Box<dyn Retired>> = Vec::new();
        let mut cursor = 0;
        loop {
            pending.append(&mut self.pending.lock());
            let mut index = 0;
            while index < pending.len() {
                if pending[index]
                    .thread()
                    .state()
                    .expect("failed to query retired thread")
                    .0
                    == ThreadState::Dead
                {
                    drop(pending.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            let mut items: ArrayVec<WaitItem<'_>, { abi::ipc::WAIT_ITEMS }> = ArrayVec::new();
            items.push(WaitItem::new(&self.wake, 1));
            if pending.len() < abi::ipc::WAIT_ITEMS || cursor >= pending.len() {
                cursor = 0;
            }
            for thread in pending.iter().skip(cursor).take(items.capacity() - 1) {
                items.push(WaitItem::new(thread.thread(), crate::ipc::EXITED));
            }
            let mode = if pending.len() >= abi::ipc::WAIT_ITEMS {
                let now = super::now_ns().expect("failed to read reaper clock");
                cursor += items.len() - 1;
                if cursor >= pending.len() {
                    cursor = 0;
                }
                WaitMode::Until(now.saturating_add(1_000_000).min(u64::MAX - 1))
            } else {
                cursor = 0;
                WaitMode::Forever
            };
            match wait_many(&items, mode) {
                Ok(_) => {}
                Err(Error::Kernel(abi::Error::TimedOut)) => {
                    super::yield_now().expect("failed to yield thread reaper");
                }
                Err(error) => panic!("failed to wait for retired threads: {error:?}"),
            }
        }
    }
}
