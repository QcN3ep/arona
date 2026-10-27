// SPDX-License-Identifier: MPL-2.0

//! 对象调用的鉴权与分派。

mod error;

use crate::{thread, trap::TrapOutcome};
use abi::{Error, Handle, Return, Rights, method};

pub(crate) fn dispatch(handle: usize, arguments: [usize; 6]) -> TrapOutcome {
    let result = invoke(
        Handle::from_raw(handle as u64),
        arguments.map(|value| value as u64),
    );
    let result = result.unwrap_or_else(Return::failure);
    TrapOutcome::Return {
        status: result.status as usize,
        values: result.values.map(|value| value as usize),
    }
}

fn invoke(handle: Handle, arguments: [u64; 6]) -> Result<Return, Error> {
    let caller = thread::current()
        .and_then(|thread| thread.task())
        .ok_or(Error::InvalidState)?;
    let (kind, rights) = caller.handle_info(handle)?;
    let required = method::required_rights(kind, arguments[0]).ok_or(Error::Unsupported)?;
    if !rights.contains(required) {
        return Err(Error::Denied);
    }
    let values = match arguments[0] {
        method::INFO => [kind as u64, rights.bits()],
        method::CLOSE => {
            caller.close(handle)?;
            [0; 2]
        }
        method::DUPLICATE => {
            let rights = Rights::from_bits(arguments[1]).ok_or(Error::InvalidArgument)?;
            [caller.duplicate(handle, rights)?.raw(), 0]
        }
        _ => unreachable!("unimplemented authorized method"),
    };
    Ok(Return::success(values))
}
