// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Linux whole-file transfers between file descriptors.

use super::super::{
    Outcome,
    abi::{EAGAIN, EBADF, EINVAL, EIO, EOVERFLOW, EPIPE, Errno, SyscallResult},
};
use super::{Pending, completion, filesystem_error, outcome};
use crate::{
    file::{FilePosition, O_APPEND, O_NONBLOCK, OpenFileDescription},
    process::Process,
    syscall::socket,
};
use alloc::{rc::Rc, vec::Vec};
use runtime::{Call, Handle};

// Transfers are staged one page at a time, matching the file service's per-page I/O.
const CHUNK: usize = 4096;

/// A file-to-file, file-to-pipe, or file-to-socket transfer.
pub(in crate::syscall) struct Sendfile<'a> {
    source: Rc<OpenFileDescription<'a>>,
    destination: Rc<OpenFileDescription<'a>>,
    offset_address: usize,
    position: Option<FilePosition<'a>>,
    destination_position: Option<FilePosition<'a>>,
    offset: usize,
    remaining: usize,
    transferred: usize,
    buffer: Vec<u8>,
    written: usize,
    read: Option<fs::client::PendingRead>,
    write: Option<Write<'a>>,
    eof: bool,
}

enum Write<'a> {
    File(fs::client::PendingWrite),
    Append(fs::client::PendingAppend),
    Socket(socket::Stream<'a>),
}

enum WriteResult {
    At(usize),
    Append((usize, usize)),
}

enum Step {
    Progress(usize),
    Pending,
    Blocked,
}

pub(in crate::syscall) fn start<'a>(process: &Process<'a>, arguments: [usize; 6]) -> Outcome<'a> {
    outcome(build(process, arguments))
}

fn build<'a>(process: &Process<'a>, arguments: [usize; 6]) -> Result<Pending<'a>, Errno> {
    let count = arguments[3] as isize;
    if count < 0 {
        return Err(EINVAL);
    }
    let count = count as usize;
    let output = descriptor(process, arguments[0])?;
    let input = descriptor(process, arguments[1])?;
    if !matches!(input.as_ref(), OpenFileDescription::Regular { .. }) {
        return Err(EINVAL);
    }
    if input.flags() & 3 == 1 || output.flags() & 3 == 0 {
        return Err(EBADF);
    }
    let destination = match output.as_ref() {
        OpenFileDescription::Regular { .. }
        | OpenFileDescription::Pipe { .. }
        | OpenFileDescription::Socket { .. } => Rc::clone(&output),
        _ => return Err(EINVAL),
    };
    let offset_address = arguments[2];
    let mut offset = 0;
    if offset_address != 0 {
        let mut bytes = [0; 8];
        process
            .address_space
            .read(offset_address, &mut bytes)
            .map_err(|_| super::super::abi::EFAULT)?;
        let value = i64::from_le_bytes(bytes);
        if value < 0 {
            return Err(EINVAL);
        }
        offset = usize::try_from(value).map_err(|_| EOVERFLOW)?;
    }
    if count == 0 {
        return Ok(Pending::Sendfile(Sendfile {
            source: Rc::clone(&input),
            destination,
            offset_address,
            position: None,
            destination_position: None,
            offset,
            remaining: 0,
            transferred: 0,
            buffer: Vec::new(),
            written: 0,
            read: None,
            write: None,
            eof: true,
        }));
    }
    let position = if offset_address == 0 {
        let position = input.position().ok_or(EBADF)?;
        offset = position.get();
        Some(position)
    } else {
        None
    };
    let destination_position = match destination.as_ref() {
        OpenFileDescription::Regular { .. } => Some(destination.position().ok_or(EBADF)?),
        _ => None,
    };
    Ok(Pending::Sendfile(Sendfile {
        source: Rc::clone(&input),
        destination,
        offset_address,
        position,
        destination_position,
        offset,
        remaining: count,
        transferred: 0,
        buffer: Vec::new(),
        written: 0,
        read: None,
        write: None,
        eof: false,
    }))
}

fn descriptor<'a>(process: &Process<'a>, raw: usize) -> Result<Rc<OpenFileDescription<'a>>, Errno> {
    let index = usize::try_from(raw as i32).map_err(|_| EBADF)?;
    Ok(Rc::clone(
        &process.files.get(index).ok_or(EBADF)?.description,
    ))
}

impl<'a> Sendfile<'a> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        if let Some(read) = &self.read {
            return Some(read.call());
        }
        match &self.write {
            Some(Write::File(request)) => Some(request.call()),
            Some(Write::Append(request)) => Some(request.call()),
            Some(Write::Socket(stream)) => stream.call(),
            None => None,
        }
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<runtime::ipc::WaitItem<'b>>) {
        if let Some(Write::Socket(stream)) = &self.write {
            stream.waits(items);
        }
    }

    pub fn poll(&mut self, process: &mut Process<'a>) -> Option<SyscallResult> {
        loop {
            if self.written < self.buffer.len() {
                let step = match self.write_chunk(process) {
                    Ok(step) => step,
                    Err(error) => return Some(Err(error)),
                };
                match step {
                    Step::Blocked => return Some(self.finish(process)),
                    Step::Pending => return None,
                    Step::Progress(0) => return None,
                    Step::Progress(count) => {
                        self.offset += count;
                        self.written += count;
                        self.transferred += count;
                        self.remaining -= count;
                        continue;
                    }
                }
            }
            if self.remaining == 0 || self.eof {
                return Some(self.finish(process));
            }
            if self.read.is_none() {
                let length = self.remaining.min(CHUNK);
                let OpenFileDescription::Regular { file, .. } = self.source.as_ref() else {
                    return Some(Err(EINVAL));
                };
                match file.start_read_at(self.offset, length) {
                    Ok(request) => self.read = Some(request),
                    Err(error) => return Some(Err(filesystem_error(error))),
                }
            }
            let bytes = match completion(self.read.as_mut().unwrap().try_complete())? {
                Ok(bytes) => bytes,
                Err(error) => return Some(Err(filesystem_error(error))),
            };
            self.read = None;
            if bytes.is_empty() {
                self.eof = true;
                continue;
            }
            self.buffer = bytes;
            self.written = 0;
        }
    }

    fn write_chunk(&mut self, process: &mut Process<'a>) -> Result<Step, Errno> {
        let pending = &self.buffer[self.written..];
        let nonblocking = self.destination.flags() & O_NONBLOCK != 0;
        match self.destination.as_ref() {
            OpenFileDescription::Regular { file, .. } => {
                if self.write.is_none() {
                    let request = if self.destination.flags() & O_APPEND != 0 {
                        match file.start_append(pending) {
                            Ok(request) => Write::Append(request),
                            Err(error) => return Err(filesystem_error(error)),
                        }
                    } else {
                        let position = self.destination_position.as_ref().map_or(0, |p| p.get());
                        match file.start_write_at(position, pending) {
                            Ok(request) => Write::File(request),
                            Err(error) => return Err(filesystem_error(error)),
                        }
                    };
                    self.write = Some(request);
                }
                let finished = match self.write.as_mut().unwrap() {
                    Write::File(request) => {
                        completion(request.try_complete()).map(|result| result.map(WriteResult::At))
                    }
                    Write::Append(request) => completion(request.try_complete())
                        .map(|result| result.map(WriteResult::Append)),
                    Write::Socket(_) => unreachable!(),
                };
                let result = match finished {
                    None => return Ok(Step::Pending),
                    Some(Err(error)) => return Err(filesystem_error(error)),
                    Some(Ok(result)) => result,
                };
                self.write = None;
                let (offset, count) = match result {
                    WriteResult::At(count) => (
                        self.destination_position.as_ref().map_or(0, |p| p.get()),
                        count,
                    ),
                    WriteResult::Append(result) => result,
                };
                if let Some(position) = &self.destination_position {
                    position.set(offset + count);
                }
                if count == 0 {
                    return Err(EIO);
                }
                Ok(Step::Progress(count))
            }
            OpenFileDescription::Pipe { end, .. } => {
                if end.read || end.peer_closed() {
                    return Err(if end.read { EBADF } else { EPIPE });
                }
                let free = end.free();
                if free == 0 {
                    return if !nonblocking {
                        Ok(Step::Pending)
                    } else if self.transferred == 0 {
                        Err(EAGAIN)
                    } else {
                        Ok(Step::Blocked)
                    };
                }
                let count = free.min(pending.len());
                end.write(&pending[..count]);
                Ok(Step::Progress(count))
            }
            OpenFileDescription::Socket { .. } => {
                if self.write.is_none() {
                    let stream = socket::Stream::start(
                        process,
                        Rc::clone(&self.destination),
                        pending.to_vec(),
                    )?;
                    self.write = Some(Write::Socket(stream));
                }
                let Some(Write::Socket(stream)) = self.write.as_mut() else {
                    unreachable!()
                };
                let count = match stream.poll(process) {
                    Some(Ok(count)) => count,
                    Some(Err(EAGAIN)) if self.transferred != 0 => {
                        self.write = None;
                        return Ok(Step::Blocked);
                    }
                    Some(Err(error)) => return Err(error),
                    None => return Ok(Step::Pending),
                };
                self.write = None;
                if count == 0 && self.transferred != 0 {
                    return Ok(Step::Blocked);
                }
                Ok(Step::Progress(count))
            }
            _ => Err(EINVAL),
        }
    }

    fn finish(&mut self, process: &Process<'a>) -> SyscallResult {
        if self.offset_address != 0 {
            let bytes = (self.offset as i64).to_le_bytes();
            process
                .address_space
                .write(self.offset_address, &bytes)
                .map_err(|_| super::super::abi::EFAULT)?;
        } else if let Some(position) = &self.position {
            position.set(self.offset);
        }
        Ok(self.transferred)
    }
}
