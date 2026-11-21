// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    vec::Vec,
};
use core::cell::RefCell;

use super::{Node, cache::Cache, directory::Entry, half, word};
use crate::{
    Error,
    block::{Disk, MAX_BLOCK_SIZE},
};

pub(super) const ROOT: u64 = u64::MAX;
pub(super) const END: u32 = 0x0fff_ffff;

pub(super) struct Volume {
    pub disk: Rc<Disk>,
    pub start: u64,
    pub length: u64,
    pub sector_size: usize,
    pub sectors_per_cluster: u32,
    pub fat_start: u32,
    pub fat_sectors: u32,
    pub active_fat: u8,
    pub fats: u8,
    pub mirrored: bool,
    pub data_start: u32,
    pub clusters: u32,
    pub root: u32,
    pub device: u64,
    pub mount: u64,
    pub nodes: BTreeMap<u64, Rc<RefCell<Node>>>,
    pub next_inode: u64,
    pub closed: bool,
    pub failed: bool,
    pub pending: BTreeMap<u64, Vec<u8>>,
    pub(super) cache: RefCell<Cache>,
    pub next_free: u32,
    pub info: Vec<u32>,
}

impl Volume {
    // REFERENCE: https://elm-chan.org/docs/fat_e.html
    pub fn mount(disk: Rc<Disk>, start: u64, length: u64) -> Result<Self, Error> {
        if !start.is_multiple_of(512)
            || length < MAX_BLOCK_SIZE as u64
            || start
                .checked_add(length)
                .is_none_or(|end| end > disk.capacity())
        {
            return Err(Error::InvalidData);
        }
        let mut boot = [0; MAX_BLOCK_SIZE];
        disk.read(start, &mut boot)?;
        let sector_size = half(&boot, 11) as usize;
        let sectors_per_cluster = u32::from(boot[13]);
        let fat_start = u32::from(half(&boot, 14));
        let fats = boot[16];
        let fat_sectors = word(&boot, 36);
        let total = word(&boot, 32);
        let flags = half(&boot, 40);
        if !matches!(sector_size, 512 | 1024 | 2048 | 4096)
            || !start.is_multiple_of(sector_size as u64)
            || !sectors_per_cluster.is_power_of_two()
            || sectors_per_cluster > 128
            || fat_start == 0
            || !matches!(fats, 1 | 2)
            || fat_sectors == 0
            || half(&boot, 17) != 0
            || half(&boot, 19) != 0
            || half(&boot, 22) != 0
            || half(&boot, 42) != 0
            || half(&boot, 510) != 0xaa55
            || flags & !0x8f != 0
            || u64::from(total) * sector_size as u64 > length
        {
            return Err(Error::InvalidData);
        }
        let data_start = u64::from(fat_start) + u64::from(fats) * u64::from(fat_sectors);
        let clusters = u64::from(total)
            .checked_sub(data_start)
            .ok_or(Error::InvalidData)?
            / u64::from(sectors_per_cluster);
        let root = word(&boot, 44);
        let mirrored = flags & 0x80 == 0;
        let active_fat = if mirrored { 0 } else { (flags & 15) as u8 };
        if !(65525..0x0fff_fff5).contains(&clusters)
            || (clusters + 2) * 4 > u64::from(fat_sectors) * sector_size as u64
            || active_fat >= fats
            || !(2..clusters as u32 + 2).contains(&root)
        {
            return Err(Error::InvalidData);
        }
        let device = crate::metadata::device_identity()?;
        let mount = crate::metadata::mount_identity()?;
        disk.claim(start, length)?;
        let mut volume = Self {
            disk,
            start,
            length: u64::from(total) * sector_size as u64,
            sector_size,
            sectors_per_cluster,
            fat_start,
            fat_sectors,
            active_fat,
            fats,
            mirrored,
            data_start: data_start as u32,
            clusters: clusters as u32,
            root,
            device,
            mount,
            nodes: BTreeMap::new(),
            next_inode: 2,
            closed: false,
            failed: false,
            pending: BTreeMap::new(),
            cache: RefCell::new(Cache::new()),
            next_free: 2,
            info: Vec::new(),
        };
        volume.node(Entry::root(volume.root))?;
        volume.chain(root)?;
        volume.load_info(&boot)?;
        Ok(volume)
    }

    fn load_info(&mut self, boot: &[u8]) -> Result<(), Error> {
        let info = u32::from(half(boot, 48));
        let backup = u32::from(half(boot, 50));
        for sector in [info, backup.saturating_add(info)] {
            if info == 0 || sector >= self.fat_start || self.info.contains(&sector) {
                continue;
            }
            let mut bytes = [0; MAX_BLOCK_SIZE];
            self.read_bytes(
                u64::from(sector) * self.sector_size as u64,
                &mut bytes[..self.sector_size],
            )?;
            if word(&bytes, 0) == 0x4161_5252
                && word(&bytes, 484) == 0x6141_7272
                && word(&bytes, 508) == 0xaa55_0000
            {
                self.info.push(sector);
                let next = word(&bytes, 492);
                if (2..self.clusters + 2).contains(&next) {
                    self.next_free = next;
                }
            }
        }
        Ok(())
    }

    pub fn check(&self) -> Result<(), Error> {
        if self.closed || self.failed {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    }

    pub fn cluster_size(&self) -> usize {
        self.sector_size * self.sectors_per_cluster as usize
    }

    pub fn cluster_offset(&self, cluster: u32) -> Result<u64, Error> {
        if !(2..self.clusters + 2).contains(&cluster) {
            return Err(Error::InvalidData);
        }
        Ok((u64::from(self.data_start)
            + u64::from(cluster - 2) * u64::from(self.sectors_per_cluster))
            * self.sector_size as u64)
    }

    pub fn read_bytes(&self, mut offset: u64, mut output: &mut [u8]) -> Result<(), Error> {
        self.check()?;
        if offset
            .checked_add(output.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::InvalidData);
        }
        while !output.is_empty() {
            let within = offset as usize % self.sector_size;
            let count = output.len().min(self.sector_size - within);
            if let Some(bytes) = self.pending.get(&(offset - within as u64)) {
                output[..count].copy_from_slice(&bytes[within..within + count]);
            } else if !self.cache.borrow().read(
                offset - within as u64,
                within,
                &mut output[..count],
            ) {
                let mut sector = [0; MAX_BLOCK_SIZE];
                let base = offset - within as u64;
                self.disk
                    .read(self.start + base, &mut sector[..self.sector_size])?;
                self.cache
                    .borrow_mut()
                    .insert(base, &sector[..self.sector_size]);
                output[..count].copy_from_slice(&sector[within..within + count]);
            }
            output = &mut output[count..];
            offset += count as u64;
        }
        Ok(())
    }

    pub fn fat(&self, cluster: u32) -> Result<u32, Error> {
        if cluster >= self.clusters + 2 {
            return Err(Error::InvalidData);
        }
        let sector =
            u64::from(self.fat_start) + u64::from(self.active_fat) * u64::from(self.fat_sectors);
        let mut entry = [0; 4];
        self.read_bytes(
            sector * self.sector_size as u64 + u64::from(cluster) * 4,
            &mut entry,
        )?;
        Ok(u32::from_le_bytes(entry) & END)
    }

    pub fn chain(&self, first: u32) -> Result<Vec<u32>, Error> {
        let mut chain = Vec::new();
        if first == 0 {
            return Ok(chain);
        }
        let mut current = first;
        let mut seen = BTreeSet::new();
        loop {
            self.cluster_offset(current)?;
            if !seen.insert(current) {
                return Err(Error::InvalidData);
            }
            chain.push(current);
            let next = self.fat(current)?;
            if next >= 0x0fff_fff8 {
                return Ok(chain);
            }
            current = next;
        }
    }

    pub fn node(&mut self, entry: Entry) -> Result<Rc<RefCell<Node>>, Error> {
        if let Some(node) = self.nodes.get(&entry.location) {
            return Ok(Rc::clone(node));
        }
        let inode = if entry.location == ROOT {
            1
        } else {
            let inode = self.next_inode;
            self.next_inode = inode.checked_add(1).ok_or(Error::NoSpace)?;
            inode
        };
        let node = Rc::new(RefCell::new(Node {
            inode,
            entry,
            linked: true,
        }));
        self.nodes
            .insert(node.borrow().entry.location, Rc::clone(&node));
        Ok(node)
    }

    pub fn statistics(&self) -> Result<crate::Statistics, Error> {
        self.check()?;
        let mut free = 0;
        let mut sector = [0; MAX_BLOCK_SIZE];
        let entries = self.sector_size / 4;
        for first in (0..self.clusters + 2).step_by(entries) {
            let fat = self.fat_start + u32::from(self.active_fat) * self.fat_sectors;
            self.read_bytes(
                u64::from(fat) * self.sector_size as u64 + u64::from(first) * 4,
                &mut sector[..self.sector_size],
            )?;
            for index in first.max(2)..(first + entries as u32).min(self.clusters + 2) {
                free += u64::from(word(&sector, (index - first) as usize * 4) & END == 0);
            }
        }
        Ok(crate::Statistics {
            kind: 0x4d44,
            device: self.device,
            block_size: self.cluster_size() as u64,
            blocks: u64::from(self.clusters),
            free_blocks: free,
            available_blocks: free,
            files: 0,
            free_files: 0,
            name_length: 255,
            flags: u64::from(self.disk.read_only()),
        })
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        if !self.closed {
            self.disk.release(self.start);
        }
    }
}
