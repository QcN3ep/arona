// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! lwIP in a native Arona program, with owned sockets and IPv4 endpoints.
//!
//! Available with `baremetal` on native `target_os = "none"` targets.
//! Initialize once with device entropy before creating sockets. Each socket has
//! exclusive C API access through mutable methods and can move between threads.
//! I/O is nonblocking; initialization and local protocol dispatch can sleep.
//! Shutdown and close run on a shared worker with exclusive socket ownership.
//! The stack and its protocol thread live until the program exits.

#![no_std]
#![cfg(all(feature = "baremetal", target_os = "none"))]
#![feature(allocator_api, thread_local)]

extern crate alloc;

mod ffi;
mod readiness;
mod teardown;
pub use readiness::{Readiness, Subscription};
pub use teardown::{PendingClose, PendingShutdown};
mod memory;
mod random;
mod sys;

use alloc::sync::Arc;
use core::{
    cell::Cell,
    marker::PhantomData,
    sync::atomic::{AtomicU8, Ordering},
};
use teardown::Shared;

static STATE: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    WouldBlock,
    InProgress,
    AddressInUse,
    ConnectionRefused,
    ConnectionReset,
    NotConnected,
    NoMemory,
    InvalidArgument,
    AlreadyInitialized,
    Runtime(runtime::Error),
    Other(i32),
}

impl From<runtime::Error> for Error {
    fn from(error: runtime::Error) -> Self {
        match error {
            runtime::Error::Kernel(runtime::KernelError::NoMemory) => Self::NoMemory,
            runtime::Error::Kernel(runtime::KernelError::WouldBlock) => Self::WouldBlock,
            error => Self::Runtime(error),
        }
    }
}

impl Error {
    fn last() -> Self {
        // SAFETY: The port returns the calling thread's errno value.
        match unsafe { ffi::arona_lwip_last_error() } {
            11 => Self::WouldBlock,
            115 | 114 => Self::InProgress,
            98 => Self::AddressInUse,
            111 => Self::ConnectionRefused,
            104 => Self::ConnectionReset,
            107 => Self::NotConnected,
            12 | 105 => Self::NoMemory,
            22 => Self::InvalidArgument,
            error => Self::Other(error),
        }
    }
}

/// An IPv4 address and a host-order port, independent of C socket layouts.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}

const _: () = assert!(core::mem::size_of::<Endpoint>() == 6);
const _: () = assert!(core::mem::align_of::<Endpoint>() == 2);

impl Endpoint {
    pub const fn loopback(port: u16) -> Self {
        Self {
            address: [127, 0, 0, 1],
            port,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketKind {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shutdown {
    Read,
    Write,
    Both,
}

/// Access to the single stack instance in this program.
pub struct Stack {
    _private: (),
}

impl Stack {
    /// Seed the CSPRNG and start the protocol thread, waiting for initialization.
    ///
    /// `seed` must contain 32 unpredictable bytes obtained from the device RNG.
    /// Initialization is attempted once; after a startup failure, restart the
    /// program. No interface can be exposed before initialization succeeds.
    pub fn initialize(seed: [u8; 32]) -> Result<Self, Error> {
        STATE
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::AlreadyInitialized)?;
        // SAFETY: seed is readable for the call, and this thread owns startup.
        let result = unsafe { ffi::arona_lwip_start(seed.as_ptr()) };
        if result != 0 {
            return Err(Error::NoMemory);
        }
        STATE.store(2, Ordering::Release);
        Ok(Self { _private: () })
    }

    pub fn socket(&self, kind: SocketKind) -> Result<Socket, Error> {
        let shared = Shared::new(kind)?;
        // SAFETY: Initialization completed; the bridge returns a fresh C socket.
        let fd = unsafe {
            ffi::arona_lwip_socket(match kind {
                SocketKind::Tcp => 1,
                SocketKind::Udp => 2,
            })
        };
        Socket::from_result(fd, shared)
    }
}

/// Exclusive ownership of a nonblocking C socket.
///
/// Shutdown transfers ownership to a worker and returns it on completion.
/// Dropping a socket queues its close without waiting for protocol cleanup.
/// Use `close` when cleanup completion is needed. A close failure terminates
/// the program because the descriptor's ownership cannot be recovered.
pub struct Socket {
    shared: Option<Arc<Shared>>,
    _exclusive: PhantomData<Cell<()>>,
}

impl Socket {
    fn from_result(fd: i32, mut shared: Arc<Shared>) -> Result<Self, Error> {
        if fd < 0 {
            return Err(Error::last());
        }
        Arc::get_mut(&mut shared).unwrap().fd = fd;
        // SAFETY: fd is exclusively owned. C retains this Arc until its final
        // entry reference leaves, including callbacks running during close.
        unsafe { ffi::arona_lwip_observe(fd, Arc::into_raw(Arc::clone(&shared)).cast()) };
        Ok(Self::from_shared(shared))
    }

    fn from_shared(shared: Arc<Shared>) -> Self {
        Self {
            shared: Some(shared),
            _exclusive: PhantomData,
        }
    }

    fn shared(&self) -> &Arc<Shared> {
        self.shared.as_ref().unwrap()
    }

    /// Observe readiness without consuming data or clearing `SO_ERROR`.
    /// Readability includes accept, EOF, and errors that receive will report.
    pub fn readiness(&self) -> Readiness {
        self.shared().notices.snapshot()
    }

    /// Register an independent waiter, then obtain its initial snapshot.
    pub fn subscribe(&self) -> Result<Subscription, Error> {
        Subscription::new(Arc::clone(self.shared()))
    }

    pub fn bind(&mut self, endpoint: Endpoint) -> Result<(), Error> {
        // SAFETY: The socket is exclusively borrowed and endpoint is readable.
        status(unsafe { ffi::arona_lwip_bind(self.shared().fd, &endpoint) })
    }

    pub fn listen(&mut self, backlog: u8) -> Result<(), Error> {
        // SAFETY: This method exclusively owns C API access to the socket.
        status(unsafe { ffi::arona_lwip_listen(self.shared().fd, backlog.into()) })
    }

    pub fn connect(&mut self, endpoint: Endpoint) -> Result<(), Error> {
        // SAFETY: The socket is exclusively borrowed and endpoint is readable.
        status(unsafe { ffi::arona_lwip_connect(self.shared().fd, &endpoint) })
    }

    pub fn accept(&mut self) -> Result<(Self, Endpoint), Error> {
        let shared = Shared::new(SocketKind::Tcp)?;
        let mut peer = Endpoint::loopback(0);
        // SAFETY: The listener is exclusively borrowed; peer is writable.
        let fd = unsafe { ffi::arona_lwip_accept(self.shared().fd, &mut peer) };
        Ok((Self::from_result(fd, shared)?, peer))
    }

    pub fn local_endpoint(&mut self) -> Result<Endpoint, Error> {
        let mut endpoint = Endpoint::loopback(0);
        // SAFETY: The socket is exclusively borrowed; endpoint is writable.
        status(unsafe { ffi::arona_lwip_endpoint(self.shared().fd, &mut endpoint, 0) })?;
        Ok(endpoint)
    }

    pub fn peer_endpoint(&mut self) -> Result<Endpoint, Error> {
        let mut endpoint = Endpoint::loopback(0);
        // SAFETY: The socket is exclusively borrowed; endpoint is writable.
        status(unsafe { ffi::arona_lwip_endpoint(self.shared().fd, &mut endpoint, 1) })?;
        Ok(endpoint)
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        // SAFETY: The bridge copies readable bytes before returning.
        length(unsafe {
            ffi::arona_lwip_send(
                self.shared().fd,
                bytes.as_ptr(),
                bytes.len(),
                core::ptr::null(),
            )
        })
    }

    pub fn send_to(&mut self, bytes: &[u8], endpoint: Endpoint) -> Result<usize, Error> {
        // SAFETY: Both inputs are readable for the call; no pointer is retained.
        length(unsafe {
            ffi::arona_lwip_send(self.shared().fd, bytes.as_ptr(), bytes.len(), &endpoint)
        })
    }

    pub fn receive(&mut self, bytes: &mut [u8]) -> Result<usize, Error> {
        // SAFETY: bytes is exclusively writable and not retained by the bridge.
        length(unsafe {
            ffi::arona_lwip_receive(
                self.shared().fd,
                bytes.as_mut_ptr(),
                bytes.len(),
                core::ptr::null_mut(),
            )
        })
    }

    pub fn receive_from(&mut self, bytes: &mut [u8]) -> Result<(usize, Endpoint), Error> {
        let mut peer = Endpoint::loopback(0);
        // SAFETY: The socket and both output regions are exclusively borrowed.
        let count = length(unsafe {
            ffi::arona_lwip_receive(self.shared().fd, bytes.as_mut_ptr(), bytes.len(), &mut peer)
        })?;
        Ok((count, peer))
    }

    /// Read and clear the pending connection error, as with `SO_ERROR`.
    pub fn take_error(&mut self) -> Result<(), Error> {
        // SAFETY: Only this method accesses the socket during the C call.
        status(unsafe { ffi::arona_lwip_take_error(self.shared().fd) })
    }

    /// Transfer ownership to the teardown worker until shutdown completes.
    /// Collect the returned operation to regain access to the socket.
    pub fn shutdown(mut self, direction: Shutdown) -> PendingShutdown {
        PendingShutdown::new(self.shared.take().unwrap(), direction)
    }

    /// Queue final close. Dropping the returned operation leaves it running.
    pub fn close(mut self) -> PendingClose {
        PendingClose::new(self.shared.take().unwrap())
    }

    pub fn kind(&self) -> SocketKind {
        self.shared().kind
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            drop(PendingClose::new(shared));
        }
    }
}

fn status(value: i32) -> Result<(), Error> {
    if value < 0 {
        Err(Error::last())
    } else {
        Ok(())
    }
}

fn length(value: isize) -> Result<usize, Error> {
    if value < 0 {
        Err(Error::last())
    } else {
        Ok(value as usize)
    }
}
