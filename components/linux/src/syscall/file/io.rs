// SPDX-License-Identifier: MPL-2.0

//! 文件内容访问、定位、截断与同步。

use super::super::{
    Outcome,
    abi::{EBADF, EFBIG, EINVAL, EIO, EISDIR, EOVERFLOW, ESPIPE, SyscallResult},
};
use super::{Buffer, Pending, buffer::IoVectors, completion, fd, filesystem_error, outcome};
use crate::{
    file::{FilePosition, O_APPEND, O_DSYNC, OpenFileDescription},
    process::Process,
};
use alloc::{rc::Rc, vec::Vec};
use runtime::{Call, Handle};

pub(in crate::syscall) struct Read<'a> {
    description: Rc<OpenFileDescription<'a>>,
    buffers: IoVectors,
    offset: Option<usize>,
    position: Option<FilePosition<'a>>,
    request: Option<fs::client::PendingRead>,
}

pub(in crate::syscall) struct Seek<'a> {
    description: Rc<OpenFileDescription<'a>>,
    displacement: i64,
    whence: u32,
    position: Option<FilePosition<'a>>,
    request: Option<fs::client::PendingSize>,
}

pub(in crate::syscall) struct Write<'a> {
    description: Rc<OpenFileDescription<'a>>,
    input: Vec<u8>,
    offset: Option<usize>,
    position: Option<FilePosition<'a>>,
    request: Option<WriteRequest>,
}

enum WriteRequest {
    At(fs::client::PendingWrite),
    Append(fs::client::PendingAppend),
    Flush {
        request: fs::client::Pending<()>,
        count: usize,
    },
}

pub(in crate::syscall) fn read<'a>(
    process: &Process<'a>,
    fd_raw: usize,
    buffer: Buffer,
    offset: Option<usize>,
) -> Outcome<'a> {
    if process
        .files
        .get(fd_raw as i32 as usize)
        .is_some_and(|descriptor| {
            matches!(
                descriptor.description.as_ref(),
                OpenFileDescription::Pipe { .. }
            )
        })
    {
        return super::pipe::start(process, fd_raw, buffer, offset, false);
    }
    if process
        .files
        .get(fd_raw as i32 as usize)
        .is_some_and(|descriptor| {
            matches!(
                descriptor.description.as_ref(),
                OpenFileDescription::Terminal { .. }
            )
        })
    {
        if offset.is_some() {
            return Outcome::Complete(Err(ESPIPE));
        }
        return super::terminal::read(process, fd_raw, buffer);
    }
    let operation = (|| {
        let descriptor = process.files.get(fd(fd_raw)?).ok_or(EBADF)?;
        match descriptor.description.as_ref() {
            OpenFileDescription::Pipe { .. } => unreachable!(),
            OpenFileDescription::Terminal { .. } => unreachable!(),
            OpenFileDescription::Directory { .. } => return Err(EISDIR),
            OpenFileDescription::Regular { .. } => {
                if descriptor.description.flags() & 3 == 1 {
                    return Err(EBADF);
                }
            }
        }
        if offset.is_some_and(|offset| offset > i64::MAX as usize) {
            return Err(EINVAL);
        }
        Ok(Pending::Read(Read {
            description: Rc::clone(&descriptor.description),
            buffers: buffer.import(process)?,
            offset,
            position: None,
            request: None,
        }))
    })();
    outcome(operation)
}

pub(in crate::syscall) fn write<'a>(
    process: &Process<'a>,
    fd_raw: usize,
    buffer: Buffer,
    offset: Option<usize>,
) -> Outcome<'a> {
    if process
        .files
        .get(fd_raw as i32 as usize)
        .is_some_and(|descriptor| {
            matches!(
                descriptor.description.as_ref(),
                OpenFileDescription::Pipe { .. }
            )
        })
    {
        return super::pipe::start(process, fd_raw, buffer, offset, true);
    }
    let operation = (|| {
        let descriptor = process.files.get(fd(fd_raw)?).ok_or(EBADF)?;
        if descriptor.description.flags() & 3 == 0 {
            return Err(EBADF);
        }
        if offset.is_some()
            && matches!(
                descriptor.description.as_ref(),
                OpenFileDescription::Terminal { .. }
            )
        {
            return Err(ESPIPE);
        }
        if offset.is_some_and(|offset| offset > i64::MAX as usize) {
            return Err(EINVAL);
        }
        let buffers = buffer.import(process)?;
        let limit = if matches!(
            descriptor.description.as_ref(),
            OpenFileDescription::Terminal { .. }
        ) {
            devmgr::uart::MAX_BYTES
        } else {
            buffers.length()
        };
        let input = buffers.gather(process, limit)?;
        if input.is_empty() {
            return Ok(None);
        }
        let description = Rc::clone(&descriptor.description);
        Ok(Some(match description.as_ref() {
            OpenFileDescription::Terminal { terminal, .. } => {
                let (bytes, consumed) = terminal.output(&input);
                Pending::Output {
                    request: terminal.output.start_write(&bytes).map_err(|_| EIO)?,
                    consumed,
                    _description: description,
                }
            }
            OpenFileDescription::Regular { .. } => Pending::Write(Write {
                description,
                input,
                offset,
                position: None,
                request: None,
            }),
            OpenFileDescription::Pipe { .. } => unreachable!(),
            OpenFileDescription::Directory { .. } => return Err(EBADF),
        }))
    })();
    match operation {
        Ok(Some(operation)) => Outcome::Pending(super::super::Operation::File(operation)),
        Ok(None) => Outcome::Complete(Ok(0)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

pub(in crate::syscall) fn truncate<'a>(
    process: &Process<'a>,
    raw: usize,
    length: usize,
) -> Outcome<'a> {
    let operation = (|| {
        let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
        if length > i64::MAX as usize || descriptor.description.flags() & 3 == 0 {
            return Err(EINVAL);
        }
        let OpenFileDescription::Regular { file, .. } = descriptor.description.as_ref() else {
            return Err(EINVAL);
        };
        Ok(Pending::Truncate {
            request: file.start_truncate(length).map_err(filesystem_error)?,
            _description: Rc::clone(&descriptor.description),
        })
    })();
    outcome(operation)
}

pub(in crate::syscall) fn sync<'a>(process: &Process<'a>, raw: usize) -> Outcome<'a> {
    let operation = (|| {
        let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
        let request = match descriptor.description.as_ref() {
            OpenFileDescription::Regular { file, .. } => file.start_fsync(),
            OpenFileDescription::Directory { session, .. } => session.start_fsync(),
            OpenFileDescription::Terminal { .. } | OpenFileDescription::Pipe { .. } => {
                return Err(EINVAL);
            }
        }
        .map_err(filesystem_error)?;
        Ok(Pending::Sync {
            request,
            _description: Rc::clone(&descriptor.description),
        })
    })();
    outcome(operation)
}

pub(in crate::syscall) fn seek<'a>(
    process: &Process<'a>,
    fd_raw: usize,
    offset: usize,
    whence: usize,
) -> Outcome<'a> {
    let operation = (|| {
        let descriptor = process.files.get(fd(fd_raw)?).ok_or(EBADF)?;
        match descriptor.description.as_ref() {
            OpenFileDescription::Terminal { .. } | OpenFileDescription::Pipe { .. } => {
                return Err(ESPIPE);
            }
            OpenFileDescription::Directory { .. } if whence as u32 == 2 => return Err(EINVAL),
            OpenFileDescription::Directory { .. } => {}
            OpenFileDescription::Regular { .. } => {}
        }
        if whence as u32 > 2 {
            return Err(EINVAL);
        }
        Ok(Pending::Seek(Seek {
            description: Rc::clone(&descriptor.description),
            displacement: offset as i64,
            whence: whence as u32,
            position: None,
            request: None,
        }))
    })();
    outcome(operation)
}

impl Read<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        self.request.as_ref().map(fs::client::PendingRead::call)
    }

    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        if self.buffers.length() == 0 {
            return Some(Ok(0));
        }
        let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
            unreachable!()
        };
        if self.request.is_none() {
            let offset = if let Some(offset) = self.offset {
                offset
            } else {
                let position = self.description.position()?;
                let offset = position.get();
                self.position = Some(position);
                offset
            };
            match file.start_read_at(
                offset,
                self.buffers.length().min(i64::MAX as usize - offset),
            ) {
                Ok(request) => self.request = Some(request),
                Err(error) => return Some(Err(filesystem_error(error))),
            }
        }
        let mut bytes = match completion(self.request.as_mut().unwrap().try_complete())? {
            Ok(bytes) => bytes,
            Err(error) => return Some(Err(filesystem_error(error))),
        };
        let count = match self.buffers.scatter(process, &mut bytes) {
            Ok(count) => count,
            Err(error) => return Some(Err(error)),
        };
        if let Some(position) = &self.position {
            position.set(position.get() + count);
        }
        Some(Ok(count))
    }
}

impl Write<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        match &self.request {
            Some(WriteRequest::At(request)) => Some(request.call()),
            Some(WriteRequest::Append(request)) => Some(request.call()),
            Some(WriteRequest::Flush { request, .. }) => Some(request.call()),
            None => None,
        }
    }

    pub fn poll(&mut self) -> Option<SyscallResult> {
        let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
            unreachable!()
        };
        if self.request.is_none() {
            if self.offset.is_none() {
                self.position = Some(self.description.position()?);
            }
            let request = if self.description.flags() & O_APPEND != 0 {
                // REFERENCE: https://man7.org/linux/man-pages/man2/pwrite.2.html
                file.start_append(&self.input).map(WriteRequest::Append)
            } else {
                let offset = self
                    .offset
                    .unwrap_or_else(|| self.position.as_ref().unwrap().get());
                let length = self.input.len().min(i64::MAX as usize - offset);
                if length == 0 {
                    return Some(Err(EFBIG));
                }
                file.start_write_at(offset, &self.input[..length])
                    .map(WriteRequest::At)
            };
            match request {
                Ok(request) => self.request = Some(request),
                Err(error) => return Some(Err(filesystem_error(error))),
            }
        }
        let (offset, count) = match self.request.as_mut().unwrap() {
            WriteRequest::At(request) => match completion(request.try_complete())? {
                Ok(count) => (
                    self.offset
                        .unwrap_or_else(|| self.position.as_ref().unwrap().get()),
                    count,
                ),
                Err(error) => return Some(Err(filesystem_error(error))),
            },
            WriteRequest::Append(request) => match completion(request.try_complete())? {
                Ok(result) => result,
                Err(error) => return Some(Err(filesystem_error(error))),
            },
            WriteRequest::Flush { request, count } => {
                return Some(
                    completion(request.try_complete())?
                        .map(|()| *count)
                        .map_err(filesystem_error),
                );
            }
        };
        if let Some(position) = &self.position {
            position.set(offset + count);
        }
        if self.description.flags() & O_DSYNC != 0 {
            return match file.start_fsync() {
                Ok(request) => {
                    self.request = Some(WriteRequest::Flush { request, count });
                    None
                }
                Err(error) => Some(Err(filesystem_error(error))),
            };
        }
        Some(Ok(count))
    }
}
impl Seek<'_> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        self.request.as_ref().map(fs::client::PendingSize::call)
    }

    pub fn poll(&mut self) -> Option<SyscallResult> {
        if self.position.is_none() {
            self.position = Some(self.description.position()?);
        }
        let position = self.position.as_ref().unwrap();
        let base = match self.whence {
            0 => 0,
            1 => position.get(),
            _ => {
                let OpenFileDescription::Regular { file, .. } = self.description.as_ref() else {
                    unreachable!()
                };
                if self.request.is_none() {
                    match file.start_size() {
                        Ok(request) => self.request = Some(request),
                        Err(error) => return Some(Err(filesystem_error(error))),
                    }
                }
                match completion(self.request.as_mut().unwrap().try_complete())? {
                    Ok(size) => size,
                    Err(error) => return Some(Err(filesystem_error(error))),
                }
            }
        };
        let offset = base as i128 + i128::from(self.displacement);
        if offset < 0 {
            return Some(Err(EINVAL));
        }
        if offset > i128::from(i64::MAX) {
            return Some(Err(EOVERFLOW));
        }
        position.set(offset as usize);
        Some(Ok(offset as usize))
    }
}
