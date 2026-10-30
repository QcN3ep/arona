// SPDX-License-Identifier: MPL-2.0

//! 文件会话的建立、读取与回收。

use alloc::{vec, vec::Vec};
use runtime::{
    Channel, FrameAllocation, Handle, KernelError, Rights,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

use crate::{Error, FileSystem, ReadAt, archive::Archive, protocol};

const PAGE_SIZE: usize = 4096;

struct FileSession<F> {
    channel: Handle<Channel>,
    file: F,
}

pub fn run(control: Handle<Channel>) -> Result<(), Error> {
    let mut startup = control.receive(WaitMode::Forever)?;
    let reply = startup.take_reply().ok_or(Error::InvalidRequest)?;
    match load_archive(startup) {
        Ok(archive) => {
            protocol::respond(reply, Ok(Message::new(protocol::SUCCESS, &[])?))?;
            serve(control, &archive)
        }
        Err(error) => {
            protocol::respond(reply, Err(error))?;
            Err(error)
        }
    }
}

fn load_archive(mut startup: ReceivedMessage) -> Result<Archive, Error> {
    if startup.tag != protocol::START || startup.handles().len() != 1 {
        return Err(Error::InvalidRequest);
    }
    let [length] = startup.words() else {
        return Err(Error::InvalidRequest);
    };
    let length = usize::try_from(*length).map_err(|_| Error::InvalidRequest)?;
    let frames = take_frames(&mut startup, Rights::READ)?;
    Archive::new(frames, length)
}

pub fn serve(control: Handle<Channel>, filesystem: &impl FileSystem) -> Result<(), Error> {
    let mut files = Vec::new();
    loop {
        match control.receive(WaitMode::Poll) {
            Ok(mut request) => {
                if let Some(reply) = request.take_reply() {
                    let response = open(filesystem, request).and_then(|session| {
                        let length = session.file.len();
                        files.push(session);
                        Ok(Message::new(protocol::SUCCESS, &[length as u64])?)
                    });
                    protocol::respond(reply, response)?;
                }
            }
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(KernelError::EndOfStream)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let mut index = 0;
        while index < files.len() {
            match files[index].channel.receive(WaitMode::Poll) {
                Ok(mut request) => {
                    if let Some(reply) = request.take_reply() {
                        protocol::respond(reply, read(&files[index].file, request))?;
                    }
                    index += 1;
                }
                Err(runtime::Error::Kernel(KernelError::WouldBlock)) => index += 1,
                Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                    files.swap_remove(index);
                }
                Err(error) => return Err(error.into()),
            }
        }
        let mut items = Vec::with_capacity(files.len() + 1);
        items.push(WaitItem::new(&control, ipc::READABLE | ipc::PEER_CLOSED));
        items.extend(
            files
                .iter()
                .map(|session| WaitItem::new(&session.channel, ipc::READABLE | ipc::PEER_CLOSED)),
        );
        ipc::wait_many(&items, WaitMode::Forever)?;
    }
}

fn open<'a, F: FileSystem>(
    filesystem: &'a F,
    mut request: ReceivedMessage,
) -> Result<FileSession<F::File<'a>>, Error> {
    if request.tag != protocol::OPEN || request.handles().len() != 2 {
        return Err(Error::InvalidRequest);
    }
    let [length] = request.words() else {
        return Err(Error::InvalidRequest);
    };
    let length = usize::try_from(*length).map_err(|_| Error::InvalidRequest)?;
    let frames = take_frames(&mut request, Rights::READ)?;
    if length == 0 || length as u64 > frames.frames()?.saturating_mul(PAGE_SIZE as u64) {
        return Err(Error::InvalidRequest);
    }
    let channel = request
        .take_handle(0)
        .unwrap()
        .downcast::<Channel>()
        .map_err(|_| Error::InvalidRequest)?;
    if !channel.info()?.rights.contains(Rights::READ | Rights::WAIT) {
        return Err(Error::InvalidRequest);
    }
    let mut path = vec![0; length];
    frames.read(0, &mut path)?;
    let path = core::str::from_utf8(&path).map_err(|_| Error::InvalidRequest)?;
    Ok(FileSession {
        channel,
        file: filesystem.open(path)?,
    })
}

fn read(file: &impl ReadAt, mut request: ReceivedMessage) -> Result<Message, Error> {
    if request.tag != protocol::READ || request.handles().len() != 1 {
        return Err(Error::InvalidRequest);
    }
    let [offset, length] = request.words() else {
        return Err(Error::InvalidRequest);
    };
    let mut offset = usize::try_from(*offset).map_err(|_| Error::InvalidRequest)?;
    let length = usize::try_from(*length)
        .map_err(|_| Error::InvalidRequest)?
        .min(file.len().saturating_sub(offset));
    let mut output = take_frames(&mut request, Rights::WRITE)?;
    if length as u64 > output.frames()?.saturating_mul(PAGE_SIZE as u64) {
        return Err(Error::InvalidRequest);
    }
    let mut buffer = [0; PAGE_SIZE];
    let mut done = 0;
    while done < length {
        let count = file.read_at(offset, &mut buffer[..(length - done).min(PAGE_SIZE)])?;
        if count == 0 {
            break;
        }
        output.write(done, &buffer[..count])?;
        done += count;
        offset += count;
    }
    Ok(Message::new(protocol::SUCCESS, &[done as u64])?)
}

fn take_frames(
    request: &mut ReceivedMessage,
    rights: Rights,
) -> Result<Handle<FrameAllocation>, Error> {
    let frames = request
        .take_handle(0)
        .ok_or(Error::InvalidRequest)?
        .downcast::<FrameAllocation>()
        .map_err(|_| Error::InvalidRequest)?;
    if !frames.info()?.rights.contains(rights) {
        return Err(Error::InvalidRequest);
    }
    Ok(frames)
}
