// SPDX-License-Identifier: MPL-2.0

//! 按游标读取内核日志。

use crate::{Error, Handle, KernelLog};
pub use abi::logger::{Level, MAX_RECORD_SIZE, Record};

impl Handle<KernelLog> {
    /// cursor 为零时读取最早记录，之后使用上一条记录的 sequence + 1。
    pub fn read<'a>(&self, cursor: u64, output: &'a mut [u8]) -> Result<Option<Record<'a>>, Error> {
        let [length, _] = self.borrow().invoke(
            abi::method::kernel_log::READ,
            [
                cursor,
                output.as_mut_ptr() as u64,
                output.len() as u64,
                0,
                0,
            ],
        )?;
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
