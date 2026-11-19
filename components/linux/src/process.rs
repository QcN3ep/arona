// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

pub(crate) mod futex;
pub(crate) mod image;
pub(crate) mod mqueue;
pub(crate) mod namespace;
pub(crate) mod signal;
pub(crate) mod terminal;
pub(crate) mod timer;

use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};
use namespace::PidNamespace;
use runtime::{
    Handle, Rights, Task, TrapPort,
    ipc::{EXITED, WaitItem},
    thread::{Lifecycle, UserEntry},
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

pub(crate) mod scheduling;
pub(crate) use scheduling::{SCHED_BATCH, SCHED_IDLE, SCHED_OTHER};

pub(crate) struct Thread<'a> {
    pub tid: usize,
    pub clear_tid: usize,
    /// User address of this thread's `struct robust_list_head`, or zero.
    pub robust_list: usize,
    pub native: Handle<runtime::Thread>,
    pub native_id: u64,
    pub policy: i32,
    pub nice: i8,
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

/// Process-independent state shared by every guest of the Linux service.
pub(crate) struct Shared {
    pub namespace: Rc<PidNamespace>,
    pub shm: Rc<RefCell<crate::shm::Table>>,
    pub mappings: Rc<crate::memory::files::Registry>,
}

/// Options that are not part of the image or argument environment of a new process.
pub(crate) struct StartOptions<'a> {
    pub random: Option<&'a [u8; 16]>,
    pub mount: bool,
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
    pub group_status: Option<ExitStatus>,
    pub credentials: Credentials,
    pub limits: crate::limits::Limits,
    pub cpu_limited: bool,
    /// Granted by the launching program and inherited by forks; user mount calls
    /// require it together with root credentials.
    pub mount: bool,
    pub shm: Rc<RefCell<crate::shm::Table>>,
    pub attaches: BTreeMap<usize, crate::shm::Attach>,
    pub membarrier: Membarrier,
}

/// Process-wide `membarrier` registrations, inherited across fork and cleared on exec.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Membarrier {
    pub private_expedited: bool,
    pub global_expedited: bool,
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

impl Drop for Process<'_> {
    fn drop(&mut self) {
        self.detach_shared_memory();
    }
}

impl<'a> Process<'a> {
    pub fn start(
        parent: &'a Handle<Task>,
        port: &Rc<Handle<TrapPort>>,
        images: &ImageSet<'_>,
        arguments: &Arguments,
        terminal: alloc::rc::Rc<crate::file::terminal::Terminal>,
        options: StartOptions<'_>,
        shared: &Shared,
    ) -> Result<Self, Error> {
        let namespace = Rc::clone(&shared.namespace);
        let pid = namespace.allocate().ok_or(Error::NoMemory)?;
        let start_ns = runtime::thread::now_ns().unwrap_or(0);
        let machine = images.image.machine;
        let entry = images.entry();
        let limits = crate::limits::Limits::default();
        let (address_space, stack) = AddressSpace::load(
            parent,
            port,
            images,
            arguments,
            options.random,
            limits.stack(),
            Rc::clone(&shared.mappings),
        )?;
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
                | Rights::READ
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
                robust_list: 0,
                native,
                native_id: id,
                policy: SCHED_OTHER,
                nice: 0,
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
            shm: Rc::clone(&shared.shm),
            attaches: BTreeMap::new(),
            membarrier: Membarrier::default(),
            status: ExitStatus { code: 0, signal: 0 },
            group_status: None,
            limits,
            cpu_limited: false,
            mount: options.mount,
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
        if self.threads[index].native.state()?.0 == Lifecycle::Exited {
            return Ok(());
        }
        self.threads[index].interrupt_requested = false;
        match trap.info.kind {
            TrapInfo::SYSCALL => syscall::dispatch(self, index, trap, files, services),
            TrapInfo::INTERRUPTED => {
                let state = trap.general_state()?;
                delivery::resume(
                    self,
                    index,
                    syscall::Context::new(self.machine, trap, state),
                )
            }
            _ => {
                let state = trap.general_state()?;
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
        progress |= self.namespace.poll_terminals();
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
            if self.threads[index].native.state()?.0 == Lifecycle::Exited {
                self.cpu_ns = self
                    .cpu_ns
                    .saturating_add(self.threads[index].native.cpu_time_ns().unwrap_or(0));
                self.robust_list_cleanup(index);
                let status = self.threads[index].native.wait(0)?;
                self.clear_tid_cleanup(index);
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
                && self.threads[index].native.state()?.0 != Lifecycle::Exited
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

    /// Release attachments fully covered by an address range the process
    /// unmapped or moved without `shmdt`.
    pub fn detach_shared_range(&mut self, _start: usize, _end: usize) {
        let stale: Vec<usize> = self
            .attaches
            .iter()
            .filter_map(|(address, attach)| {
                let end = address.saturating_add(attach.mapped_length);
                (!self.address_space.has_system_v(*address..end, attach.id)).then_some(*address)
            })
            .collect();
        let now = runtime::thread::now_ns().unwrap_or(0);
        for address in stale {
            if let Some(attach) = self.attaches.remove(&address)
                && attach.counted
            {
                self.shm.borrow_mut().detach(attach.id, self.pid, now);
            }
        }
    }

    /// Release every System V attachment and account its detachment.
    pub fn detach_shared_memory(&mut self) {
        if self.attaches.is_empty() {
            return;
        }
        let attaches = core::mem::take(&mut self.attaches);
        let shm = Rc::clone(&self.shm);
        let mut table = shm.borrow_mut();
        let now = runtime::thread::now_ns().unwrap_or(0);
        for attach in attaches.into_values() {
            if attach.counted {
                table.detach(attach.id, self.pid, now);
            }
        }
    }

    /// Resume a suspended vfork parent once this process replaces its address space or exits.
    pub fn release_vfork(&self) {
        if let Some(vfork) = &self.vfork {
            vfork.release();
        }
    }

    /// Clear the exiting thread's TID word and wake one process-shared waiter.
    pub(crate) fn clear_tid_cleanup(&mut self, index: usize) {
        let address = core::mem::take(&mut self.threads[index].clear_tid);
        if let Some(clear) = self.address_space.prepare_clear_tid(self.pid, address) {
            clear.complete(&self.namespace.futexes);
        }
    }

    pub(crate) fn terminate_thread(&mut self, index: usize, status: usize) -> Result<(), Error> {
        let address = self.threads[index].clear_tid;
        let clear = self.address_space.prepare_clear_tid(self.pid, address);
        self.threads[index].native.terminate(status)?;
        self.threads[index].clear_tid = 0;
        if let Some(clear) = clear {
            clear.complete(&self.namespace.futexes);
        }
        Ok(())
    }

    pub(crate) fn terminate_threads(&mut self, status: usize) -> Result<(), Error> {
        let targets: Vec<_> = self
            .threads
            .iter()
            .map(|thread| {
                self.address_space
                    .prepare_clear_tid(self.pid, thread.clear_tid)
            })
            .collect();
        for thread in &self.threads {
            thread.native.terminate(status)?;
        }
        for thread in &mut self.threads {
            thread.clear_tid = 0;
        }
        for clear in targets.into_iter().flatten() {
            clear.complete(&self.namespace.futexes);
        }
        Ok(())
    }

    /// Release the robust futexes owned by an exiting thread.
    ///
    /// Each list node points at a futex word through the head's `futex_offset`.
    /// A word whose owner bits still name the exiting thread is marked
    /// `FUTEX_OWNER_DIED` and one waiter is woken. The head, the nodes, and the
    /// futex words are user memory, so an unreadable or malformed list ends the
    /// walk instead of failing the exit.
    pub(crate) fn robust_list_cleanup(&self, index: usize) {
        let thread = &self.threads[index];
        let head = thread.robust_list;
        let tid = thread.tid as u32;
        if head == 0 {
            return;
        }
        let mut header = [0; futex::ROBUST_LIST_HEAD_SIZE];
        if self.address_space.read(head, &mut header).is_err() {
            return;
        }
        let mut entry = (u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize) & !1;
        let offset = i64::from_le_bytes(header[8..16].try_into().unwrap());
        let pending = (u64::from_le_bytes(header[16..24].try_into().unwrap()) as usize) & !1;
        for _ in 0..futex::ROBUST_LIST_LIMIT {
            if entry == head {
                break;
            }
            let mut next = [0; 8];
            let fetched = self.address_space.read(entry, &mut next);
            if entry != pending {
                self.robust_futex_cleanup(entry, offset, tid, false);
            }
            if fetched.is_err() {
                return;
            }
            entry = (u64::from_le_bytes(next) as usize) & !1;
        }
        if pending != 0 {
            self.robust_futex_cleanup(pending, offset, tid, true);
        }
    }

    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/kernel/futex/core.c
    /// Mark one robust-list futex as owner-died and wake a waiter.
    ///
    /// A pending entry with no recorded owner is the unlock vs. thread-death
    /// race in Linux: the value was released but the wakeup was not issued yet,
    /// so the waiter must be woken without changing the word.
    fn robust_futex_cleanup(&self, entry: usize, offset: i64, tid: u32, pending: bool) {
        let address = (entry as i64).wrapping_add(offset);
        if address < 0 {
            return;
        }
        let address = address as usize;
        if address & 3 != 0 {
            return;
        }
        // A competing update changes the expected word, so re-read and retry a
        // bounded number of times instead of overwriting another owner's bits.
        for _ in 0..16 {
            let mut word = [0; 4];
            if self.address_space.read(address, &mut word).is_err() {
                return;
            }
            let value = u32::from_le_bytes(word);
            if pending && value & futex::FUTEX_TID_MASK == 0 {
                self.robust_wake(address);
                return;
            }
            if value & futex::FUTEX_TID_MASK != tid {
                return;
            }
            let updated = (value & futex::FUTEX_WAITERS) | futex::FUTEX_OWNER_DIED;
            match self.address_space.compare_exchange(address, value, updated) {
                Ok(Some(_)) => {
                    self.robust_wake(address);
                    return;
                }
                Ok(None) => {}
                Err(_) => return,
            }
        }
    }

    /// Wake one waiter on a robust futex without changing its word.
    fn robust_wake(&self, address: usize) {
        // A robust list does not record FUTEX_PRIVATE_FLAG, while arona derives
        // the wait key from it. Wake both interpretations of the futex word.
        for private in [true, false] {
            if let Ok((key, _)) = self.address_space.futex_key(self.pid, address, private) {
                self.namespace.futexes.wake(key, 1, u32::MAX);
            }
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
        !self.signals.borrow().stopped
            && self.threads.iter().any(|thread| {
                thread.pending.is_none()
                    && thread.held.is_none()
                    && matches!(thread.native.state(), Ok((Lifecycle::Live, _)))
            })
    }

    /// Earliest deadline among asynchronous memory operations.
    pub fn deadline(&self) -> Option<u64> {
        if self.signals.borrow().stopped {
            return None;
        }
        self.threads
            .iter()
            .filter_map(|thread| thread.pending.as_ref())
            .filter_map(Pending::deadline)
            .min()
    }

    pub fn status(&self) -> ExitStatus {
        self.group_status.unwrap_or(self.status)
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
        } else if self.runnable() {
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
