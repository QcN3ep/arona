// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Layout validation and machine identification for ELF64 images.
//!
//! An ET_EXEC image keeps the addresses recorded in its program headers. An
//! ET_DYN image is placed at a page-aligned base selected by the loader, so
//! its segments and entry are relocated before the caller maps them. A
//! PT_INTERP request is reported through [`Image::interpreter`]; the caller
//! loads that interpreter at a distinct base and transfers control to its
//! entry.

// PT_DYNAMIC entries and relocations are interpreted by the guest dynamic loader.

use alloc::vec::Vec;
use core::ops::Range;
use runtime::memory::Permissions;

use crate::Error;

pub const PAGE_SIZE: usize = 4096;

/// Lowest address used for a position-independent image.
pub const PIE_BASE: usize = 0x1_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Machine {
    Riscv64,
    LoongArch64,
}

#[derive(Clone, Copy)]
enum Kind {
    Executable,
    PositionIndependent,
}

pub struct Image<'a> {
    pub machine: Machine,
    /// Link-time entry for ET_EXEC and relocated entry for ET_DYN.
    pub entry: usize,
    pub segments: Vec<Segment<'a>>,
    pub headers: &'a [u8],
    pub phdr: Option<usize>,
    /// PT_INTERP path without its trailing NUL.
    pub interpreter: Option<&'a [u8]>,
    /// Mapping base selected for ET_DYN, or zero for ET_EXEC.
    pub base: usize,
}

/// A main image and the interpreter selected by its PT_INTERP request.
pub struct ImageSet<'a> {
    pub image: Image<'a>,
    pub interpreter: Option<Image<'a>>,
}

impl ImageSet<'_> {
    /// Entry point that receives control after the loader maps the image set.
    pub fn entry(&self) -> usize {
        self.interpreter
            .as_ref()
            .map_or(self.image.entry, |image| image.entry)
    }
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
        Self::parse_at(bytes, machine, PIE_BASE)
    }

    /// Parse an image, placing an ET_DYN image no lower than `minimum`.
    pub fn parse_at(bytes: &'a [u8], machine: Machine, minimum: usize) -> Result<Self, Error> {
        let kind = image_kind(bytes, machine)?;
        let (headers, offset, end) = program_headers(bytes)?;
        let alignment = image_alignment(headers)?;
        let base = match kind {
            Kind::Executable => 0,
            Kind::PositionIndependent => minimum
                .max(PIE_BASE)
                .checked_next_multiple_of(alignment)
                .ok_or(Error::InvalidExecutable)?,
        };
        let entry = address(bytes, 24)?
            .checked_add(base)
            .ok_or(Error::InvalidExecutable)?;
        let layout = layout(bytes, headers, offset, end, base)?;
        if !layout.segments.iter().any(|segment| {
            segment.permissions == Permissions::ReadExecute
                && (segment.memory.start..segment.memory.start + segment.data.len())
                    .contains(&entry)
        }) {
            return Err(Error::InvalidExecutable);
        }
        Ok(Self {
            machine,
            entry,
            segments: layout.segments,
            headers,
            phdr: layout.phdr,
            interpreter: layout.interpreter,
            base,
        })
    }

    /// End of the highest mapped segment page.
    pub fn end(&self) -> usize {
        self.segments
            .last()
            .map_or(self.entry, |segment| segment.pages().end)
    }
}

fn image_kind(bytes: &[u8], machine: Machine) -> Result<Kind, Error> {
    if Machine::from_header(bytes)? != machine
        || word(bytes, 20) != 1
        || half(bytes, 52) != 64
        || half(bytes, 54) != 56
    {
        return Err(Error::InvalidExecutable);
    }
    match half(bytes, 16) {
        2 => Ok(Kind::Executable),
        3 => Ok(Kind::PositionIndependent),
        _ => Err(Error::InvalidExecutable),
    }
}

fn program_headers(bytes: &[u8]) -> Result<(&[u8], usize, usize), Error> {
    let offset = address(bytes, 32)?;
    let count = usize::from(half(bytes, 56));
    if count == 0 || count == usize::from(u16::MAX) {
        return Err(Error::InvalidExecutable);
    }
    let end = offset
        .checked_add(count * 56)
        .ok_or(Error::InvalidExecutable)?;
    let headers = bytes.get(offset..end).ok_or(Error::InvalidExecutable)?;
    Ok((headers, offset, end))
}

fn image_alignment(headers: &[u8]) -> Result<usize, Error> {
    headers
        .as_chunks::<56>()
        .0
        .iter()
        .filter(|header| word(*header, 0) == 1)
        .try_fold(PAGE_SIZE, |alignment, header| {
            let align = address(header, 48)?;
            if align > 1 && !align.is_power_of_two() {
                return Err(Error::InvalidExecutable);
            }
            Ok(alignment.max(align))
        })
}

fn interpreter_path<'a>(bytes: &'a [u8], header: &[u8]) -> Result<&'a [u8], Error> {
    let offset = address(header, 8)?;
    let size = address(header, 32)?;
    let end = offset.checked_add(size).ok_or(Error::InvalidExecutable)?;
    let path = bytes
        .get(offset..end)
        .ok_or(Error::InvalidExecutable)?
        .strip_suffix(&[0])
        .ok_or(Error::InvalidExecutable)?;
    if path.is_empty() || path.contains(&0) || !path.starts_with(b"/") {
        return Err(Error::InvalidExecutable);
    }
    Ok(path)
}

struct Layout<'a> {
    segments: Vec<Segment<'a>>,
    phdr: Option<usize>,
    interpreter: Option<&'a [u8]>,
}

fn layout<'a>(
    bytes: &'a [u8],
    headers: &'a [u8],
    offset: usize,
    end: usize,
    base: usize,
) -> Result<Layout<'a>, Error> {
    let mut segments = Vec::new();
    let mut phdr = None;
    let mut interpreter = None;
    for header in headers.as_chunks::<56>().0 {
        match word(header, 0) {
            3 => {
                if interpreter.is_some() {
                    return Err(Error::InvalidExecutable);
                }
                interpreter = Some(interpreter_path(bytes, header)?);
            }
            0x6474e551 if word(header, 4) & 1 != 0 => return Err(Error::InvalidExecutable),
            1 => {
                if let Some(segment) = Segment::parse(bytes, header, base)? {
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
    Ok(Layout {
        segments,
        phdr,
        interpreter,
    })
}

impl<'a> Segment<'a> {
    fn parse(bytes: &'a [u8], header: &[u8], base: usize) -> Result<Option<Self>, Error> {
        let offset = address(header, 8)?;
        let start = address(header, 16)?
            .checked_add(base)
            .ok_or(Error::InvalidExecutable)?;
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
