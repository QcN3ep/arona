// SPDX-License-Identifier: MPL-2.0

pub(crate) mod futex;
pub(crate) mod image;
pub(crate) mod namespace;
pub(crate) mod signal;

use alloc::{rc::Rc, vec::Vec};
use core::cell::RefCell;
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
    elf::{Image, Machine},
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
    pub interrupt_requested: bool,
}

pub(crate) struct Process<'a> {
    pub namespace: Rc<PidNamespace>,
    pub pid: usize,
    pub signals: Rc<RefCell<Signals>>,
    pub signal_return: Option<usize>,
    pub children: Vec<Process<'a>>,
    pub port: Rc<Handle<TrapPort>>,
    pub supervisor: &'a Handle<Task>,
    pub address_space: AddressSpace,
    pub threads: Vec<Thread<'a>>,
    pub files: FileTable<'a>,
    pub terminal: alloc::rc::Rc<crate::file::terminal::Terminal>,
    pub cwd: Option<alloc::rc::Rc<crate::file::OpenFileDescription<'a>>>,
    pub machine: Machine,
    pub umask: u16,
    pub status: ExitStatus,
}

impl<'a> Process<'a> {
    pub fn start(
        parent: &'a Handle<Task>,
        port: &Rc<Handle<TrapPort>>,
        image: &Image<'_>,
        arguments: &Arguments,
        terminal: alloc::rc::Rc<crate::file::terminal::Terminal>,
        random: Option<&[u8; 16]>,
        namespace: Rc<PidNamespace>,
    ) -> Result<Self, Error> {
        let pid = namespace.allocate().ok_or(Error::NoMemory)?;
        let machine = image.machine;
        let (address_space, stack) = AddressSpace::load(parent, port, image, arguments, random)?;
        let native = address_space.native.create_thread(
            UserEntry {
                pc: image.entry as u64,
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
        native.set_user_state(&machine.initial(image.entry, stack))?;
        let id = native.id()?;
        let files = FileTable::new(alloc::rc::Rc::clone(&terminal));
        native.start()?;
        let signals = Rc::default();
        let thread_signals = Rc::default();
        namespace.register(pid, 0, pid, pid, &signals);
        namespace.register_thread(pid, pid, &thread_signals);
        Ok(Self {
            namespace,
            pid,
            signals,
            signal_return: None,
            children: Vec::new(),
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
                interrupt_requested: false,
            }],
            files,
            terminal,
            cwd: None,
            machine,
            umask: 0o022,
            status: ExitStatus { code: 0, signal: 0 },
        })
    }

    pub fn trap(
        &mut self,
        trap: Trap,
        files: &fs::Client<'a>,
        services: &crate::Services<'_>,
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
        services: &crate::Services<'_>,
    ) -> Result<bool, Error> {
        let mut progress = self.terminal.poll();
        let mut index = 0;
        while index < self.threads.len() {
            if self.threads[index].native.state()?.0 == ThreadState::Dead {
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
        Ok(progress)
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<WaitItem<'b>>) {
        for thread in &self.threads {
            items.push(WaitItem::new(&thread.native, EXITED));
            if !self.signals.borrow().stopped
                && let Some(pending) = &thread.pending
            {
                pending.waits(items);
            }
        }
    }

    pub fn status(&self) -> ExitStatus {
        self.status
    }
}
