// SPDX-License-Identifier: MPL-2.0

use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use runtime::{
    Handle, Rights, Task, TrapPort,
    ipc::{EXITED, READABLE, WaitItem},
    process::SupervisedTask,
    thread::{ThreadState, UserEntry},
    trap::{Trap, TrapInfo},
};

use crate::{
    Error, ExitStatus,
    elf::{Image, Machine, PAGE_SIZE},
    protocol::Arguments,
    syscall::{self, PendingWrite},
};

pub(crate) struct AddressSpace {
    native: SupervisedTask,
}

pub(crate) struct Thread {
    pub native: Handle<runtime::Thread>,
    pub native_id: u64,
    pub pending: Option<PendingWrite>,
}

pub(crate) struct OpenFileDescription {
    pub output: devmgr::uart::Session,
}

pub(crate) struct Process {
    pub address_space: AddressSpace,
    pub threads: Vec<Thread>,
    pub files: BTreeMap<usize, Arc<OpenFileDescription>>,
    pub machine: Machine,
    status: ExitStatus,
}

impl Process {
    pub fn start(
        parent: &Handle<Task>,
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
        let output = Arc::new(OpenFileDescription { output });
        let files = [(1, Arc::clone(&output)), (2, output)]
            .into_iter()
            .collect();
        native.start()?;
        Ok(Self {
            address_space,
            threads: alloc::vec![Thread {
                native,
                native_id: id,
                pending: None
            }],
            files,
            machine,
            status: ExitStatus { code: 0, signal: 0 },
        })
    }

    pub fn trap(&mut self, trap: Trap) -> Result<(), Error> {
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
            TrapInfo::SYSCALL => syscall::dispatch(self, index, trap),
            TrapInfo::INTERRUPTED => {
                let state = trap.state()?;
                trap.resume(&state).map_err(|(error, _)| error.into())
            }
            _ => {
                let signal = if trap.info.kind == TrapInfo::ILLEGAL_INSTRUCTION {
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

    pub fn poll(&mut self) -> Result<bool, Error> {
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
            progress |= syscall::poll(self.machine, &mut self.threads[index])?;
            index += 1;
        }
        Ok(progress)
    }

    pub fn waits<'a>(&'a self, items: &mut Vec<WaitItem<'a>>) {
        for thread in &self.threads {
            items.push(WaitItem::new(&thread.native, EXITED));
            if let Some(pending) = &thread.pending {
                items.push(WaitItem::new(pending.request.call(), READABLE));
            }
        }
    }

    pub fn status(&self) -> ExitStatus {
        self.status
    }
}

impl AddressSpace {
    fn load(
        parent: &Handle<Task>,
        port: &Handle<TrapPort>,
        image: &Image<'_>,
        arguments: &Arguments,
    ) -> Result<(Self, usize), Error> {
        let address_space = Self {
            native: SupervisedTask::new(parent, port)?,
        };
        for segment in &image.segments {
            let range = segment.pages();
            let count = (range.len() / PAGE_SIZE) as u64;
            let mut frames =
                parent.allocate(count, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
            frames.write(segment.memory.start - range.start, segment.data)?;
            address_space
                .native
                .map(&frames, 0, count, Some(range.start), segment.permissions)?;
        }
        let stack = crate::stack::build(&address_space.native, image, arguments)?;

        Ok((address_space, stack))
    }

    pub fn read(&self, address: usize, output: &mut [u8]) -> Result<(), runtime::Error> {
        self.native.task().read_memory(address, output)
    }
}
