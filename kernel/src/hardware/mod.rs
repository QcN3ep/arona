// SPDX-License-Identifier: MPL-2.0

//! 硬件能力绑定槽与取用接口。

use spin::Once;

pub mod cpu;
pub mod execution;
pub mod mmu;
pub mod trap;
pub use cpu::cpu;
pub use execution::execution;
pub use mmu::mmu;
pub use trap::trap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindError {
    AlreadyBound,
}

pub struct Slot<T> {
    value: Once<T>,
}

impl<T: Copy> Slot<T> {
    pub const fn new() -> Self {
        Self { value: Once::new() }
    }

    pub fn bind(&self, value: T) -> Result<(), BindError> {
        let mut inserted = false;
        self.value.call_once(|| {
            inserted = true;
            value
        });
        if inserted {
            Ok(())
        } else {
            Err(BindError::AlreadyBound)
        }
    }

    pub fn get(&self) -> T {
        self.try_get()
            .expect("hardware capability used before binding")
    }

    pub fn try_get(&self) -> Option<T> {
        self.value.get().copied()
    }
}

impl<T: Copy> Default for Slot<T> {
    fn default() -> Self {
        Self::new()
    }
}
