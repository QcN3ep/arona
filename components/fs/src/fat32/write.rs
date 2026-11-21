// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{
    directory::Entry,
    volume::{END, ROOT, Volume},
    word,
};
use crate::{Error, block::MAX_BLOCK_SIZE};
use alloc::{rc::Rc, vec, vec::Vec};
use core::cell::RefCell;

impl Volume {
    pub fn check_writable(&self) -> Result<(), Error> {
        self.check()?;
        if self.disk.read_only() {
            Err(Error::ReadOnly)
        } else {
            Ok(())
        }
    }

    // Stage allocation and directory edits until all fallible validation has succeeded.
    // A device failure can leave partial writes; stop using the volume in that case.
    pub fn transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.check_writable()?;
        let next_free = self.next_free;
        let result = operation(self).and_then(|result| {
            if self.pending.keys().any(|offset| {
                *offset >= u64::from(self.fat_start) * self.sector_size as u64
                    && *offset < u64::from(self.data_start) * self.sector_size as u64
            }) {
                for sector in self.info.clone() {
                    let offset = u64::from(sector) * self.sector_size as u64;
                    self.stage_bytes(offset + 488, &u32::MAX.to_le_bytes())?;
                    self.stage_bytes(offset + 492, &self.next_free.to_le_bytes())?;
                }
            }
            for (&offset, bytes) in &self.pending {
                self.cache.borrow_mut().remove(offset);
                if let Err(error) = self.disk.write(self.start + offset, bytes) {
                    self.failed = true;
                    return Err(error);
                }
            }
            Ok(result)
        });
        self.pending.clear();
        if result.is_err() {
            self.next_free = next_free;
            if matches!(
                result,
                Err(Error::Io
                    | Error::Unavailable
                    | Error::Device(_)
                    | Error::Runtime(_)
                    | Error::Call(_))
            ) {
                self.failed = true;
            }
        }
        result
    }

    pub fn stage_bytes(&mut self, mut offset: u64, mut input: &[u8]) -> Result<(), Error> {
        if offset
            .checked_add(input.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::InvalidData);
        }
        while !input.is_empty() {
            let within = offset as usize % self.sector_size;
            let base = offset - within as u64;
            let count = input.len().min(self.sector_size - within);
            if !self.pending.contains_key(&base) {
                let mut sector = vec![0; self.sector_size];
                self.read_bytes(base, &mut sector)?;
                self.pending.insert(base, sector);
            }
            self.pending.get_mut(&base).unwrap()[within..within + count]
                .copy_from_slice(&input[..count]);
            input = &input[count..];
            offset += count as u64;
        }
        Ok(())
    }

    fn data_bytes(&mut self, mut offset: u64, mut input: &[u8]) -> Result<(), Error> {
        while !input.is_empty() {
            let within = offset as usize % self.sector_size;
            let base = offset - within as u64;
            let count = input.len().min(self.sector_size - within);
            let mut sector = [0; MAX_BLOCK_SIZE];
            if within != 0 || count != self.sector_size {
                self.read_bytes(base, &mut sector[..self.sector_size])?;
            }
            sector[within..within + count].copy_from_slice(&input[..count]);
            self.cache.borrow_mut().remove(base);
            if let Err(error) = self
                .disk
                .write(self.start + base, &sector[..self.sector_size])
            {
                self.failed = true;
                return Err(error);
            }
            input = &input[count..];
            offset += count as u64;
        }
        Ok(())
    }

    pub fn set_fat(&mut self, cluster: u32, value: u32) -> Result<(), Error> {
        self.cluster_offset(cluster)?;
        for fat in 0..self.fats {
            if !self.mirrored && fat != self.active_fat {
                continue;
            }
            let offset = (u64::from(self.fat_start) + u64::from(fat) * u64::from(self.fat_sectors))
                * self.sector_size as u64
                + u64::from(cluster) * 4;
            let mut old = [0; 4];
            self.read_bytes(offset, &mut old)?;
            self.stage_bytes(
                offset,
                &((word(&old, 0) & 0xf000_0000) | value).to_le_bytes(),
            )?;
        }
        Ok(())
    }

    pub fn allocate(&mut self, count: usize) -> Result<Vec<u32>, Error> {
        let mut found = Vec::new();
        for range in [self.next_free..self.clusters + 2, 2..self.next_free] {
            let mut cluster = range.start;
            while cluster < range.end && found.len() < count {
                let per_sector = self.sector_size as u32 / 4;
                let first = cluster / per_sector * per_sector;
                let mut bytes = [0; MAX_BLOCK_SIZE];
                let fat = self.fat_start + u32::from(self.active_fat) * self.fat_sectors;
                self.read_bytes(
                    u64::from(fat) * self.sector_size as u64 + u64::from(first) * 4,
                    &mut bytes[..self.sector_size],
                )?;
                while cluster < (first + per_sector).min(range.end) && found.len() < count {
                    if word(&bytes, (cluster - first) as usize * 4) & END == 0 {
                        found.push(cluster);
                    }
                    cluster += 1;
                }
            }
        }
        if found.len() != count {
            return Err(Error::NoSpace);
        }
        for (index, &cluster) in found.iter().enumerate() {
            self.set_fat(cluster, found.get(index + 1).copied().unwrap_or(END))?;
            let base = self.cluster_offset(cluster)?;
            for sector in 0..self.sectors_per_cluster {
                self.data_bytes(
                    base + u64::from(sector) * self.sector_size as u64,
                    &[0; MAX_BLOCK_SIZE][..self.sector_size],
                )?;
            }
        }
        if let Some(last) = found.last() {
            self.next_free = 2 + (last - 1) % self.clusters;
        }
        Ok(found)
    }

    pub fn free_chain(&mut self, first: u32) -> Result<(), Error> {
        for cluster in self.chain(first)? {
            self.set_fat(cluster, 0)?;
            self.next_free = self.next_free.min(cluster);
        }
        Ok(())
    }

    pub fn store_entry(&mut self, entry: &Entry) -> Result<(), Error> {
        if entry.location != ROOT {
            self.stage_bytes(entry.location, &entry.raw)?;
        }
        Ok(())
    }

    fn resize_chain(&mut self, entry: &mut Entry, count: usize) -> Result<Vec<u32>, Error> {
        let mut chain = self.chain(entry.first())?;
        if chain.len() < entry.size().div_ceil(self.cluster_size()) {
            return Err(Error::InvalidData);
        }
        if count > chain.len() {
            let extra = self.allocate(count - chain.len())?;
            if let Some(&last) = chain.last() {
                self.set_fat(last, extra[0])?;
            }
            chain.extend(extra);
        } else if count < chain.len() {
            if count != 0 {
                self.set_fat(chain[count - 1], END)?;
            }
            self.free_chain(chain[count])?;
            chain.truncate(count);
        }
        entry.set_first(chain.first().copied().unwrap_or(0));
        Ok(chain)
    }

    fn zero_range(&mut self, chain: &[u32], mut first: usize, end: usize) -> Result<(), Error> {
        let size = self.cluster_size();
        while first < end {
            let count = (end - first).min(size - first % size).min(MAX_BLOCK_SIZE);
            self.data_bytes(
                self.cluster_offset(chain[first / size])? + (first % size) as u64,
                &[0; MAX_BLOCK_SIZE][..count],
            )?;
            first += count;
        }
        Ok(())
    }

    pub fn write(
        &mut self,
        node: &Rc<RefCell<super::Node>>,
        offset: usize,
        input: &[u8],
    ) -> Result<usize, Error> {
        self.check_file_writable(&node.borrow())?;
        if input.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(input.len())
            .filter(|end| *end <= u32::MAX as usize)
            .ok_or(Error::FileTooLarge)?;
        let mut entry = node.borrow().entry.clone();
        let linked = node.borrow().linked;
        self.transaction(|volume| {
            let size = volume.cluster_size();
            let count = end.max(entry.size()).div_ceil(size);
            let chain = volume.resize_chain(&mut entry, count)?;
            volume.zero_range(&chain, entry.size(), offset)?;
            let mut done = 0;
            while done < input.len() {
                let position = offset + done;
                let count = (input.len() - done).min(size - position % size);
                volume.data_bytes(
                    volume.cluster_offset(chain[position / size])? + (position % size) as u64,
                    &input[done..done + count],
                )?;
                done += count;
            }
            entry.set_size(entry.size().max(end));
            entry.raw[11] |= 0x20;
            if linked {
                volume.store_entry(&entry)?;
            }
            Ok(())
        })?;
        node.borrow_mut().entry = entry;
        Ok(input.len())
    }

    pub fn truncate(&mut self, node: &Rc<RefCell<super::Node>>, size: usize) -> Result<(), Error> {
        self.check_file_writable(&node.borrow())?;
        if size > u32::MAX as usize {
            return Err(Error::FileTooLarge);
        }
        let mut entry = node.borrow().entry.clone();
        let linked = node.borrow().linked;
        self.transaction(|volume| {
            let chain = volume.resize_chain(&mut entry, size.div_ceil(volume.cluster_size()))?;
            volume.zero_range(&chain, entry.size(), size)?;
            entry.set_size(size);
            entry.raw[11] |= 0x20;
            if linked {
                volume.store_entry(&entry)?;
            }
            Ok(())
        })?;
        node.borrow_mut().entry = entry;
        Ok(())
    }

    pub fn check_file_writable(&self, node: &super::Node) -> Result<(), Error> {
        self.check_writable()?;
        if node.entry.read_only() {
            return Err(Error::ReadOnly);
        }
        if node.entry.directory() {
            return Err(Error::IsDirectory);
        }
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), Error> {
        self.check()?;
        if self.disk.read_only() {
            return Ok(());
        }
        if !self.disk.supports_flush() {
            return Err(Error::Unsupported);
        }
        if let Err(error) = self.disk.flush() {
            self.failed = true;
            return Err(error);
        }
        Ok(())
    }
}
