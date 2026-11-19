// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{BorrowedHandle, Error, Handle, Rights, Task, Thread};
use abi::method::{task, thread};
pub use abi::thread::{
    Lifecycle, PRIORITY_DEFAULT, PRIORITY_MAX, PRIORITY_MIN, SchedulingPolicy, UserEntry,
};

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
    if deadline_ns == 0 {
        return Ok(());
    }
    match current().invoke(thread::WAIT_MANY, [0, 0, deadline_ns, 0, 0]) {
        Err(Error::Kernel(abi::Error::TimedOut)) => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(Error::Protocol),
    }
}
pub fn now_ns() -> Result<u64, Error> {
    Ok(current().invoke(thread::NOW, [0; 5])?[0])
}
/// Returns the current thread's nice value.
pub fn priority() -> Result<i32, Error> {
    current().priority()
}
/// Changes the current thread scheduling priority within the priority range.
pub fn set_priority(priority: i32) -> Result<(), Error> {
    current().set_priority(priority)
}
/// Terminate the current thread without unwinding the stack or running thread-local destructors.
pub fn exit(status: usize) -> ! {
    let _ = current().invoke(thread::TERMINATE, [status as u64, 0, 0, 0, 0]);
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
    pub fn state(&self) -> Result<(Lifecycle, usize), Error> {
        let [state, status] = self.borrow().query(abi::info::task::LIFECYCLE)?;
        lifecycle(state, status)
    }
    pub fn terminate(&self, status: usize) -> Result<(), Error> {
        self.borrow()
            .invoke(task::TERMINATE, [status as u64, 0, 0, 0, 0])
            .map(|_| ())
    }
    pub fn wait(&self, deadline_ns: u64) -> Result<usize, Error> {
        wait_for_exit(self, deadline_ns)?;
        Ok(self.state()?.1)
    }
}
impl Handle<Thread> {
    pub fn scheduling(&self) -> Result<abi::thread::SchedulingPolicy, Error> {
        let value = self.borrow().invoke(thread::GET_SCHEDULING, [0; 5])?[0];
        abi::thread::SchedulingPolicy::from_raw(value).ok_or(Error::Protocol)
    }
    pub fn set_scheduling(&self, policy: abi::thread::SchedulingPolicy) -> Result<(), Error> {
        self.borrow()
            .invoke(thread::SET_SCHEDULING, [policy as u64, 0, 0, 0, 0])
            .map(|_| ())
    }

    pub fn start(&self) -> Result<(), Error> {
        self.borrow().invoke(thread::START, [0; 5]).map(|_| ())
    }
    pub fn priority(&self) -> Result<i32, Error> {
        self.borrow().priority()
    }
    pub fn set_priority(&self, priority: i32) -> Result<(), Error> {
        self.borrow().set_priority(priority)
    }
    pub fn state(&self) -> Result<(Lifecycle, usize), Error> {
        self.borrow().state()
    }
    pub fn cpu_time_ns(&self) -> Result<u64, Error> {
        Ok(self.borrow().query(abi::info::thread::CPU_TIME)?[0])
    }
    pub fn terminate(&self, status: usize) -> Result<(), Error> {
        self.borrow()
            .invoke(thread::TERMINATE, [status as u64, 0, 0, 0, 0])
            .map(|_| ())
    }
    pub fn wait(&self, deadline_ns: u64) -> Result<usize, Error> {
        wait_for_exit(self, deadline_ns)?;
        Ok(self.state()?.1)
    }
}
impl BorrowedHandle<'_, Thread> {
    pub fn priority(&self) -> Result<i32, Error> {
        Ok(self.invoke(thread::GET_PRIORITY, [0; 5])?[0] as u32 as i32)
    }
    pub fn set_priority(&self, priority: i32) -> Result<(), Error> {
        self.invoke(thread::SET_PRIORITY, [priority as i64 as u64, 0, 0, 0, 0])
            .map(|_| ())
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
