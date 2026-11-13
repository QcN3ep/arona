// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! One worker owns pending shutdown and close calls. Socket storage also holds
//! its queue link, so submission and final-reference close need no allocation.

use crate::{AddressFamily, Error, Shutdown, Socket, SocketKind, ffi, readiness::Notices, status};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicI32, Ordering};
use runtime::{
    Event, Handle, Rights,
    ipc::{self, WaitItem, WaitMode},
    process, thread,
};

static QUEUE: spin::Mutex<Queue> = spin::Mutex::new(Queue {
    head: None,
    tail: None,
});
static WAKE: spin::Once<Handle<Event>> = spin::Once::new();

pub(super) struct Shared {
    pub(super) fd: AtomicI32,
    pub(super) kind: SocketKind,
    pub(super) family: AddressFamily,
    pub(super) notices: Notices,
    completion: Handle<Event>,
    operation: spin::Mutex<Operation>,
    next: spin::Mutex<Option<Arc<Self>>>,
}

enum Operation {
    Idle,
    Shutdown { direction: Shutdown, close: bool },
    ShutdownDone(Result<(), Error>),
    Close,
    Closed,
}

struct Queue {
    head: Option<Arc<Shared>>,
    tail: Option<Arc<Shared>>,
}

impl Shared {
    pub(super) fn new(kind: SocketKind, family: AddressFamily) -> Result<Arc<Self>, Error> {
        let completion = process::task().event(Rights::WAIT | Rights::SIGNAL)?;
        Arc::try_new(Self {
            fd: AtomicI32::new(-1),
            kind,
            family,
            notices: Notices::new(),
            completion,
            operation: spin::Mutex::new(Operation::Idle),
            next: spin::Mutex::new(None),
        })
        .map_err(|_| Error::NoMemory)
    }

    fn request_close(self: &Arc<Self>) {
        let enqueue = {
            let mut operation = lock(&self.operation);
            match &mut *operation {
                Operation::Shutdown { close, .. } => {
                    *close = true;
                    false
                }
                Operation::Idle | Operation::ShutdownDone(_) => {
                    *operation = Operation::Close;
                    true
                }
                Operation::Close | Operation::Closed => false,
            }
        };
        if enqueue {
            submit(Arc::clone(self));
        }
    }

    fn wait(&self, mode: WaitMode) -> Result<(), Error> {
        ipc::wait_many(&[WaitItem::new(&self.completion, 1)], mode)?;
        Ok(())
    }
}

/// An owned shutdown in progress. Timeouts leave it pending.
/// Dropping it queues close after shutdown instead of returning the socket.
#[must_use]
pub struct PendingShutdown {
    shared: Option<Arc<Shared>>,
}

impl PendingShutdown {
    pub(super) fn new(shared: Arc<Shared>, direction: Shutdown) -> Self {
        *lock(&shared.operation) = Operation::Shutdown {
            direction,
            close: false,
        };
        submit(Arc::clone(&shared));
        Self {
            shared: Some(shared),
        }
    }

    /// Return exclusive socket ownership and the shutdown result together.
    /// Failure of shutdown still returns a socket that the caller can close.
    pub fn try_complete(&mut self) -> Option<(Socket, Result<(), Error>)> {
        let shared = self.shared.as_ref()?;
        let result = {
            let mut operation = lock(&shared.operation);
            let Operation::ShutdownDone(result) = *operation else {
                return None;
            };
            *operation = Operation::Idle;
            result
        };
        Some((Socket::from_shared(self.shared.take().unwrap()), result))
    }

    /// Wake capability for one completion consumer. It is absent after the
    /// socket has been collected. Recheck `try_complete` after waking.
    pub fn event(&self) -> Option<&Handle<Event>> {
        self.shared.as_ref().map(|shared| &shared.completion)
    }

    pub fn wait(&mut self, mode: WaitMode) -> Result<(Socket, Result<(), Error>), Error> {
        loop {
            if let Some(result) = self.try_complete() {
                return Ok(result);
            }
            self.shared
                .as_ref()
                .ok_or(Error::InvalidArgument)?
                .wait(mode)?;
        }
    }

    /// Close after this shutdown, whether or not shutdown has finished yet.
    pub fn close(mut self) -> PendingClose {
        PendingClose::new(self.shared.take().unwrap())
    }
}

impl Drop for PendingShutdown {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            shared.request_close();
        }
    }
}

/// Final close in progress. Dropping this object leaves the worker responsible
/// for cleanup. A successful wait means the C close call has completed.
#[must_use]
pub struct PendingClose {
    shared: Arc<Shared>,
}

impl PendingClose {
    pub(super) fn new(shared: Arc<Shared>) -> Self {
        shared.request_close();
        Self { shared }
    }

    pub fn is_complete(&self) -> bool {
        matches!(*lock(&self.shared.operation), Operation::Closed)
    }

    /// Wake capability for one completion consumer. Recheck `is_complete`
    /// after waking because a preceding shutdown can also signal this Event.
    pub fn event(&self) -> &Handle<Event> {
        &self.shared.completion
    }

    pub fn wait(&mut self, mode: WaitMode) -> Result<(), Error> {
        loop {
            if self.is_complete() {
                return Ok(());
            }
            self.shared.wait(mode)?;
        }
    }
}

pub(super) fn lock<T>(mutex: &spin::Mutex<T>) -> spin::MutexGuard<'_, T> {
    loop {
        if let Some(guard) = mutex.try_lock() {
            return guard;
        }
        thread::yield_now().expect("failed to yield for socket state");
    }
}

fn submit(shared: Arc<Shared>) {
    {
        let mut queue = lock(&QUEUE);
        if let Some(tail) = queue.tail.replace(Arc::clone(&shared)) {
            *lock(&tail.next) = Some(shared);
        } else {
            queue.head = Some(shared);
        }
    }
    WAKE.get()
        .expect("teardown worker not started")
        .signal(1)
        .expect("failed to wake teardown worker");
}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_prepare_teardown() -> i32 {
    match process::task().event(Rights::WAIT | Rights::SIGNAL) {
        Ok(event) => {
            WAKE.call_once(|| event);
            0
        }
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_teardown(_: *mut core::ffi::c_void) {
    loop {
        let shared = {
            let mut queue = lock(&QUEUE);
            let head = queue.head.take();
            if let Some(head) = &head {
                queue.head = lock(&head.next).take();
                if queue.head.is_none() {
                    queue.tail = None;
                }
            }
            head
        };
        if let Some(shared) = shared {
            execute(&shared);
        } else {
            ipc::wait_many(&[WaitItem::new(WAKE.get().unwrap(), 1)], WaitMode::Forever)
                .expect("failed to wait for teardown work");
        }
    }
}

fn execute(shared: &Shared) {
    let direction = match *lock(&shared.operation) {
        Operation::Shutdown { direction, .. } => Some(direction),
        Operation::Close => None,
        _ => unreachable!(),
    };
    if let Some(direction) = direction {
        // SAFETY: Socket ownership was transferred to this operation. No safe
        // socket exists until shutdown completes and its result is collected.
        let result = status(unsafe {
            ffi::arona_lwip_shutdown(
                shared.fd.load(Ordering::Acquire),
                match direction {
                    Shutdown::Read => 0,
                    Shutdown::Write => 1,
                    Shutdown::Both => 2,
                },
            )
        });
        let close = {
            let mut operation = lock(&shared.operation);
            let Operation::Shutdown { close, .. } = *operation else {
                unreachable!()
            };
            *operation = if close {
                Operation::Close
            } else {
                Operation::ShutdownDone(result)
            };
            close
        };
        shared
            .completion
            .signal(1)
            .expect("failed to signal shutdown completion");
        if !close {
            return;
        }
    }
    // SAFETY: The queue owns final C API access. No socket can be recovered.
    let result = unsafe { ffi::arona_lwip_close(shared.fd.load(Ordering::Acquire)) };
    assert_eq!(result, 0, "failed to close an owned lwIP socket");
    shared.notices.close();
    *lock(&shared.operation) = Operation::Closed;
    shared
        .completion
        .signal(1)
        .expect("failed to signal close completion");
}
