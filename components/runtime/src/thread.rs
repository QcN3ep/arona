// SPDX-License-Identifier: MPL-2.0

use crate::{BorrowedHandle, Error, Handle, Rights, Task, Thread};
use abi::method::{task, thread};
pub use abi::thread::{TaskState, ThreadState, UserEntry};

#[cfg(all(feature = "program", target_os = "none"))]
mod reaper;
#[cfg(all(feature = "program", target_os = "none"))]
mod spawn;
#[cfg(all(feature = "program", target_os = "none"))]
pub use spawn::{Builder, JoinError, JoinHandle, spawn};

pub fn current() -> BorrowedHandle<'static, Thread> {
    BorrowedHandle::current()
}
pub fn yield_now() -> Result<(), Error> {
    current().invoke(thread::YIELD, [0; 5]).map(|_| ())
}
pub fn sleep_until(deadline_ns: u64) -> Result<(), Error> {
    current()
        .invoke(thread::SLEEP, [deadline_ns, 0, 0, 0, 0])
        .map(|_| ())
}
pub fn now_ns() -> Result<u64, Error> {
    Ok(current().invoke(thread::NOW, [0; 5])?[0])
}
/// Terminate the current thread without unwinding the stack or running thread-local destructors.
pub fn exit(status: usize) -> ! {
    let _ = current().invoke(thread::EXIT, [status as u64, 0, 0, 0, 0]);
    loop {
        core::hint::spin_loop();
    }
}
impl Handle<Task> {
    pub fn create_task(&self, rights: Rights) -> Result<Handle<Task>, Error> {
        let [raw, _] = self
            .borrow()
            .invoke(task::CREATE, [rights.bits(), 0, 0, 0, 0])?;
        // SAFETY: A successful CREATE delivers an exclusive Task handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
    /// # Safety
    ///
    /// If the target shares an address space with the caller, the code and storage referenced by entry must remain alive,
    /// and the thread must have an exclusive stack and satisfy the entry's calling convention.
    pub unsafe fn create_thread(
        &self,
        entry: UserEntry,
        rights: Rights,
    ) -> Result<Handle<Thread>, Error> {
        let bytes = entry.encode();
        let [raw, _] = self.borrow().invoke(
            task::CREATE_THREAD,
            [bytes.as_ptr() as u64, rights.bits(), 0, 0, 0],
        )?;
        // SAFETY: A successful CREATE_THREAD delivers an exclusive Thread handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
    pub fn install<T: crate::Object>(
        &self,
        handle: Handle<T>,
        rights: Rights,
    ) -> Result<abi::Handle, (Error, Handle<T>)> {
        match self.borrow().invoke(
            task::INSTALL,
            [handle.borrow().raw().raw(), rights.bits(), 0, 0, 0],
        ) {
            Ok([raw, _]) => {
                let _ = handle.into_raw();
                Ok(abi::Handle::from_raw(raw))
            }
            Err(error) => Err((error, handle)),
        }
    }
    pub fn state(&self) -> Result<(TaskState, usize), Error> {
        let [state, status] = self.borrow().invoke(task::STATE, [0; 5])?;
        Ok((
            TaskState::from_raw(state).ok_or(Error::Protocol)?,
            status as usize,
        ))
    }
    pub fn terminate(&self, status: usize) -> Result<(), Error> {
        self.borrow()
            .invoke(task::TERMINATE, [status as u64, 0, 0, 0, 0])
            .map(|_| ())
    }
    pub fn wait(&self, deadline_ns: u64) -> Result<usize, Error> {
        Ok(self
            .borrow()
            .invoke(task::WAIT, [deadline_ns, 0, 0, 0, 0])?[0] as usize)
    }
}
impl Handle<Thread> {
    pub fn start(&self) -> Result<(), Error> {
        self.borrow().invoke(thread::START, [0; 5]).map(|_| ())
    }
    pub fn state(&self) -> Result<(ThreadState, usize), Error> {
        self.borrow().state()
    }
    pub fn terminate(&self, status: usize) -> Result<(), Error> {
        self.borrow()
            .invoke(thread::TERMINATE, [status as u64, 0, 0, 0, 0])
            .map(|_| ())
    }
    pub fn wait(&self, deadline_ns: u64) -> Result<usize, Error> {
        Ok(self
            .borrow()
            .invoke(thread::WAIT, [deadline_ns, 0, 0, 0, 0])?[0] as usize)
    }
}
impl BorrowedHandle<'_, Thread> {
    pub fn state(&self) -> Result<(ThreadState, usize), Error> {
        let [state, status] = self.invoke(thread::STATE, [0; 5])?;
        Ok((
            ThreadState::from_raw(state).ok_or(Error::Protocol)?,
            status as usize,
        ))
    }
}
