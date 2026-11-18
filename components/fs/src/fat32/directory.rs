// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{
    Node, half,
    volume::{ROOT, Volume},
    word,
};
use crate::{Error, block::MAX_BLOCK_SIZE};
use alloc::{rc::Rc, string::String, vec::Vec};
use core::cell::RefCell;

#[derive(Clone)]
pub(super) struct Entry {
    pub location: u64,
    pub parent: u32,
    pub long: Vec<u64>,
    pub name: Vec<u8>,
    pub raw: [u8; 32],
}

impl Entry {
    pub fn root(cluster: u32) -> Self {
        let mut entry = Self {
            location: ROOT,
            parent: cluster,
            long: Vec::new(),
            name: Vec::new(),
            raw: [0; 32],
        };
        entry.raw[11] = 0x10;
        entry.set_first(cluster);
        entry
    }
    pub fn first(&self) -> u32 {
        (u32::from(half(&self.raw, 20)) << 16) | u32::from(half(&self.raw, 26))
    }
    pub fn set_first(&mut self, first: u32) {
        self.raw[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
        self.raw[26..28].copy_from_slice(&(first as u16).to_le_bytes());
    }
    pub fn set_size(&mut self, size: usize) {
        self.raw[28..32].copy_from_slice(&(size as u32).to_le_bytes());
    }
    pub fn size(&self) -> usize {
        word(&self.raw, 28) as usize
    }
    pub fn directory(&self) -> bool {
        self.raw[11] & 0x10 != 0
    }
    pub fn read_only(&self) -> bool {
        self.raw[11] & 1 != 0
    }
}

// REFERENCE: https://elm-chan.org/docs/fat_e.html#lfn
struct LongName {
    words: [u16; 260],
    expected: u8,
    checksum: u8,
    slots: Vec<u64>,
}

impl LongName {
    fn new() -> Self {
        Self {
            words: [0xffff; 260],
            expected: 0,
            checksum: 0,
            slots: Vec::new(),
        }
    }

    fn push(&mut self, raw: &[u8], location: u64) {
        let order = raw[0] & 31;
        if raw[0] & 0x40 != 0 {
            *self = Self::new();
            self.expected = order;
            self.checksum = raw[13];
        }
        if order == 0
            || order > 20
            || order != self.expected
            || raw[13] != self.checksum
            || raw[0] & 0xa0 != 0
            || raw[12] != 0
            || half(raw, 26) != 0
        {
            *self = Self::new();
            return;
        }
        for (index, offset) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
            .into_iter()
            .enumerate()
        {
            self.words[(order as usize - 1) * 13 + index] = half(raw, offset);
        }
        self.slots.push(location);
        self.expected -= 1;
    }

    fn take(&mut self, raw: &[u8]) -> Option<(Vec<u8>, Vec<u64>)> {
        if self.slots.is_empty() || self.expected != 0 || checksum(&raw[..11]) != self.checksum {
            return None;
        }
        let size = self
            .words
            .iter()
            .position(|word| *word == 0)
            .unwrap_or(self.slots.len() * 13);
        if size == 0 || size > 255 {
            return None;
        }
        let text = String::from_utf16(&self.words[..size]).ok()?;
        if text.bytes().any(|byte| byte == 0 || byte == b'/') {
            return None;
        }
        Some((text.into_bytes(), core::mem::take(&mut self.slots)))
    }
}

pub(super) fn checksum(name: &[u8]) -> u8 {
    name.iter()
        .fold(0u8, |sum, byte| sum.rotate_right(1).wrapping_add(*byte))
}

// FAT does not store its OEM code page. Decode short aliases using CP437.
const OEM: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ ";

fn short_name(raw: &[u8]) -> Vec<u8> {
    let mut name = Vec::new();
    for (index, part) in [&raw[..8], &raw[8..11]].into_iter().enumerate() {
        let end = part
            .iter()
            .rposition(|byte| *byte != b' ')
            .map_or(0, |last| last + 1);
        if index == 1 && end != 0 {
            name.push(b'.');
        }
        for byte in &part[..end] {
            let byte = if name.is_empty() && *byte == 5 {
                0xe5
            } else {
                *byte
            };
            let character = if raw[12] & (8 << index) != 0 {
                byte.to_ascii_lowercase()
            } else {
                byte
            };
            let mut encoded = [0; 4];
            let character = if character < 128 {
                char::from(character)
            } else {
                OEM.chars().nth((character - 128) as usize).unwrap()
            };
            name.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
        }
    }
    name
}

impl Volume {
    pub fn entries(&self, directory: u32) -> Result<Vec<Entry>, Error> {
        self.cluster_offset(directory)?;
        let mut entries = Vec::new();
        let mut long = LongName::new();
        for cluster in self.chain(directory)? {
            let base = self.cluster_offset(cluster)?;
            for sector in 0..self.sectors_per_cluster {
                let start = base + u64::from(sector) * self.sector_size as u64;
                let mut bytes = [0; MAX_BLOCK_SIZE];
                self.read_bytes(start, &mut bytes[..self.sector_size])?;
                for (index, raw) in bytes[..self.sector_size].chunks_exact(32).enumerate() {
                    if raw[0] == 0 {
                        return Ok(entries);
                    }
                    let location = start + (index * 32) as u64;
                    if raw[0] == 0xe5 {
                        long = LongName::new();
                        continue;
                    }
                    if raw[11] == 15 {
                        long.push(raw, location);
                        continue;
                    }
                    if raw[11] & 8 != 0 {
                        long = LongName::new();
                        continue;
                    }
                    let (name, slots) = long
                        .take(raw)
                        .unwrap_or_else(|| (short_name(raw), Vec::new()));
                    long = LongName::new();
                    if name != b"." && name != b".." {
                        entries.push(Entry {
                            location,
                            parent: directory,
                            long: slots,
                            name,
                            raw: raw.try_into().unwrap(),
                        });
                    }
                }
            }
        }
        Ok(entries)
    }

    pub fn lookup(&mut self, path: &[u8]) -> Result<Rc<RefCell<Node>>, Error> {
        self.check()?;
        let mut current = Rc::clone(self.nodes.get(&ROOT).unwrap());
        for name in crate::path::components(path)? {
            let entry = current.borrow().entry.clone();
            if !entry.directory() {
                return Err(Error::NotDirectory);
            }
            let found = self
                .entries(entry.first())?
                .into_iter()
                .find(|entry| {
                    equal_name(&entry.name, name) || equal_name(&short_name(&entry.raw), name)
                })
                .ok_or(Error::NotFound)?;
            current = self.node(found)?;
        }
        Ok(current)
    }
}

pub(super) fn equal_name(first: &[u8], second: &[u8]) -> bool {
    match (core::str::from_utf8(first), core::str::from_utf8(second)) {
        (Ok(first), Ok(second)) => {
            // Expanding a character would merge distinct FAT names such as straße and strasse.
            let uppercase = |character: char| {
                let mut mapped = character.to_uppercase();
                let first = mapped.next().unwrap();
                if mapped.next().is_none() {
                    first
                } else {
                    character
                }
            };
            first
                .chars()
                .map(uppercase)
                .eq(second.chars().map(uppercase))
        }
        _ => false,
    }
}
