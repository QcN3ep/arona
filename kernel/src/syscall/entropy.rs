// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::error::CallError;
use crate::task::Task;
use abi::Error;

pub(super) fn generate(caller: &Task, args: [u64; 5]) -> Result<[u64; 2], CallError> {
    if args[2..] != [0; 3] {
        return Err(Error::InvalidArgument.into());
    }
    let length = usize::try_from(args[1]).map_err(|_| Error::InvalidArgument)?;
    let output = caller.pin_user_memory(args[0] as usize, length, true)?;
    if length == 0 {
        crate::random::fill(&mut [])?;
    }
    let mut bytes = [0; 256];
    for offset in (0..length).step_by(bytes.len()) {
        let count = bytes.len().min(length - offset);
        crate::random::fill(&mut bytes[..count])?;
        output.write(offset, &bytes[..count])?;
    }
    Ok([0; 2])
}
