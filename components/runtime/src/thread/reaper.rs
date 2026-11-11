// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{ThreadState, spawn::Resources};
use crate::{
    Error, Event, Handle, Rights, Thread,
    ipc::{WaitItem, WaitMode, wait_many},
    process,
};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
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
            let mut items = Vec::with_capacity(pending.len() + 1);
            items.push(WaitItem::new(&self.wake, 1));
            for thread in &pending {
                items.push(WaitItem::new(thread.thread(), crate::ipc::EXITED));
            }
            wait_many(&items, WaitMode::Forever).expect("failed to wait for retired threads");
        }
    }
}
