// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Read the kernel log by cursor.

use crate::{Error, Handle, KernelLog};
pub use abi::logger::{Level, MAX_RECORD_SIZE, Record};

impl Handle<KernelLog> {
    pub fn capacity(&self) -> Result<usize, Error> {
        usize::try_from(self.borrow().query(abi::info::kernel_log::CAPACITY)?[0])
            .map_err(|_| Error::Protocol)
    }
    /// When cursor is zero, read the earliest record; afterward use the previous record's sequence + 1.
    pub fn read<'a>(&self, cursor: u64, output: &'a mut [u8]) -> Result<Option<Record<'a>>, Error> {
        // SAFETY: The output buffer remains exclusively borrowed until the call returns.
        let [length, _] = unsafe {
            self.borrow().invoke(
                abi::method::kernel_log::READ,
                [
                    cursor,
                    output.as_mut_ptr() as u64,
                    output.len() as u64,
                    0,
                    0,
                ],
            )
        }?;
        if length == 0 {
            return Ok(None);
        }
        let bytes = output
            .get(..usize::try_from(length).map_err(|_| Error::Protocol)?)
            .ok_or(Error::Protocol)?;
        let record = Record::decode(bytes).ok_or(Error::Protocol)?;
        if record.sequence < cursor {
            return Err(Error::Protocol);
        }
        Ok(Some(record))
    }
}
