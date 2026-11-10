// SPDX-License-Identifier: MPL-2.0

//! Linux 进程与线程的标识空间。

use core::cell::Cell;

pub(crate) struct PidNamespace {
    next: Cell<usize>,
}

impl PidNamespace {
    pub fn new() -> Self {
        Self { next: Cell::new(1) }
    }

    pub fn allocate(&self) -> Option<usize> {
        let id = self.next.get();
        if id > i32::MAX as usize {
            return None;
        }
        self.next.set(id + 1);
        Some(id)
    }
}
