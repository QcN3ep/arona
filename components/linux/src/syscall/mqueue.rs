// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! POSIX message queues and the `/dev/mqueue` attribute view.

use alloc::{rc::Rc, vec::Vec};
use core::cell::Cell;

use super::{
    Outcome,
    abi::{
        self, EACCES, EBADF, EBUSY, EEXIST, EFAULT, EINVAL, EIO, EMFILE, EMSGSIZE, ENAMETOOLONG,
        ENOENT, ENOMEM, EOPNOTSUPP, ETIMEDOUT, Errno, SyscallResult,
    },
    wait::{self, Deadline},
};
use crate::{
    Services,
    file::{Descriptor, O_NONBLOCK, OpenFileDescription},
    process::{
        Process,
        mqueue::{
            DEFAULT_MAXMSG, DEFAULT_MSGSIZE, DIRECTORY_INODE, MAXMSG_LIMIT, MSGSIZE_LIMIT,
            NAME_MAX, Notify, PRIORITY_MAX, Queue, Queues,
        },
    },
};

const O_ACCMODE: u32 = 3;
const O_CREAT: u32 = 0x40;
const O_EXCL: u32 = 0x80;
const O_CLOEXEC: u32 = 0x80000;

const SIGEV_SIGNAL: i32 = 0;
const SIGEV_NONE: i32 = 1;
const SIGEV_THREAD: i32 = 2;
const SIGEV_THREAD_ID: i32 = 3;
const SI_MESGQ: i32 = -3;

const AT_REMOVEDIR: usize = 0x200;
const MQ_ATTR_SIZE: usize = 64;
const SIGEVENT_SIZE: usize = 64;
const DEV_PREFIX: &[u8] = b"/dev/mqueue";

pub(super) fn dispatch<'a>(
    process: &mut Process<'a>,
    services: &Services<'_>,
    number: usize,
    arguments: [usize; 6],
) -> Option<Outcome<'a>> {
    Some(match number {
        abi::MQ_OPEN => Outcome::Complete(open(process, arguments)),
        abi::MQ_UNLINK => Outcome::Complete(unlink(process, arguments[0])),
        abi::MQ_TIMEDSEND => send(process, services, arguments),
        abi::MQ_TIMEDRECEIVE => receive(process, services, arguments),
        abi::MQ_NOTIFY => Outcome::Complete(notify(process, arguments)),
        abi::MQ_GETSETATTR => Outcome::Complete(getsetattr(process, arguments)),
        _ => return None,
    })
}

fn open(process: &mut Process<'_>, arguments: [usize; 6]) -> SyscallResult {
    let name = name(process, arguments[0])?;
    let oflag = arguments[1] as u32;
    if oflag & !(O_ACCMODE | O_CREAT | O_EXCL | O_NONBLOCK | O_CLOEXEC) != 0
        || !matches!(oflag & O_ACCMODE, 0 | 1)
    {
        return Err(EINVAL);
    }
    let key = name[1..].to_vec();
    let access = oflag & O_ACCMODE;
    let close_on_exec = oflag & O_CLOEXEC != 0;
    if let Some(queue) = process.namespace.mqueues.get(&key) {
        if oflag & O_CREAT != 0 && oflag & O_EXCL != 0 {
            return Err(EEXIST);
        }
        permitted(process, &queue, access)?;
        return install(process, queue, access, oflag, close_on_exec);
    }
    if oflag & O_CREAT == 0 {
        return Err(ENOENT);
    }
    let (maxmsg, msgsize) = create_attributes(process, arguments[3])?;
    let mode = (arguments[2] as u32 & 0o777) & !u32::from(process.umask);
    let queue = Rc::new(Queue::new(
        name,
        process.namespace.mqueues.next_inode(),
        maxmsg,
        msgsize,
        process.credentials.euid,
        process.credentials.egid,
        mode,
    ));
    if !process.namespace.mqueues.insert(key, Rc::clone(&queue)) {
        return Err(EEXIST);
    }
    install(process, queue, access, oflag, close_on_exec)
}

fn install(
    process: &mut Process<'_>,
    queue: Rc<Queue>,
    access: u32,
    oflag: u32,
    close_on_exec: bool,
) -> SyscallResult {
    process
        .files
        .insert(
            Descriptor::message_queue(queue, access | (oflag & O_NONBLOCK), close_on_exec),
            0,
        )
        .ok_or(EMFILE)
}

fn unlink(process: &mut Process<'_>, address: usize) -> SyscallResult {
    let name = name(process, address)?;
    let key = &name[1..];
    let queue = process.namespace.mqueues.get(key).ok_or(ENOENT)?;
    if process.credentials.euid != 0 && process.credentials.euid != queue.uid() {
        return Err(EACCES);
    }
    process
        .namespace
        .mqueues
        .remove(key)
        .map(|_| 0)
        .ok_or(ENOENT)
}

fn create_attributes(process: &Process<'_>, address: usize) -> Result<(usize, usize), Errno> {
    if address == 0 {
        return Ok((DEFAULT_MAXMSG, DEFAULT_MSGSIZE));
    }
    let mut bytes = [0; MQ_ATTR_SIZE];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    let maxmsg = i64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let msgsize = i64::from_le_bytes(bytes[16..24].try_into().unwrap());
    if maxmsg <= 0 || msgsize <= 0 {
        return Err(EINVAL);
    }
    let (maxmsg, msgsize) = (maxmsg as usize, msgsize as usize);
    if maxmsg > MAXMSG_LIMIT || msgsize > MSGSIZE_LIMIT {
        return Err(EINVAL);
    }
    Ok((maxmsg, msgsize))
}

fn getsetattr(process: &mut Process<'_>, arguments: [usize; 6]) -> SyscallResult {
    let description = description(process, arguments[0])?;
    let OpenFileDescription::MessageQueue { queue, .. } = description.as_ref() else {
        return Err(EBADF);
    };
    let previous = description.flags() & O_NONBLOCK;
    if arguments[1] != 0 {
        let mut bytes = [0; MQ_ATTR_SIZE];
        process
            .address_space
            .read(arguments[1], &mut bytes)
            .map_err(|_| EFAULT)?;
        let flags = i64::from_le_bytes(bytes[..8].try_into().unwrap());
        if flags < 0 || flags as u32 & !O_NONBLOCK != 0 {
            return Err(EINVAL);
        }
        description.set_flags(flags as u32);
    }
    if arguments[2] != 0 {
        write_attributes(process, arguments[2], queue, previous)?;
    }
    Ok(0)
}

fn notify(process: &mut Process<'_>, arguments: [usize; 6]) -> SyscallResult {
    let description = description(process, arguments[0])?;
    let OpenFileDescription::MessageQueue { queue, .. } = description.as_ref() else {
        return Err(EBADF);
    };
    if arguments[1] == 0 {
        queue.clear_notify(process.pid);
        return Ok(0);
    }
    if queue.receivers() != 0 {
        return Err(EBUSY);
    }
    let mut bytes = [0; SIGEVENT_SIZE];
    process
        .address_space
        .read(arguments[1], &mut bytes)
        .map_err(|_| EFAULT)?;
    let value = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    let signo = i32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let kind = i32::from_le_bytes(bytes[12..16].try_into().unwrap());
    match kind {
        SIGEV_SIGNAL => {
            if !(1..=64).contains(&signo) {
                return Err(EINVAL);
            }
        }
        SIGEV_NONE => {}
        SIGEV_THREAD | SIGEV_THREAD_ID => {
            // SIGEV_THREAD would need a guest thread that runs a userspace callback.
            return Err(EOPNOTSUPP);
        }
        _ => return Err(EINVAL),
    }
    let notification = Notify {
        pid: process.pid,
        value,
        signo,
        notify: kind,
    };
    if !queue.register_notify(notification) {
        return Err(EBUSY);
    }
    process
        .namespace
        .mqueues
        .track_notification(process.pid, queue);
    if !queue.is_empty()
        && let Some(notification) = queue.take_notify()
    {
        signal(process, notification);
    }
    Ok(0)
}

fn send<'a>(
    process: &mut Process<'a>,
    services: &Services<'_>,
    arguments: [usize; 6],
) -> Outcome<'a> {
    let operation = (|| -> Result<Started<'a>, Errno> {
        let description = description(process, arguments[0])?;
        let OpenFileDescription::MessageQueue { queue, flags } = description.as_ref() else {
            return Err(EBADF);
        };
        if !matches!(flags.get() & O_ACCMODE, 1 | 2) {
            return Err(EBADF);
        }
        if arguments[2] > queue.msgsize() {
            return Err(EMSGSIZE);
        }
        let priority = arguments[3] as u32;
        if priority >= PRIORITY_MAX {
            return Err(EINVAL);
        }
        let data = wait::read_bytes(process, arguments[1], arguments[2])?;
        let queue = Rc::clone(queue);
        if queue.is_full() {
            if flags.get() & O_NONBLOCK != 0 {
                return Err(abi::EAGAIN);
            }
            return Ok(Started::Pending(Pending::Send {
                description,
                data,
                priority,
                deadline: deadline(process, services, arguments[4])?,
            }));
        }
        let was_empty = queue.is_empty();
        queue.push(priority, data).map_err(|_| ENOMEM)?;
        if was_empty && queue.receivers() == 0 {
            deliver(process, &queue);
        }
        Ok(Started::Complete(Ok(0)))
    })();
    match operation {
        Ok(Started::Pending(pending)) => Outcome::Pending(super::Operation::Mqueue(pending)),
        Ok(Started::Complete(result)) => Outcome::Complete(result),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

fn receive<'a>(
    process: &mut Process<'a>,
    services: &Services<'_>,
    arguments: [usize; 6],
) -> Outcome<'a> {
    let operation = (|| -> Result<Started<'a>, Errno> {
        let description = description(process, arguments[0])?;
        let OpenFileDescription::MessageQueue { queue, flags } = description.as_ref() else {
            return Err(EBADF);
        };
        if flags.get() & O_ACCMODE == 1 {
            return Err(EBADF);
        }
        if arguments[2] < queue.msgsize() {
            return Err(EMSGSIZE);
        }
        if arguments[1] == 0 || arguments[3] == 0 {
            return Err(EFAULT);
        }
        let queue = Rc::clone(queue);
        if let Some(message) = queue.pop() {
            return Ok(Started::Complete(copy_message(
                process,
                arguments[1],
                arguments[3],
                message.data,
                message.priority,
            )));
        }
        if flags.get() & O_NONBLOCK != 0 {
            return Err(abi::EAGAIN);
        }
        Ok(Started::Pending(Pending::Receive {
            description,
            address: arguments[1],
            priority: arguments[3],
            deadline: deadline(process, services, arguments[4])?,
            _receiver: queue.receive_guard(),
        }))
    })();
    match operation {
        Ok(Started::Pending(pending)) => Outcome::Pending(super::Operation::Mqueue(pending)),
        Ok(Started::Complete(result)) => Outcome::Complete(result),
        Err(error) => Outcome::Complete(Err(error)),
    }
}
enum Started<'a> {
    Complete(SyscallResult),
    Pending(Pending<'a>),
}

pub(super) enum Pending<'a> {
    Send {
        description: Rc<OpenFileDescription<'a>>,
        data: Vec<u8>,
        priority: u32,
        deadline: Deadline,
    },
    Receive {
        description: Rc<OpenFileDescription<'a>>,
        address: usize,
        priority: usize,
        deadline: Deadline,
        _receiver: crate::process::mqueue::Receiver,
    },
}

impl Pending<'_> {
    pub fn waits<'a>(&'a self, items: &mut Vec<runtime::ipc::WaitItem<'a>>) {
        match self {
            Self::Send { deadline, .. } | Self::Receive { deadline, .. } => deadline.waits(items),
        }
    }

    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        match self {
            Self::Send {
                description,
                data,
                priority,
                deadline,
            } => {
                let OpenFileDescription::MessageQueue { queue, .. } = description.as_ref() else {
                    unreachable!()
                };
                if queue.is_full() {
                    return expired(deadline);
                }
                let was_empty = queue.is_empty();
                let data = core::mem::take(data);
                let priority = *priority;
                if queue.push(priority, data).is_err() {
                    return Some(Err(ENOMEM));
                }
                if was_empty && queue.receivers() == 0 {
                    deliver(process, queue);
                }
                Some(Ok(0))
            }
            Self::Receive {
                description,
                address,
                priority,
                deadline,
                ..
            } => {
                let OpenFileDescription::MessageQueue { queue, .. } = description.as_ref() else {
                    unreachable!()
                };
                if let Some(message) = queue.pop() {
                    return Some(copy_message(
                        process,
                        *address,
                        *priority,
                        message.data,
                        message.priority,
                    ));
                }
                expired(deadline)
            }
        }
    }
}

fn expired(deadline: &Deadline) -> Option<SyscallResult> {
    match deadline.expired() {
        Ok(true) => Some(Err(ETIMEDOUT)),
        Ok(false) => None,
        Err(error) => Some(Err(error)),
    }
}

fn copy_message(
    process: &Process<'_>,
    address: usize,
    priority: usize,
    data: Vec<u8>,
    value: u32,
) -> SyscallResult {
    wait::write_bytes(process, address, &data)?;
    wait::write_bytes(process, priority, &value.to_le_bytes())?;
    Ok(data.len())
}

fn deadline(
    process: &Process<'_>,
    services: &Services<'_>,
    address: usize,
) -> Result<Deadline, Errno> {
    if address == 0 {
        return Deadline::new(process.supervisor, None);
    }
    let target = absolute(process, address)?;
    let clock = services.clock.as_ref().ok_or(EOPNOTSUPP)?;
    let now = runtime::thread::now_ns().map_err(|_| EIO)?;
    let timestamp = clock
        .now(now.saturating_add(1_000_000_000))
        .map_err(|_| EIO)?;
    let current = i128::from(timestamp.seconds) * 1_000_000_000 + i128::from(timestamp.nanoseconds);
    let remaining = u64::try_from((i128::from(target) - current).max(0)).unwrap_or(u64::MAX);
    Deadline::new(process.supervisor, Some(remaining))
}

fn absolute(process: &Process<'_>, address: usize) -> Result<u64, Errno> {
    let mut bytes = [0; 16];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    let seconds = i64::from_le_bytes(bytes[..8].try_into().unwrap());
    let nanoseconds = i64::from_le_bytes(bytes[8..].try_into().unwrap());
    if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(EINVAL);
    }
    (seconds as u64)
        .checked_mul(1_000_000_000)
        .and_then(|time| time.checked_add(nanoseconds as u64))
        .ok_or(EINVAL)
}

fn description<'a>(
    process: &Process<'a>,
    raw: usize,
) -> Result<Rc<OpenFileDescription<'a>>, Errno> {
    let fd = usize::try_from(raw as i32).map_err(|_| EBADF)?;
    Ok(Rc::clone(&process.files.get(fd).ok_or(EBADF)?.description))
}

fn name(process: &Process<'_>, address: usize) -> Result<Vec<u8>, Errno> {
    let path = super::file::path_bytes(process, address)?;
    if path.is_empty() {
        return Err(ENOENT);
    }
    if path[0] != b'/' || path.len() == 1 {
        return Err(EINVAL);
    }
    if path.len() > NAME_MAX + 1 {
        return Err(ENAMETOOLONG);
    }
    if path[1..].contains(&b'/') {
        return Err(EINVAL);
    }
    Ok(path)
}

fn permitted(process: &Process<'_>, queue: &Queue, access: u32) -> Result<(), Errno> {
    let euid = process.credentials.euid;
    if euid == 0 {
        return Ok(());
    }
    let bits = if euid == queue.uid() {
        (queue.mode() >> 6) & 7
    } else if process.credentials.egid == queue.gid() {
        (queue.mode() >> 3) & 7
    } else {
        queue.mode() & 7
    };
    let required = if access == 1 { 2 } else { 4 };
    if bits & required == 0 {
        return Err(EACCES);
    }
    Ok(())
}

fn deliver(process: &Process<'_>, queue: &Queue) {
    if let Some(notification) = queue.take_notify() {
        signal(process, notification);
    }
}

fn signal(process: &Process<'_>, notification: Notify) {
    if notification.notify != SIGEV_SIGNAL {
        return;
    }
    let _ = process.namespace.signal_process(
        notification.pid,
        crate::process::signal::Info {
            number: notification.signo as u8,
            code: SI_MESGQ,
            sender: process.pid,
            uid: process.credentials.euid,
            address: 0,
            value: notification.value,
        },
    );
}

/// Format the `/dev/mqueue/<name>` attribute file.
fn attributes(queue: &Queue) -> Vec<u8> {
    let (notify, signo) = queue
        .notify()
        .map_or((0, 0), |notify| (notify.notify, notify.signo));
    alloc::format!(
        "QSIZE:{:<10} NOTIFY:{:<5} SIGEV:{:<5}\n",
        queue.current(),
        notify,
        signo
    )
    .into_bytes()
}

fn write_attributes(
    process: &Process<'_>,
    address: usize,
    queue: &Queue,
    flags: u32,
) -> Result<(), Errno> {
    let mut bytes = [0; MQ_ATTR_SIZE];
    for (offset, value) in [
        (0, u64::from(flags)),
        (8, queue.maxmsg() as u64),
        (16, queue.msgsize() as u64),
        (24, queue.current() as u64),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)
}

fn split_path(path: &[u8]) -> Option<Result<Option<&[u8]>, Errno>> {
    if path == DEV_PREFIX || path == b"/dev/mqueue/" {
        return Some(Ok(None));
    }
    let name = path.strip_prefix(b"/dev/mqueue/")?;
    if name.is_empty() {
        return Some(Ok(None));
    }
    if name.len() > NAME_MAX {
        return Some(Err(ENAMETOOLONG));
    }
    if name.contains(&b'/') {
        return Some(Err(no_dir()));
    }
    Some(Ok(Some(name)))
}

fn no_dir() -> Errno {
    abi::ENOTDIR
}

/// Open `/dev/mqueue` or one queue's attribute file.
pub(crate) fn open_path(
    process: &mut Process<'_>,
    path: &[u8],
    flags: u32,
) -> Option<SyscallResult> {
    let entry = match split_path(path)? {
        Ok(entry) => entry,
        Err(error) => return Some(Err(error)),
    };
    let close_on_exec = flags & O_CLOEXEC != 0;
    Some(match entry {
        None => {
            if flags & (O_ACCMODE | O_CREAT | 0x200) != 0 {
                return Some(Err(abi::EISDIR));
            }
            process
                .files
                .insert(
                    Descriptor::virtual_directory(DIRECTORY_INODE, flags, close_on_exec),
                    0,
                )
                .ok_or(EMFILE)
        }
        Some(name) => {
            let Some(queue) = process.namespace.mqueues.get(name) else {
                return Some(Err(ENOENT));
            };
            if flags & 0x10000 != 0 {
                return Some(Err(abi::ENOTDIR));
            }
            if flags & (O_ACCMODE | O_CREAT | 0x200) != 0 {
                return Some(Err(abi::EROFS));
            }
            if let Err(error) = permitted(process, &queue, 0) {
                return Some(Err(error));
            }
            process
                .files
                .insert(Descriptor::mqueue_file(queue, flags, close_on_exec), 0)
                .ok_or(EMFILE)
        }
    })
}

/// Remove a queue through `unlinkat("/dev/mqueue/<name>")`.
pub(crate) fn unlink_path(
    process: &mut Process<'_>,
    path: &[u8],
    flags: usize,
) -> Option<SyscallResult> {
    let entry = match split_path(path)? {
        Ok(entry) => entry,
        Err(error) => return Some(Err(error)),
    };
    Some(match entry {
        None => Err(EBUSY),
        Some(name) => {
            if flags & !AT_REMOVEDIR != 0 {
                return Some(Err(EINVAL));
            }
            if flags & AT_REMOVEDIR != 0 {
                return Some(Err(abi::ENOTDIR));
            }
            let queue = match process.namespace.mqueues.get(name) {
                Some(queue) => queue,
                None => return Some(Err(ENOENT)),
            };
            if process.credentials.euid != 0 && process.credentials.euid != queue.uid() {
                return Some(Err(EACCES));
            }
            process
                .namespace
                .mqueues
                .remove(name)
                .map(|_| 0)
                .ok_or(ENOENT)
        }
    })
}

/// Produce metadata for `/dev/mqueue` entries without touching the filesystem service.
pub(crate) fn path_metadata(
    process: &Process<'_>,
    path: &[u8],
) -> Option<Result<fs::Metadata, Errno>> {
    let entry = match split_path(path)? {
        Ok(entry) => entry,
        Err(error) => return Some(Err(error)),
    };
    Some(match entry {
        None => Ok(fs::Metadata {
            inode: DIRECTORY_INODE,
            mode: 0o041777,
            links: 2,
            ..fs::Metadata::default()
        }),
        Some(name) => {
            let queue = match process.namespace.mqueues.get(name) {
                Some(queue) => queue,
                None => return Some(Err(ENOENT)),
            };
            Ok(fs::Metadata {
                inode: queue.inode(),
                mode: 0o100444,
                links: 1,
                size: attributes(&queue).len() as u64,
                uid: queue.uid(),
                gid: queue.gid(),
                ..fs::Metadata::default()
            })
        }
    })
}

/// Build the dynamic directory entries exposed at `/dev/mqueue`.
pub(crate) fn entries(queues: &Queues) -> Vec<fs::DirEntry> {
    let mut entries = alloc::vec![
        fs::DirEntry {
            name: b".".to_vec(),
            inode: DIRECTORY_INODE,
            kind: fs::FileType::Directory,
        },
        fs::DirEntry {
            name: b"..".to_vec(),
            inode: DIRECTORY_INODE,
            kind: fs::FileType::Directory,
        },
    ];
    for queue in queues.list() {
        entries.push(fs::DirEntry {
            name: queue.name()[1..].to_vec(),
            inode: queue.inode(),
            kind: fs::FileType::Regular,
        });
    }
    entries
}

pub(crate) fn attribute_len(queue: &Queue) -> usize {
    attributes(queue).len()
}

pub(crate) fn read_attribute(
    process: &Process<'_>,
    queue: &Queue,
    offset: usize,
    buffer: super::file::Buffer,
) -> SyscallResult {
    let data = attributes(queue);
    if offset > data.len() {
        return Ok(0);
    }
    let buffers = buffer.import(process)?;
    let length = (data.len() - offset).min(buffers.length());
    let mut bytes = data[offset..offset + length].to_vec();
    buffers.scatter(process, &mut bytes)
}

pub(crate) fn read_attribute_file(
    process: &Process<'_>,
    queue: &Queue,
    offset: &Cell<usize>,
    buffer: super::file::Buffer,
) -> SyscallResult {
    let start = offset.get();
    let count = read_attribute(process, queue, start, buffer)?;
    offset.set(start.saturating_add(count));
    Ok(count)
}
