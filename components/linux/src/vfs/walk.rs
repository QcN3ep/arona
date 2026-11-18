// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Resumable pathname traversal with native directory anchors.

use super::{Lookup, local};
use crate::{
    Services,
    file::OpenFileDescription,
    process::Process,
    syscall::{
        abi::{ELOOP, ENAMETOOLONG, ENOENT, ENOTDIR, Errno},
        file::{completion, filesystem_error},
    },
};
use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use runtime::{Call, Handle};

pub(crate) struct Resolved<'a> {
    pub lookup: Lookup<'a>,
    pub path: Vec<u8>,
    pub info: Option<fs::Metadata>,
    pub local: bool,
}

pub(crate) struct Walk<'a> {
    anchor: Option<Rc<OpenFileDescription<'a>>>,
    path: Vec<u8>,
    pending: VecDeque<Vec<u8>>,
    name: Vec<u8>,
    candidate: Vec<u8>,
    follow: bool,
    missing: bool,
    directory: bool,
    links: usize,
    phase: Phase<'a>,
}

enum Phase<'a> {
    Ready,
    Base(fs::client::PendingRead),
    Metadata(fs::client::PendingMetadata),
    Link(fs::client::PendingRead),
    Directory(fs::client::PendingNode<'a>),
    DirectoryPath(fs::client::PendingRead),
}

impl<'a> Walk<'a> {
    pub fn new(lookup: Lookup<'a>, follow: bool, missing: bool) -> Result<Self, Errno> {
        let phase = match lookup.anchor.as_deref() {
            Some(OpenFileDescription::Directory { session, .. }) => {
                Phase::Base(session.start_path().map_err(filesystem_error)?)
            }
            _ => Phase::Ready,
        };
        let path = match lookup.anchor.as_deref() {
            Some(OpenFileDescription::VirtualDirectory { path, .. }) => path.clone(),
            _ => b"/".to_vec(),
        };
        let pending = lookup
            .name
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        Ok(Self {
            anchor: lookup.anchor,
            path,
            pending,
            name: Vec::new(),
            candidate: Vec::new(),
            follow,
            missing,
            directory: lookup.name.ends_with(b"/"),
            links: 0,
            phase,
        })
    }

    pub fn call(&self) -> Option<&Handle<Call>> {
        match &self.phase {
            Phase::Ready => None,
            Phase::Base(request) | Phase::Link(request) | Phase::DirectoryPath(request) => {
                Some(request.call())
            }
            Phase::Metadata(request) => Some(request.call()),
            Phase::Directory(request) => Some(request.call()),
        }
    }

    pub fn poll(
        &mut self,
        process: &Process<'a>,
        files: &fs::Client<'a>,
        services: &Services<'_>,
    ) -> Option<Result<Resolved<'a>, Errno>> {
        loop {
            match self.step(process, files, services) {
                Ok(Some(result)) => return Some(Ok(result)),
                Ok(None) if matches!(self.phase, Phase::Ready) => {}
                Ok(None) => return None,
                Err(error) => return Some(Err(error)),
            }
        }
    }

    fn step(
        &mut self,
        process: &Process<'a>,
        files: &fs::Client<'a>,
        services: &Services<'_>,
    ) -> Result<Option<Resolved<'a>>, Errno> {
        match &mut self.phase {
            Phase::Base(request) => {
                let Some(result) = completion(request.try_complete()) else {
                    return Ok(None);
                };
                self.path = result.map_err(filesystem_error)?;
                self.phase = Phase::Ready;
            }
            Phase::Ready => {
                self.name = self.pending.pop_front().unwrap_or_else(|| b".".to_vec());
                self.candidate = join(&self.path, &self.name);
                if self.name.len() > 255 || self.candidate.len() >= 4096 {
                    return Err(ENAMETOOLONG);
                }
                if local::contains(&self.candidate) {
                    let info = local::metadata(process, services, &self.candidate);
                    match info {
                        Ok(info) if info.mode & 0o170000 == 0o120000 && self.follows() => {
                            let target = local::link(process, &self.candidate).ok_or(ENOENT)?;
                            self.expand(target)?;
                        }
                        result => return self.accept(files, result, true),
                    }
                } else {
                    let lookup = self.lookup();
                    self.phase = Phase::Metadata(
                        files
                            .start_metadata_at(lookup.directory(), &lookup.name, false)
                            .map_err(filesystem_error)?,
                    );
                }
            }
            Phase::Metadata(request) => {
                let Some(result) = completion(request.try_complete()) else {
                    return Ok(None);
                };
                let result = result.map_err(filesystem_error);
                if result
                    .as_ref()
                    .is_ok_and(|info| info.mode & 0o170000 == 0o120000)
                    && self.follows()
                {
                    let lookup = self.lookup();
                    self.phase = Phase::Link(
                        files
                            .start_read_link_at(lookup.directory(), &lookup.name)
                            .map_err(filesystem_error)?,
                    );
                } else {
                    return self.accept(files, result, false);
                }
            }
            Phase::Link(request) => {
                let Some(result) = completion(request.try_complete()) else {
                    return Ok(None);
                };
                self.expand(result.map_err(filesystem_error)?)?;
            }
            Phase::Directory(request) => {
                let Some(result) = completion(request.try_complete()) else {
                    return Ok(None);
                };
                let fs::client::OpenedNode::Directory(session) =
                    result.map_err(filesystem_error)?
                else {
                    return Err(ENOTDIR);
                };
                self.phase = Phase::DirectoryPath(session.start_path().map_err(filesystem_error)?);
                self.anchor = Some(
                    crate::syscall::file::path::directory_descriptor(session, 0, false).description,
                );
            }
            Phase::DirectoryPath(request) => {
                let Some(result) = completion(request.try_complete()) else {
                    return Ok(None);
                };
                self.path = result.map_err(filesystem_error)?;
                self.phase = Phase::Ready;
            }
        }
        Ok(None)
    }

    fn follows(&self) -> bool {
        self.follow || self.directory || !self.pending.is_empty()
    }

    fn expand(&mut self, target: Vec<u8>) -> Result<(), Errno> {
        self.links += 1;
        if self.links > 40 {
            return Err(ELOOP);
        }
        if target.is_empty() {
            return Err(ENOENT);
        }
        if target.len() >= 4096 {
            return Err(ENAMETOOLONG);
        }
        if target.starts_with(b"/") {
            self.path = b"/".to_vec();
            self.anchor = None;
        }
        if self.pending.is_empty() && target.ends_with(b"/") {
            self.directory = true;
        }
        for part in target
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty())
            .rev()
        {
            self.pending.push_front(part.to_vec());
        }
        self.phase = Phase::Ready;
        Ok(())
    }

    fn accept(
        &mut self,
        files: &fs::Client<'a>,
        result: Result<fs::Metadata, Errno>,
        local: bool,
    ) -> Result<Option<Resolved<'a>>, Errno> {
        let last = self.pending.is_empty();
        let info = match result {
            Err(ENOENT) if last && self.missing && !self.directory => None,
            result => Some(result?),
        };
        if (!last || self.directory) && info.is_none_or(|info| info.mode & 0o170000 != 0o040000) {
            return Err(ENOTDIR);
        }
        if last {
            let lookup = if local {
                Lookup {
                    name: self.candidate.clone(),
                    anchor: None,
                }
            } else {
                self.lookup()
            };
            return Ok(Some(Resolved {
                lookup,
                path: self.candidate.clone(),
                info,
                local,
            }));
        }
        if local {
            self.path = self.candidate.clone();
            self.anchor = None;
            self.phase = Phase::Ready;
        } else {
            let lookup = self.lookup();
            self.phase = Phase::Directory(
                files
                    .start_open_at(
                        lookup.directory(),
                        &lookup.name,
                        fs::OpenOptions {
                            path_only: true,
                            write: false,
                            create: false,
                            exclusive: false,
                            truncate: false,
                            permissions: 0,
                        },
                        false,
                        true,
                    )
                    .map_err(filesystem_error)?,
            );
        }
        Ok(None)
    }

    fn lookup(&self) -> Lookup<'a> {
        if local::contains(&self.path) || self.anchor.is_none() {
            Lookup {
                name: self.candidate.clone(),
                anchor: None,
            }
        } else {
            Lookup {
                name: self.name.clone(),
                anchor: self.anchor.clone(),
            }
        }
    }
}

fn join(base: &[u8], name: &[u8]) -> Vec<u8> {
    if name == b"." {
        return base.to_vec();
    }
    if name == b".." {
        let end = base.iter().rposition(|byte| *byte == b'/').unwrap_or(0);
        return base[..end.max(1)].to_vec();
    }
    let mut path = base.to_vec();
    if path != b"/" {
        path.push(b'/');
    }
    path.extend_from_slice(name);
    path
}
