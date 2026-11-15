// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

pub(crate) mod futex;
pub(crate) mod image;
pub(crate) mod mqueue;
pub(crate) mod namespace;
pub(crate) mod signal;
pub(crate) mod terminal;
pub(crate) mod timer;

use alloc::{rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};
use namespace::PidNamespace;
use runtime::{
    Handle, Rights, Task, TrapPort,
    ipc::{EXITED, WaitItem},
    thread::{ThreadState, UserEntry},
    trap::{Trap, TrapInfo},
};
use signal::{Signals, ThreadSignals, delivery};

use crate::{
    Error, ExitStatus,
    elf::{ImageSet, Machine},
    file::FileTable,
    memory::AddressSpace,
    protocol::Arguments,
    syscall::{self, Pending},
};

pub(crate) struct Thread<'a> {
    pub tid: usize,
    pub clear_tid: usize,
    pub native: Handle<runtime::Thread>,
    pub native_id: u64,
    pub pending: Option<Pending<'a>>,
    pub signals: Rc<RefCell<ThreadSignals>>,
    pub held: Option<syscall::Context>,
    pub vfork: Option<Rc<Vfork>>,
    pub interrupt_requested: bool,
}

/// Completion state shared by a vfork parent and the child that owns its suspended context.
pub(crate) struct Vfork {
    child: usize,
    done: Cell<bool>,
}

impl Vfork {
    pub fn new(child: usize) -> Rc<Self> {
        Rc::new(Self {
            child,
            done: Cell::new(false),
        })
    }

    pub fn child(&self) -> usize {
        self.child
    }

    pub fn done(&self) -> bool {
        self.done.get()
    }

    /// Resume the parent after the child replaces its address space or exits.
    pub fn release(&self) {
        self.done.set(true);
    }
}

pub(crate) struct Process<'a> {
    pub namespace: Rc<PidNamespace>,
    pub pid: usize,
    pub signals: Rc<RefCell<Signals>>,
    pub signal_return: Option<usize>,
    pub children: Vec<Process<'a>>,
    pub cpu_ns: u64,
    pub timers: timer::Timers,
    pub port: Rc<Handle<TrapPort>>,
    pub supervisor: &'a Handle<Task>,
    pub address_space: AddressSpace,
    pub threads: Vec<Thread<'a>>,
    pub files: FileTable<'a>,
    pub terminal: alloc::rc::Rc<crate::file::terminal::Terminal>,
    pub cwd: Option<alloc::rc::Rc<crate::file::OpenFileDescription<'a>>>,
    pub program: Vec<u8>,
    pub argv: Vec<Vec<u8>>,
    /// Peak resident bytes reached by this process.
    pub max_rss: u64,
    pub machine: Machine,
    pub vfork: Option<Rc<Vfork>>,
    pub umask: u16,
    pub status: ExitStatus,
    pub credentials: Credentials,
    pub limits: crate::limits::Limits,
    pub cpu_limited: bool,
}

/// Real, effective, and saved user and group identities inherited by child processes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Credentials {
    pub uid: u32,
    pub euid: u32,
    pub suid: u32,
    pub gid: u32,
    pub egid: u32,
    pub sgid: u32,
}

impl<'a> Process<'a> {
    pub fn start(
        parent: &'a Handle<Task>,
        port: &Rc<Handle<TrapPort>>,
        images: &ImageSet<'_>,
        arguments: &Arguments,
        terminal: alloc::rc::Rc<crate::file::terminal::Terminal>,
        random: Option<&[u8; 16]>,
        namespace: Rc<PidNamespace>,
    ) -> Result<Self, Error> {
        let pid = namespace.allocate().ok_or(Error::NoMemory)?;
        let start_ns = runtime::thread::now_ns().unwrap_or(0);
        let machine = images.image.machine;
        let entry = images.entry();
        let limits = crate::limits::Limits::default();
        let (address_space, stack) =
            AddressSpace::load(parent, port, images, arguments, random, limits.stack())?;
        let native = address_space.native.create_thread(
            UserEntry {
                pc: entry as u64,
                stack_pointer: stack as u64,
                thread_pointer: 0,
                argument: 0,
            },
            Rights::WAIT
                | Rights::START
                | Rights::SET_STATE
                | Rights::TERMINATE
                | Rights::INTERRUPT,
        )?;
        native.set_user_state(&machine.initial(entry, stack))?;
        let id = native.id()?;
        let files = FileTable::new(alloc::rc::Rc::clone(&terminal));
        native.start()?;
        let signals = Rc::default();
        let thread_signals = Rc::default();
        namespace.register(
            pid,
            0,
            pid,
            pid,
            &signals,
            &arguments.path,
            &arguments.argv,
            start_ns,
        );
        namespace.register_thread(pid, pid, &thread_signals);
        if terminal.has_input() {
            namespace
                .attach_terminal(pid, &terminal)
                .map_err(|_| Error::InvalidRequest)?;
        }
        Ok(Self {
            namespace,
            pid,
            signals,
            signal_return: None,
            children: Vec::new(),
            cpu_ns: 0,
            timers: timer::Timers::default(),
            port: Rc::clone(port),
            supervisor: parent,
            address_space,
            threads: alloc::vec![Thread {
                tid: pid,
                clear_tid: 0,
                native,
                native_id: id,
                pending: None,
                signals: thread_signals,
                held: None,
                vfork: None,
                interrupt_requested: false,
            }],
            files,
            terminal,
            cwd: None,
            program: arguments.path.clone(),
            argv: arguments.argv.clone(),
            max_rss: 0,
            machine,
            vfork: None,
            umask: 0o022,
            credentials: Credentials::default(),
            status: ExitStatus { code: 0, signal: 0 },
            limits,
            cpu_limited: false,
        })
    }

    pub fn trap(
        &mut self,
        trap: Trap,
        files: &fs::Client<'a>,
        services: &crate::Services<'a>,
    ) -> Result<(), Error> {
        let Some(index) = self
            .threads
            .iter()
            .position(|thread| thread.native_id == trap.info.thread)
        else {
            return Ok(());
        };
        if self.threads[index].native.state()?.0 == ThreadState::Dead {
            return Ok(());
        }
        self.threads[index].interrupt_requested = false;
        match trap.info.kind {
            TrapInfo::SYSCALL => syscall::dispatch(self, index, trap, files, services),
            TrapInfo::INTERRUPTED => {
                let state = trap.state()?;
                delivery::resume(
                    self,
                    index,
                    syscall::Context::new(self.machine, trap, state),
                )
            }
            _ => {
                let state = trap.state()?;
                delivery::fault(
                    self,
                    index,
                    syscall::Context::new(self.machine, trap, state),
                )
            }
        }
    }

    pub fn poll(
        &mut self,
        files: &fs::Client<'a>,
        services: &crate::Services<'a>,
    ) -> Result<bool, Error> {
        self.refresh_record();
        let mut progress = timer::poll(self)?;
        progress |= self.enforce_cpu_limit();
        progress |= self.terminal.poll();
        if self.terminal.failed() {
            progress |= self.namespace.hangup_terminal(&self.terminal);
        }
        while let Some(number) = self.terminal.take_signal() {
            if let Some((session, group)) = self.terminal.control() {
                self.namespace.signal_group(session, group, number);
            }
            progress = true;
        }
        let mut index = 0;
        while index < self.threads.len() {
            if self.threads[index].native.state()?.0 == ThreadState::Dead {
                self.cpu_ns = self
                    .cpu_ns
                    .saturating_add(self.threads[index].native.cpu_time_ns().unwrap_or(0));
                let status = self.threads[index].native.wait(0)?;
                let clear_tid = self.threads[index].clear_tid;
                if clear_tid != 0
                    && self.address_space.write(clear_tid, &[0; 4]).is_ok()
                    && let Ok((key, _)) = self.address_space.futex_key(self.pid, clear_tid, false)
                {
                    self.namespace.futexes.wake(key, 1, u32::MAX);
                }
                self.status = if status == usize::MAX {
                    ExitStatus { code: 0, signal: 9 }
                } else {
                    ExitStatus::decode(status)
                };
                self.namespace.remove_thread(self.threads[index].tid);
                self.threads.swap_remove(index);
                progress = true;
                continue;
            }
            progress |= delivery::poll(self, index)?;
            if !self.signals.borrow().stopped
                && self.threads[index].native.state()?.0 != ThreadState::Dead
            {
                progress |= syscall::poll(self, index, files, services)?;
            }
            index += 1;
        }
        if self.threads.is_empty() {
            self.release_vfork();
        }
        Ok(progress)
    }

    /// Reports the CPU time limit as `SIGXCPU`, or `SIGKILL` past the hard limit.
    fn enforce_cpu_limit(&mut self) -> bool {
        let limit = self.limits.cpu();
        if limit.current == crate::limits::INFINITY && limit.maximum == crate::limits::INFINITY {
            return false;
        }
        let mut used = self.cpu_ns;
        for thread in &self.threads {
            used = used.saturating_add(thread.native.cpu_time_ns().unwrap_or(0));
        }
        let seconds = used / 1_000_000_000;
        let hard = limit.maximum != crate::limits::INFINITY && seconds >= limit.maximum;
        let soft = limit.current != crate::limits::INFINITY && seconds >= limit.current;
        if !hard && (!soft || self.cpu_limited) {
            return false;
        }
        let number = if hard { 9 } else { 24 };
        let info = signal::Info {
            number,
            code: 128,
            sender: 0,
            uid: 0,
            address: 0,
            value: 0,
        };
        let _ = self.namespace.signal_process(self.pid, info);
        if !hard {
            self.cpu_limited = true;
        }
        true
    }

    /// Resume a suspended vfork parent once this process replaces its address space or exits.
    pub fn release_vfork(&self) {
        if let Some(vfork) = &self.vfork {
            vfork.release();
        }
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<WaitItem<'b>>) {
        self.timers.waits(items);
        for thread in &self.threads {
            items.push(WaitItem::new(&thread.native, EXITED));
            if !self.signals.borrow().stopped
                && let Some(pending) = &thread.pending
            {
                pending.waits(items);
            }
        }
    }

    /// Total CPU time consumed by the process, including the current dispatch interval.
    pub fn cpu_time_ns(&self) -> u64 {
        self.threads.iter().fold(self.cpu_ns, |total, thread| {
            total.saturating_add(thread.native.cpu_time_ns().unwrap_or(0))
        })
    }

    fn runnable(&self) -> bool {
        self.threads.iter().any(|thread| {
            matches!(
                thread.native.state(),
                Ok((ThreadState::Ready | ThreadState::Running, _))
            )
        })
    }

    pub fn status(&self) -> ExitStatus {
        self.status
    }

    /// Total CPU time of every thread that has run or is running in this process.
    pub fn cpu_ns(&self) -> u64 {
        self.threads.iter().fold(self.cpu_ns, |total, thread| {
            total.saturating_add(thread.native.cpu_time_ns().unwrap_or(0))
        })
    }

    /// Bytes represented by the process's current mappings.
    pub fn resident_bytes(&self) -> u64 {
        self.address_space.mapped_bytes() as u64
    }

    /// Publish the values that other processes read from `/proc/<pid>`.
    fn refresh_record(&mut self) {
        let cpu_ns = self.cpu_ns();
        let resident = self.resident_bytes();
        self.max_rss = self.max_rss.max(resident);
        let state = if self.signals.borrow().stopped {
            b'T'
        } else if self.threads.iter().any(|thread| {
            matches!(
                thread.native.state().map(|state| state.0),
                Ok(ThreadState::Running | ThreadState::Ready)
            )
        }) {
            b'R'
        } else {
            b'S'
        };
        let threads = self.threads.len();
        let max_rss = self.max_rss;
        if let Some(record) = self.namespace.processes.borrow_mut().get_mut(&self.pid) {
            record.state = state;
            record.threads = threads;
            record.vsize = resident;
            record.rss = resident;
            record.usage.cpu_ns = cpu_ns;
            record.usage.max_rss = max_rss;
        }
    }
}
