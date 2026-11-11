// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Path operations anchored to directory sessions.

use super::{
    FileSession, Node, PAGE_SIZE, node_request, path, session_response, take_channel, take_frames,
};
use crate::{
    Error,
    namespace::{Directory, Namespace},
    protocol,
};
use alloc::{rc::Rc, vec, vec::Vec};
use runtime::{
    Rights,
    ipc::{Message, ReceivedMessage},
};

fn resolve(
    namespace: &Namespace,
    files: &[FileSession],
    anchor: u64,
    path: &[u8],
    follow: bool,
    missing: bool,
) -> Result<Vec<u8>, Error> {
    let base = if path.starts_with(b"/") || anchor == 0 {
        vec![b'/']
    } else {
        files
            .iter()
            .find_map(|session| match &session.node {
                Node::Directory(directory) if directory.id == anchor => Some(directory.path()),
                _ => None,
            })
            .ok_or(Error::NotFound)??
    };
    namespace.resolve(&base, path, follow, missing)
}

pub(super) fn directory(
    _namespace: &Namespace,
    directory: &Rc<Directory>,
    mut request: ReceivedMessage,
) -> Result<Message, Error> {
    if request.tag != protocol::DIRECTORY_PATH {
        return node_request(&Node::Directory(Rc::clone(directory)), request);
    }
    if !request.words().is_empty() || request.handles().len() != 1 {
        return Err(Error::InvalidRequest);
    }
    let mut output = take_frames(&mut request, Rights::WRITE)?;
    let path = directory.path()?;
    if path.len() >= PAGE_SIZE {
        return Err(Error::NameTooLong);
    }
    output.write(0, &path)?;
    Ok(Message::new(protocol::SUCCESS, &[path.len() as u64])?)
}

pub(super) fn relative(
    namespace: &Namespace,
    files: &mut Vec<FileSession>,
    mut request: ReceivedMessage,
) -> Result<Message, Error> {
    let words = request.words().to_vec();
    let [method, anchor, flags, rest @ ..] = words.as_slice() else {
        return Err(Error::InvalidRequest);
    };
    match (*method, rest) {
        (protocol::STATFS, [length]) if request.handles().len() == 2 && *flags == 0 => {
            let name = path(&mut request, *length)?;
            let name = resolve(namespace, files, *anchor, &name, true, false)?;
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            output.write(0, &namespace.statistics(&name)?.encode())?;
            Ok(Message::new(protocol::SUCCESS, &[])?)
        }
        (protocol::OPEN | protocol::OPEN_DIRECTORY, _) => {
            open_at(namespace, files, *method, *anchor, *flags, rest, request)
        }
        (protocol::METADATA | protocol::READ_LINK, [length]) if request.handles().len() == 2 => {
            if flags & !1 != 0 || *method == protocol::READ_LINK && *flags != 0 {
                return Err(Error::InvalidRequest);
            }
            let path = path(&mut request, *length)?;
            let path = resolve(namespace, files, *anchor, &path, *flags == 1, false)?;
            let mut output = take_frames(&mut request, Rights::WRITE)?;
            if *method == protocol::METADATA {
                output.write(0, &namespace.metadata(&path)?.encode())?;
                Ok(Message::new(protocol::SUCCESS, &[])?)
            } else {
                let bytes = namespace.read_link(&path)?;
                output.write(0, &bytes)?;
                Ok(Message::new(protocol::SUCCESS, &[bytes.len() as u64])?)
            }
        }
        (protocol::MKDIR, [length, permissions]) if request.handles().len() == 1 && *flags == 0 => {
            let permissions = u16::try_from(*permissions).map_err(|_| Error::InvalidRequest)?;
            let mut path = path(&mut request, *length)?;
            trim_slashes(&mut path);
            let path = resolve(namespace, files, *anchor, &path, false, true)?;
            namespace.mkdir_with_permissions(&path, permissions)?;
            Ok(Message::new(protocol::SUCCESS, &[])?)
        }
        (protocol::UNLINK, [length]) if request.handles().len() == 1 && *flags <= 1 => remove(
            namespace,
            files,
            *anchor,
            *flags == 1,
            &mut request,
            *length,
        ),
        (protocol::RENAME | protocol::HARD_LINK, [first, second, target_anchor])
            if request.handles().len() == 2 && *flags <= 1 =>
        {
            if *method == protocol::RENAME && *flags != 0 {
                return Err(Error::InvalidRequest);
            }
            let source = path(&mut request, *first)?;
            let target = path(&mut request, *second)?;
            if *method == protocol::RENAME {
                entry_name(&source)?;
            }
            entry_name(&target)?;
            let source = resolve(namespace, files, *anchor, &source, *flags == 1, false)?;
            let target = resolve(namespace, files, *target_anchor, &target, false, true)?;
            if *method == protocol::RENAME {
                namespace.rename(&source, &target)?;
            } else {
                namespace.hard_link(&source, &target)?;
            }
            Ok(Message::new(protocol::SUCCESS, &[])?)
        }
        (protocol::SYMLINK, [first, second]) if request.handles().len() == 2 && *flags == 0 => {
            let target = path(&mut request, *first)?;
            let path = path(&mut request, *second)?;
            let path = resolve(namespace, files, *anchor, &path, false, true)?;
            namespace.symlink(&target, &path)?;
            Ok(Message::new(protocol::SUCCESS, &[])?)
        }
        _ => Err(Error::InvalidRequest),
    }
}

fn open_at(
    namespace: &Namespace,
    files: &mut Vec<FileSession>,
    method: u64,
    anchor: u64,
    flags: u64,
    words: &[u64],
    mut request: ReceivedMessage,
) -> Result<Message, Error> {
    if request.handles().len() != 2 || flags > 1 {
        return Err(Error::InvalidRequest);
    }
    let node = match (method, words) {
        (protocol::OPEN_DIRECTORY, [length]) => {
            let path = path(&mut request, *length)?;
            let path = resolve(namespace, files, anchor, &path, flags == 1, false)?;
            Node::Directory(Rc::new(namespace.open_directory(&path)?))
        }
        (protocol::OPEN, [length, options, permissions]) => {
            let options = crate::OpenOptions::decode(*options, *permissions)?;
            let path = path(&mut request, *length)?;
            let follow = flags == 1 && !(options.create && options.exclusive);
            let path = resolve(namespace, files, anchor, &path, follow, options.create)?;
            if namespace
                .metadata(&path)
                .is_ok_and(|info| info.mode & 0o170000 == 0o120000)
            {
                return Err(if options.create && options.exclusive {
                    Error::AlreadyExists
                } else {
                    Error::TooManyLinks
                });
            }
            match namespace.open_with(&path, options) {
                Ok(file) => Node::File(file),
                Err(Error::IsDirectory)
                    if !options.write && !options.create && !options.truncate =>
                {
                    Node::Directory(Rc::new(namespace.open_directory(&path)?))
                }
                Err(error) => return Err(error),
            }
        }
        _ => return Err(Error::InvalidRequest),
    };
    let channel = take_channel(&mut request, Rights::READ | Rights::WAIT)?;
    let response = session_response(&node)?;
    files.push(FileSession { channel, node });
    Ok(response)
}

fn trim_slashes(path: &mut Vec<u8>) {
    while path.len() > 1 && path.last() == Some(&b'/') {
        path.pop();
    }
}

fn entry_name(path: &[u8]) -> Result<(), Error> {
    match path
        .rsplit(|byte| *byte == b'/')
        .find(|part| !part.is_empty())
    {
        None | Some(b"." | b"..") => Err(Error::InvalidRequest),
        Some(_) => Ok(()),
    }
}

fn remove(
    namespace: &Namespace,
    files: &[FileSession],
    anchor: u64,
    remove_directory: bool,
    request: &mut ReceivedMessage,
    length: u64,
) -> Result<Message, Error> {
    let mut path = path(request, length)?;
    let trailing = path.ends_with(b"/");
    trim_slashes(&mut path);
    entry_name(&path)?;
    let path = resolve(namespace, files, anchor, &path, false, false)?;
    let info = namespace.metadata(&path)?;
    if trailing && info.mode & 0o170000 != 0o040000 {
        return Err(Error::NotDirectory);
    }
    let directory = info.mode & 0o170000 == 0o040000;
    if directory != remove_directory {
        return Err(if directory {
            Error::IsDirectory
        } else {
            Error::NotDirectory
        });
    }
    namespace.unlink(&path)?;
    Ok(Message::new(protocol::SUCCESS, &[])?)
}
