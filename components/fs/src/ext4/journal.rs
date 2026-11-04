// SPDX-License-Identifier: MPL-2.0

//! JBD2 事务提交与恢复。

// REFERENCE: https://www.kernel.org/doc/html/v6.12/filesystems/ext4/journal.html

use crate::{
    Error,
    block::{BLOCK_SIZE, Disk},
};
use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    vec::Vec,
};

const MAGIC: u32 = 0xc03b3998;
const CSUM_V3: u32 = 0x10;
const TAGS: usize = (BLOCK_SIZE - 12 - 16 - 4) / 16;
const CRC32C: [u32; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ (0x82f63b78 & 0u32.wrapping_sub(crc & 1));
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
};

pub(super) struct Journal {
    locations: Vec<u32>,
    reserved: BTreeSet<u32>,
    superblock: Box<[u8; BLOCK_SIZE]>,
    first: u32,
    length: u32,
    sequence: u32,
    seed: u32,
    checksummed: bool,
}

struct Transaction {
    writes: BTreeMap<u32, Rc<[u8; BLOCK_SIZE]>>,
    revoked: BTreeSet<u32>,
}

impl Journal {
    pub fn open(
        disk: &Disk,
        start: u64,
        locations: Vec<u32>,
        nodes: Vec<u32>,
        uuid: &[u8],
    ) -> Result<Self, Error> {
        let number = *locations.first().ok_or(Error::InvalidData)?;
        let mut bytes = Box::new([0; BLOCK_SIZE]);
        disk.read(start + u64::from(number) * BLOCK_SIZE as u64, &mut bytes)?;
        let first = be(&bytes[..], 20);
        let length = be(&bytes[..], 16);
        let features = be(&bytes[..], 40);
        let checksummed = features & CSUM_V3 != 0;
        if be(&bytes[..], 0) != MAGIC
            || be(&bytes[..], 4) != 4
            || be(&bytes[..], 12) != BLOCK_SIZE as u32
            || first == 0
            || length as usize > locations.len()
            || first >= length
            || be(&bytes[..], 28) != 0 && !(first..length).contains(&be(&bytes[..], 28))
            || be(&bytes[..], 32) != 0
            || be(&bytes[..], 36) != 0
            || features & !(1 | CSUM_V3) != 0
            || be(&bytes[..], 44) != 0
            || bytes[48..64] != *uuid
            || be(&bytes[..], 64) != 1
        {
            return Err(Error::Unsupported);
        }
        if checksummed && (bytes[80] != 4 || !verify(&bytes[..1024], 252, !0)) {
            return Err(Error::InvalidData);
        }
        let reserved = locations
            .iter()
            .copied()
            .chain(nodes.iter().copied())
            .collect::<BTreeSet<_>>();
        if reserved.len() != locations.len() + nodes.len() {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            locations,
            reserved,
            first,
            length,
            sequence: be(&bytes[..], 24),
            seed: crc32c(!0, uuid),
            checksummed,
            superblock: bytes,
        })
    }

    pub fn contains(&self, number: u32) -> bool {
        self.reserved.contains(&number)
    }

    pub fn pending(&self) -> bool {
        be(&self.superblock[..], 28) != 0
    }

    pub fn check_capacity(&self, count: usize) -> Result<(), Error> {
        if count
            .checked_add(count.div_ceil(TAGS))
            .and_then(|n| n.checked_add(1))
            .is_none_or(|n| n >= (self.length - self.first) as usize)
        {
            return Err(Error::NoSpace);
        }
        Ok(())
    }

    pub fn commit(
        &mut self,
        disk: &Disk,
        start: u64,
        pending: &BTreeMap<u32, Rc<[u8; BLOCK_SIZE]>>,
    ) -> Result<(), Error> {
        self.check_capacity(pending.len())?;
        let entries: Vec<_> = pending.iter().collect();
        let mut position = self.first;
        for chunk in entries.chunks(TAGS) {
            let mut descriptor = header(1, self.sequence);
            let mut tag = 12;
            for (index, &(number, bytes)) in chunk.iter().enumerate() {
                let escaped = be(&bytes[..], 0) == MAGIC;
                let flags = u32::from(escaped)
                    | if index != 0 { 2 } else { 0 }
                    | if index + 1 == chunk.len() { 8 } else { 0 };
                put(&mut descriptor[..], tag, *number);
                put(&mut descriptor[..], tag + 4, flags);
                let mut data = Box::new(**bytes);
                if escaped {
                    data[..4].fill(0);
                }
                put(
                    &mut descriptor[..],
                    tag + 12,
                    crc32c(crc32c(self.seed, &self.sequence.to_be_bytes()), &data[..]),
                );
                tag += 16;
                if index == 0 {
                    descriptor[tag..tag + 16].copy_from_slice(&self.superblock[48..64]);
                    tag += 16;
                }
                self.write(disk, start, position + 1 + index as u32, &data)?;
            }
            seal(&mut descriptor[..], BLOCK_SIZE - 4, self.seed);
            self.write(disk, start, position, &descriptor)?;
            position += chunk.len() as u32 + 1;
        }
        self.write(disk, start, position, &[0; BLOCK_SIZE])?;
        self.checksummed = true;
        let features = be(&self.superblock[..], 40) | CSUM_V3;
        put(&mut self.superblock[..], 40, features);
        self.superblock[80] = 4;
        self.update(disk, start, self.first, self.sequence)?;
        let mut commit = header(2, self.sequence);
        seal(&mut commit[..], 16, self.seed);
        self.write(disk, start, position, &commit)?;
        disk.flush()?;
        for (&number, bytes) in pending {
            disk.write(start + u64::from(number) * BLOCK_SIZE as u64, bytes)?;
        }
        disk.flush()?;
        self.update(disk, start, 0, self.sequence.wrapping_add(1))
    }

    pub fn recover(&mut self, disk: &Disk, start: u64, blocks: u32) -> Result<(), Error> {
        let position = be(&self.superblock[..], 28);
        if position == 0 {
            return Ok(());
        }
        let mut replay = Replay {
            journal: self,
            disk,
            start,
            blocks,
            position,
            consumed: 0,
            sequence: self.sequence,
        };
        let mut transactions = Vec::new();
        while replay.consumed < self.length - self.first {
            let Some(transaction) = replay.scan()? else {
                break;
            };
            transactions.push(transaction);
            replay.sequence = replay.sequence.wrapping_add(1);
        }
        let sequence = replay.sequence;
        let mut revoked = BTreeMap::new();
        for (index, transaction) in transactions.iter().enumerate() {
            for number in &transaction.revoked {
                revoked.insert(*number, index);
            }
        }
        for (index, transaction) in transactions.into_iter().enumerate() {
            for (number, bytes) in transaction.writes {
                if revoked.get(&number).is_none_or(|last| *last < index) {
                    disk.write(start + u64::from(number) * BLOCK_SIZE as u64, &bytes)?;
                }
            }
        }
        disk.flush()?;
        self.update(disk, start, 0, sequence)
    }

    fn write(
        &self,
        disk: &Disk,
        start: u64,
        position: u32,
        bytes: &[u8; BLOCK_SIZE],
    ) -> Result<(), Error> {
        disk.write(
            start + u64::from(self.locations[position as usize]) * BLOCK_SIZE as u64,
            bytes,
        )
    }

    fn update(
        &mut self,
        disk: &Disk,
        start: u64,
        position: u32,
        sequence: u32,
    ) -> Result<(), Error> {
        put(&mut self.superblock[..], 24, sequence);
        put(&mut self.superblock[..], 28, position);
        if self.checksummed {
            seal(&mut self.superblock[..1024], 252, !0);
        }
        self.write(disk, start, 0, &self.superblock)?;
        disk.flush()?;
        self.sequence = sequence;
        Ok(())
    }
}

struct Replay<'a> {
    journal: &'a Journal,
    disk: &'a Disk,
    start: u64,
    blocks: u32,
    position: u32,
    consumed: u32,
    sequence: u32,
}

impl Replay<'_> {
    fn scan(&mut self) -> Result<Option<Transaction>, Error> {
        let mut transaction = Transaction {
            writes: BTreeMap::new(),
            revoked: BTreeSet::new(),
        };
        loop {
            if self.consumed >= self.journal.length - self.journal.first {
                return Ok(None);
            }
            let bytes = self.next()?;
            if be(&bytes[..], 0) != MAGIC || be(&bytes[..], 8) != self.sequence {
                return Ok(None);
            }
            match be(&bytes[..], 4) {
                1 => self.descriptor(&bytes, &mut transaction)?,
                2 => {
                    if self.journal.checksummed && !verify(&bytes[..], 16, self.journal.seed) {
                        return Ok(None);
                    }
                    return Ok(Some(transaction));
                }
                5 => {
                    let end = be(&bytes[..], 12) as usize;
                    let limit = BLOCK_SIZE - if self.journal.checksummed { 4 } else { 0 };
                    if end < 16
                        || end > limit
                        || !end.is_multiple_of(4)
                        || self.journal.checksummed
                            && !verify(&bytes[..], BLOCK_SIZE - 4, self.journal.seed)
                    {
                        return Err(Error::InvalidData);
                    }
                    for offset in (16..end).step_by(4) {
                        let number = be(&bytes[..], offset);
                        self.validate_target(number)?;
                        transaction.revoked.insert(number);
                    }
                }
                _ => return Ok(None),
            }
        }
    }

    fn descriptor(
        &mut self,
        bytes: &[u8; BLOCK_SIZE],
        transaction: &mut Transaction,
    ) -> Result<(), Error> {
        let size = if self.journal.checksummed { 16 } else { 8 };
        let limit = BLOCK_SIZE - if self.journal.checksummed { 4 } else { 0 };
        if self.journal.checksummed && !verify(bytes, BLOCK_SIZE - 4, self.journal.seed) {
            return Err(Error::InvalidData);
        }
        let mut offset = 12;
        let mut first = true;
        while offset + size <= limit {
            let number = be(bytes, offset);
            let flags = if self.journal.checksummed {
                be(bytes, offset + 4)
            } else {
                u32::from(u16::from_be_bytes(
                    bytes[offset + 6..offset + 8].try_into().unwrap(),
                ))
            };
            if flags & !15 != 0
                || flags & 4 != 0
                || self.journal.checksummed && be(bytes, offset + 8) != 0
            {
                return Err(Error::InvalidData);
            }
            self.validate_target(number)?;
            let checksum = if self.journal.checksummed {
                be(bytes, offset + 12)
            } else {
                0
            };
            offset += size;
            if flags & 2 == 0 {
                if offset + 16 > limit
                    || bytes[offset..offset + 16] != self.journal.superblock[48..64]
                {
                    return Err(Error::InvalidData);
                }
                offset += 16;
            } else if first {
                return Err(Error::InvalidData);
            }
            first = false;
            let mut data = self.next()?;
            if self.journal.checksummed
                && checksum
                    != crc32c(
                        crc32c(self.journal.seed, &self.sequence.to_be_bytes()),
                        &data[..],
                    )
            {
                return Err(Error::InvalidData);
            }
            if flags & 1 != 0 {
                data[..4].copy_from_slice(&MAGIC.to_be_bytes());
            }
            transaction.writes.insert(number, Rc::new(*data));
            if flags & 8 != 0 {
                return Ok(());
            }
        }
        Err(Error::InvalidData)
    }

    fn validate_target(&self, number: u32) -> Result<(), Error> {
        if number >= self.blocks || self.journal.contains(number) {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn next(&mut self) -> Result<Box<[u8; BLOCK_SIZE]>, Error> {
        if self.consumed >= self.journal.length - self.journal.first {
            return Err(Error::InvalidData);
        }
        let mut bytes = Box::new([0; BLOCK_SIZE]);
        let number = self.journal.locations[self.position as usize];
        self.disk.read(
            self.start + u64::from(number) * BLOCK_SIZE as u64,
            &mut bytes,
        )?;
        self.consumed += 1;
        self.position += 1;
        if self.position == self.journal.length {
            self.position = self.journal.first;
        }
        Ok(bytes)
    }
}

fn header(kind: u32, sequence: u32) -> Box<[u8; BLOCK_SIZE]> {
    let mut bytes = Box::new([0; BLOCK_SIZE]);
    put(&mut bytes[..], 0, MAGIC);
    put(&mut bytes[..], 4, kind);
    put(&mut bytes[..], 8, sequence);
    bytes
}

fn be(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn crc32c(mut crc: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        crc = (crc >> 8) ^ CRC32C[((crc ^ u32::from(*byte)) & 0xff) as usize];
    }
    crc
}

fn seal(bytes: &mut [u8], offset: usize, seed: u32) {
    put(bytes, offset, 0);
    let checksum = crc32c(seed, bytes);
    put(bytes, offset, checksum);
}

fn verify(bytes: &[u8], offset: usize, seed: u32) -> bool {
    let crc = crc32c(seed, &bytes[..offset]);
    let crc = crc32c(crc, &[0; 4]);
    be(bytes, offset) == crc32c(crc, &bytes[offset + 4..])
}
