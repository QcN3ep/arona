// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Timestamp updates for paths and descriptors.

use super::super::{
    Outcome,
    abi::{EBADF, EINVAL, EIO, ENOENT, EOPNOTSUPP, EPERM, Errno, SyscallResult},
};
use super::{Pending, completion, filesystem_error, open, outcome};
use crate::{Services, file::OpenFileDescription, process::Process};
use alloc::{rc::Rc, vec::Vec};
use runtime::{Call, Handle};

/// A requested timestamp field.
enum Time {
    Now,
    Omit,
    At(fs::Timestamp),
}

/// The node whose timestamps are updated.
enum Target<'a> {
    Path {
        directory: Option<Rc<OpenFileDescription<'a>>>,
        name: Vec<u8>,
        follow: bool,
    },
    Descriptor(Rc<OpenFileDescription<'a>>),
    Node(fs::client::OpenedNode<'a>),
}

/// An in-flight timestamp update.
pub(in crate::syscall) struct Utimens<'a> {
    target: Target<'a>,
    times: [Time; 2],
    now: Option<fs::Timestamp>,
    phase: Phase<'a>,
    uid: u32,
    gid: u32,
}

enum Phase<'a> {
    Resolve(fs::client::PendingNode<'a>),
    Clock(devmgr::device::Pending<devmgr::rtc::Timestamp>),
    Metadata(fs::client::PendingMetadata),
    Update(fs::client::Pending<()>),
}

const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
const AT_EMPTY_PATH: u32 = 0x1000;
const UTIME_NOW: i64 = 0x3fff_ffff;
const UTIME_OMIT: i64 = 0x3fff_fffe;
const AT_FDCWD: i32 = -100;

pub(in crate::syscall) fn start<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    services: &Services<'_>,
    arguments: [usize; 6],
) -> Outcome<'a> {
    let prepared = (|| {
        let flags = arguments[3] as u32;
        if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
            return Err(EINVAL);
        }
        let times = read_times(process, arguments[2])?;
        let path = open::path_bytes(process, arguments[1])?;
        let target = if path.is_empty() {
            if flags & AT_EMPTY_PATH == 0 {
                return Err(ENOENT);
            }
            if arguments[0] as i32 == AT_FDCWD {
                Target::Path {
                    directory: process.cwd.clone(),
                    name: b".".to_vec(),
                    follow: true,
                }
            } else {
                descriptor(process, arguments[0])?
            }
        } else {
            let lookup = super::path::Lookup::from_name(process, arguments[0], path)?;
            Target::Path {
                directory: lookup.anchor,
                name: lookup.name,
                follow: flags & AT_SYMLINK_NOFOLLOW == 0,
            }
        };
        let phase = if matches!(times, [Time::Omit, Time::Omit]) {
            start_target(&target, files)?
        } else {
            let clock = services.clock.as_ref().ok_or(EOPNOTSUPP)?;
            Phase::Clock(clock.start_now().map_err(|_| EIO)?)
        };
        Ok(Pending::Utimens(Utimens {
            target,
            times,
            now: None,
            phase,
            uid: process.credentials.euid,
            gid: process.credentials.egid,
        }))
    })();
    outcome(prepared)
}

fn descriptor<'a>(process: &Process<'a>, raw: usize) -> Result<Target<'a>, Errno> {
    let index = usize::try_from(raw as i32).map_err(|_| EBADF)?;
    let description = Rc::clone(&process.files.get(index).ok_or(EBADF)?.description);
    if !matches!(
        description.as_ref(),
        OpenFileDescription::Regular { .. } | OpenFileDescription::Directory { .. }
    ) {
        return Err(EINVAL);
    }
    Ok(Target::Descriptor(description))
}

fn read_times(process: &Process<'_>, address: usize) -> Result<[Time; 2], Errno> {
    if address == 0 {
        return Ok([Time::Now, Time::Now]);
    }
    let mut bytes = [0; 32];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| super::super::abi::EFAULT)?;
    let parse = |offset: usize| {
        let seconds = i64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let nanoseconds = i64::from_le_bytes(bytes[offset + 8..offset + 16].try_into().unwrap());
        match nanoseconds {
            UTIME_NOW => Ok(Time::Now),
            UTIME_OMIT => Ok(Time::Omit),
            _ if !(0..1_000_000_000).contains(&nanoseconds) || seconds < 0 => Err(EINVAL),
            _ => Ok(Time::At(fs::Timestamp {
                seconds,
                nanoseconds: nanoseconds as u32,
            })),
        }
    };
    Ok([parse(0)?, parse(16)?])
}

fn start_target<'a>(target: &Target<'a>, files: &fs::Client<'a>) -> Result<Phase<'a>, Errno> {
    if let Target::Path {
        directory,
        name,
        follow,
    } = target
    {
        let directory = directory.as_ref().and_then(|anchor| match anchor.as_ref() {
            OpenFileDescription::Directory { session, .. } => Some(session),
            _ => None,
        });
        let request = files
            .start_open_at(
                directory,
                name,
                fs::OpenOptions {
                    path_only: true,
                    write: false,
                    create: false,
                    exclusive: false,
                    truncate: false,
                    permissions: 0,
                },
                *follow,
                false,
            )
            .map_err(filesystem_error)?;
        Ok(Phase::Resolve(request))
    } else {
        Ok(Phase::Metadata(metadata(target)?))
    }
}

fn metadata(target: &Target<'_>) -> Result<fs::client::PendingMetadata, Errno> {
    match target {
        Target::Node(fs::client::OpenedNode::File(file)) => file.start_metadata(),
        Target::Node(fs::client::OpenedNode::Directory(directory)) => directory.start_metadata(),
        Target::Descriptor(description) => match description.as_ref() {
            OpenFileDescription::Regular { file, .. } => file.start_metadata(),
            OpenFileDescription::Directory { session, .. } => session.start_metadata(),
            _ => unreachable!(),
        },
        Target::Path { .. } => unreachable!(),
    }
    .map_err(filesystem_error)
}

impl<'a> Utimens<'a> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        match &self.phase {
            Phase::Resolve(request) => Some(request.call()),
            Phase::Clock(request) => Some(request.call()),
            Phase::Metadata(request) => Some(request.call()),
            Phase::Update(request) => Some(request.call()),
        }
    }

    pub fn poll(&mut self, files: &fs::Client<'a>) -> Option<SyscallResult> {
        match &mut self.phase {
            Phase::Resolve(request) => {
                let node = match completion(request.try_complete())? {
                    Ok(node) => node,
                    Err(error) => return Some(Err(filesystem_error(error))),
                };
                self.target = Target::Node(node);
                let request = match metadata(&self.target) {
                    Ok(request) => request,
                    Err(error) => return Some(Err(error)),
                };
                self.phase = Phase::Metadata(request);
                None
            }
            Phase::Clock(request) => {
                let timestamp = match request.try_complete() {
                    Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                        runtime::KernelError::WouldBlock,
                    ))) => return None,
                    Err(_) => return Some(Err(EIO)),
                    Ok(timestamp) => timestamp,
                };
                self.now = Some(fs::Timestamp {
                    seconds: timestamp.seconds,
                    nanoseconds: timestamp.nanoseconds,
                });
                let phase = match start_target(&self.target, files) {
                    Ok(request) => request,
                    Err(error) => return Some(Err(error)),
                };
                self.phase = phase;
                None
            }
            Phase::Metadata(request) => {
                let info = match completion(request.try_complete())? {
                    Ok(info) => info,
                    Err(error) => return Some(Err(filesystem_error(error))),
                };
                if matches!(self.times, [Time::Omit, Time::Omit]) {
                    return Some(Ok(0));
                }
                if self.uid != 0 && self.uid != info.uid {
                    if !matches!(self.times, [Time::Now, Time::Now]) {
                        return Some(Err(EPERM));
                    }
                    if let Err(error) =
                        super::access::permit(&info, super::access::W_OK, self.uid, self.gid)
                    {
                        return Some(Err(error));
                    }
                }
                let now = self.now.unwrap_or_default();
                let resolve = |time: &Time, current: fs::Timestamp| match time {
                    Time::Now => now,
                    Time::Omit => current,
                    Time::At(timestamp) => *timestamp,
                };
                let attributes = fs::Attributes {
                    permissions: info.mode & 0o7777,
                    uid: info.uid,
                    gid: info.gid,
                    accessed: resolve(&self.times[0], info.accessed),
                    modified: resolve(&self.times[1], info.modified),
                    changed: now,
                };
                let request = match start_update(&self.target, attributes) {
                    Ok(request) => request,
                    Err(error) => return Some(Err(error)),
                };
                self.phase = Phase::Update(request);
                None
            }
            Phase::Update(request) => Some(
                completion(request.try_complete())?
                    .map(|()| 0)
                    .map_err(filesystem_error),
            ),
        }
    }
}

fn start_update(
    target: &Target<'_>,
    attributes: fs::Attributes,
) -> Result<fs::client::Pending<()>, Errno> {
    match target {
        Target::Node(fs::client::OpenedNode::File(file)) => file.start_set_attributes(attributes),
        Target::Node(fs::client::OpenedNode::Directory(directory)) => {
            directory.start_set_attributes(attributes)
        }
        Target::Descriptor(description) => match description.as_ref() {
            OpenFileDescription::Regular { file, .. } => file.start_set_attributes(attributes),
            OpenFileDescription::Directory { session, .. } => {
                session.start_set_attributes(attributes)
            }
            _ => unreachable!(),
        },
        Target::Path { .. } => unreachable!(),
    }
    .map_err(filesystem_error)
}
