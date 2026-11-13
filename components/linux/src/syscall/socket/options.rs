// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux socket options and per-description I/O timeout values.

use super::{Action, Operation, Output, Pending, Socket};
use crate::{
    file::OpenFileDescription,
    process::Process,
    syscall::{abi::*, wait},
};
use alloc::{boxed::Box, rc::Rc};

pub(super) fn dispatch<'a>(
    process: &Process<'a>,
    description: Rc<OpenFileDescription<'a>>,
    args: [usize; 6],
    write: bool,
) -> Result<Pending<'a>, Errno> {
    let state = Socket::state(&description);
    let level = args[1] as u32;
    let name = args[2] as u32;
    if level == 1 && matches!(name, 20 | 21) {
        return timeout(process, state, args, write);
    }
    if !write && level == 1 && name == 3 {
        return Ok(Pending::Value(
            Output::new(process, args[3], args[4])?,
            if state.kind == net::SocketKind::Tcp {
                1
            } else {
                2
            },
        ));
    }
    let option = match (level, name) {
        (1, 2) => Some(net::OptionName::ReuseAddress),
        (1, 4) if !write => None,
        (1, 7) => Some(net::OptionName::SendBuffer),
        (1, 8) => Some(net::OptionName::ReceiveBuffer),
        (1, 9) => Some(net::OptionName::KeepAlive),
        (6, 1) => Some(net::OptionName::NoDelay),
        (6, 4) => Some(net::OptionName::KeepIdle),
        (6, 5) => Some(net::OptionName::KeepInterval),
        (6, 6) => Some(net::OptionName::KeepCount),
        (41, 26) if state.family == net::AddressFamily::Ipv6 => Some(net::OptionName::Ipv6Only),
        _ => return Err(ENOPROTOOPT),
    };
    if level == 6 && state.kind != net::SocketKind::Tcp {
        return Err(ENOPROTOOPT);
    }
    let action = if write {
        if (args[4] as i32) < 4 {
            return Err(EINVAL);
        }
        let bytes = wait::read_bytes(process, args[3], 4)?;
        let value = i32::from_le_bytes(bytes.try_into().unwrap());
        let value = match (level, name) {
            (1, 2 | 9) | (6, 1) | (41, 26) => u32::from(value != 0),
            (1, 7) => return Err(EOPNOTSUPP),
            (1, 8) if state.kind == net::SocketKind::Tcp => return Err(EOPNOTSUPP),
            (1, 8) if value >= 0 => (value as u32).saturating_mul(2).clamp(256, i32::MAX as u32),
            _ if value > 0 => value as u32,
            _ => return Err(EINVAL),
        };
        Action::SetOption(option.unwrap(), value)
    } else {
        Action::GetOption {
            name: option,
            output: Output::new(process, args[3], args[4])?,
            checking: false,
        }
    };
    Ok(Pending::Operation(Box::new(Operation::new(
        description,
        action,
        false,
        process,
    )?)))
}

fn timeout<'a>(
    process: &Process<'a>,
    state: &Socket<'a>,
    args: [usize; 6],
    write: bool,
) -> Result<Pending<'a>, Errno> {
    let setting = if args[2] as u32 == 20 {
        &state.receive_timeout
    } else {
        &state.send_timeout
    };
    if write {
        if (args[4] as i32) < 16 {
            return Err(EINVAL);
        }
        let bytes = wait::read_bytes(process, args[3], 16)?;
        let seconds = i64::from_le_bytes(bytes[..8].try_into().unwrap());
        let micros = i64::from_le_bytes(bytes[8..].try_into().unwrap());
        if !(0..1_000_000).contains(&micros) {
            return Err(EDOM);
        }
        if seconds < 0 {
            return Err(EINVAL);
        }
        let duration = (seconds as u64)
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(micros as u64 * 1000))
            .ok_or(EINVAL)?;
        setting.set((duration != 0).then_some(duration));
        Ok(Pending::Done)
    } else {
        let duration = setting.get().unwrap_or(0);
        let mut bytes = alloc::vec![0; 16];
        bytes[..8].copy_from_slice(&((duration / 1_000_000_000) as i64).to_le_bytes());
        bytes[8..].copy_from_slice(&((duration % 1_000_000_000 / 1000) as i64).to_le_bytes());
        Ok(Pending::Bytes(
            Output::new(process, args[3], args[4])?,
            bytes,
        ))
    }
}
