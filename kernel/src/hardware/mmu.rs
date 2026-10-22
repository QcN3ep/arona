// SPDX-License-Identifier: MPL-2.0

//! 页表条目访问、地址翻译切换与缓存同步的硬件能力。

use core::ops::Range;

use super::{BindError, Slot};
use crate::memory::Frame;

pub const PAGE_SIZE: usize = 4096;

static SLOT: Slot<&'static dyn Mmu> = Slot::new();

pub fn bind(mmu: &'static dyn Mmu) -> Result<(), BindError> {
    SLOT.bind(mmu)
}

pub fn mmu() -> &'static dyn Mmu {
    SLOT.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MmuError {
    InvalidAddress,
    InvalidAlignment,
    InvalidRange,
    InvalidAttributes,
    InvalidTable,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagePermissions {
    ReadOnly,
    ReadWrite,
    ReadExecute,
    ExecuteOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryType {
    Normal,
    Device,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageAttributes {
    pub permissions: PagePermissions,
    pub memory_type: MemoryType,
    pub user_accessible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageMapping {
    pub physical_address: u64,
    pub attributes: PageAttributes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageTableEntry {
    Empty,
    Table(Frame),
    Mapping(PageMapping),
}

/// arch 访问物理内存所需的固定页表分支，不归调用方所有。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableWindow {
    pub virtual_address: usize,
    pub table: Frame,
    pub level: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageTableLayout {
    pub levels: usize,
    pub index_bits: u32,
    pub window: TableWindow,
}

/// 页表层级从叶级的 0 开始，物理读写的 offset 以 u64 为单位。
///
/// # Safety
///
/// 当前处理器须允许执行分页与缓存维护指令，本地中断须关闭，当前页表须保留物理访问窗口。
/// 启用的页表及其后备页框须持续存活，内存访问不得与活动 Rust 引用冲突，修改不得破坏当前执行所依赖的映射。
pub trait Mmu: Sync {
    fn layout(&self) -> PageTableLayout;
    fn is_valid_virtual_address(&self, address: usize) -> bool;
    fn physical_address_bits(&self) -> u32;

    /// 设备寄存器的分页属性。
    fn device_memory_type(&self) -> MemoryType;

    fn read_entry(
        &self,
        table: Frame,
        index: usize,
        level: usize,
    ) -> Result<PageTableEntry, MmuError>;

    /// 以完整条目提交修改，不刷新 TLB；失败时保留原条目。
    fn write_entry(
        &self,
        table: Frame,
        index: usize,
        level: usize,
        entry: PageTableEntry,
    ) -> Result<(), MmuError>;

    /// 切换根页表并完成本地 TLB 同步；失败时保留原根。
    fn switch_root(&self, root: Frame) -> Result<(), MmuError>;

    fn read_frame(
        &self,
        physical_address: u64,
        offset: usize,
        words: &mut [u64],
    ) -> Result<(), MmuError>;

    fn write_frame(
        &self,
        physical_address: u64,
        offset: usize,
        words: &[u64],
    ) -> Result<(), MmuError>;

    fn zero_frame(&self, physical_address: u64) -> Result<(), MmuError>;

    /// # Safety
    ///
    /// 访问期间须保持完整的普通内存页可读。
    fn read_mapped_frame(
        &self,
        address: usize,
        offset: usize,
        words: &mut [u64],
    ) -> Result<(), MmuError>;

    /// # Safety
    ///
    /// 访问期间须保持完整的普通内存页可写。
    fn write_mapped_frame(
        &self,
        address: usize,
        offset: usize,
        words: &[u64],
    ) -> Result<(), MmuError>;
    /// 使该页的本地 TLB 项失效，包括全局项。
    fn invalidate_page(&self, address: usize) -> Result<(), MmuError>;
    /// 清除本地全部 TLB 项，包括全局项。
    fn invalidate_all(&self);
    /// # Safety
    ///
    /// 同步期间须保持普通内存映射有效，且不得写入该范围。
    fn synchronize_code(&self, range: Range<usize>) -> Result<(), MmuError>;
    /// 同步物理页框的此前写入与后续取指，不依赖目标虚拟映射。
    fn synchronize_frame(&self, frame: Frame) -> Result<(), MmuError>;
}
