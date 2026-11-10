// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use runtime::{
    FrameAllocation, Handle, Object, Rights,
    ipc::{Message, ReceivedMessage, Transfer},
};

use crate::Error;

pub(crate) const START: u64 = 0;
pub(crate) const RUN: u64 = 1;
pub(crate) const CHANNEL_RIGHTS: Rights = Rights::READ_WRITE.union(Rights::WAIT);

pub(crate) struct Arguments {
    pub path: Vec<u8>,
    pub argv: Vec<Vec<u8>>,
    pub envp: Vec<Vec<u8>>,
}

impl Arguments {
    pub fn decode(
        frames: &Handle<FrameAllocation>,
        length: usize,
        argc: usize,
        envc: usize,
    ) -> Result<Self, Error> {
        if length as u64 > frames.frames()?.saturating_mul(4096)
            || argc == 0
            || argc
                .checked_add(envc)
                .and_then(|n| n.checked_add(1))
                .is_none_or(|n| n > length)
        {
            return Err(Error::InvalidRequest);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| Error::NoMemory)?;
        bytes.resize(length, 0);
        frames.read(0, &mut bytes)?;
        let mut strings = bytes.split_inclusive(|byte| *byte == 0);
        let mut next = || {
            let bytes = strings
                .next()
                .and_then(|bytes| bytes.strip_suffix(&[0]))
                .ok_or(Error::InvalidRequest)?;
            Ok(bytes.to_vec())
        };
        let path = next()?;
        if path.first() != Some(&b'/') {
            return Err(Error::InvalidRequest);
        }
        let argv = (0..argc).map(|_| next()).collect::<Result<_, Error>>()?;
        let envp = (0..envc).map(|_| next()).collect::<Result<_, Error>>()?;
        if strings.next().is_some() {
            return Err(Error::InvalidRequest);
        }
        Ok(Self { path, argv, envp })
    }
}

pub(crate) fn transfer<T: Object>(
    request: &mut Message,
    handle: Handle<T>,
    rights: Rights,
) -> Result<(), Error> {
    request
        .push(Transfer {
            handle: handle.into(),
            rights,
        })
        .map_err(|_| Error::InvalidRequest)
}

pub(crate) fn response(response: ReceivedMessage) -> Result<ReceivedMessage, Error> {
    if !response.handles().is_empty() || response.tag != 0 && !response.words().is_empty() {
        return Err(runtime::Error::Protocol.into());
    }
    match response.tag {
        0 => Ok(response),
        1 => Err(Error::InvalidRequest),
        2 => Err(Error::InvalidExecutable),
        3 => Err(Error::NoMemory),
        4 => Err(Error::Filesystem(fs::Error::NotFound)),
        5 => Err(Error::Filesystem(fs::Error::Io)),
        6 => Err(Error::PermissionDenied),
        _ => Err(runtime::Error::Protocol.into()),
    }
}

pub(crate) fn error(error: Error) -> u64 {
    match error {
        Error::InvalidRequest => 1,
        Error::InvalidExecutable => 2,
        Error::PermissionDenied => 6,
        Error::NoMemory
        | Error::Runtime(runtime::Error::Kernel(runtime::KernelError::NoMemory)) => 3,
        Error::Filesystem(fs::Error::NotFound) => 4,
        _ => 5,
    }
}
