// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Cryptographic random bytes from the kernel's firmware-seeded generator.

use crate::{Error, Handle, Task};

impl Handle<Task> {
    /// Fill the buffer with random bytes. Returns `Unsupported` when firmware did
    /// not supply a seed. Earlier chunks may have been written if a call fails.
    pub fn random(&self, output: &mut [u8]) -> Result<(), Error> {
        for chunk in output.chunks_mut(256) {
            self.borrow().invoke(
                abi::method::task::RANDOM,
                [chunk.as_mut_ptr() as u64, chunk.len() as u64, 0, 0, 0],
            )?;
        }
        Ok(())
    }
}
