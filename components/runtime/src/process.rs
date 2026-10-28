// SPDX-License-Identifier: MPL-2.0

use crate::Rights;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessStart {
    pub task: abi::Handle,
    pub bootstrap: Option<abi::Handle>,
}

impl ProcessStart {
    pub const SIZE: usize = 16;
    pub const REQUIRED_RIGHTS: Rights = Rights::FRAME_ALLOCATE
        .union(Rights::MAP_MEMORY)
        .union(Rights::TERMINATE);

    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        bytes[..8].copy_from_slice(&self.task.raw().to_le_bytes());
        bytes[8..].copy_from_slice(&self.bootstrap.map_or(0, abi::Handle::raw).to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        let task = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let bootstrap = u64::from_le_bytes(bytes[8..].try_into().unwrap());
        Self {
            task: abi::Handle::from_raw(task),
            bootstrap: (bootstrap != 0).then_some(abi::Handle::from_raw(bootstrap)),
        }
    }
}

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
