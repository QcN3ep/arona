// SPDX-License-Identifier: MPL-2.0

//! ext4 路径、目录项与命名操作。

use super::{
    components,
    extent::Extent,
    inode::{Entry, Inode},
    volume::Volume,
};
use crate::{Error, block::BLOCK_SIZE};
use alloc::{collections::BTreeSet, rc::Rc, vec::Vec};

impl Volume {
    pub fn directory_path(&mut self, mut number: u32) -> Result<Vec<u8>, Error> {
        let mut parts = Vec::new();
        let mut visited = BTreeSet::new();
        while number != 2 {
            if !visited.insert(number) {
                return Err(Error::InvalidData);
            }
            let inode = self.inode(number)?;
            if !inode.directory() {
                return Err(Error::NotDirectory);
            }
            let parent = self
                .entries(&inode)?
                .into_iter()
                .find(|entry| entry.name == b"..")
                .ok_or(Error::InvalidData)?
                .inode;
            let parent_inode = self.inode(parent)?;
            let entry = self
                .entries(&parent_inode)?
                .into_iter()
                .find(|entry| entry.inode == number && entry.name != b"." && entry.name != b"..")
                .ok_or(Error::NotFound)?;
            parts.push(entry.name);
            number = parent;
        }
        parts.reverse();
        Ok(parts.join(&b'/'))
    }

    pub fn lookup(&mut self, path: &[u8]) -> Result<Inode, Error> {
        let mut inode = self.inode(2)?;
        for name in components(path)? {
            if !inode.directory() {
                return Err(Error::NotDirectory);
            }
            let number = self
                .entries(&inode)?
                .iter()
                .find(|entry| entry.name == name)
                .ok_or(Error::NotFound)?
                .inode;
            inode = self.inode(number)?;
        }
        Ok(inode)
    }

    pub(super) fn parent(&mut self, path: &[u8]) -> Result<(Inode, Vec<u8>), Error> {
        let parts = components(path)?;
        let (name, parents) = parts.split_last().ok_or(Error::InvalidRequest)?;
        let mut inode = self.inode(2)?;
        for part in parents {
            if !inode.directory() {
                return Err(Error::NotDirectory);
            }
            let number = self
                .entries(&inode)?
                .iter()
                .find(|entry| entry.name == *part)
                .ok_or(Error::NotFound)?
                .inode;
            inode = self.inode(number)?;
        }
        if !inode.directory() {
            return Err(Error::NotDirectory);
        }
        Ok((inode, name.to_vec()))
    }

    pub fn create(&mut self, path: &[u8], directory: bool, permissions: u16) -> Result<u32, Error> {
        let (mut parent, name) = self.parent(path)?;
        let mut entries = self.entries(&parent)?;
        if entries.iter().any(|entry| entry.name == name) {
            return Err(Error::AlreadyExists);
        }
        self.transaction(|volume| {
            let number = volume.allocate_inode(directory)?;
            volume.edit_inode(number, |bytes| {
                bytes.fill(0);
                if bytes.len() >= 160 {
                    bytes[128..130].copy_from_slice(&32u16.to_le_bytes());
                }
                Ok(())
            })?;
            let mut inode = Inode {
                number,
                mode: (if directory { 0x4000 } else { 0x8000 }) | permissions,
                size: 0,
                sectors: 0,
                orphan_next: 0,
                links: if directory { 2 } else { 1 },
                root: [0; 60],
                uid: 0,
                gid: 0,
                accessed: crate::Timestamp::default(),
                modified: crate::Timestamp::default(),
                changed: crate::Timestamp::default(),
                flags: 0x80000,
            };
            inode.root[..2].copy_from_slice(&0xf30au16.to_le_bytes());
            inode.root[4..6].copy_from_slice(&4u16.to_le_bytes());
            if directory {
                volume.set_entries(
                    &mut inode,
                    &[
                        Entry {
                            inode: number,
                            name: b".".to_vec(),
                            file_type: 2,
                        },
                        Entry {
                            inode: parent.number,
                            name: b"..".to_vec(),
                            file_type: 2,
                        },
                    ],
                )?;
                parent.links = parent.links.checked_add(1).ok_or(Error::NoSpace)?;
            } else {
                volume.store_inode(&inode)?;
            }
            entries.push(Entry {
                inode: number,
                name,
                file_type: if directory { 2 } else { 1 },
            });
            volume.set_entries(&mut parent, &entries)?;
            Ok(number)
        })
    }

    pub fn unlink(&mut self, path: &[u8]) -> Result<(), Error> {
        let (mut parent, name) = self.parent(path)?;
        let mut entries = self.entries(&parent)?;
        let index = entries
            .iter()
            .position(|entry| entry.name == name)
            .ok_or(Error::NotFound)?;
        let mut inode = self.inode(entries[index].inode)?;
        if inode.directory() && self.entries(&inode)?.len() != 2 {
            return Err(Error::NotEmpty);
        }
        self.transaction(|volume| {
            entries.remove(index);
            if inode.directory() {
                parent.links = parent.links.checked_sub(1).ok_or(Error::InvalidData)?;
            }
            volume.set_entries(&mut parent, &entries)?;
            volume.remove_link(&mut inode)
        })
    }

    pub fn rename(&mut self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let (mut old_parent, old_name) = self.parent(source)?;
        let (mut new_parent, new_name) = self.parent(target)?;
        let mut old_entries = self.entries(&old_parent)?;
        let old_index = old_entries
            .iter()
            .position(|entry| entry.name == old_name)
            .ok_or(Error::NotFound)?;
        let mut inode = self.inode(old_entries[old_index].inode)?;
        let same = old_parent.number == new_parent.number;
        if same && old_name == new_name {
            return Ok(());
        }
        let mut new_entries = if same {
            old_entries.clone()
        } else {
            self.entries(&new_parent)?
        };
        let replacement = new_entries
            .iter()
            .position(|entry| entry.name == new_name)
            .map(|index| self.inode(new_entries[index].inode))
            .transpose()?;
        if replacement
            .as_ref()
            .is_some_and(|other| other.number == inode.number)
        {
            return Ok(());
        }
        if let Some(other) = &replacement {
            if other.directory() != inode.directory() {
                return Err(if other.directory() {
                    Error::IsDirectory
                } else {
                    Error::NotDirectory
                });
            }
            if other.directory() && self.entries(other)?.len() != 2 {
                return Err(Error::NotEmpty);
            }
        }
        if inode.directory() {
            self.check_ancestry(inode.number, new_parent.number)?;
        }
        self.transaction(|volume| {
            old_entries.remove(old_index);
            if same {
                new_entries = old_entries.clone();
            }
            if let Some(mut other) = replacement {
                new_entries.retain(|entry| entry.name != new_name);
                if other.directory() {
                    new_parent.links = new_parent.links.checked_sub(1).ok_or(Error::InvalidData)?;
                }
                volume.remove_link(&mut other)?;
            }
            new_entries.push(Entry {
                inode: inode.number,
                name: new_name,
                file_type: inode.file_type(),
            });
            if inode.directory() && !same {
                let mut children = volume.entries(&inode)?;
                children
                    .iter_mut()
                    .find(|entry| entry.name == b"..")
                    .ok_or(Error::InvalidData)?
                    .inode = new_parent.number;
                volume.set_entries(&mut inode, &children)?;
                old_parent.links = old_parent.links.checked_sub(1).ok_or(Error::InvalidData)?;
                new_parent.links = new_parent.links.checked_add(1).ok_or(Error::NoSpace)?;
            }
            if !same {
                volume.set_entries(&mut old_parent, &old_entries)?;
            }
            volume.set_entries(&mut new_parent, &new_entries)
        })
    }

    fn check_ancestry(&mut self, source: u32, mut parent: u32) -> Result<(), Error> {
        let mut visited = BTreeSet::new();
        loop {
            if source == parent {
                return Err(Error::InvalidRequest);
            }
            if parent == 2 {
                return Ok(());
            }
            if !visited.insert(parent) {
                return Err(Error::InvalidData);
            }
            let inode = self.inode(parent)?;
            parent = self
                .entries(&inode)?
                .iter()
                .find(|entry| entry.name == b"..")
                .ok_or(Error::InvalidData)?
                .inode;
        }
    }

    pub fn remove_link(&mut self, inode: &mut Inode) -> Result<(), Error> {
        inode.links = if inode.directory() {
            0
        } else {
            inode.links.checked_sub(1).ok_or(Error::InvalidData)?
        };
        self.store_inode(inode)?;
        if inode.links == 0 {
            if self.is_open(inode.number) {
                self.add_orphan(inode)?;
            } else {
                self.destroy_inode(inode)?;
            }
        }
        Ok(())
    }

    pub fn destroy_inode(&mut self, inode: &mut Inode) -> Result<(), Error> {
        if inode.symlink() && inode.flags == 0 {
            inode.size = 0;
            inode.root.fill(0);
        } else {
            self.resize(inode, 0)?;
        }
        self.free_inode(inode.number, inode.directory())
    }

    pub fn set_entries(&mut self, inode: &mut Inode, entries: &[Entry]) -> Result<(), Error> {
        let old_extents = self.extents(&inode.root)?;
        let mut blocks: Vec<Rc<[u8; BLOCK_SIZE]>> = Vec::new();
        let mut inside = BLOCK_SIZE;
        let mut previous = 0;
        for entry in entries {
            let size = (8 + entry.name.len()).next_multiple_of(4);
            if inside + size > BLOCK_SIZE {
                blocks.push(Rc::new([0; BLOCK_SIZE]));
                inside = 0;
            } else if inside != 0 {
                let bytes = Rc::make_mut(blocks.last_mut().unwrap());
                bytes[previous + 4..previous + 6]
                    .copy_from_slice(&((inside - previous) as u16).to_le_bytes());
            }
            let bytes = Rc::make_mut(blocks.last_mut().unwrap());
            bytes[inside..inside + 4].copy_from_slice(&entry.inode.to_le_bytes());
            bytes[inside + 4..inside + 6]
                .copy_from_slice(&((BLOCK_SIZE - inside) as u16).to_le_bytes());
            bytes[inside + 6] = entry.name.len() as u8;
            bytes[inside + 7] = entry.file_type;
            bytes[inside + 8..inside + 8 + entry.name.len()].copy_from_slice(&entry.name);
            previous = inside;
            inside += size;
        }
        let mut old_blocks = old_extents
            .iter()
            .flat_map(|extent| extent.physical..extent.physical + extent.length);
        let mut extents = Vec::new();
        for (logical, bytes) in blocks.into_iter().enumerate() {
            let physical = match old_blocks.next() {
                Some(number) => number,
                None => self.allocate_block()?,
            };
            self.stage(physical, bytes);
            extents.push(Extent {
                logical: logical as u32,
                physical,
                length: 1,
                unwritten: false,
            });
        }
        inode.size = extents.len() as u64 * BLOCK_SIZE as u64;
        self.replace_extents(inode, extents)?;
        self.store_inode(inode)?;
        for number in old_blocks {
            self.free_block(number)?;
        }
        Ok(())
    }
}
