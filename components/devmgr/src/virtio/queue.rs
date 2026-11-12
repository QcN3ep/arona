// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Split virtqueue descriptor publication and used ring consumption.

use alloc::vec::Vec;
use runtime::{
    DmaDomain, Handle, Task,
    dma::{self, DmaBuffer, DmaDirection},
};

use super::{Notification, Transport};
use crate::Error;

pub(crate) const MAX_SIZE: usize = 256;
const PAGE_SIZE: usize = 4096;

pub(crate) struct Queue<'a> {
    notification: Notification,
    descriptors: DmaBuffer<'a>,
    available: DmaBuffer<'a>,
    used: DmaBuffer<'a>,
    size: u16,
    next_available: u16,
    next_used: u16,
    pending: Vec<Option<Pending>>,
    occupied: Vec<bool>,
    in_flight: u16,
}

#[derive(Clone, Copy)]
struct Pending {
    descriptors: usize,
    writable: Option<u32>,
}

#[derive(Clone, Copy)]
pub(crate) struct Descriptor {
    pub(crate) address: u64,
    pub(crate) length: u32,
    pub(crate) writable: bool,
}

impl<'a> Queue<'a> {
    pub(crate) fn new(
        domain: &'a Handle<DmaDomain>,
        task: &Handle<Task>,
        transport: &mut Transport<'_>,
        index: u16,
    ) -> Result<Self, Error> {
        let maximum = transport.queue_size(index)?;
        if maximum == 0 || !maximum.is_power_of_two() {
            return Err(Error::InvalidConfiguration);
        }
        let maximum = maximum.min(MAX_SIZE as u16);
        let mut descriptors = domain.allocate(task, 1, DmaDirection::Bidirectional)?;
        let mut available = domain.allocate(task, 1, DmaDirection::Bidirectional)?;
        let mut used = domain.allocate(task, 1, DmaDirection::Bidirectional)?;
        for buffer in [&mut descriptors, &mut available, &mut used] {
            buffer.write(0, &[0; PAGE_SIZE])?;
        }
        let notification = transport.configure_queue(
            index,
            maximum,
            descriptors.bus_address().unwrap(),
            available.bus_address().unwrap(),
            used.bus_address().unwrap(),
        )?;
        Ok(Self {
            notification,
            descriptors,
            available,
            used,
            size: maximum,
            next_available: 0,
            next_used: 0,
            pending: alloc::vec![None; usize::from(maximum)],
            occupied: alloc::vec![false; usize::from(maximum)],
            in_flight: 0,
        })
    }

    pub(crate) fn size(&self) -> usize {
        usize::from(self.size)
    }
    pub(crate) fn in_flight(&self) -> u16 {
        self.in_flight
    }

    pub(crate) fn notify(&self, transport: &mut Transport<'_>) -> Result<(), Error> {
        transport.notify(self.notification)
    }

    pub(crate) fn publish(&mut self, head: u16, entries: &[Descriptor]) -> Result<(), Error> {
        let first = usize::from(head);
        let end = first
            .checked_add(entries.len())
            .ok_or(Error::InvalidRequest)?;
        if entries.is_empty() || end > self.size() || self.occupied[first..end].iter().any(|x| *x) {
            return Err(Error::Busy);
        }
        let mut writable = None::<u32>;
        for (index, entry) in entries.iter().enumerate() {
            if entry.address.checked_add(u64::from(entry.length)).is_none() {
                return Err(Error::InvalidRequest);
            }
            if entry.writable {
                writable = Some(
                    writable
                        .unwrap_or(0)
                        .checked_add(entry.length)
                        .ok_or(Error::InvalidRequest)?,
                );
            }
            let mut bytes = [0; 16];
            bytes[..8].copy_from_slice(&entry.address.to_le_bytes());
            bytes[8..12].copy_from_slice(&entry.length.to_le_bytes());
            let next = index + 1 < entries.len();
            let flags = u16::from(next) | (u16::from(entry.writable) << 1);
            bytes[12..14].copy_from_slice(&flags.to_le_bytes());
            bytes[14..].copy_from_slice(&(head + index as u16 + 1).to_le_bytes());
            self.descriptors
                .write(((first + index) * 16) as u64, &bytes)?;
        }
        self.available.write(
            4 + u64::from(self.next_available % self.size) * 2,
            &head.to_le_bytes(),
        )?;
        dma::synchronize();
        self.pending[first] = Some(Pending {
            descriptors: entries.len(),
            writable,
        });
        self.occupied[first..end].fill(true);
        self.in_flight += 1;
        self.next_available = self.next_available.wrapping_add(1);
        self.available
            .write(2, &self.next_available.to_le_bytes())?;
        Ok(())
    }

    /// Return the completed head and the bytes written by the device.
    /// All-readable chains require zero bytes. Writable chains require a
    /// nonzero length within their writable capacity, then driver validation.
    pub(crate) fn complete(&mut self) -> Result<Option<(u16, u32)>, Error> {
        let mut index = [0; 2];
        self.used.read(2, &mut index)?;
        let count = u16::from_le_bytes(index).wrapping_sub(self.next_used);
        if count > self.in_flight {
            return Err(Error::DeviceFault);
        }
        if count == 0 {
            return Ok(None);
        }
        dma::synchronize();
        let mut bytes = [0; 8];
        self.used
            .read(4 + u64::from(self.next_used % self.size) * 8, &mut bytes)?;
        let head = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(bytes[4..].try_into().unwrap());
        let pending = self
            .pending
            .get(head)
            .copied()
            .flatten()
            .ok_or(Error::DeviceFault)?;
        if pending
            .writable
            .map_or(length != 0, |capacity| length == 0 || length > capacity)
        {
            return Err(Error::DeviceFault);
        }
        self.pending[head] = None;
        self.occupied[head..head + pending.descriptors].fill(false);
        self.in_flight -= 1;
        self.next_used = self.next_used.wrapping_add(1);
        Ok(Some((head as u16, length)))
    }
}
