// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! `/proc/net` snapshots built from the network service.

use alloc::{format, string::String, vec::Vec};
use net::{AddressFamily, DeviceInfo, IpAddr, SocketInfo, SocketKind, SocketState};

use crate::{
    Services,
    syscall::abi::{EIO, ENODEV, EOVERFLOW, Errno},
    syscall::socket,
};

const TCP_HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
const UDP_HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n";
const UNIX_HEADER: &str = "Num       RefCount Protocol Flags    Type St Inode Path\n";
const DEV_HEADER: &str = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n";
const ROUTE_HEADER: &str =
    "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";

/// A table under `/proc/net`.
#[derive(Clone, Copy)]
pub(crate) enum File {
    Dev,
    Route,
    Tcp,
    Tcp6,
    Udp,
    Udp6,
    Unix,
}

impl File {
    pub(crate) fn inode(self) -> u64 {
        0x7072_1000
            + match self {
                Self::Dev => 0,
                Self::Route => 1,
                Self::Tcp => 2,
                Self::Tcp6 => 3,
                Self::Udp => 4,
                Self::Udp6 => 5,
                Self::Unix => 6,
            }
    }
}

/// Build one snapshot, blocking the open until the network service replies.
pub(crate) fn content(file: File, services: &Services<'_>) -> Result<Vec<u8>, Errno> {
    let net = services.net.as_ref().ok_or(ENODEV)?;
    let deadline = runtime::thread::now_ns()
        .map_err(|_| EIO)?
        .checked_add(10_000_000_000)
        .ok_or(EOVERFLOW)?;
    let table = net.table(deadline).map_err(socket::error)?;
    Ok(match file {
        File::Dev => devices(&table.devices),
        File::Route => routes(&table.devices),
        File::Tcp => sockets(&table.sockets, SocketKind::Tcp, AddressFamily::Ipv4),
        File::Tcp6 => sockets(&table.sockets, SocketKind::Tcp, AddressFamily::Ipv6),
        File::Udp => sockets(&table.sockets, SocketKind::Udp, AddressFamily::Ipv4),
        File::Udp6 => sockets(&table.sockets, SocketKind::Udp, AddressFamily::Ipv6),
        File::Unix => UNIX_HEADER.as_bytes().to_vec(),
    })
}

fn name(device: &DeviceInfo) -> &str {
    core::str::from_utf8(&device.name).unwrap_or("?")
}

fn devices(table: &[DeviceInfo]) -> Vec<u8> {
    let mut text = String::from(DEV_HEADER);
    for device in table {
        // Counter updates are not tracked by the loopback-only stack yet.
        text.push_str(&format!(
            "{:>6}: {:>8} {:>7} {:>4} {:>4} {:>4} {:>5} {:>10} {:>9} {:>8} {:>7} {:>4} {:>4} {:>4} {:>5} {:>7} {:>10}\n",
            name(device),
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ));
    }
    text.into_bytes()
}

fn routes(table: &[DeviceInfo]) -> Vec<u8> {
    let mut text = String::from(ROUTE_HEADER);
    for device in table {
        let Some(address) = device.ipv4 else {
            continue;
        };
        let mask = u32::from_le_bytes(device.netmask.octets());
        // The lwIP loopback interface names itself as its gateway; Linux
        // reports loopback routes without one.
        let gateway = if address.is_loopback() {
            0
        } else {
            u32::from_le_bytes(device.gateway.octets())
        };
        let destination = u32::from_le_bytes(address.octets()) & mask;
        let flags = if gateway == 0 { 0x0001 } else { 0x0003 };
        text.push_str(&format!(
            "{}\t{destination:08X}\t{gateway:08X}\t{flags:04X}\t0\t0\t0\t{mask:08X}\t{}\t0\t0\n",
            name(device),
            device.mtu,
        ));
    }
    text.into_bytes()
}

fn sockets(table: &[SocketInfo], kind: SocketKind, family: AddressFamily) -> Vec<u8> {
    let mut text = String::from(match kind {
        SocketKind::Tcp => TCP_HEADER,
        SocketKind::Udp => UDP_HEADER,
    });
    for (index, socket) in table
        .iter()
        .filter(|socket| socket.kind == kind && socket.family == family)
        .enumerate()
    {
        let (local_address, local_port) = address(&socket.local.address, socket.local.port);
        let (remote_address, remote_port) = socket.peer.as_ref().map_or_else(
            || (zero(socket.family), 0),
            |peer| address(&peer.address, peer.port),
        );
        // UDP sockets without a peer are shown as closed, matching Linux.
        let state = match socket.state {
            SocketState::Established => 0x01,
            SocketState::Listen => 0x0a,
            SocketState::Close | SocketState::Unconnected => 0x07,
        };
        let trailer = match kind {
            SocketKind::Tcp => "1 0000000000000000 100 0 0 10 0",
            SocketKind::Udp => "1 0000000000000000 0",
        };
        text.push_str(&format!(
            "{index:>4}: {local_address}:{local_port:04X} {remote_address}:{remote_port:04X} {state:02X} 00000000:00000000 00:00000000 00000000 {:>5} 0 {} {trailer}\n",
            socket.uid, socket.inode,
        ));
    }
    text.into_bytes()
}

fn zero(family: AddressFamily) -> String {
    match family {
        AddressFamily::Ipv4 => String::from("00000000"),
        AddressFamily::Ipv6 => String::from("00000000000000000000000000000000"),
    }
}

fn address(address: &IpAddr, port: u16) -> (String, u16) {
    let text = match address {
        IpAddr::V4(address) => format!("{:08X}", u32::from_le_bytes(address.octets())),
        IpAddr::V6(address) => {
            let octets = address.octets();
            let words = octets.as_chunks::<4>().0;
            format!(
                "{:08X}{:08X}{:08X}{:08X}",
                u32::from_le_bytes(words[0]),
                u32::from_le_bytes(words[1]),
                u32::from_le_bytes(words[2]),
                u32::from_le_bytes(words[3]),
            )
        }
    };
    (text, port)
}
