// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Validate the initial static ELF and provide loadable segments.

use super::Image;
use kernel::{
    BootProgram, ProgramSegment,
    hardware::mmu::{PAGE_SIZE, PagePermissions},
    memory::{USER_ADDRESS_RANGE, VirtRange},
};

#[derive(Debug)]
pub struct Elf<'a> {
    image: Image<'a>,
    entry: usize,
    headers: usize,
    count: usize,
}

impl<'a> Elf<'a> {
    pub fn new(image: Image<'a>, machine: u16) -> Result<Self, &'static str> {
        let mut header = [0; 64];
        image.read(0, &mut header)?;
        if &header[..7] != b"\x7fELF\x02\x01\x01"
            || half(&header, 16) != 2
            || half(&header, 18) != machine
            || word(&header, 20) != 1
            || half(&header, 52) != 64
            || half(&header, 54) != 56
        {
            return Err("unsupported init ELF header");
        }
        let this = Self {
            image,
            entry: address(&header, 24)?,
            headers: address(&header, 32)?,
            count: half(&header, 56) as usize,
        };
        if this.count == 0 || this.count == u16::MAX as usize {
            return Err("unsupported ELF program header count");
        }
        image.slice(
            this.headers,
            this.count
                .checked_mul(56)
                .ok_or("ELF header table overflow")?,
        )?;
        let mut executable_entry = false;
        for index in 0..this.count {
            let mut header = [0; 56];
            image.read(this.headers + index * 56, &mut header)?;
            let kind = word(&header, 0);
            if kind == 2 || kind == 3 {
                return Err("dynamic init ELF is not supported");
            }
            if let Some(segment) = this.load_segment(index)? {
                let pages = envelope(segment.memory)?;
                for previous in 0..index {
                    if let Some(other) = this.load_segment(previous)?
                        && pages.overlaps(envelope(other.memory)?)
                    {
                        return Err("ELF load segments share a page");
                    }
                }
                if let Some(data) = segment.data {
                    executable_entry |= segment.permissions == PagePermissions::ReadExecute
                        && (segment.memory.start..segment.memory.start + data.len()?)
                            .contains(&this.entry);
                }
            }
        }
        if !executable_entry {
            return Err("ELF entry is not in an executable segment");
        }
        Ok(this)
    }

    fn load_segment(&self, index: usize) -> Result<Option<ProgramSegment>, &'static str> {
        let mut header = [0; 56];
        self.image.read(self.headers + index * 56, &mut header)?;
        if word(&header, 0) != 1 {
            return Ok(None);
        }
        let flags = word(&header, 4);
        let offset = address(&header, 8)?;
        let start = address(&header, 16)?;
        let filesz = address(&header, 32)?;
        let memsz = address(&header, 40)?;
        let align = address(&header, 48)?;
        if filesz > memsz
            || (align > 1 && (!align.is_power_of_two() || start % align != offset % align))
        {
            return Err("invalid ELF segment size or alignment");
        }
        let data = self.image.slice(offset, filesz)?;
        if memsz == 0 {
            return Ok(None);
        }
        let memory = VirtRange {
            start,
            end: start
                .checked_add(memsz)
                .ok_or("ELF virtual range overflow")?,
        };
        if !USER_ADDRESS_RANGE.contains(envelope(memory)?) {
            return Err("ELF segment exceeds user address space");
        }
        let permissions = match flags {
            4 => PagePermissions::ReadOnly,
            5 => PagePermissions::ReadExecute,
            6 => PagePermissions::ReadWrite,
            _ => return Err("unsupported ELF segment permissions"),
        };
        Ok(Some(ProgramSegment {
            memory,
            data: (filesz != 0).then_some(data.data()),
            permissions,
        }))
    }
}

impl BootProgram for Elf<'_> {
    fn entry(&self) -> usize {
        self.entry
    }

    fn segment(&self, index: usize) -> Result<Option<ProgramSegment>, &'static str> {
        let mut loaded = 0;
        for header in 0..self.count {
            if let Some(segment) = self.load_segment(header)? {
                if loaded == index {
                    return Ok(Some(segment));
                }
                loaded += 1;
            }
        }
        Ok(None)
    }
}

fn envelope(range: VirtRange) -> Result<VirtRange, &'static str> {
    Ok(VirtRange {
        start: range.start & !(PAGE_SIZE - 1),
        end: range
            .end
            .checked_next_multiple_of(PAGE_SIZE)
            .ok_or("ELF page range overflow")?,
    })
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn address(bytes: &[u8], offset: usize) -> Result<usize, &'static str> {
    usize::try_from(u64::from_le_bytes(
        bytes[offset..offset + 8].try_into().unwrap(),
    ))
    .map_err(|_| "ELF address is too large")
}
