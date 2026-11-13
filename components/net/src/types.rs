// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use runtime::{Error as RuntimeError, KernelError};

/// An IPv4 address and a host-order port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}

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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptionName {
    ReuseAddress,
    NoDelay,
    KeepAlive,
    KeepIdle,
    KeepInterval,
    KeepCount,
    ReceiveBuffer,
    SendBuffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    WouldBlock,
    InProgress,
    Busy,
    Canceled,
    InvalidRequest,
    NoMemory,
    Unavailable,
    Closed,
    AddressInUse,
    ConnectionRefused,
    ConnectionReset,
    NotConnected,
    BrokenPipe,
    TimedOut,
    Unsupported,
    Io,
    Runtime(RuntimeError),
}

impl From<RuntimeError> for Error {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::Kernel(KernelError::NoMemory) => Self::NoMemory,
            RuntimeError::Kernel(KernelError::WouldBlock) => Self::WouldBlock,
            RuntimeError::Kernel(KernelError::TimedOut) => Self::TimedOut,
            RuntimeError::Kernel(KernelError::EndOfStream) => Self::Unavailable,
            error => Self::Runtime(error),
        }
    }
}

/// One nonblocking attempt. A stream transfer is limited to 64 KiB, and a
/// datagram transfer preserves one whole IPv4 UDP payload, including zero bytes.
pub enum Request<'a> {
    Bind(Endpoint),
    Listen(u8),
    Connect(Endpoint),
    /// Remove a UDP socket's default peer without closing the socket.
    Disconnect,
    Accept,
    Send(&'a [u8]),
    SendTo(&'a [u8], Endpoint),
    Receive(usize),
    ReceiveDatagram,
    LocalEndpoint,
    PeerEndpoint,
    TakeError,
    GetOption(OptionName),
    SetOption(OptionName, u32),
    Shutdown(Shutdown),
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelOutcome {
    Canceled,
    Committed,
    Pending,
}

/// Non-consuming state. A notification is a hint to take another snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Readiness {
    pub sequence: u64,
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
    pub read_closed: bool,
    pub write_closed: bool,
    pub closed: bool,
}
