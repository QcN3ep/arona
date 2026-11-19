// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! System-wide memory and scheduler statistics.

use crate::{Error, Handle, SystemInfo};
pub use abi::system::RunnableIntegral;

impl Handle<SystemInfo> {
    /// Total and free physical page-frame bytes.
    pub fn memory_stats(&self) -> Result<(u64, u64), Error> {
        let [total, free] = self.borrow().query(abi::info::system::MEMORY)?;
        Ok((total, free))
    }

    pub fn runnable_integral(&self) -> Result<RunnableIntegral, Error> {
        let mut bytes = [0; RunnableIntegral::SIZE];
        let [written, _] = self.borrow().invoke(
            abi::method::INFO,
            [
                abi::info::system::RUNNABLE_INTEGRAL,
                0,
                0,
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
            ],
        )?;
        if written != bytes.len() as u64 {
            return Err(Error::Protocol);
        }
        Ok(RunnableIntegral::decode(&bytes))
    }
}
