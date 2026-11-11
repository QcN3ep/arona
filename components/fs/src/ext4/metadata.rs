// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{inode::Entry, volume::Volume};
use crate::{Attributes, Error};
use alloc::vec::Vec;

impl Volume {
    pub fn set_attributes(&mut self, number: u32, attributes: Attributes) -> Result<(), Error> {
        attributes.validate()?;
        let mut inode = self.inode(number)?;
        inode.mode = (inode.mode & 0o170000) | attributes.permissions as u16;
        inode.uid = attributes.uid;
        inode.gid = attributes.gid;
        inode.accessed = attributes.accessed;
        inode.modified = attributes.modified;
        inode.changed = attributes.changed;
        self.transaction(|volume| volume.store_inode(&inode))
    }

    pub fn hard_link(&mut self, source: &[u8], target: &[u8]) -> Result<(), Error> {
        let mut inode = self.lookup(source)?;
        if inode.directory() {
            return Err(Error::IsDirectory);
        }
        let (mut parent, name) = self.parent(target)?;
        let mut entries = self.entries(&parent)?;
        if entries.iter().any(|entry| entry.name == name) {
            return Err(Error::AlreadyExists);
        }
        inode.links = inode.links.checked_add(1).ok_or(Error::NoSpace)?;
        entries.push(Entry {
            inode: inode.number,
            name,
            file_type: inode.file_type(),
        });
        self.transaction(|volume| {
            volume.store_inode(&inode)?;
            volume.set_entries(&mut parent, &entries)
        })
    }

    pub fn symlink(&mut self, target: &[u8], path: &[u8]) -> Result<(), Error> {
        if target.is_empty() || target.len() >= 4096 || target.contains(&0) {
            return Err(Error::InvalidRequest);
        }
        let (mut parent, name) = self.parent(path)?;
        let mut entries = self.entries(&parent)?;
        if entries.iter().any(|entry| entry.name == name) {
            return Err(Error::AlreadyExists);
        }
        self.transaction(|volume| {
            let number = volume.allocate_inode(false)?;
            volume.edit_inode(number, |bytes| {
                bytes.fill(0);
                bytes[..2].copy_from_slice(&0xa1ffu16.to_le_bytes());
                bytes[4..8].copy_from_slice(&(target.len() as u32).to_le_bytes());
                bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
                if bytes.len() >= 160 {
                    bytes[128..130].copy_from_slice(&32u16.to_le_bytes());
                }
                if target.len() >= 60 {
                    bytes[32..36].copy_from_slice(&0x80000u32.to_le_bytes());
                    bytes[40..42].copy_from_slice(&0xf30au16.to_le_bytes());
                    bytes[44..46].copy_from_slice(&4u16.to_le_bytes());
                }
                Ok(())
            })?;
            let mut inode = volume.inode(number)?;
            if target.len() < 60 {
                inode.root[..target.len()].copy_from_slice(target);
            } else {
                let physical = volume.allocate_block()?;
                let mut bytes = [0; crate::block::BLOCK_SIZE];
                bytes[..target.len()].copy_from_slice(target);
                volume.stage(physical, alloc::rc::Rc::new(bytes));
                volume.replace_extents(
                    &mut inode,
                    alloc::vec![super::extent::Extent {
                        logical: 0,
                        physical,
                        length: 1,
                        unwritten: false
                    }],
                )?;
            }
            inode.size = target.len() as u64;
            volume.store_inode(&inode)?;
            entries.push(Entry {
                inode: number,
                name,
                file_type: 7,
            });
            volume.set_entries(&mut parent, &entries)
        })
    }

    pub fn read_link(&mut self, path: &[u8]) -> Result<Vec<u8>, Error> {
        let inode = self.lookup(path)?;
        if !inode.symlink() {
            return Err(Error::InvalidRequest);
        }
        if inode.size > 4096 {
            return Err(Error::InvalidData);
        }
        let mut bytes = alloc::vec![0; inode.size as usize];
        if self.read(&inode, 0, &mut bytes)? != bytes.len() {
            return Err(Error::InvalidData);
        }
        Ok(bytes)
    }
}
