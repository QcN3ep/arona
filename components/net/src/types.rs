// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

pub use alloc::vec::Vec;
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

/// State shown by `/proc/net/tcp` and `/proc/net/udp`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketState {
    Established,
    Listen,
    Close,
    Unconnected,
}

/// One live socket as observed by the network service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketInfo {
    pub kind: SocketKind,
    pub family: AddressFamily,
    pub local: Endpoint,
    pub peer: Option<Endpoint>,
    pub state: SocketState,
    pub inode: u64,
    pub uid: u32,
}

/// Fixed-width wire record used for socket table snapshots.
pub const SOCKET_RECORD: usize = 56;

/// One network interface as observed by the network service.
///
/// `flags` uses the bits in [`DeviceInfo::UP`], [`DeviceInfo::LINK_UP`], and
/// [`DeviceInfo::BROADCAST`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    pub name: [u8; 2],
    pub flags: u8,
    pub ipv4: Option<Ipv4Addr>,
    pub netmask: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub mtu: u16,
}

impl DeviceInfo {
    pub const UP: u8 = 1;
    pub const LINK_UP: u8 = 2;
    pub const BROADCAST: u8 = 4;
}

/// Fixed-width wire record used for interface snapshots.
pub const DEVICE_RECORD: usize = 20;

/// A snapshot of the sockets and interfaces owned by the network service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Table {
    pub sockets: Vec<SocketInfo>,
    pub devices: Vec<DeviceInfo>,
}

impl DeviceInfo {
    pub fn encode(self) -> [u8; DEVICE_RECORD] {
        let mut output = [0; DEVICE_RECORD];
        output[..2].copy_from_slice(&self.name);
        output[2] = self.flags;
        output[3] = u8::from(self.ipv4.is_some());
        if let Some(ipv4) = self.ipv4 {
            output[4..8].copy_from_slice(&ipv4.octets());
        }
        output[8..12].copy_from_slice(&self.netmask.octets());
        output[12..16].copy_from_slice(&self.gateway.octets());
        output[16..18].copy_from_slice(&self.mtu.to_le_bytes());
        output
    }

    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != DEVICE_RECORD {
            return None;
        }
        let name = <[u8; 2]>::try_from(&bytes[..2]).ok()?;
        let flags = bytes[2];
        let ipv4 = if bytes[3] & 1 != 0 {
            Some(Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[4..8]).ok()?))
        } else {
            None
        };
        let netmask = Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[8..12]).ok()?);
        let gateway = Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[12..16]).ok()?);
        Some(Self {
            name,
            flags,
            ipv4,
            netmask,
            gateway,
            mtu: u16::from_le_bytes(bytes[16..18].try_into().ok()?),
        })
    }
}

impl SocketInfo {
    pub fn encode(self) -> [u8; SOCKET_RECORD] {
        let mut output = [0; SOCKET_RECORD];
        output[0] = match self.kind {
            SocketKind::Tcp => 0,
            SocketKind::Udp => 1,
        };
        output[1] = match self.family {
            AddressFamily::Ipv4 => 4,
            AddressFamily::Ipv6 => 6,
        };
        output[2] = match self.state {
            SocketState::Established => 0,
            SocketState::Listen => 1,
            SocketState::Close => 2,
            SocketState::Unconnected => 3,
        };
        output[3] = u8::from(self.peer.is_some());
        output[4..6].copy_from_slice(&self.local.port.to_le_bytes());
        output[6..8].copy_from_slice(&self.peer.map_or(0, |peer| peer.port).to_le_bytes());
        write_address(&mut output[8..24], self.local.address);
        if let Some(peer) = self.peer {
            write_address(&mut output[24..40], peer.address);
        }
        output[40..48].copy_from_slice(&self.inode.to_le_bytes());
        output[48..52].copy_from_slice(&self.uid.to_le_bytes());
        output
    }

    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != SOCKET_RECORD {
            return None;
        }
        let kind = match bytes[0] {
            0 => SocketKind::Tcp,
            1 => SocketKind::Udp,
            _ => return None,
        };
        let family = match bytes[1] {
            4 => AddressFamily::Ipv4,
            6 => AddressFamily::Ipv6,
            _ => return None,
        };
        let state = match bytes[2] {
            0 => SocketState::Established,
            1 => SocketState::Listen,
            2 => SocketState::Close,
            3 => SocketState::Unconnected,
            _ => return None,
        };
        let local = Endpoint {
            address: read_address(&bytes[8..24], family)?,
            port: u16::from_le_bytes(bytes[4..6].try_into().ok()?),
        };
        let peer = if bytes[3] & 1 != 0 {
            Some(Endpoint {
                address: read_address(&bytes[24..40], family)?,
                port: u16::from_le_bytes(bytes[6..8].try_into().ok()?),
            })
        } else {
            None
        };
        Some(Self {
            kind,
            family,
            local,
            peer,
            state,
            inode: u64::from_le_bytes(bytes[40..48].try_into().ok()?),
            uid: u32::from_le_bytes(bytes[48..52].try_into().ok()?),
        })
    }
}

fn write_address(output: &mut [u8], address: IpAddr) {
    match address {
        IpAddr::V4(address) => output[..4].copy_from_slice(&address.octets()),
        IpAddr::V6(address) => output.copy_from_slice(&address.octets()),
    }
}

fn read_address(bytes: &[u8], family: AddressFamily) -> Option<IpAddr> {
    match family {
        AddressFamily::Ipv4 => Some(IpAddr::V4(Ipv4Addr::from(
            <[u8; 4]>::try_from(&bytes[..4]).ok()?,
        ))),
        AddressFamily::Ipv6 => Some(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(bytes).ok()?,
        ))),
    }
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
