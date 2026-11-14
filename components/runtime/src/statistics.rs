// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! System-wide memory and scheduler statistics.

use crate::{Error, Handle, Task};
use abi::method::task;

impl Handle<Task> {
    /// Total and free page-frame bytes of the physical memory pools.
    pub fn memory_stats(&self) -> Result<(u64, u64), Error> {
        let [total, free] = self.borrow().invoke(task::MEMORY_STATS, [0; 5])?;
        Ok((total, free))
    }

    /// Runnable-thread averages over 1, 5, and 15 minutes in 16.16 fixed point.
    pub fn load_average(&self) -> Result<[u32; 3], Error> {
        let [packed, fifteen] = self.borrow().invoke(task::LOAD_AVERAGE, [0; 5])?;
        Ok([(packed >> 32) as u32, packed as u32, fifteen as u32])
    }
}
