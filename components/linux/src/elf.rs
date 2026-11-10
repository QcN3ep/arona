// SPDX-License-Identifier: MPL-2.0

//! 静态 ELF64 的布局校验与机器标识。

use alloc::vec::Vec;
use core::ops::Range;
use runtime::memory::Permissions;

use crate::Error;

pub const PAGE_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Machine {
    Riscv64,
    LoongArch64,
}

pub struct Image<'a> {
    pub machine: Machine,
    pub entry: usize,
    pub segments: Vec<Segment<'a>>,
    pub headers: &'a [u8],
    pub phdr: Option<usize>,
}

pub struct Segment<'a> {
    pub memory: Range<usize>,
    pub data: &'a [u8],
    pub permissions: Permissions,
}

impl Machine {
    pub fn from_header(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 64 || &bytes[..7] != b"\x7fELF\x02\x01\x01" {
            return Err(Error::InvalidExecutable);
        }
        match half(bytes, 18) {
            243 => Ok(Self::Riscv64),
            258 => Ok(Self::LoongArch64),
            _ => Err(Error::InvalidExecutable),
        }
    }
}

impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8], machine: Machine) -> Result<Self, Error> {
        if Machine::from_header(bytes)? != machine
            || half(bytes, 16) != 2
            || word(bytes, 20) != 1
            || half(bytes, 52) != 64
            || half(bytes, 54) != 56
        {
            return Err(Error::InvalidExecutable);
        }
        let entry = address(bytes, 24)?;
        let offset = address(bytes, 32)?;
        let count = usize::from(half(bytes, 56));
        if count == 0 || count == usize::from(u16::MAX) {
            return Err(Error::InvalidExecutable);
        }
        let end = offset
            .checked_add(count * 56)
            .ok_or(Error::InvalidExecutable)?;
        let headers = bytes.get(offset..end).ok_or(Error::InvalidExecutable)?;
        let mut segments = Vec::new();
        let mut phdr = None;
        for header in headers.as_chunks::<56>().0 {
            match word(header, 0) {
                2 | 3 => return Err(Error::InvalidExecutable),
                0x6474e551 if word(header, 4) & 1 != 0 => return Err(Error::InvalidExecutable),
                1 => {
                    if let Some(segment) = Segment::parse(bytes, header)? {
                        let file_offset = address(header, 8)?;
                        if file_offset <= offset && end - file_offset <= segment.data.len() {
                            phdr = Some(segment.memory.start + offset - file_offset);
                        }
                        segments.push(segment);
                    }
                }
                _ => {}
            }
        }
        segments.sort_unstable_by_key(|segment| segment.memory.start);
        for pair in segments.windows(2) {
            if pair[0].pages().end > pair[1].pages().start {
                return Err(Error::InvalidExecutable);
            }
        }
        if !segments.iter().any(|segment| {
            segment.permissions == Permissions::ReadExecute
                && (segment.memory.start..segment.memory.start + segment.data.len())
                    .contains(&entry)
        }) {
            return Err(Error::InvalidExecutable);
        }
        Ok(Self {
            machine,
            entry,
            segments,
            headers,
            phdr,
        })
    }
}

impl<'a> Segment<'a> {
    fn parse(bytes: &'a [u8], header: &[u8]) -> Result<Option<Self>, Error> {
        let offset = address(header, 8)?;
        let start = address(header, 16)?;
        let filesz = address(header, 32)?;
        let memsz = address(header, 40)?;
        let align = address(header, 48)?;
        if filesz > memsz
            || start % PAGE_SIZE != offset % PAGE_SIZE
            || (align > 1 && (!align.is_power_of_two() || start % align != offset % align))
        {
            return Err(Error::InvalidExecutable);
        }
        let file_end = offset.checked_add(filesz).ok_or(Error::InvalidExecutable)?;
        let data = bytes
            .get(offset..file_end)
            .ok_or(Error::InvalidExecutable)?;
        if memsz == 0 {
            return Ok(None);
        }
        let end = start
            .checked_add(memsz)
            .filter(|&end| start >= PAGE_SIZE && end <= isize::MAX as usize - PAGE_SIZE)
            .ok_or(Error::InvalidExecutable)?;
        let permissions = match word(header, 4) {
            4 => Permissions::ReadOnly,
            5 => Permissions::ReadExecute,
            6 => Permissions::ReadWrite,
            _ => return Err(Error::InvalidExecutable),
        };
        Ok(Some(Self {
            memory: start..end,
            data,
            permissions,
        }))
    }

    pub fn pages(&self) -> Range<usize> {
        (self.memory.start & !(PAGE_SIZE - 1))..self.memory.end.next_multiple_of(PAGE_SIZE)
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
    .map_err(|_| Error::InvalidExecutable)
}
