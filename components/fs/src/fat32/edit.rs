// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{
    Node,
    directory::{Entry, checksum, equal_name},
    half,
    volume::{ROOT, Volume},
};
use crate::{Error, block::MAX_BLOCK_SIZE};
use alloc::{collections::BTreeSet, rc::Rc, vec::Vec};
use core::cell::RefCell;

impl Volume {
    fn parent(&mut self, path: &[u8]) -> Result<(Rc<RefCell<Node>>, Vec<u8>), Error> {
        let parts = crate::path::components(path)?;
        let (name, parents) = parts.split_last().ok_or(Error::Busy)?;
        validate_name(name)?;
        let path = parents.join(&b'/');
        let parent = self.lookup(if path.is_empty() { b"/" } else { &path })?;
        if !parent.borrow().entry.directory() {
            return Err(Error::NotDirectory);
        }
        if parent.borrow().entry.read_only() {
            return Err(Error::ReadOnly);
        }
        Ok((parent, name.to_vec()))
    }

    fn reserve_entries(&mut self, directory: u32, count: usize) -> Result<Vec<u64>, Error> {
        loop {
            let chain = self.chain(directory)?;
            let mut slots = Vec::new();
            for &cluster in &chain {
                let base = self.cluster_offset(cluster)?;
                for sector in 0..self.sectors_per_cluster {
                    let offset = base + u64::from(sector) * self.sector_size as u64;
                    let mut bytes = [0; MAX_BLOCK_SIZE];
                    self.read_bytes(offset, &mut bytes[..self.sector_size])?;
                    for (index, raw) in bytes[..self.sector_size].chunks_exact(32).enumerate() {
                        slots.push((offset + (index * 32) as u64, raw[0]));
                    }
                }
            }
            let mut run = Vec::new();
            let mut end = false;
            for (index, &(location, first)) in slots.iter().enumerate() {
                end |= first == 0;
                if first == 0xe5 || end {
                    run.push(location);
                } else {
                    run.clear();
                }
                if run.len() == count {
                    if end && let Some(&(next, _)) = slots.get(index + 1) {
                        self.stage_bytes(next, &[0; 32])?;
                    }
                    return Ok(run);
                }
            }
            let extra = self.allocate((count - run.len()).div_ceil(self.cluster_size() / 32))?;
            self.set_fat(*chain.last().ok_or(Error::InvalidData)?, extra[0])?;
        }
    }

    fn insert_entry(
        &mut self,
        parent: u32,
        name: &[u8],
        mut raw: [u8; 32],
    ) -> Result<Entry, Error> {
        validate_name(name)?;
        let entries = self.entries(parent)?;
        if entries.iter().any(|entry| equal_name(&entry.name, name)) {
            return Err(Error::AlreadyExists);
        }
        let alias = alias(name, &entries)?;
        raw[..11].copy_from_slice(&alias);
        raw[12] = 0;
        let words: Vec<u16> = core::str::from_utf8(name)
            .map_err(|_| Error::InvalidRequest)?
            .encode_utf16()
            .collect();
        let count = words.len().div_ceil(13);
        let mut slots = self.reserve_entries(parent, count + 1)?;
        let location = slots.pop().unwrap();
        for (index, &slot) in slots.iter().enumerate() {
            let order = count - index;
            let mut bytes = [0xff; 32];
            bytes[0] = order as u8 | if index == 0 { 0x40 } else { 0 };
            bytes[11] = 15;
            bytes[12] = 0;
            bytes[13] = checksum(&alias);
            bytes[26..28].fill(0);
            for (index, offset) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                .into_iter()
                .enumerate()
            {
                let position = (order - 1) * 13 + index;
                let word = words
                    .get(position)
                    .copied()
                    .unwrap_or(if position == words.len() { 0 } else { 0xffff });
                bytes[offset..offset + 2].copy_from_slice(&word.to_le_bytes());
            }
            self.stage_bytes(slot, &bytes)?;
        }
        self.stage_bytes(location, &raw)?;
        Ok(Entry {
            location,
            parent,
            long: slots,
            name: name.to_vec(),
            raw,
        })
    }

    fn delete_entry(&mut self, entry: &Entry) -> Result<(), Error> {
        for &slot in entry.long.iter().chain(core::iter::once(&entry.location)) {
            self.stage_bytes(slot, &[0xe5])?;
        }
        Ok(())
    }

    pub fn create(
        &mut self,
        path: &[u8],
        directory: bool,
        permissions: u16,
    ) -> Result<Rc<RefCell<Node>>, Error> {
        self.check_writable()?;
        if permissions & !0o7777 != 0 {
            return Err(Error::InvalidRequest);
        }
        match self.lookup(path) {
            Ok(_) => return Err(Error::AlreadyExists),
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        let (parent, name) = self.parent(path)?;
        let parent = parent.borrow().entry.first();
        let entry = self.transaction(|volume| {
            let mut raw = [0; 32];
            raw[11] = if directory { 0x10 } else { 0x20 } | u8::from(permissions & 0o222 == 0);
            // New files have the earliest representable FAT date until attributes are set.
            for offset in [16, 18, 24] {
                raw[offset..offset + 2].copy_from_slice(&0x21u16.to_le_bytes());
            }
            if directory {
                let cluster = volume.allocate(1)?[0];
                raw[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
                raw[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
                let base = volume.cluster_offset(cluster)?;
                let mut dot = raw;
                dot[..11].copy_from_slice(b".          ");
                volume.stage_bytes(base, &dot)?;
                dot[..11].copy_from_slice(b"..         ");
                let parent = if parent == volume.root { 0 } else { parent };
                dot[20..22].copy_from_slice(&((parent >> 16) as u16).to_le_bytes());
                dot[26..28].copy_from_slice(&(parent as u16).to_le_bytes());
                volume.stage_bytes(base + 32, &dot)?;
            }
            volume.insert_entry(parent, &name, raw)
        })?;
        self.node(entry)
    }

    fn has_open(&self, entry: &Entry) -> bool {
        self.nodes
            .get(&entry.location)
            .is_some_and(|node| Rc::strong_count(node) > 1)
    }

    fn detach(&mut self, entry: &Entry) {
        if let Some(node) = self.nodes.remove(&entry.location) {
            node.borrow_mut().linked = false;
        }
    }

    pub fn unlink(&mut self, path: &[u8]) -> Result<(), Error> {
        self.check_writable()?;
        let (parent, _) = self.parent(path)?;
        drop(parent);
        let node = self.lookup(path)?;
        let entry = node.borrow().entry.clone();
        if entry.read_only() {
            return Err(Error::ReadOnly);
        }
        if entry.directory() && !self.entries(entry.first())?.is_empty() {
            return Err(Error::NotEmpty);
        }
        drop(node);
        let open = self.has_open(&entry);
        self.transaction(|volume| {
            volume.delete_entry(&entry)?;
            if !open {
                volume.free_chain(entry.first())?;
            }
            Ok(())
        })?;
        self.detach(&entry);
        Ok(())
    }

    pub fn rename(&mut self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        self.check_writable()?;
        let (old_parent, _) = self.parent(source)?;
        drop(old_parent);
        let (parent, name) = self.parent(target)?;
        let parent = parent.borrow().entry.first();
        let node = self.lookup(source)?;
        let old = node.borrow().entry.clone();
        if old.read_only() {
            return Err(Error::ReadOnly);
        }
        if old.directory() {
            self.check_ancestry(old.first(), parent)?;
        }
        let replaced = match self.lookup(target) {
            Ok(target) if Rc::ptr_eq(&target, &node) => None,
            Ok(target) => Some(target.borrow().entry.clone()),
            Err(Error::NotFound) => None,
            Err(error) => return Err(error),
        };
        if let Some(entry) = &replaced {
            if old.directory() != entry.directory() {
                return Err(if old.directory() {
                    Error::NotDirectory
                } else {
                    Error::IsDirectory
                });
            }
            if entry.read_only() {
                return Err(Error::ReadOnly);
            }
            if entry.directory() && !self.entries(entry.first())?.is_empty() {
                return Err(Error::NotEmpty);
            }
        }
        let open = replaced.as_ref().is_some_and(|entry| self.has_open(entry));
        let entry = self.transaction(|volume| {
            volume.delete_entry(&old)?;
            if let Some(entry) = &replaced {
                volume.delete_entry(entry)?;
                if !open {
                    volume.free_chain(entry.first())?;
                }
            }
            let entry = volume.insert_entry(parent, &name, old.raw)?;
            if old.directory() && old.parent != parent {
                let mut dot = old.raw;
                dot[..11].copy_from_slice(b"..         ");
                let first = if parent == volume.root { 0 } else { parent };
                dot[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
                dot[26..28].copy_from_slice(&(first as u16).to_le_bytes());
                volume.stage_bytes(volume.cluster_offset(old.first())? + 32, &dot)?;
            }
            Ok(entry)
        })?;
        if let Some(entry) = replaced {
            self.detach(&entry);
        }
        self.nodes.remove(&old.location);
        node.borrow_mut().entry = entry;
        self.nodes
            .insert(node.borrow().entry.location, Rc::clone(&node));
        Ok(())
    }

    pub fn parent_cluster(&self, directory: u32) -> Result<u32, Error> {
        if directory == self.root {
            return Ok(self.root);
        }
        let mut bytes = [0; 64];
        self.read_bytes(self.cluster_offset(directory)?, &mut bytes)?;
        let raw = &bytes[32..];
        if &raw[..11] != b"..         " || raw[11] & 0x10 == 0 {
            return Err(Error::InvalidData);
        }
        let first = (u32::from(half(raw, 20)) << 16) | u32::from(half(raw, 26));
        if first == 0 {
            Ok(self.root)
        } else {
            self.cluster_offset(first)?;
            Ok(first)
        }
    }

    fn check_ancestry(&self, source: u32, mut parent: u32) -> Result<(), Error> {
        let mut visited = BTreeSet::new();
        loop {
            if source == parent {
                return Err(Error::InvalidRequest);
            }
            if parent == self.root {
                return Ok(());
            }
            if !visited.insert(parent) {
                return Err(Error::InvalidData);
            }
            parent = self.parent_cluster(parent)?;
        }
    }

    pub fn directory_node(&mut self, cluster: u32) -> Result<Rc<RefCell<Node>>, Error> {
        if cluster == self.root {
            return Ok(Rc::clone(self.nodes.get(&ROOT).unwrap()));
        }
        let parent = self.parent_cluster(cluster)?;
        let entry = self
            .entries(parent)?
            .into_iter()
            .find(|entry| entry.directory() && entry.first() == cluster)
            .ok_or(Error::NotFound)?;
        self.node(entry)
    }

    pub fn directory_path(&mut self, node: &Rc<RefCell<Node>>) -> Result<Vec<u8>, Error> {
        let mut entry = node.borrow().entry.clone();
        if !node.borrow().linked {
            return Err(Error::NotFound);
        }
        let mut parts = Vec::new();
        let mut visited = BTreeSet::new();
        while entry.location != ROOT {
            if !visited.insert(entry.first()) {
                return Err(Error::InvalidData);
            }
            parts.push(entry.name);
            entry = self.directory_node(entry.parent)?.borrow().entry.clone();
        }
        parts.reverse();
        Ok(parts.join(&b'/'))
    }
}

fn validate_name(name: &[u8]) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 255
        || name.ends_with(b" ")
        || name.ends_with(b".")
        || name
            .iter()
            .any(|byte| *byte < 32 || b"\"*:<>?\\|/".contains(byte))
        || core::str::from_utf8(name).is_err()
    {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}

fn alias(name: &[u8], entries: &[Entry]) -> Result<[u8; 11], Error> {
    let split = name.iter().rposition(|byte| *byte == b'.');
    let (stem, extension) =
        split.map_or((name, b"".as_slice()), |at| (&name[..at], &name[at + 1..]));
    let legal = |byte: &u8| byte.is_ascii_alphanumeric() || b"$%'-_@~`!(){}^#&".contains(byte);
    let available = |raw: &[u8; 11]| !entries.iter().any(|entry| &entry.raw[..11] == raw);
    let mut raw = [b' '; 11];
    if !stem.is_empty()
        && stem.len() <= 8
        && extension.len() <= 3
        && stem.iter().chain(extension).all(legal)
    {
        for (out, &byte) in raw[..8].iter_mut().zip(stem) {
            *out = byte.to_ascii_uppercase();
        }
        for (out, &byte) in raw[8..].iter_mut().zip(extension) {
            *out = byte.to_ascii_uppercase();
        }
        if available(&raw) {
            return Ok(raw);
        }
    }
    let stem: Vec<u8> = stem
        .iter()
        .filter(|byte| **byte != b' ' && **byte != b'.')
        .map(|byte| {
            if legal(byte) {
                byte.to_ascii_uppercase()
            } else {
                b'_'
            }
        })
        .collect();
    for sequence in 1..=999999 {
        let suffix = alloc::format!("~{sequence}");
        raw.fill(b' ');
        for (out, &byte) in raw[..8 - suffix.len()].iter_mut().zip(&stem) {
            *out = byte;
        }
        let length = stem.len().min(8 - suffix.len());
        raw[length..length + suffix.len()].copy_from_slice(suffix.as_bytes());
        for (out, &byte) in raw[8..].iter_mut().zip(extension) {
            *out = if legal(&byte) {
                byte.to_ascii_uppercase()
            } else {
                b'_'
            };
        }
        if available(&raw) {
            return Ok(raw);
        }
    }
    Err(Error::NoSpace)
}
