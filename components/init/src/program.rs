// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;
use fs::{Error, ReadAt};
use runtime::{
    Rights,
    memory::Permissions,
    process::{self, ChildTask},
};

const PAGE_SIZE: usize = 4096;

pub struct Loader {
    machine: u16,
}

struct Elf<F> {
    file: F,
    machine: u16,
    entry: usize,
    headers: usize,
    count: usize,
}

struct Segment {
    memory: Range<usize>,
    data: Range<usize>,
    permissions: Permissions,
}

impl Loader {
    pub fn new(native: impl ReadAt) -> Result<Self, Error> {
        Ok(Self {
            machine: Elf::new(native)?.machine,
        })
    }

    pub fn load(&self, file: impl ReadAt, rights: Rights) -> Result<(ChildTask, usize), Error> {
        let elf = Elf::new(file)?;
        if elf.machine != self.machine {
            return Err(Error::InvalidData);
        }
        let mut child = ChildTask::new(process::task(), rights)?;
        let mut buffer = [0; PAGE_SIZE];
        for index in 0..elf.count {
            let Some(segment) = elf.segment(index)? else {
                continue;
            };
            let pages = segment.pages()?;
            let count = (pages.len() / PAGE_SIZE) as u64;
            let rights = Rights::READ_WRITE.union(Rights::MAP).union(Rights::EXECUTE);
            let mut frames = child.allocate(count, rights)?;
            for offset in (0..segment.data.len()).step_by(buffer.len()) {
                let length = buffer.len().min(segment.data.len() - offset);
                elf.file
                    .read_exact_at(segment.data.start + offset, &mut buffer[..length])?;
                frames.write(
                    segment.memory.start - pages.start + offset,
                    &buffer[..length],
                )?;
            }
            if child.map(&frames, 0, count, Some(pages.start), segment.permissions)? != pages {
                return Err(Error::InvalidData);
            }
        }
        Ok((child, elf.entry))
    }
}

impl<F: ReadAt> Elf<F> {
    fn new(file: F) -> Result<Self, Error> {
        let mut header = [0; 64];
        file.read_exact_at(0, &mut header)?;
        if &header[..7] != b"\x7fELF\x02\x01\x01"
            || half(&header, 16) != 2
            || word(&header, 20) != 1
            || half(&header, 52) != 64
            || half(&header, 54) != 56
        {
            return Err(Error::InvalidData);
        }
        let this = Self {
            file,
            machine: half(&header, 18),
            entry: address(&header, 24)?,
            headers: address(&header, 32)?,
            count: half(&header, 56) as usize,
        };
        if this.count == 0 || this.count == u16::MAX as usize {
            return Err(Error::InvalidData);
        }
        this.range(
            this.headers,
            this.count.checked_mul(56).ok_or(Error::InvalidData)?,
        )?;
        let mut executable_entry = false;
        for index in 0..this.count {
            let header = this.header(index)?;
            if matches!(word(&header, 0), 2 | 3) {
                return Err(Error::InvalidData);
            }
            if let Some(segment) = this.segment(index)? {
                let pages = segment.pages()?;
                for previous in 0..index {
                    if let Some(other) = this.segment(previous)? {
                        let other = other.pages()?;
                        if pages.start < other.end && other.start < pages.end {
                            return Err(Error::InvalidData);
                        }
                    }
                }
                executable_entry |= segment.permissions == Permissions::ReadExecute
                    && (segment.memory.start..segment.memory.start + segment.data.len())
                        .contains(&this.entry);
            }
        }
        if !executable_entry {
            return Err(Error::InvalidData);
        }
        Ok(this)
    }

    fn segment(&self, index: usize) -> Result<Option<Segment>, Error> {
        let header = self.header(index)?;
        if word(&header, 0) != 1 {
            return Ok(None);
        }
        let offset = address(&header, 8)?;
        let start = address(&header, 16)?;
        let filesz = address(&header, 32)?;
        let memsz = address(&header, 40)?;
        let align = address(&header, 48)?;
        if filesz > memsz
            || (align > 1 && (!align.is_power_of_two() || start % align != offset % align))
        {
            return Err(Error::InvalidData);
        }
        let data = self.range(offset, filesz)?;
        if memsz == 0 {
            return Ok(None);
        }
        let end = start.checked_add(memsz).ok_or(Error::InvalidData)?;
        let permissions = match word(&header, 4) {
            4 => Permissions::ReadOnly,
            5 => Permissions::ReadExecute,
            6 => Permissions::ReadWrite,
            _ => return Err(Error::InvalidData),
        };
        Ok(Some(Segment {
            memory: start..end,
            data,
            permissions,
        }))
    }

    fn header(&self, index: usize) -> Result<[u8; 56], Error> {
        let mut bytes = [0; 56];
        self.file
            .read_exact_at(self.headers + index * bytes.len(), &mut bytes)?;
        Ok(bytes)
    }

    fn range(&self, offset: usize, length: usize) -> Result<Range<usize>, Error> {
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= self.file.len())
            .ok_or(Error::InvalidData)?;
        Ok(offset..end)
    }
}

impl Segment {
    fn pages(&self) -> Result<Range<usize>, Error> {
        Ok((self.memory.start & !(PAGE_SIZE - 1))
            ..self
                .memory
                .end
                .checked_next_multiple_of(PAGE_SIZE)
                .ok_or(Error::InvalidData)?)
    }
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn address(bytes: &[u8], offset: usize) -> Result<usize, Error> {
    usize::try_from(u64::from_le_bytes(
        bytes[offset..offset + 8].try_into().unwrap(),
    ))
    .map_err(|_| Error::InvalidData)
}
