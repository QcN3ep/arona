// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use runtime::{
    Handle, Rights, Task, TrapPort,
    ipc::{EXITED, WaitItem},
    thread::{ThreadState, UserEntry},
    trap::{Trap, TrapInfo},
};

use crate::{
    Error, ExitStatus,
    elf::{Image, Machine},
    file::FileTable,
    memory::AddressSpace,
    protocol::Arguments,
    syscall::{self, Pending},
};

pub(crate) struct Thread<'a> {
    pub native: Handle<runtime::Thread>,
    pub native_id: u64,
    pub pending: Option<Pending<'a>>,
}

pub(crate) struct Process<'a> {
    pub supervisor: &'a Handle<Task>,
    pub address_space: AddressSpace,
    pub threads: Vec<Thread<'a>>,
    pub files: FileTable<'a>,
    pub cwd: Option<alloc::rc::Rc<crate::file::OpenFileDescription<'a>>>,
    pub machine: Machine,
    pub umask: u16,
    status: ExitStatus,
}

impl<'a> Process<'a> {
    pub fn start(
        parent: &'a Handle<Task>,
        port: &Handle<TrapPort>,
        machine: Machine,
        image: &Image<'_>,
        arguments: &Arguments,
        output: devmgr::uart::Session,
    ) -> Result<Self, Error> {
        let (address_space, stack) = AddressSpace::load(parent, port, image, arguments)?;
        let native = address_space.native.create_thread(
            UserEntry {
                pc: image.entry as u64,
                stack_pointer: stack as u64,
                thread_pointer: 0,
                argument: 0,
            },
            Rights::WAIT | Rights::START | Rights::SET_STATE | Rights::TERMINATE,
        )?;
        native.set_user_state(&machine.initial(image.entry, stack))?;
        let id = native.id()?;
        let files = FileTable::new(output);
        native.start()?;
        Ok(Self {
            supervisor: parent,
            address_space,
            threads: alloc::vec![Thread {
                native,
                native_id: id,
                pending: None
            }],
            files,
            cwd: None,
            machine,
            umask: 0o022,
            status: ExitStatus { code: 0, signal: 0 },
        })
    }

    pub fn trap(&mut self, trap: Trap, files: &fs::Client<'a>) -> Result<(), Error> {
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
        match trap.info.kind {
            TrapInfo::SYSCALL => syscall::dispatch(self, index, trap, files),
            TrapInfo::INTERRUPTED => {
                let state = trap.state()?;
                trap.resume(&state).map_err(|(error, _)| error.into())
            }
            _ => {
                let signal = if trap.info.kind == TrapInfo::PAGE_FAULT
                    && self
                        .address_space
                        .bus_fault(trap.info.address as usize, trap.info.detail)
                {
                    7
                } else if trap.info.kind == TrapInfo::ILLEGAL_INSTRUCTION {
                    4
                } else {
                    11
                };
                self.threads[index]
                    .native
                    .terminate(ExitStatus { code: 0, signal }.encode())?;
                Ok(())
            }
        }
    }

    pub fn poll(&mut self, files: &fs::Client<'a>) -> Result<bool, Error> {
        let mut progress = false;
        let mut index = 0;
        while index < self.threads.len() {
            if self.threads[index].native.state()?.0 == ThreadState::Dead {
                let status = self.threads[index].native.wait(0)?;
                self.status = if status == usize::MAX {
                    ExitStatus { code: 0, signal: 9 }
                } else {
                    ExitStatus::decode(status)
                };
                self.threads.swap_remove(index);
                progress = true;
                continue;
            }
            progress |= syscall::poll(self, index, files)?;
            index += 1;
        }
        Ok(progress)
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<WaitItem<'b>>) {
        for thread in &self.threads {
            items.push(WaitItem::new(&thread.native, EXITED));
            if let Some(pending) = &thread.pending {
                pending.waits(items);
            }
        }
    }

    pub fn status(&self) -> ExitStatus {
        self.status
    }
}
