// SPDX-License-Identifier: MPL-2.0

//! 内核映像的装载范围与常驻分段布局。

use super::PhysRange;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelImage {
    pub physical: PhysRange,
    pub text: ImageSegment,
    pub rodata: ImageSegment,
    /// 包含 BSS、启动页表和启动栈的可写段。
    pub data: ImageSegment,
    pub boot_tables: PhysRange,
    pub boot_stack: PhysRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageSegment {
    pub physical: PhysRange,
    pub virtual_start: usize,
}
