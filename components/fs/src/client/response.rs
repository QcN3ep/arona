// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Reply validation shared by blocking and asynchronous file sessions.

use super::{Directory, File};
use crate::{
    Error, Metadata,
    protocol::{self, response_result},
};
use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};
use runtime::{Channel, FrameAllocation, Handle, Task, ipc::ReceivedMessage};

pub(super) fn complete_open<'a>(
    (task, channel): (&'a Handle<Task>, Handle<Channel>),
    response: ReceivedMessage,
) -> Result<File<'a>, Error> {
    let length = response_size(response_result(response)?)?;
    Ok(File {
        task,
        channel,
        length: Arc::new(AtomicUsize::new(length)),
    })
}

pub(super) fn response_size(response: ReceivedMessage) -> Result<usize, Error> {
    let [size] = *response.words() else {
        return Err(Error::InvalidData);
    };
    usize::try_from(size).map_err(|_| Error::InvalidData)
}

pub(super) fn read_count(response: ReceivedMessage, limit: usize) -> Result<usize, Error> {
    let count = response_size(response_result(response)?)?;
    if count > limit {
        return Err(Error::InvalidData);
    }
    Ok(count)
}

pub(super) fn complete_read(
    (frames, limit): (Handle<FrameAllocation>, usize),
    response: ReceivedMessage,
) -> Result<Vec<u8>, Error> {
    let count = read_count(response, limit)?;
    let mut bytes = alloc::vec![0; count];
    frames.read(0, &mut bytes)?;
    Ok(bytes)
}

/// Decodes a page mapping reply carrying exactly one frame allocation.
pub(super) fn complete_page_map(
    response: ReceivedMessage,
) -> Result<Handle<FrameAllocation>, Error> {
    let mut response = protocol::response_status(response)?;
    if !response.words().is_empty() || response.handles().len() != 1 {
        return Err(Error::InvalidData);
    }
    response
        .take_handle(0)
        .ok_or(Error::InvalidData)?
        .downcast::<FrameAllocation>()
        .map_err(|_| Error::InvalidData)
}

pub(super) fn complete_metadata(
    output: Handle<FrameAllocation>,
    response: ReceivedMessage,
) -> Result<Metadata, Error> {
    complete_empty(response)?;
    let mut bytes = [0; Metadata::SIZE];
    output.read(0, &mut bytes)?;
    Metadata::decode(&bytes).map_err(|_| Error::InvalidData)
}

pub(super) fn complete_empty(response: ReceivedMessage) -> Result<(), Error> {
    if response_result(response)?.words().is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidData)
    }
}

pub(super) fn complete_write(
    (length, limit): (&AtomicUsize, usize),
    response: ReceivedMessage,
) -> Result<usize, Error> {
    let response = response_result(response)?;
    let [count, size] = *response.words() else {
        return Err(Error::InvalidData);
    };
    if count > limit as u64 {
        return Err(Error::InvalidData);
    }
    length.store(
        usize::try_from(size).map_err(|_| Error::InvalidData)?,
        Ordering::Relaxed,
    );
    Ok(count as usize)
}

pub(super) fn complete_append(
    (length, limit): (&AtomicUsize, usize),
    response: ReceivedMessage,
) -> Result<(usize, usize), Error> {
    let response = response_result(response)?;
    let [count, size, offset] = *response.words() else {
        return Err(Error::InvalidData);
    };
    if count > limit as u64 || offset.checked_add(count) != Some(size) {
        return Err(Error::InvalidData);
    }
    length.store(
        usize::try_from(size).map_err(|_| Error::InvalidData)?,
        Ordering::Relaxed,
    );
    Ok((offset as usize, count as usize))
}

pub(super) fn complete_truncate(
    (length, size): (&AtomicUsize, usize),
    response: ReceivedMessage,
) -> Result<(), Error> {
    if response_result(response)?.words() != [size as u64] {
        return Err(Error::InvalidData);
    }
    length.store(size, Ordering::Relaxed);
    Ok(())
}

pub(super) fn complete_size(
    length: &AtomicUsize,
    response: ReceivedMessage,
) -> Result<usize, Error> {
    let size = response_size(response_result(response)?)?;
    length.store(size, Ordering::Relaxed);
    Ok(size)
}

pub(super) fn directory_entries(bytes: &[u8], count: usize) -> Result<Vec<crate::DirEntry>, Error> {
    if count > bytes.len() / crate::DirEntry::HEADER_SIZE {
        return Err(Error::InvalidData);
    }
    let mut entries = Vec::new();
    let mut remaining = bytes;
    for _ in 0..count {
        let (entry, length) = crate::DirEntry::decode_prefix(remaining)?;
        entries.push(entry);
        remaining = &remaining[length..];
    }
    if !remaining.is_empty() {
        return Err(Error::InvalidData);
    }
    Ok(entries)
}

pub(super) fn complete_directory<'a>(
    (task, channel, owner): (&'a Handle<Task>, Handle<Channel>, Arc<()>),
    response: ReceivedMessage,
) -> Result<Directory<'a>, Error> {
    let response = response_result(response)?;
    let [0, id] = *response.words() else {
        return Err(Error::InvalidData);
    };
    if id == 0 {
        return Err(Error::InvalidData);
    }
    Ok(Directory {
        file: File {
            task,
            channel,
            length: Arc::new(AtomicUsize::new(0)),
        },
        id,
        owner,
    })
}
