// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{ThreadState, UserEntry, reaper};
use crate::{
    Error, Handle, Rights, Thread,
    memory::{Mapping, Permissions, Reservation},
    process,
    tls::TlsBlock,
};
use alloc::boxed::Box;
use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

const PAGE_SIZE: usize = 4096;
const DEFAULT_STACK_SIZE: usize = 2 * 1024 * 1024;

pub struct Builder {
    stack_size: usize,
}

/// Dropping the handle detaches the thread; resources after exit are reclaimed by the runtime.
#[must_use]
pub struct JoinHandle<T: Send + 'static> {
    resources: Option<Box<Resources<T>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinError {
    Exited(usize),
    Runtime(Error),
}

pub(super) struct Resources<T: Send + 'static> {
    thread: Handle<Thread>,
    packet: Box<Packet<T>>,
    _stack: Stack,
    _tls: TlsBlock,
}

struct Stack {
    mapping: Mapping<'static>,
    _guard: Reservation<'static>,
}

struct Job<T> {
    function: Box<dyn FnOnce() -> T + Send>,
}

struct Packet<T> {
    job: AtomicPtr<Job<T>>,
    result: UnsafeCell<MaybeUninit<T>>,
    returned: AtomicBool,
}

pub fn spawn<F, T>(function: F) -> Result<JoinHandle<T>, Error>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(function)
}

impl Builder {
    pub const fn new() -> Self {
        Self {
            stack_size: DEFAULT_STACK_SIZE,
        }
    }

    pub const fn stack_size(mut self, bytes: usize) -> Self {
        self.stack_size = bytes;
        self
    }

    pub fn spawn<F, T>(self, function: F) -> Result<JoinHandle<T>, Error>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let size = stack_size(self.stack_size)?;
        reaper::initialize()?;
        let resources = Resources::start(size, function)?;
        Ok(JoinHandle {
            resources: Some(resources),
        })
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> JoinHandle<T> {
    pub fn thread(&self) -> &Handle<Thread> {
        self.resources.as_ref().unwrap().thread()
    }

    pub fn is_finished(&self) -> Result<bool, Error> {
        Ok(self.thread().state()?.0 == ThreadState::Dead)
    }

    pub fn join(mut self) -> Result<T, JoinError> {
        let status = self.thread().wait(u64::MAX).map_err(JoinError::Runtime)?;
        let mut resources = self.resources.take().unwrap();
        if status != 0 {
            return Err(JoinError::Exited(status));
        }
        resources
            .packet
            .take_result()
            .ok_or(JoinError::Exited(status))
    }
}

impl<T: Send + 'static> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if let Some(resources) = self.resources.take() {
            reaper::retire(resources);
        }
    }
}

impl<T: Send + 'static> Resources<T> {
    pub(super) fn start(
        stack_size: usize,
        function: impl FnOnce() -> T + Send + 'static,
    ) -> Result<Box<Self>, Error> {
        let stack = Stack::new(stack_size)?;
        let tls = TlsBlock::new().map_err(|_| Error::Kernel(abi::Error::NoMemory))?;
        let packet = Box::new(Packet {
            job: AtomicPtr::new(ptr::null_mut()),
            result: UnsafeCell::new(MaybeUninit::uninit()),
            returned: AtomicBool::new(false),
        });
        packet.job.store(
            Box::into_raw(Box::new(Job {
                function: Box::new(function),
            })),
            Ordering::Release,
        );
        // SAFETY: The stack, TLS, and packet must be delivered exclusively and retained until the kernel confirms the thread has exited.
        let thread = unsafe {
            process::task().create_thread(
                UserEntry {
                    pc: enter::<T> as *const () as u64,
                    stack_pointer: stack.mapping.range().unwrap().end as u64,
                    thread_pointer: tls.thread_pointer() as u64,
                    argument: ptr::from_ref(&*packet) as u64,
                },
                Rights::START | Rights::WAIT | Rights::TERMINATE,
            )?
        };
        let resources = Box::new(Self {
            thread,
            packet,
            _stack: stack,
            _tls: tls,
        });
        if let Err(error) = resources.thread.start() {
            resources
                .thread
                .terminate(1)
                .and_then(|_| resources.thread.wait(u64::MAX))
                .unwrap_or_else(|_| process::exit(1));
            return Err(error);
        }
        Ok(resources)
    }

    pub(super) fn thread(&self) -> &Handle<Thread> {
        &self.thread
    }
}

impl Stack {
    fn new(size: usize) -> Result<Self, Error> {
        let task = process::task();
        let count = (size / PAGE_SIZE) as u64;
        let frames = task.allocate(count, Rights::MAP | Rights::READ_WRITE)?;
        loop {
            let mut candidate = task.reserve(size + PAGE_SIZE, PAGE_SIZE, None)?;
            let start = candidate.range().unwrap().start;
            candidate.release()?;
            let result = (|| {
                let guard = task.reserve(PAGE_SIZE, PAGE_SIZE, Some(start))?;
                let mapping = task.map(
                    &frames,
                    0,
                    count,
                    Some(start + PAGE_SIZE),
                    Permissions::ReadWrite,
                )?;
                Ok(Self {
                    mapping,
                    _guard: guard,
                })
            })();
            match result {
                Err(Error::Kernel(abi::Error::Busy)) => super::yield_now()?,
                result => return result,
            }
        }
    }
}

fn stack_size(size: usize) -> Result<usize, Error> {
    if size < 16 * 1024 {
        return Err(Error::Kernel(abi::Error::InvalidArgument));
    }
    size.checked_add(PAGE_SIZE - 1)
        .map(|size| size & !(PAGE_SIZE - 1))
        .filter(|size| size.checked_add(PAGE_SIZE).is_some())
        .ok_or(Error::Kernel(abi::Error::InvalidArgument))
}

/// # Safety
///
/// argument must point to a Packet<T> delivered exclusively to the current thread, and its resources must be retained until the kernel confirms the thread has exited.
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
unsafe extern "C" fn enter<T: Send + 'static>(_: *const Packet<T>) -> ! {
    core::arch::naked_asm!(
        ".option push",
        ".option norelax",
        "lla gp, __global_pointer$",
        ".option pop",
        "tail {run}",
        run = sym run::<T>,
    );
}

/// # Safety
///
/// argument must satisfy run's thread startup convention.
#[cfg(target_arch = "loongarch64")]
#[unsafe(naked)]
unsafe extern "C" fn enter<T: Send + 'static>(_: *const Packet<T>) -> ! {
    core::arch::naked_asm!("b {run}", run = sym run::<T>);
}

/// # Safety
///
/// argument must point to a Packet<T> exclusive to this thread, and the stack and TLS must be ready and alive until the thread exits.
unsafe extern "C" fn run<T: Send + 'static>(argument: *const Packet<T>) -> ! {
    // SAFETY: The startup argument must point to a packet that is alive for this thread.
    let packet = unsafe { &*argument };
    let job = packet.job.swap(ptr::null_mut(), Ordering::Acquire);
    // SAFETY: job must be the Box<Job<T>> taken only once by this thread.
    let Job { function } = *unsafe { Box::from_raw(job) };
    let result = function();
    crate::tls::destroy();
    // SAFETY: Only the current thread writes the result; the reader must wait for the thread to exit and obtain returned.
    unsafe { (*packet.result.get()).write(result) };
    packet.returned.store(true, Ordering::Release);
    super::exit(0)
}

impl<T> Packet<T> {
    fn take_result(&mut self) -> Option<T> {
        if !self.returned.load(Ordering::Acquire) {
            return None;
        }
        *self.returned.get_mut() = false;
        // SAFETY: returned must publish the complete result, the thread must have exited, and the result must be taken only once.
        Some(unsafe { self.result.get_mut().assume_init_read() })
    }
}

impl<T> Drop for Packet<T> {
    fn drop(&mut self) {
        let job = *self.job.get_mut();
        if !job.is_null() {
            // SAFETY: A non-null job must not yet have been taken by the thread, and the thread must have stopped or not have started.
            drop(unsafe { Box::from_raw(job) });
        }
        drop(self.take_result());
    }
}
