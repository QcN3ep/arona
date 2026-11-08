// SPDX-License-Identifier: MPL-2.0

use super::Outcome;
use crate::{Error, ExitStatus, process::Process};

pub(super) fn exit<'a>(
    process: &Process<'a>,
    index: usize,
    code: usize,
) -> Result<Outcome<'a>, Error> {
    process.threads[index].native.terminate(
        ExitStatus {
            code: code as u8,
            signal: 0,
        }
        .encode(),
    )?;
    Ok(Outcome::Terminated)
}

pub(super) fn exit_group<'a>(process: &Process<'a>, code: usize) -> Result<Outcome<'a>, Error> {
    let status = ExitStatus {
        code: code as u8,
        signal: 0,
    }
    .encode();
    for thread in &process.threads {
        thread.native.terminate(status)?;
    }
    Ok(Outcome::Terminated)
}
