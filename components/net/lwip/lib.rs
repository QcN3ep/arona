// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! lwIP in a native Arona program, with owned sockets and IPv4 endpoints.
//!
//! Initialize once with device entropy before creating sockets. Each socket has
//! exclusive C API access through mutable methods and can move between threads.
//! I/O is nonblocking; initialization, dispatch, shutdown, and close can sleep.
//! The stack and its protocol thread live until the program exits.

#![no_std]
#![cfg_attr(all(feature = "baremetal", target_os = "none"), feature(thread_local))]

extern crate alloc;

mod ffi;
#[cfg(all(feature = "baremetal", target_os = "none"))]
mod memory;
#[cfg(all(feature = "baremetal", target_os = "none"))]
mod random;
#[cfg(all(feature = "baremetal", target_os = "none"))]
mod sys;

use core::{
    cell::Cell,
    marker::PhantomData,
    sync::atomic::{AtomicU8, Ordering},
};

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
    Other(i32),
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
        // SAFETY: Initialization completed; the bridge returns a fresh C socket.
        let fd = unsafe {
            ffi::arona_lwip_socket(match kind {
                SocketKind::Tcp => 1,
                SocketKind::Udp => 2,
            })
        };
        Socket::from_result(fd, kind)
    }
}

/// Exclusive ownership of a nonblocking C socket.
///
/// Dropping it closes it synchronously and can wait for protocol cleanup. Do not
/// drop or shut down sockets on a dispatcher that must keep serving other work.
/// A close failure panics and terminates the program.
pub struct Socket {
    fd: i32,
    kind: SocketKind,
    _exclusive: PhantomData<Cell<()>>,
}

impl Socket {
    fn from_result(fd: i32, kind: SocketKind) -> Result<Self, Error> {
        if fd < 0 {
            return Err(Error::last());
        }
        Ok(Self {
            fd,
            kind,
            _exclusive: PhantomData,
        })
    }

    pub fn bind(&mut self, endpoint: Endpoint) -> Result<(), Error> {
        // SAFETY: The socket is exclusively borrowed and endpoint is readable.
        status(unsafe { ffi::arona_lwip_bind(self.fd, &endpoint) })
    }

    pub fn listen(&mut self, backlog: u8) -> Result<(), Error> {
        // SAFETY: This method exclusively owns C API access to the socket.
        status(unsafe { ffi::arona_lwip_listen(self.fd, backlog.into()) })
    }

    pub fn connect(&mut self, endpoint: Endpoint) -> Result<(), Error> {
        // SAFETY: The socket is exclusively borrowed and endpoint is readable.
        status(unsafe { ffi::arona_lwip_connect(self.fd, &endpoint) })
    }

    pub fn accept(&mut self) -> Result<(Self, Endpoint), Error> {
        let mut peer = Endpoint::loopback(0);
        // SAFETY: The listener is exclusively borrowed; peer is writable.
        let fd = unsafe { ffi::arona_lwip_accept(self.fd, &mut peer) };
        Ok((Self::from_result(fd, SocketKind::Tcp)?, peer))
    }

    pub fn local_endpoint(&mut self) -> Result<Endpoint, Error> {
        let mut endpoint = Endpoint::loopback(0);
        // SAFETY: The socket is exclusively borrowed; endpoint is writable.
        status(unsafe { ffi::arona_lwip_endpoint(self.fd, &mut endpoint, 0) })?;
        Ok(endpoint)
    }

    pub fn peer_endpoint(&mut self) -> Result<Endpoint, Error> {
        let mut endpoint = Endpoint::loopback(0);
        // SAFETY: The socket is exclusively borrowed; endpoint is writable.
        status(unsafe { ffi::arona_lwip_endpoint(self.fd, &mut endpoint, 1) })?;
        Ok(endpoint)
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        // SAFETY: The bridge copies readable bytes before returning.
        length(unsafe {
            ffi::arona_lwip_send(self.fd, bytes.as_ptr(), bytes.len(), core::ptr::null())
        })
    }

    pub fn send_to(&mut self, bytes: &[u8], endpoint: Endpoint) -> Result<usize, Error> {
        // SAFETY: Both inputs are readable for the call; no pointer is retained.
        length(unsafe { ffi::arona_lwip_send(self.fd, bytes.as_ptr(), bytes.len(), &endpoint) })
    }

    pub fn receive(&mut self, bytes: &mut [u8]) -> Result<usize, Error> {
        // SAFETY: bytes is exclusively writable and not retained by the bridge.
        length(unsafe {
            ffi::arona_lwip_receive(
                self.fd,
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
            ffi::arona_lwip_receive(self.fd, bytes.as_mut_ptr(), bytes.len(), &mut peer)
        })?;
        Ok((count, peer))
    }

    /// Read and clear the pending connection error, as with `SO_ERROR`.
    pub fn take_error(&mut self) -> Result<(), Error> {
        // SAFETY: Only this method accesses the socket during the C call.
        status(unsafe { ffi::arona_lwip_take_error(self.fd) })
    }

    pub fn shutdown(&mut self, direction: Shutdown) -> Result<(), Error> {
        // SAFETY: Shutdown has exclusive access until protocol work completes.
        status(unsafe {
            ffi::arona_lwip_shutdown(
                self.fd,
                match direction {
                    Shutdown::Read => 0,
                    Shutdown::Write => 1,
                    Shutdown::Both => 2,
                },
            )
        })
    }

    pub fn kind(&self) -> SocketKind {
        self.kind
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: The owned descriptor is closed exactly once with exclusive access.
        let result = unsafe { ffi::arona_lwip_close(self.fd) };
        assert_eq!(result, 0, "failed to close an owned lwIP socket");
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
