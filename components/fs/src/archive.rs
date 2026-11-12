// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Read-only index of CPIO newc archives.

use alloc::{collections::BTreeMap, rc::Rc, vec, vec::Vec};
use core::ops::Range;
use runtime::{FrameAllocation, Handle};

use crate::{Error, FileSystem, ReadAt, path::normalize};

const PAGE_SIZE: usize = 4096;
const HEADER_SIZE: usize = 110;

pub struct Archive {
    device: u64,
    mount: u64,
    frames: Rc<Handle<FrameAllocation>>,
    length: usize,
    entries: BTreeMap<Vec<u8>, Entry>,
}

struct Entry {
    range: Range<usize>,
    metadata: crate::Metadata,
    identity: (usize, usize, usize),
}

pub struct File {
    frames: Rc<Handle<FrameAllocation>>,
    range: Range<usize>,
    metadata: crate::Metadata,
}

pub struct Directory {
    _frames: Rc<Handle<FrameAllocation>>,
    entries: Vec<crate::DirEntry>,
    metadata: crate::Metadata,
}

impl Directory {
    pub fn metadata(&self) -> crate::Metadata {
        self.metadata
    }
    pub(crate) fn set_entries(&mut self, entries: Vec<crate::DirEntry>) {
        self.entries = entries;
    }

    pub fn read_dir(&self) -> Vec<crate::DirEntry> {
        self.entries.clone()
    }
}

impl Archive {
    pub fn new(frames: Handle<FrameAllocation>, length: usize) -> Result<Self, Error> {
        let capacity = frames
            .frames()?
            .checked_mul(PAGE_SIZE as u64)
            .ok_or(Error::InvalidData)?;
        if length == 0 || length as u64 > capacity {
            return Err(Error::InvalidData);
        }
        let mut archive = Self {
            device: crate::metadata::device_identity()?,
            mount: crate::metadata::mount_identity()?,
            frames: Rc::new(frames),
            length,
            entries: BTreeMap::new(),
        };
        archive.index()?;
        Ok(archive)
    }

    pub fn read_dir(&self, path: &[u8]) -> Result<Vec<crate::DirEntry>, Error> {
        let path = normalize(path)?;
        if let Some(entry) = self.entries.get(&path)
            && entry.metadata.mode & 0o170000 != 0o040000
        {
            return Err(Error::NotDirectory);
        }
        let mut children = BTreeMap::new();
        for (name, entry) in &self.entries {
            let rest = if path.is_empty() {
                &name[..]
            } else if name.starts_with(&path) && name.get(path.len()) == Some(&b'/') {
                &name[path.len() + 1..]
            } else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            if !rest.contains(&b'/') {
                children.insert(rest.to_vec(), entry.metadata);
            }
        }
        if children.is_empty() && !path.is_empty() && !self.entries.contains_key(&path) {
            return Err(Error::NotFound);
        }
        Ok(children
            .into_iter()
            .map(|(name, metadata)| crate::DirEntry {
                name,
                inode: metadata.inode,
                kind: crate::FileType::from_mode(metadata.mode),
            })
            .collect())
    }

    pub fn into_parts(self) -> Result<(Handle<FrameAllocation>, usize), Error> {
        Ok((
            Rc::try_unwrap(self.frames).map_err(|_| Error::Busy)?,
            self.length,
        ))
    }

    // REFERENCE: https://www.kernel.org/doc/html/latest/driver-api/early-userspace/buffer-format.html
    fn index(&mut self) -> Result<(), Error> {
        let mut offset = 0;
        loop {
            let mut header = [0; HEADER_SIZE];
            self.read(offset, &mut header)?;
            if &header[..6] != b"070701" || field(&header, 102)? != 0 {
                return Err(Error::InvalidData);
            }
            let mode = field(&header, 14)?;
            let links = field(&header, 38)?;
            let size = field(&header, 54)?;
            let name_size = field(&header, 94)?;
            let name_start = offset.checked_add(HEADER_SIZE).ok_or(Error::InvalidData)?;
            let name_end = name_start
                .checked_add(name_size)
                .filter(|&end| name_size != 0 && end <= self.length)
                .ok_or(Error::InvalidData)?;
            let data_start = name_end
                .checked_next_multiple_of(4)
                .ok_or(Error::InvalidData)?;
            let data_end = data_start
                .checked_add(size)
                .filter(|&end| end <= self.length)
                .ok_or(Error::InvalidData)?;
            let mut name = vec![0; name_size];
            self.read(name_start, &mut name)?;
            if name.pop() != Some(0) || name.contains(&0) {
                return Err(Error::InvalidData);
            }
            if name == b"TRAILER!!!" {
                if size != 0 {
                    return Err(Error::InvalidData);
                }
                return self.finish_index();
            }
            if name.starts_with(b"/") {
                return Err(Error::InvalidData);
            }
            let path = normalize(&name).map_err(|_| Error::InvalidData)?;
            let directory = match mode & 0o170000 {
                0o040000 if size == 0 => true,
                0o100000 if links != 0 => false,
                0o120000 if links != 0 && size > 0 && size < 4096 => false,
                _ => return Err(Error::InvalidData),
            };
            if path.is_empty() && !directory {
                return Err(Error::InvalidData);
            }
            if self
                .entries
                .insert(
                    path,
                    Entry {
                        range: data_start..data_end,
                        metadata: crate::Metadata {
                            device: self.device,
                            mount: self.mount,
                            inode: field(&header, 6)? as u64,
                            size: size as u64,
                            blocks: (size as u64).div_ceil(512),
                            links: links as u32,
                            mode: mode as u32,
                            uid: field(&header, 22)? as u32,
                            gid: field(&header, 30)? as u32,
                            modified: crate::Timestamp {
                                seconds: field(&header, 46)? as i64,
                                nanoseconds: 0,
                            },
                            ..crate::Metadata::default()
                        },
                        identity: (field(&header, 62)?, field(&header, 70)?, field(&header, 6)?),
                    },
                )
                .is_some()
            {
                return Err(Error::InvalidData);
            }
            offset = data_end
                .checked_next_multiple_of(4)
                .ok_or(Error::InvalidData)?;
        }
    }

    fn finish_index(&mut self) -> Result<(), Error> {
        let mut data = BTreeMap::new();
        for entry in self
            .entries
            .values()
            .filter(|entry| entry.metadata.links > 1 && entry.metadata.mode & 0o170000 == 0o100000)
        {
            if !entry.range.is_empty() && data.insert(entry.identity, entry.range.clone()).is_some()
            {
                return Err(Error::InvalidData);
            }
        }
        for entry in self.entries.values_mut() {
            if entry.metadata.links > 1 && entry.metadata.mode & 0o170000 == 0o100000 {
                if let Some(range) = data.get(&entry.identity) {
                    entry.range = range.clone();
                }
                entry.metadata.size = entry.range.len() as u64;
                entry.metadata.blocks = entry.metadata.size.div_ceil(512);
            }
        }
        let names: Vec<_> = self.entries.keys().cloned().collect();
        let mut synthetic = 1u64 << 32;
        for name in names.into_iter().chain(core::iter::once(Vec::new())) {
            let mut parent = name.as_slice();
            loop {
                parent = parent
                    .iter()
                    .rposition(|byte| *byte == b'/')
                    .map_or(b"".as_slice(), |end| &parent[..end]);
                if let Some(entry) = self.entries.get(parent) {
                    if entry.metadata.mode & 0o170000 != 0o040000 {
                        return Err(Error::InvalidData);
                    }
                } else {
                    self.entries.insert(
                        parent.to_vec(),
                        Entry {
                            range: 0..0,
                            metadata: crate::Metadata {
                                device: self.device,
                                mount: self.mount,
                                inode: synthetic,
                                mode: 0o040755,
                                links: 2,
                                ..crate::Metadata::default()
                            },
                            identity: (0, 0, 0),
                        },
                    );
                    synthetic += 1;
                }
                if parent.is_empty() {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn metadata(&self, path: &[u8]) -> Result<crate::Metadata, Error> {
        self.entries
            .get(&normalize(path)?)
            .map(|entry| entry.metadata)
            .ok_or(Error::NotFound)
    }

    pub fn read_link(&self, path: &[u8]) -> Result<Vec<u8>, Error> {
        let entry = self.entries.get(&normalize(path)?).ok_or(Error::NotFound)?;
        if entry.metadata.mode & 0o170000 != 0o120000 {
            return Err(Error::InvalidRequest);
        }
        let mut bytes = vec![0; entry.range.len()];
        self.read(entry.range.start, &mut bytes)?;
        Ok(bytes)
    }

    pub fn open_directory(&self, path: &[u8]) -> Result<Directory, Error> {
        let metadata = self.metadata(path)?;
        if metadata.mode & 0o170000 != 0o040000 {
            return Err(Error::NotDirectory);
        }
        Ok(Directory {
            _frames: Rc::clone(&self.frames),
            entries: self.read_dir(path)?,
            metadata,
        })
    }

    fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(output.len())
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::InvalidData);
        }
        self.frames.read(offset, output)?;
        Ok(())
    }
}

impl FileSystem for Archive {
    type File<'a> = File;

    fn open(&self, path: &str) -> Result<File, Error> {
        self.open_bytes(path.as_bytes())
    }
}

impl Archive {
    pub fn open_bytes(&self, path: &[u8]) -> Result<File, Error> {
        let path = normalize(path)?;
        let entry = self.entries.get(&path).ok_or(Error::NotFound)?;
        if entry.metadata.mode & 0o170000 == 0o040000 {
            return Err(Error::IsDirectory);
        }
        if entry.metadata.mode & 0o170000 == 0o120000 {
            return Err(Error::InvalidRequest);
        }
        Ok(File {
            metadata: entry.metadata,
            frames: Rc::clone(&self.frames),
            range: entry.range.clone(),
        })
    }
}

impl ReadAt for File {
    fn len(&self) -> usize {
        self.range.len()
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let length = output.len().min(self.len().saturating_sub(offset));
        if length != 0 {
            self.frames
                .read(self.range.start + offset, &mut output[..length])?;
        }
        Ok(length)
    }
}

fn field(header: &[u8; HEADER_SIZE], offset: usize) -> Result<usize, Error> {
    header[offset..offset + 8]
        .iter()
        .try_fold(0usize, |value, &digit| {
            let digit = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                b'A'..=b'F' => digit - b'A' + 10,
                _ => return Err(Error::InvalidData),
            };
            value
                .checked_mul(16)
                .and_then(|value| value.checked_add(digit as usize))
                .ok_or(Error::InvalidData)
        })
}

impl File {
    pub fn metadata(&self) -> crate::Metadata {
        self.metadata
    }
}
