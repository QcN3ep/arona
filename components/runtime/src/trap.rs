// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

pub use crate::register::UserState;
use crate::{
    Error, Handle, Rights, Task, Thread, TrapContinuation, TrapPort,
    ipc::{self, WaitItem, WaitMode},
};
use abi::method;
pub use abi::trap::{ExceptionClass, TrapInfo};

#[derive(Debug)]
pub struct Trap {
    pub info: TrapInfo,
    continuation: Handle<TrapContinuation>,
}

impl Handle<Task> {
    pub fn trap_port(&self, rights: Rights) -> Result<Handle<TrapPort>, Error> {
        // SAFETY: TRAP_PORT takes only handles and scalar values, with no local buffers.
        let [raw, _] = unsafe {
            self.borrow()
                .invoke(method::task::TRAP_PORT, [rights.bits(), 0, 0, 0, 0])
        }?;
        // SAFETY: A successful TRAP_PORT must deliver an exclusive TrapPort handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }

    pub fn bind_trap(&self, port: &Handle<TrapPort>) -> Result<(), Error> {
        // SAFETY: SET_TRAP takes only handles and scalar values, with no local buffers.
        unsafe {
            self.borrow().invoke(
                method::task::SET_TRAP,
                [port.borrow().raw().raw(), 0, 0, 0, 0],
            )
        }?;
        Ok(())
    }
}

impl Handle<Thread> {
    pub fn id(&self) -> Result<u64, Error> {
        Ok(self.borrow().query(abi::info::thread::IDENTITY)?[0])
    }

    pub fn set_registers(&self, groups: &[crate::register::Write<'_>]) -> Result<(), Error> {
        let records = crate::register::requests(groups)?;
        // SAFETY: The request records and the register slices they reference remain borrowed until the call returns.
        unsafe {
            self.borrow().invoke(
                method::thread::SET_REGISTERS,
                [
                    if records.is_empty() {
                        0
                    } else {
                        records.as_ptr() as u64
                    },
                    records.len() as u64,
                    0,
                    0,
                    0,
                ],
            )
        }?;
        Ok(())
    }
    pub fn set_user_state(&self, state: &UserState) -> Result<(), Error> {
        state.with_groups(crate::register::float_size()?, |groups| {
            self.set_registers(groups)
        })?
    }

    pub fn interrupt(&self) -> Result<(), Error> {
        // SAFETY: INTERRUPT takes only handles and scalar values, with no local buffers.
        unsafe { self.borrow().invoke(method::thread::INTERRUPT, [0; 5]) }?;
        Ok(())
    }
}

impl Handle<TrapPort> {
    pub fn receive(&self, mode: WaitMode) -> Result<Trap, Error> {
        mode.validate()?;
        loop {
            let mut bytes = [0; TrapInfo::SIZE];
            // SAFETY: The output buffer remains exclusively borrowed until the call returns.
            let result = unsafe {
                self.borrow().invoke(
                    method::trap_port::RECEIVE,
                    [bytes.as_mut_ptr() as u64, 0, 0, 0, 0],
                )
            };
            match result {
                Ok([raw, length]) => {
                    // SAFETY: A successful RECEIVE must deliver an exclusive TrapContinuation handle.
                    let continuation = unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) };
                    if length != TrapInfo::SIZE as u64 {
                        return Err(Error::Protocol);
                    }
                    return Ok(Trap {
                        info: TrapInfo::decode(&bytes).ok_or(Error::Protocol)?,
                        continuation,
                    });
                }
                Err(Error::Kernel(abi::Error::WouldBlock)) if !matches!(mode, WaitMode::Poll) => {
                    ipc::wait_many(&[WaitItem::new(self, ipc::READABLE)], mode)?;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Trap {
    pub fn general_state(&self) -> Result<UserState, Error> {
        crate::register::state(
            self.continuation.borrow(),
            abi::info::trap_continuation::REGISTERS,
            false,
        )
    }
    pub fn state(&self) -> Result<UserState, Error> {
        crate::register::state(
            self.continuation.borrow(),
            abi::info::trap_continuation::REGISTERS,
            true,
        )
    }
    pub fn load_floating(&self, state: &mut UserState) -> Result<(), Error> {
        if state.floating.is_none() {
            state.floating = Some(crate::register::read_floating(
                self.continuation.borrow(),
                abi::info::trap_continuation::REGISTERS,
            )?);
        }
        Ok(())
    }
    pub fn resume_registers(
        self,
        groups: &[crate::register::Write<'_>],
    ) -> Result<(), (Error, Self)> {
        let records = match crate::register::requests(groups) {
            Ok(records) => records,
            Err(error) => return Err((error, self)),
        };
        // SAFETY: The request records and the register slices they reference remain borrowed until the call returns.
        match unsafe {
            self.continuation.borrow().invoke(
                method::trap_continuation::RESUME,
                [
                    if records.is_empty() {
                        0
                    } else {
                        records.as_ptr() as u64
                    },
                    records.len() as u64,
                    0,
                    0,
                    0,
                ],
            )
        } {
            Ok(_) => Ok(()),
            Err(error) => Err((error, self)),
        }
    }
    pub fn resume(self, state: &UserState) -> Result<(), (Error, Self)> {
        let float_size = match crate::register::float_size() {
            Ok(size) => size,
            Err(error) => return Err((error, self)),
        };
        let general = state.general().encode();
        let floating = match state
            .floating
            .as_ref()
            .map(|floating| floating.encode(float_size))
            .transpose()
        {
            Ok(floating) => floating,
            Err(error) => return Err((Error::Kernel(error), self)),
        };
        let mut groups = arrayvec::ArrayVec::<crate::register::Write<'_>, 2>::new();
        groups.push(crate::register::Write {
            group: abi::register::GENERAL,
            bytes: &general,
        });
        if let Some((bytes, length)) = &floating {
            groups.push(crate::register::Write {
                group: abi::register::FLOAT,
                bytes: &bytes[..*length],
            });
        }
        self.resume_registers(&groups)
    }
}
