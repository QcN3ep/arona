// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Cryptographic random bytes from the kernel's firmware-seeded generator.

use crate::{Entropy, Error, Handle};

impl Handle<Entropy> {
    /// Fills the buffer from the firmware-seeded generator.
    /// Unseeded state returns InvalidState. A failed call may have written earlier bytes.
    pub fn random(&self, output: &mut [u8]) -> Result<(), Error> {
        self.borrow().invoke(
            abi::method::entropy::GENERATE,
            [output.as_mut_ptr() as u64, output.len() as u64, 0, 0, 0],
        )?;
        Ok(())
    }
}
