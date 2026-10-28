// SPDX-License-Identifier: MPL-2.0

pub use abi::process::ProcessStart;

#[cfg(all(feature = "program", target_os = "none"))]
pub use current::{bootstrap, exit, task};

#[cfg(all(feature = "program", target_os = "none"))]
pub(crate) mod current {
    use super::ProcessStart;
    use crate::{Channel, Error, Handle, Task};
    use spin::{Mutex, Once};

    static TASK: Once<Handle<Task>> = Once::new();
    static BOOTSTRAP: Mutex<Option<Handle<Channel>>> = Mutex::new(None);

    pub fn task() -> &'static Handle<Task> {
        TASK.get().expect("runtime is not initialized")
    }

    pub fn bootstrap() -> Option<Handle<Channel>> {
        BOOTSTRAP.lock().take()
    }

    pub fn exit(status: usize) -> ! {
        if let Some(task) = TASK.get() {
            let _ = task.terminate(status);
        }
        crate::thread::exit(status)
    }

    /// # Safety
    ///
    /// 启动句柄须独占移交给当前进程，task 须指向当前 Task，且初始化须仅执行一次。
    pub(crate) unsafe fn initialize(start: ProcessStart) -> Result<(), Error> {
        // SAFETY: 启动方须移交当前 Task 句柄的独占所有权。
        let task = unsafe { Handle::<Task>::from_raw(start.task) };
        let info = task.info()?;
        if info.kind != crate::ObjectType::Task
            || !info.rights.contains(ProcessStart::REQUIRED_RIGHTS)
        {
            return Err(Error::Kernel(abi::Error::Denied));
        }
        let bootstrap = if let Some(raw) = start.bootstrap {
            // SAFETY: 启动方须移交 Channel 句柄的独占所有权。
            let channel = unsafe { Handle::<Channel>::from_raw(raw) };
            if channel.info()?.kind != crate::ObjectType::Channel {
                return Err(Error::Kernel(abi::Error::WrongType));
            }
            Some(channel)
        } else {
            None
        };
        TASK.call_once(|| task);
        *BOOTSTRAP.lock() = bootstrap;
        Ok(())
    }
}
