// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Native threads, lifecycle waits, and access to the calling thread.

use crate::{BorrowedHandle, Error, Handle, HandleInfo, Rights, Task, Thread};
use abi::method::{task, thread};
pub use abi::thread::{Lifecycle, UserEntry};
use core::marker::PhantomData;

#[cfg(all(feature = "program", target_os = "none"))]
mod reaper;
#[cfg(all(feature = "program", target_os = "none"))]
mod spawn;
#[cfg(all(feature = "program", target_os = "none"))]
pub use spawn::{Builder, JoinError, JoinHandle, spawn};

/// Access to the calling thread that cannot move or be shared across threads.
/// Duplicate it to obtain an owned handle with a fixed target.
pub struct CurrentThread {
    _local: PhantomData<*mut ()>,
}

pub fn current() -> CurrentThread {
    CurrentThread {
        _local: PhantomData,
    }
}

impl CurrentThread {
    pub fn info(&self) -> Result<HandleInfo, Error> {
        self.handle().info()
    }

    pub fn try_clone(&self, rights: Rights) -> Result<Handle<Thread>, Error> {
        self.handle().try_clone(rights)
    }

    pub fn weight(&self) -> Result<u32, Error> {
        self.handle().weight()
    }

    pub fn set_weight(&self, weight: u32) -> Result<(), Error> {
        self.handle().set_weight(weight)
    }

    pub fn state(&self) -> Result<(Lifecycle, usize), Error> {
        self.handle().state()
    }

    /// # Safety
    ///
    /// Any buffers encoded in arguments must remain valid for the call, and output buffers must be exclusively borrowed.
    pub(crate) unsafe fn invoke(
        &self,
        method: u64,
        arguments: [u64; 5],
    ) -> Result<[u64; 2], Error> {
        // SAFETY: The caller retains the encoded buffers and their required access.
        unsafe { self.handle().invoke(method, arguments) }
    }

    fn handle(&self) -> BorrowedHandle<'_, Thread> {
        BorrowedHandle::current()
    }
}
pub fn yield_now() -> Result<(), Error> {
    // SAFETY: YIELD takes only handles and scalar values, with no local buffers.
    unsafe { current().invoke(thread::YIELD, [0; 5]) }.map(|_| ())
}
pub fn sleep_until(deadline_ns: u64) -> Result<(), Error> {
    if deadline_ns == 0 {
        return Ok(());
    }
    // SAFETY: The item count is zero, so WAIT_MANY accesses no item buffer.
    match unsafe { current().invoke(thread::WAIT_MANY, [0, 0, deadline_ns, 0, 0]) } {
        Err(Error::Kernel(abi::Error::TimedOut)) => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(Error::Protocol),
    }
}
pub fn now_ns() -> Result<u64, Error> {
    crate::process::clock()
        .ok_or(Error::Kernel(abi::Error::Denied))?
        .now_ns()
}
/// Terminate the current thread without unwinding the stack or running thread-local destructors.
pub fn exit(status: usize) -> ! {
    // SAFETY: TERMINATE takes only handles and scalar values, with no local buffers.
    let _ = unsafe { current().invoke(thread::TERMINATE, [status as u64, 0, 0, 0, 0]) };
    loop {
        core::hint::spin_loop();
    }
}
impl Handle<Task> {
    pub fn create_task(&self, rights: Rights) -> Result<Handle<Task>, Error> {
        // SAFETY: CREATE takes only handles and scalar values, with no local buffers.
        let [raw, _] = unsafe {
            self.borrow()
                .invoke(task::CREATE, [rights.bits(), 0, 0, 0, 0])
        }?;
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
        // SAFETY: The input buffer remains borrowed until the call returns.
        let [raw, _] = unsafe {
            self.borrow().invoke(
                task::CREATE_THREAD,
                [bytes.as_ptr() as u64, rights.bits(), 0, 0, 0],
            )
        }?;
        // SAFETY: A successful CREATE_THREAD delivers an exclusive Thread handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
    pub fn install<T: crate::Object>(
        &self,
        handle: Handle<T>,
        rights: Rights,
    ) -> Result<abi::Handle, (Error, Handle<T>)> {
        // SAFETY: INSTALL takes only handles and scalar values, with no local buffers.
        match unsafe {
            self.borrow().invoke(
                task::INSTALL,
                [handle.borrow().raw().raw(), rights.bits(), 0, 0, 0],
            )
        } {
            Ok([raw, _]) => {
                let _ = handle.into_raw();
                Ok(abi::Handle::from_raw(raw))
            }
            Err(error) => Err((error, handle)),
        }
    }
    pub fn state(&self) -> Result<(Lifecycle, usize), Error> {
        let [state, status] = self.borrow().query(abi::info::task::LIFECYCLE)?;
        lifecycle(state, status)
    }
    pub fn terminate(&self, status: usize) -> Result<(), Error> {
        // SAFETY: TERMINATE takes only handles and scalar values, with no local buffers.
        unsafe {
            self.borrow()
                .invoke(task::TERMINATE, [status as u64, 0, 0, 0, 0])
        }
        .map(|_| ())
    }
    pub fn wait(&self, deadline_ns: u64) -> Result<usize, Error> {
        wait_for_exit(self, deadline_ns)?;
        Ok(self.state()?.1)
    }
}
impl Handle<Thread> {
    pub fn weight(&self) -> Result<u32, Error> {
        self.borrow().weight()
    }
    pub fn set_weight(&self, weight: u32) -> Result<(), Error> {
        self.borrow().set_weight(weight)
    }

    pub fn start(&self) -> Result<(), Error> {
        // SAFETY: START takes only handles and scalar values, with no local buffers.
        unsafe { self.borrow().invoke(thread::START, [0; 5]) }.map(|_| ())
    }
    pub fn state(&self) -> Result<(Lifecycle, usize), Error> {
        self.borrow().state()
    }
    pub fn cpu_time_ns(&self) -> Result<u64, Error> {
        Ok(self.borrow().query(abi::info::thread::CPU_TIME)?[0])
    }
    pub fn terminate(&self, status: usize) -> Result<(), Error> {
        // SAFETY: TERMINATE takes only handles and scalar values, with no local buffers.
        unsafe {
            self.borrow()
                .invoke(thread::TERMINATE, [status as u64, 0, 0, 0, 0])
        }
        .map(|_| ())
    }
    pub fn wait(&self, deadline_ns: u64) -> Result<usize, Error> {
        wait_for_exit(self, deadline_ns)?;
        Ok(self.state()?.1)
    }
}
impl BorrowedHandle<'_, Thread> {
    pub fn weight(&self) -> Result<u32, Error> {
        u32::try_from(self.query(abi::info::thread::SCHEDULING)?[0]).map_err(|_| Error::Protocol)
    }
    pub fn set_weight(&self, weight: u32) -> Result<(), Error> {
        // SAFETY: SET_SCHEDULING takes only handles and scalar values, with no local buffers.
        unsafe { self.invoke(thread::SET_SCHEDULING, [u64::from(weight), 0, 0, 0, 0]) }.map(|_| ())
    }
    pub fn state(&self) -> Result<(Lifecycle, usize), Error> {
        let [state, status] = self.query(abi::info::thread::LIFECYCLE)?;
        lifecycle(state, status)
    }
}

fn lifecycle(state: u64, status: u64) -> Result<(Lifecycle, usize), Error> {
    let state = if state == Lifecycle::Live as u64 {
        if status != 0 {
            return Err(Error::Protocol);
        }
        Lifecycle::Live
    } else if state == Lifecycle::Exited as u64 {
        Lifecycle::Exited
    } else {
        return Err(Error::Protocol);
    };
    Ok((state, status as usize))
}

fn wait_for_exit<T: crate::Object>(target: &Handle<T>, deadline: u64) -> Result<(), Error> {
    let mode = match deadline {
        0 => crate::ipc::WaitMode::Poll,
        u64::MAX => crate::ipc::WaitMode::Forever,
        value => crate::ipc::WaitMode::Until(value),
    };
    crate::ipc::wait_many(
        &[crate::ipc::WaitItem::new(target, crate::ipc::EXITED)],
        mode,
    )
    .map(|_| ())
}
