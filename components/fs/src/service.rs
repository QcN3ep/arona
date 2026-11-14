// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Mount management, file sessions, and request dispatch.

mod path;

use crate::{
    Error, ReadAt,
    archive::Archive,
    block::Disk,
    namespace::{Directory, File, Namespace},
    protocol,
};
use alloc::{rc::Rc, vec, vec::Vec};
use runtime::{
    Channel, FrameAllocation, Handle, KernelError, Rights,
    ipc::{self, Message, ReceivedMessage, WaitItem, WaitMode},
};

const PAGE_SIZE: usize = 4096;

struct FileSession {
    channel: Handle<Channel>,
    node: Node,
}

enum Node {
    File(File),
    Path(File),
    Directory(Rc<Directory>),
}

pub fn run(task: &Handle<runtime::Task>, control: Handle<Channel>) -> Result<(), Error> {
    let mut startup = control.receive(WaitMode::Forever)?;
    let reply = startup.take_reply().ok_or(Error::InvalidRequest)?;
    let archive = match load_archive(startup) {
        Ok(archive) => archive,
        Err(error) => {
            protocol::respond(reply, Err(error))?;
            return Err(error);
        }
    };
    protocol::respond(reply, Ok(Message::new(protocol::SUCCESS, &[])?))?;
    serve(task, control, &Namespace::new(archive))
}

fn load_archive(mut startup: ReceivedMessage) -> Result<Archive, Error> {
    if startup.tag != protocol::START || startup.handles().len() != 1 {
        return Err(Error::InvalidRequest);
    }
    let [length] = *startup.words() else {
        return Err(Error::InvalidRequest);
    };
    let frames = take_frames(&mut startup, Rights::READ)?;
    Archive::new(
        frames,
        usize::try_from(length).map_err(|_| Error::InvalidRequest)?,
    )
}

pub fn serve(
    task: &Handle<runtime::Task>,
    control: Handle<Channel>,
    namespace: &Namespace,
) -> Result<(), Error> {
    let mut clients = Vec::new();
    let mut files = Vec::new();
    loop {
        poll_files(namespace, &mut files)?;
        match control.receive(WaitMode::Poll) {
            Ok(mut request) => {
                if let Some(reply) = request.take_reply() {
                    protocol::respond(reply, admin(task, namespace, &mut clients, request))?;
                }
            }
            Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                drop(files);
                return namespace.shutdown();
            }
            Err(error) => return Err(error.into()),
        }
        let mut index = 0;
        while index < clients.len() {
            match clients[index].receive(WaitMode::Poll) {
                Ok(mut request) => {
                    if let Some(reply) = request.take_reply() {
                        let response = client(namespace, &mut files, request);
                        protocol::respond(reply, response)?;
                    }
                    index += 1;
                }
                Err(runtime::Error::Kernel(KernelError::WouldBlock)) => index += 1,
                Err(runtime::Error::Kernel(KernelError::EndOfStream)) => {
                    clients.swap_remove(index);
                }
                Err(error) => return Err(error.into()),
            }
        }
        let mut items = Vec::with_capacity(files.len() + clients.len() + 1);
        items.push(WaitItem::new(&control, ipc::READABLE | ipc::PEER_CLOSED));
        items.extend(
            clients
                .iter()
                .map(|channel| WaitItem::new(channel, ipc::READABLE | ipc::PEER_CLOSED)),
        );
        items.extend(
            files
                .iter()
                .map(|session| WaitItem::new(&session.channel, ipc::READABLE | ipc::PEER_CLOSED)),
        );
        ipc::wait_many(&items, WaitMode::Forever)?;
    }
}

fn poll_files(namespace: &Namespace, files: &mut Vec<FileSession>) -> Result<(), Error> {
    let mut index = 0;
    while index < files.len() {
        match files[index].channel.receive(WaitMode::Poll) {
            Ok(mut request) => {
                if let Some(reply) = request.take_reply() {
                    let response = if let Node::Directory(directory) = &files[index].node {
                        let directory = Rc::clone(directory);
                        path::directory(namespace, &directory, request)
                    } else {
                        node_request(&files[index].node, request)
                    };
                    protocol::respond(reply, response)?;
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
    Ok(())
}

fn admin(
    task: &Handle<runtime::Task>,
    namespace: &Namespace,
    clients: &mut Vec<Handle<Channel>>,
    mut request: ReceivedMessage,
) -> Result<Message, Error> {
    match (request.tag, request.words()) {
        (protocol::CONNECT, []) if request.handles().len() == 1 => {
            clients.push(take_channel(&mut request, Rights::READ | Rights::WAIT)?);
        }
        (protocol::MOUNT, [length, start, size]) if request.handles().len() == 2 => {
            let (length, start, size) = (*length, *start, *size);
            let path = path(&mut request, length)?;
            let channel = take_channel(&mut request, protocol::CHANNEL_RIGHTS)?;
            let session = devmgr::block::Session::from_channel(channel)?;
            let disk = Rc::new(Disk::new(task, session)?);
            namespace.mount(&path, disk, start, size)?;
        }
        (protocol::TMPFS, [length, capacity]) if request.handles().len() == 1 => {
            let (length, capacity) = (*length, *capacity);
            let capacity = usize::try_from(capacity).map_err(|_| Error::InvalidRequest)?;
            namespace.mount_tmpfs(&path(&mut request, length)?, capacity)?;
        }
        (protocol::UNMOUNT, [length]) if request.handles().len() == 1 => {
            let length = *length;
            namespace.unmount(&path(&mut request, length)?)?;
        }
        _ => return Err(Error::InvalidRequest),
    }
    Ok(Message::new(protocol::SUCCESS, &[])?)
}

fn client(
    namespace: &Namespace,
    files: &mut Vec<FileSession>,
    request: ReceivedMessage,
) -> Result<Message, Error> {
    if request.tag == protocol::RELATIVE {
        return path::relative(namespace, files, request);
    }
    if matches!(
        request.tag,
        protocol::OPEN | protocol::CREATE | protocol::OPEN_DIRECTORY
    ) {
        let session = open(namespace, request)?;
        let response = session_response(&session.node)?;
        files.push(session);
        return Ok(response);
    }
    directory(namespace, request)
}

fn directory(namespace: &Namespace, mut request: ReceivedMessage) -> Result<Message, Error> {
    match (request.tag, request.words()) {
        (protocol::MKDIR | protocol::UNLINK, [length]) if request.handles().len() == 1 => {
            let length = *length;
            let name = path(&mut request, length)?;
            if request.tag == protocol::MKDIR {
                namespace.mkdir(&name)?;
            } else {
                namespace.unlink(&name)?;
            }
        }
        (protocol::RENAME | protocol::HARD_LINK | protocol::SYMLINK, [first, second])
            if request.handles().len() == 2 =>
        {
            let (first, second) = (*first, *second);
            let source = path(&mut request, first)?;
            let target = path(&mut request, second)?;
            match request.tag {
                protocol::HARD_LINK => namespace.hard_link(&source, &target)?,
                protocol::SYMLINK => namespace.symlink(&source, &target)?,
                _ => namespace.rename(&source, &target)?,
            }
        }
        (protocol::METADATA | protocol::READ_LINK, [length]) if request.handles().len() == 2 => {
            let length = *length;
            let name = path(&mut request, length)?;
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            if request.tag == protocol::METADATA {
                output.write(0, &namespace.metadata(&name)?.encode())?;
                return Ok(Message::new(protocol::SUCCESS, &[])?);
            }
            let bytes = namespace.read_link(&name)?;
            output.write(0, &bytes)?;
            return Ok(Message::new(protocol::SUCCESS, &[bytes.len() as u64])?);
        }
        (protocol::LIST, [length, start]) if request.handles().len() == 2 => {
            let (length, start) = (*length, *start);
            let name = path(&mut request, length)?;
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            return list(namespace.read_dir(&name)?, start, &mut output);
        }
        _ => return Err(Error::InvalidRequest),
    }
    Ok(Message::new(protocol::SUCCESS, &[])?)
}

fn list(
    entries: Vec<crate::DirEntry>,
    start: u64,
    output: &mut Handle<FrameAllocation>,
) -> Result<Message, Error> {
    if output.frames()? == 0 {
        return Err(Error::InvalidRequest);
    }
    let start = usize::try_from(start).map_err(|_| Error::InvalidRequest)?;
    let mut bytes = [0; PAGE_SIZE];
    let (mut done, mut count) = (0, 0);
    for entry in entries.iter().skip(start) {
        let size = entry.name.len();
        if done + 11 + size > bytes.len() {
            break;
        }
        bytes[done..done + 8].copy_from_slice(&entry.inode.to_le_bytes());
        bytes[done + 8] = entry.kind as u8;
        bytes[done + 9..done + 11].copy_from_slice(&(size as u16).to_le_bytes());
        bytes[done + 11..done + 11 + size].copy_from_slice(&entry.name);
        done += 11 + size;
        count += 1;
    }
    output.write(0, &bytes[..done])?;
    Ok(Message::new(protocol::SUCCESS, &[count, done as u64])?)
}

fn node_request(node: &Node, mut request: ReceivedMessage) -> Result<Message, Error> {
    if matches!(node, Node::Path(_))
        && !matches!(
            request.tag,
            protocol::METADATA | protocol::STATFS | protocol::READ_LINK
        )
    {
        return Err(Error::InvalidRequest);
    }
    match (request.tag, request.words()) {
        (protocol::LIST, [start, 1]) if request.handles().len() == 1 => {
            let start = *start;
            let Node::Directory(directory) = node else {
                return Err(Error::NotDirectory);
            };
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            list(directory.read_dir_all()?, start, &mut output)
        }
        (protocol::FSYNC, []) if request.handles().is_empty() => {
            match node {
                Node::File(file) | Node::Path(file) => file.fsync()?,
                Node::Directory(directory) => directory.fsync()?,
            }
            Ok(Message::new(protocol::SUCCESS, &[])?)
        }
        (protocol::METADATA, []) if request.handles().len() == 1 => {
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            let metadata = match node {
                Node::File(file) | Node::Path(file) => file.metadata()?,
                Node::Directory(directory) => directory.metadata()?,
            };
            output.write(0, &metadata.encode())?;
            Ok(Message::new(protocol::SUCCESS, &[])?)
        }
        (protocol::STATFS, []) if request.handles().len() == 1 => {
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            let statistics = match node {
                Node::File(file) | Node::Path(file) => file.statistics()?,
                Node::Directory(directory) => directory.statistics()?,
            };
            output.write(0, &statistics.encode())?;
            Ok(Message::new(protocol::SUCCESS, &[])?)
        }
        (protocol::READ_LINK, []) if request.handles().len() == 1 => {
            let Node::Path(file) = node else {
                return Err(Error::InvalidRequest);
            };
            let bytes = file.read_link()?;
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            output.write(0, &bytes)?;
            Ok(Message::new(protocol::SUCCESS, &[bytes.len() as u64])?)
        }
        (protocol::SET_ATTRIBUTES, []) if request.handles().len() == 1 => {
            let input = take_frames(&mut request, Rights::READ)?;
            let mut bytes = [0; crate::Attributes::SIZE];
            input.read(0, &mut bytes)?;
            let attributes = crate::Attributes::decode(&bytes)?;
            match node {
                Node::File(file) | Node::Path(file) => file.set_attributes(attributes)?,
                Node::Directory(directory) => directory.set_attributes(attributes)?,
            }
            Ok(Message::new(protocol::SUCCESS, &[])?)
        }
        (protocol::LIST, [start]) if request.handles().len() == 1 => {
            let start = *start;
            let Node::Directory(directory) = node else {
                return Err(Error::NotDirectory);
            };
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            list(directory.read_dir()?, start, &mut output)
        }
        _ => match node {
            Node::File(file) | Node::Path(file) => file_request(file, request),
            Node::Directory(_) => Err(Error::IsDirectory),
        },
    }
}

fn file_request(file: &File, mut request: ReceivedMessage) -> Result<Message, Error> {
    match (request.tag, request.words()) {
        (protocol::READ, _) => read(file, request),
        (protocol::WRITE | protocol::APPEND, [offset, length]) if request.handles().len() == 1 => {
            let offset = usize::try_from(*offset).map_err(|_| Error::InvalidRequest)?;
            let length = usize::try_from(*length).map_err(|_| Error::InvalidRequest)?;
            if request.tag == protocol::APPEND && offset != 0 {
                return Err(Error::InvalidRequest);
            }
            offset
                .checked_add(length)
                .filter(|end| *end <= 2 * 1024 * 1024 * 1024)
                .ok_or(Error::FileTooLarge)?;
            let input = take_frames(&mut request, Rights::READ)?;
            if length as u64 > input.frames()?.saturating_mul(PAGE_SIZE as u64) {
                return Err(Error::InvalidRequest);
            }
            let mut bytes = vec![0; length];
            input.read(0, &mut bytes)?;
            if request.tag == protocol::APPEND {
                let (offset, count) = file.append(&bytes)?;
                return Ok(Message::new(
                    protocol::SUCCESS,
                    &[count as u64, file.len() as u64, offset as u64],
                )?);
            }
            let count = file.write_at(offset, &bytes)?;
            Ok(Message::new(
                protocol::SUCCESS,
                &[count as u64, file.len() as u64],
            )?)
        }
        (protocol::TRUNCATE, [size]) if request.handles().is_empty() => {
            let size = usize::try_from(*size).map_err(|_| Error::InvalidRequest)?;
            file.truncate(size)?;
            Ok(Message::new(protocol::SUCCESS, &[size as u64])?)
        }
        (protocol::FILE_INFO, []) if request.handles().is_empty() => {
            Ok(Message::new(protocol::SUCCESS, &[file.len() as u64])?)
        }
        _ => Err(Error::InvalidRequest),
    }
}

fn open(namespace: &Namespace, mut request: ReceivedMessage) -> Result<FileSession, Error> {
    if !matches!(
        request.tag,
        protocol::OPEN | protocol::CREATE | protocol::OPEN_DIRECTORY
    ) || request.handles().len() != 2
    {
        return Err(Error::InvalidRequest);
    }
    let (length, options) = match *request.words() {
        [length] => (length, None),
        [length, flags, permissions] if request.tag == protocol::OPEN => (
            length,
            Some(crate::OpenOptions::decode(flags, permissions)?),
        ),
        _ => return Err(Error::InvalidRequest),
    };
    let path = path(&mut request, length)?;
    let channel = take_channel(&mut request, Rights::READ | Rights::WAIT)?;
    Ok(FileSession {
        channel,
        node: match request.tag {
            protocol::CREATE => Node::File(namespace.create(&path)?),
            protocol::OPEN_DIRECTORY => Node::Directory(Rc::new(namespace.open_directory(&path)?)),
            protocol::OPEN if options.is_some() => {
                let options = options.unwrap();
                let file = namespace.open_with(&path, options)?;
                if options.path_only {
                    Node::Path(file)
                } else {
                    Node::File(file)
                }
            }
            _ => Node::File(namespace.open_bytes(&path)?),
        },
    })
}

fn path(request: &mut ReceivedMessage, length: u64) -> Result<Vec<u8>, Error> {
    let length = usize::try_from(length).map_err(|_| Error::InvalidRequest)?;
    if length == 0 || length > 4096 {
        return Err(Error::InvalidRequest);
    }
    let frames = take_frames(request, Rights::READ)?;
    if length as u64 > frames.frames()?.saturating_mul(PAGE_SIZE as u64) {
        return Err(Error::InvalidRequest);
    }
    let mut path = vec![0; length];
    frames.read(0, &mut path)?;
    Ok(path)
}

fn read(file: &impl ReadAt, mut request: ReceivedMessage) -> Result<Message, Error> {
    if request.tag != protocol::READ || request.handles().len() != 1 {
        return Err(Error::InvalidRequest);
    }
    let [offset, length] = *request.words() else {
        return Err(Error::InvalidRequest);
    };
    let mut offset = usize::try_from(offset).map_err(|_| Error::InvalidRequest)?;
    let length = usize::try_from(length)
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
        if count > (length - done).min(PAGE_SIZE) {
            return Err(Error::InvalidData);
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

fn take_channel(request: &mut ReceivedMessage, rights: Rights) -> Result<Handle<Channel>, Error> {
    let channel = request
        .take_handle(0)
        .ok_or(Error::InvalidRequest)?
        .downcast::<Channel>()
        .map_err(|_| Error::InvalidRequest)?;
    if !channel.info()?.rights.contains(rights) {
        return Err(Error::InvalidRequest);
    }
    Ok(channel)
}

fn session_response(node: &Node) -> Result<Message, Error> {
    Ok(match node {
        Node::File(file) | Node::Path(file) => {
            Message::new(protocol::SUCCESS, &[file.len() as u64])?
        }
        Node::Directory(directory) => Message::new(protocol::SUCCESS, &[0, directory.id])?,
    })
}
