// SPDX-License-Identifier: MPL-2.0

//! Linux 进程、线程标识及父子状态。

use super::signal::{self, Info, Signals, ThreadSignals};
use crate::{
    ExitStatus,
    syscall::abi::{ESRCH, Errno},
};
use alloc::{
    collections::BTreeMap,
    rc::{Rc, Weak},
};
use core::cell::{Cell, RefCell};

pub(crate) struct PidNamespace {
    next: Cell<usize>,
    pub processes: RefCell<BTreeMap<usize, ProcessRecord>>,
    threads: RefCell<BTreeMap<usize, ThreadRecord>>,
    pub futexes: Rc<super::futex::Futexes>,
}

pub(crate) struct ProcessRecord {
    pub parent: usize,
    pub group: usize,
    pub status: Option<ExitStatus>,
    pub change: Option<ChildChange>,
    pub signals: Rc<RefCell<Signals>>,
}

#[derive(Clone, Copy)]
pub(crate) enum ChildChange {
    Stopped(u8),
    Continued,
}

struct ThreadRecord {
    pid: usize,
    signals: Weak<RefCell<ThreadSignals>>,
}

impl PidNamespace {
    pub fn new() -> Self {
        Self {
            next: Cell::new(1),
            processes: RefCell::new(BTreeMap::new()),
            threads: RefCell::new(BTreeMap::new()),
            futexes: Rc::default(),
        }
    }
    pub fn allocate(&self) -> Option<usize> {
        let id = self.next.get();
        if id > i32::MAX as usize {
            return None;
        }
        self.next.set(id + 1);
        Some(id)
    }
    pub fn register(
        &self,
        pid: usize,
        parent: usize,
        group: usize,
        signals: &Rc<RefCell<Signals>>,
    ) {
        self.processes.borrow_mut().insert(
            pid,
            ProcessRecord {
                parent,
                group,
                status: None,
                change: None,
                signals: Rc::clone(signals),
            },
        );
    }
    pub fn register_thread(&self, pid: usize, tid: usize, signals: &Rc<RefCell<ThreadSignals>>) {
        self.threads.borrow_mut().insert(
            tid,
            ThreadRecord {
                pid,
                signals: Rc::downgrade(signals),
            },
        );
    }
    pub fn remove_thread(&self, tid: usize) {
        self.threads.borrow_mut().remove(&tid);
    }
    pub fn parent(&self, pid: usize) -> usize {
        self.processes
            .borrow()
            .get(&pid)
            .map_or(0, |record| record.parent)
    }
    pub fn group(&self, pid: usize) -> usize {
        self.processes
            .borrow()
            .get(&pid)
            .map_or(pid, |record| record.group)
    }

    pub fn signal_thread(&self, tid: usize, pid: Option<usize>, info: Info) -> Result<(), Errno> {
        let threads = self.threads.borrow();
        let thread = threads.get(&tid).ok_or(ESRCH)?;
        if pid.is_some_and(|pid| pid != thread.pid) {
            return Err(ESRCH);
        }
        let signals = thread.signals.upgrade().ok_or(ESRCH)?;
        let pid = thread.pid;
        drop(threads);
        if info.number == 0 {
            return Ok(());
        }
        if matches!(info.number, signal::KILL | signal::STOP) {
            return self.signal_process(pid, info);
        }
        let process_signals = self
            .processes
            .borrow()
            .get(&pid)
            .map(|record| Rc::clone(&record.signals))
            .ok_or(ESRCH)?;
        self.job_control(pid, info.number, &process_signals);
        signals.borrow_mut().pending.push(info)
    }

    pub fn signal_process(&self, pid: usize, info: Info) -> Result<(), Errno> {
        let records = self.processes.borrow();
        let record = records.get(&pid).ok_or(ESRCH)?;
        if info.number == 0 || record.status.is_some() {
            return Ok(());
        }
        let signals = Rc::clone(&record.signals);
        drop(records);
        self.job_control(pid, info.number, &signals);
        signals.borrow_mut().pending.push(info)
    }

    fn job_control(&self, pid: usize, number: u8, state: &Rc<RefCell<Signals>>) {
        let continued = number == signal::CONT && state.borrow().stopped;
        if number == signal::CONT {
            state.borrow_mut().stopped = false;
            for signal in [signal::STOP, 20, 21, 22] {
                self.discard(pid, signal);
            }
        } else if matches!(number, signal::STOP | 20 | 21 | 22) {
            self.discard(pid, signal::CONT);
        }
        if continued {
            self.change(pid, ChildChange::Continued);
        }
    }

    pub fn discard(&self, pid: usize, number: u8) {
        if let Some(record) = self.processes.borrow().get(&pid) {
            record.signals.borrow_mut().pending.discard(number);
        }
        self.discard_threads(pid, number);
    }
    fn discard_threads(&self, pid: usize, number: u8) {
        for thread in self
            .threads
            .borrow()
            .values()
            .filter(|thread| thread.pid == pid)
        {
            if let Some(signals) = thread.signals.upgrade() {
                signals.borrow_mut().pending.discard(number);
            }
        }
    }

    pub fn change(&self, pid: usize, change: ChildChange) {
        let parent = {
            let mut records = self.processes.borrow_mut();
            let Some(record) = records.get_mut(&pid) else {
                return;
            };
            record.change = Some(change);
            record.parent
        };
        let (code, value) = match change {
            ChildChange::Stopped(signal) => (5, signal as u64),
            ChildChange::Continued => (6, signal::CONT as u64),
        };
        self.child_signal(
            parent,
            Info {
                number: 17,
                code,
                sender: pid,
                address: 0,
                value,
            },
            true,
        );
    }
    fn child_signal(&self, parent: usize, info: Info, changed: bool) {
        let records = self.processes.borrow();
        let Some(record) = records.get(&parent) else {
            return;
        };
        let mut signals = record.signals.borrow_mut();
        if changed && signals.actions[16].flags & 1 != 0 {
            return;
        }
        let _ = signals.pending.push(info);
    }

    pub fn exit(&self, pid: usize, status: ExitStatus) {
        let parent = self.parent(pid);
        let autoreap = self.processes.borrow().get(&parent).is_some_and(|record| {
            let action = record.signals.borrow().actions[16];
            action.handler == 1 || action.flags & 2 != 0
        });
        let mut processes = self.processes.borrow_mut();
        for record in processes.values_mut().filter(|record| record.parent == pid) {
            record.parent = 0;
        }
        if let Some(record) = processes.get_mut(&pid) {
            record.status = Some(status);
            record.change = None;
        }
        if autoreap {
            processes.remove(&pid);
        }
        processes.retain(|_, record| record.parent != 0 || record.status.is_none());
        drop(processes);
        self.child_signal(
            parent,
            Info {
                number: 17,
                code: if status.signal == 0 { 1 } else { 2 },
                sender: pid,
                address: 0,
                value: if status.signal == 0 {
                    status.code as u64
                } else {
                    status.signal as u64
                },
            },
            false,
        );
    }
}
