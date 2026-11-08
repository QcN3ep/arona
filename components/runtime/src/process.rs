// SPDX-License-Identifier: MPL-2.0

//! 进程运行环境与子 Task 的构建和启动。

use crate::{
    Channel, Error, FrameAllocation, Handle, Object, Rights, Task, memory::Permissions,
    thread::UserEntry,
};
pub use abi::process::ProcessStart;
use core::ops::Range;

const PAGE_SIZE: usize = 4096;
const CONSTRUCTION_RIGHTS: Rights = Rights::DUPLICATE
    .union(Rights::TRANSFER)
    .union(Rights::WAIT)
    .union(Rights::TERMINATE)
    .union(Rights::CREATE_THREAD)
    .union(Rights::INSTALL_HANDLE)
    .union(Rights::FRAME_ALLOCATE)
    .union(Rights::MAP_MEMORY);

/// 丢弃时终止尚未启动的子 Task。
#[must_use]
pub struct ChildTask {
    task: Option<Handle<Task>>,
    rights: Rights,
}

#[must_use]
pub struct SupervisedTask {
    task: Handle<Task>,
}

impl SupervisedTask {
    pub fn new(parent: &Handle<Task>, port: &Handle<crate::TrapPort>) -> Result<Self, Error> {
        let rights = Rights::WAIT
            .union(Rights::TERMINATE)
            .union(Rights::CREATE_THREAD)
            .union(Rights::FRAME_ALLOCATE)
            .union(Rights::MAP_MEMORY)
            .union(Rights::SET_TRAP)
            .union(Rights::READ_MEMORY)
            .union(Rights::WRITE_MEMORY);
        let child = Self {
            task: parent.create_task(rights)?,
        };
        child.task.bind_trap(port)?;
        Ok(child)
    }

    pub fn task(&self) -> &Handle<Task> {
        &self.task
    }

    pub fn map(
        &self,
        frames: &Handle<FrameAllocation>,
        offset: u64,
        count: u64,
        address: Option<usize>,
        permissions: Permissions,
    ) -> Result<Range<usize>, Error> {
        self.task
            .map(frames, offset, count, address, permissions)?
            .into_range()
    }

    pub fn reserve(
        &self,
        length: usize,
        alignment: usize,
        address: Option<usize>,
    ) -> Result<Range<usize>, Error> {
        self.task.reserve(length, alignment, address)?.into_range()
    }

    pub fn unmap(&self, range: Range<usize>) -> Result<(), Error> {
        let length = range
            .end
            .checked_sub(range.start)
            .ok_or(Error::Kernel(abi::Error::InvalidArgument))?;
        self.task.borrow().invoke(
            abi::method::task::UNMAP,
            [range.start as u64, length as u64, 0, 0, 0],
        )?;
        Ok(())
    }

    pub fn create_thread(
        &self,
        entry: UserEntry,
        rights: Rights,
    ) -> Result<Handle<crate::Thread>, Error> {
        // SAFETY: 目标须为该构建器新建且与调用方隔离的 Task。
        unsafe { self.task.create_thread(entry, rights) }
    }
}

impl Drop for SupervisedTask {
    fn drop(&mut self) {
        let _ = self.task.terminate(0);
    }
}

impl ChildTask {
    /// rights 指定子进程持有的自身 Task 权限。
    pub fn new(parent: &Handle<Task>, rights: Rights) -> Result<Self, Error> {
        if !rights.contains(ProcessStart::REQUIRED_RIGHTS) {
            return Err(Error::Kernel(abi::Error::InvalidArgument));
        }
        Ok(Self {
            task: Some(parent.create_task(rights.union(CONSTRUCTION_RIGHTS))?),
            rights,
        })
    }

    pub fn allocate(&self, frames: u64, rights: Rights) -> Result<Handle<FrameAllocation>, Error> {
        self.task().allocate(frames, rights)
    }

    pub fn map(
        &mut self,
        frames: &Handle<FrameAllocation>,
        offset: u64,
        count: u64,
        address: Option<usize>,
        permissions: Permissions,
    ) -> Result<Range<usize>, Error> {
        self.task()
            .map(frames, offset, count, address, permissions)?
            .into_range()
    }

    pub fn reserve(
        &mut self,
        length: usize,
        alignment: usize,
        address: Option<usize>,
    ) -> Result<Range<usize>, Error> {
        self.task()
            .reserve(length, alignment, address)?
            .into_range()
    }

    pub fn install<T: Object>(
        &mut self,
        handle: Handle<T>,
        rights: Rights,
    ) -> Result<abi::Handle, (Error, Handle<T>)> {
        self.task().install(handle, rights)
    }

    pub fn start(
        mut self,
        entry: usize,
        stack_size: usize,
        bootstrap: Option<(Handle<Channel>, Rights)>,
    ) -> Result<Handle<Task>, Error> {
        let length = stack_size
            .checked_next_multiple_of(PAGE_SIZE)
            .filter(|length| *length != 0)
            .ok_or(Error::Kernel(abi::Error::InvalidArgument))?;
        let task = self.task();
        let mut reservation = task.reserve(
            length
                .checked_add(PAGE_SIZE)
                .ok_or(Error::Kernel(abi::Error::InvalidArgument))?,
            PAGE_SIZE,
            None,
        )?;
        let range = reservation.range().ok_or(Error::Protocol)?;
        reservation.release()?;
        task.reserve(PAGE_SIZE, PAGE_SIZE, Some(range.start))?
            .into_range()?;
        let mut stack = task.allocate(
            (length / PAGE_SIZE) as u64,
            Rights::READ_WRITE.union(Rights::MAP),
        )?;
        let own = task.try_clone(self.rights.union(Rights::TRANSFER))?;
        let own = task.install(own, self.rights).map_err(|(error, _)| error)?;
        let bootstrap = bootstrap
            .map(|(channel, rights)| task.install(channel, rights).map_err(|(error, _)| error))
            .transpose()?;
        stack.write(
            length - ProcessStart::SIZE,
            &ProcessStart {
                task: own,
                bootstrap,
            }
            .encode(),
        )?;
        task.map(
            &stack,
            0,
            (length / PAGE_SIZE) as u64,
            Some(range.start + PAGE_SIZE),
            Permissions::ReadWrite,
        )?
        .into_range()?;
        let argument = range.end - ProcessStart::SIZE;
        // SAFETY: 目标须使用与调用方隔离的地址空间。
        let thread = unsafe {
            task.create_thread(
                UserEntry {
                    pc: entry as u64,
                    stack_pointer: (argument & !15) as u64,
                    thread_pointer: 0,
                    argument: argument as u64,
                },
                Rights::START,
            )?
        };
        thread.start()?;
        drop(reservation);
        Ok(self.task.take().unwrap())
    }

    fn task(&self) -> &Handle<Task> {
        self.task.as_ref().unwrap()
    }
}

impl Drop for ChildTask {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            let _ = task.terminate(1);
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
