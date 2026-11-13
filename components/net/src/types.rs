// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

pub use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use runtime::{Error as RuntimeError, KernelError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

/// An IP address and a host-order port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub address: IpAddr,
    pub port: u16,
}

impl Endpoint {
    pub const fn loopback(port: u16) -> Self {
        Self {
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
        }
    }

    pub const fn loopback_v6(port: u16) -> Self {
        Self {
            address: IpAddr::V6(Ipv6Addr::LOCALHOST),
            port,
        }
    }

    pub const fn family(self) -> AddressFamily {
        match self.address {
            IpAddr::V4(_) => AddressFamily::Ipv4,
            IpAddr::V6(_) => AddressFamily::Ipv6,
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
    /// IPv6-only mode. The value is one and cannot be disabled.
    Ipv6Only,
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
/// datagram transfer preserves one whole UDP payload, including zero bytes.
/// UDP payload limits are 65,507 bytes for IPv4 and 65,487 bytes for IPv6.
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
