// SPDX-License-Identifier: MPL-2.0

//! CPIO newc 归档的只读索引。

use alloc::{collections::BTreeMap, string::String, vec};
use core::ops::Range;
use runtime::{FrameAllocation, Handle};

use crate::{Error, FileSystem, ReadAt};

const PAGE_SIZE: usize = 4096;
const HEADER_SIZE: usize = 110;

pub struct Archive {
    frames: Handle<FrameAllocation>,
    length: usize,
    entries: BTreeMap<String, Entry>,
}

struct Entry {
    range: Range<usize>,
    directory: bool,
}

pub struct File<'a> {
    archive: &'a Archive,
    range: Range<usize>,
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
            frames,
            length,
            entries: BTreeMap::new(),
        };
        archive.index()?;
        Ok(archive)
    }

    pub fn into_parts(self) -> (Handle<FrameAllocation>, usize) {
        (self.frames, self.length)
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
                return if size == 0 {
                    Ok(())
                } else {
                    Err(Error::InvalidData)
                };
            }
            let name = core::str::from_utf8(&name).map_err(|_| Error::InvalidData)?;
            if name.starts_with('/') {
                return Err(Error::InvalidData);
            }
            let path = normalize(name).map_err(|_| Error::InvalidData)?;
            let directory = match mode & 0o170000 {
                0o040000 if size == 0 => true,
                0o100000 if links == 1 => false,
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
                        directory,
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
    type File<'a> = File<'a>;

    fn open(&self, path: &str) -> Result<File<'_>, Error> {
        let path = normalize(path)?;
        let entry = self.entries.get(&path).ok_or(Error::NotFound)?;
        if entry.directory {
            return Err(Error::IsDirectory);
        }
        Ok(File {
            archive: self,
            range: entry.range.clone(),
        })
    }
}

impl ReadAt for File<'_> {
    fn len(&self) -> usize {
        self.range.len()
    }

    fn read_at(&self, offset: usize, output: &mut [u8]) -> Result<usize, Error> {
        let length = output.len().min(self.len().saturating_sub(offset));
        if length != 0 {
            self.archive
                .read(self.range.start + offset, &mut output[..length])?;
        }
        Ok(length)
    }
}

fn normalize(path: &str) -> Result<String, Error> {
    if path.is_empty() || path.as_bytes().contains(&0) {
        return Err(Error::InvalidRequest);
    }
    let mut normalized = String::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err(Error::InvalidRequest),
            _ => {}
        }
        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(part);
    }
    Ok(normalized)
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
