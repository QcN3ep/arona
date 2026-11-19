// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Explicit access to the common monotonic deadline timebase.

use crate::{Clock, Error, Handle};
#[cfg(all(feature = "program", target_os = "none"))]
use spin::Once;

#[cfg(all(feature = "program", target_os = "none"))]
pub(crate) static DEFAULT: Once<Handle<Clock>> = Once::new();

pub fn clock() -> Option<&'static Handle<Clock>> {
    #[cfg(all(feature = "program", target_os = "none"))]
    {
        DEFAULT.get()
    }
    #[cfg(not(all(feature = "program", target_os = "none")))]
    {
        None
    }
}

impl Handle<Clock> {
    pub fn now_ns(&self) -> Result<u64, Error> {
        Ok(self.borrow().query(abi::info::clock::MONOTONIC)?[0])
    }
}
