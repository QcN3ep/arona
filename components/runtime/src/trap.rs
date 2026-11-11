// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{
    Error, Handle, Rights, Task, Thread, TrapContinuation, TrapPort,
    ipc::{self, WaitItem, WaitMode},
};
use abi::method;
pub use abi::trap::{ExceptionClass, TrapInfo, UserState};

#[derive(Debug)]
pub struct Trap {
    pub info: TrapInfo,
    continuation: Handle<TrapContinuation>,
}

impl Handle<Task> {
    pub fn trap_port(&self, rights: Rights) -> Result<Handle<TrapPort>, Error> {
        let [raw, _] = self
            .borrow()
            .invoke(method::task::TRAP_PORT, [rights.bits(), 0, 0, 0, 0])?;
        // SAFETY: A successful TRAP_PORT must deliver an exclusive TrapPort handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }

    pub fn bind_trap(&self, port: &Handle<TrapPort>) -> Result<(), Error> {
        self.borrow().invoke(
            method::task::SET_TRAP,
            [port.borrow().raw().raw(), 0, 0, 0, 0],
        )?;
        Ok(())
    }
}

impl Handle<Thread> {
    pub fn id(&self) -> Result<u64, Error> {
        Ok(self.borrow().invoke(method::thread::ID, [0; 5])?[0])
    }

    pub fn set_user_state(&self, state: &UserState) -> Result<(), Error> {
        let bytes = state.encode();
        self.borrow().invoke(
            method::thread::SET_STATE,
            [bytes.as_ptr() as u64, 0, 0, 0, 0],
        )?;
        Ok(())
    }

    pub fn interrupt(&self) -> Result<(), Error> {
        self.borrow().invoke(method::thread::INTERRUPT, [0; 5])?;
        Ok(())
    }
}

impl Handle<TrapPort> {
    pub fn receive(&self, mode: WaitMode) -> Result<Trap, Error> {
        loop {
            let mut bytes = [0; TrapInfo::SIZE];
            let result = self.borrow().invoke(
                method::trap_port::RECEIVE,
                [bytes.as_mut_ptr() as u64, 0, 0, 0, 0],
            );
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
    pub fn state(&self) -> Result<UserState, Error> {
        let mut bytes = [0; UserState::SIZE];
        self.continuation.borrow().invoke(
            method::trap_continuation::STATE,
            [bytes.as_mut_ptr() as u64, 0, 0, 0, 0],
        )?;
        Ok(UserState::decode(&bytes))
    }

    pub fn resume(self, state: &UserState) -> Result<(), (Error, Self)> {
        let bytes = state.encode();
        match self.continuation.borrow().invoke(
            method::trap_continuation::RESUME,
            [bytes.as_ptr() as u64, 0, 0, 0, 0],
        ) {
            Ok(_) => Ok(()),
            Err(error) => Err((error, self)),
        }
    }
}
