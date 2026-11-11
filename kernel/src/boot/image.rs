// SPDX-License-Identifier: MPL-2.0

//! Load ranges and resident segment layout of the kernel image.

use crate::memory::PhysRange;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelImage {
    pub physical: PhysRange,
    pub text: ImageSegment,
    pub rodata: ImageSegment,
    /// Writable segment containing the BSS, boot page tables, and boot stack.
    pub data: ImageSegment,
    pub boot_tables: PhysRange,
    pub boot_stack: PhysRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageSegment {
    pub physical: PhysRange,
    pub virtual_start: usize,
}
