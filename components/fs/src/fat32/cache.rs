// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Bounded clean-sector storage with FIFO eviction.

use alloc::{
    collections::{BTreeMap, VecDeque},
    vec::Vec,
};

const BYTES: usize = 256 * 1024;

pub(super) struct Cache {
    sectors: BTreeMap<u64, Vec<u8>>,
    order: VecDeque<u64>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            sectors: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn read(&self, offset: u64, inside: usize, output: &mut [u8]) -> bool {
        let Some(bytes) = self.sectors.get(&offset) else {
            return false;
        };
        output.copy_from_slice(&bytes[inside..inside + output.len()]);
        true
    }

    pub fn insert(&mut self, offset: u64, bytes: &[u8]) {
        if !self.sectors.contains_key(&offset) {
            while self.sectors.len() >= BYTES / bytes.len() {
                self.sectors.remove(&self.order.pop_front().unwrap());
            }
            self.order.push_back(offset);
        }
        self.sectors.insert(offset, bytes.to_vec());
    }

    pub fn remove(&mut self, offset: u64) {
        if self.sectors.remove(&offset).is_some() {
            self.order.retain(|entry| *entry != offset);
        }
    }
}
