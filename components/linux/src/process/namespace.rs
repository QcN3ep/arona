// SPDX-License-Identifier: MPL-2.0

//! Linux 进程、线程标识、会话及父子状态。

use super::signal::{self, Info, Signals, ThreadSignals};
use crate::{
    ExitStatus,
    syscall::abi::{EACCES, EINVAL, EPERM, ESRCH, Errno},
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    rc::{Rc, Weak},
    vec::Vec,
};
use core::cell::{Cell, RefCell};

pub(crate) struct PidNamespace {
    next: Cell<usize>,
    pub processes: RefCell<BTreeMap<usize, ProcessRecord>>,
    pub(super) threads: RefCell<BTreeMap<usize, ThreadRecord>>,
    pub futexes: Rc<super::futex::Futexes>,
}

pub(crate) struct ProcessRecord {
    pub parent: usize,
    pub group: usize,
    pub(super) session: usize,
    pub(super) terminal: Option<Rc<crate::file::terminal::Terminal>>,
    executed: bool,
    pub status: Option<ExitStatus>,
    pub change: Option<ChildChange>,
    pub signals: Rc<RefCell<Signals>>,
}

#[derive(Clone, Copy)]
pub(crate) enum ChildChange {
    Stopped(u8),
    Continued,
}

pub(super) struct ThreadRecord {
    pid: usize,
    pub(super) signals: Weak<RefCell<ThreadSignals>>,
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
        session: usize,
        signals: &Rc<RefCell<Signals>>,
    ) {
        let terminal = self
            .processes
            .borrow()
            .get(&parent)
            .and_then(|record| record.terminal.clone());
        self.processes.borrow_mut().insert(
            pid,
            ProcessRecord {
                parent,
                group,
                session,
                terminal,
                executed: false,
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

    pub fn identity(&self, id: usize) -> Result<(usize, usize), Errno> {
        let pid = self
            .threads
            .borrow()
            .get(&id)
            .map_or(id, |thread| thread.pid);
        let records = self.processes.borrow();
        let record = records.get(&pid).ok_or(ESRCH)?;
        Ok((record.group, record.session))
    }

    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/kernel/sys.c?h=v6.12
    pub fn set_group(&self, caller: usize, pid: usize, group: usize) -> Result<(), Errno> {
        let mut records = self.processes.borrow_mut();
        let session = records.get(&caller).ok_or(ESRCH)?.session;
        let record = records.get(&pid).ok_or_else(|| {
            if self.threads.borrow().contains_key(&pid) {
                EINVAL
            } else {
                ESRCH
            }
        })?;
        if pid != caller {
            if record.parent != caller {
                return Err(ESRCH);
            }
            if record.session != session {
                return Err(EPERM);
            }
            if record.executed {
                return Err(EACCES);
            }
        }
        if record.session == pid
            || group != pid
                && !records
                    .values()
                    .any(|record| record.group == group && record.session == session)
        {
            return Err(EPERM);
        }
        records.get_mut(&pid).unwrap().group = group;
        Ok(())
    }

    pub fn new_session(&self, pid: usize) -> Result<usize, Errno> {
        let mut records = self.processes.borrow_mut();
        if records.values().any(|record| record.group == pid) {
            return Err(EPERM);
        }
        let record = records.get_mut(&pid).ok_or(ESRCH)?;
        if record.session == pid {
            return Err(EPERM);
        }
        record.group = pid;
        record.session = pid;
        record.terminal = None;
        Ok(pid)
    }

    pub fn exec(&self, pid: usize) {
        if let Some(record) = self.processes.borrow_mut().get_mut(&pid) {
            record.executed = true;
        }
    }

    pub fn orphaned(&self, group: usize) -> bool {
        let records = self.processes.borrow();
        !records
            .values()
            .any(|record| record.group == group && record.connected(&records))
    }

    fn connected_groups(&self) -> BTreeSet<usize> {
        let records = self.processes.borrow();
        records
            .values()
            .filter_map(|record| record.connected(&records).then_some(record.group))
            .collect()
    }

    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/kernel/exit.c?h=v6.12
    fn orphan_groups(&self, mut connected: BTreeSet<usize>) {
        let remaining = self.connected_groups();
        connected.retain(|group| !remaining.contains(group));
        let mut groups = BTreeMap::<usize, Vec<(usize, bool)>>::new();
        for (&pid, record) in self.processes.borrow().iter() {
            if record.status.is_none() && connected.contains(&record.group) {
                groups
                    .entry(record.group)
                    .or_default()
                    .push((pid, record.signals.borrow().stopped));
            }
        }
        for members in groups.values() {
            if !members.iter().any(|(_, stopped)| *stopped) {
                continue;
            }
            for number in [1, signal::CONT] {
                for &(pid, _) in members {
                    let _ = self.signal_process(
                        pid,
                        Info {
                            number,
                            code: 128,
                            sender: 0,
                            address: 0,
                            value: 0,
                        },
                    );
                }
            }
        }
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
        self.release_terminal(pid);
        let connected = self.connected_groups();
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
        self.orphan_groups(connected);
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

impl ProcessRecord {
    fn connected(&self, records: &BTreeMap<usize, Self>) -> bool {
        self.status.is_none()
            && records.get(&self.parent).is_some_and(|parent| {
                parent.status.is_none()
                    && parent.group != self.group
                    && parent.session == self.session
            })
    }
}
