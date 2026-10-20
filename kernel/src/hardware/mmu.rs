// SPDX-License-Identifier: MPL-2.0

//! 内存管理单元的原始能力接口。

use core::ops::Range;

use super::{BindError, Slot};

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
    AlreadyMapped,
    NotMapped,
    NoMemory,
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
    /// 不可缓存且不可执行的设备内存。
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

/// 持有一个根及其下级页表的后备存储，仅借出调用方预先准备的页表页。
/// 根和已借出的页须持续可访问，table 与 table_mut 对其他地址返回 InvalidTable。
///
/// # Safety
///
/// 页表页须按页对齐且固定，物理地址须与借用内容对应；所有页须在页表及硬件引用存续期间保持独占有效。
pub trait PageTableStorage {
    fn root_address(&self) -> u64;

    fn table(&self, physical_address: u64) -> Result<&[u8; PAGE_SIZE], MmuError>;

    fn table_mut(&mut self, physical_address: u64) -> Result<&mut [u8; PAGE_SIZE], MmuError>;

    /// 借出一个清零且未使用的页表页；没有备用页时返回 NoMemory。
    fn take_table(&mut self) -> Result<u64, MmuError>;

    /// # Safety
    ///
    /// 地址须由 take_table 发放，且该页已脱离页表树和硬件引用。
    fn return_table(&mut self, physical_address: u64);
}

/// 操作 4 KiB 分页，不调用全局分配器或阻塞，修改成功返回前完成本地 TLB 同步。
/// 不支持的地址或属性须返回错误。
///
/// # Safety
///
/// 修改页表或切换根时须关闭本地中断并独占页表；目标物理页须已获授权、在映射存续期间有效且满足跨映射权限约束。
/// 修改不得破坏当前代码、栈和页表访问仍依赖的映射。
pub trait Mmu: Sync {
    /// 判断地址是否符合目标 ISA 的虚拟地址格式。
    fn is_valid_virtual_address(&self, address: usize) -> bool;

    /// 初始化根页，不借出下级页表；失败时保留原存储。
    ///
    /// # Safety
    ///
    /// 根须尚未启用，且不含需要保留的映射或下级页表。
    fn initialize(&self, tables: &mut dyn PageTableStorage) -> Result<(), MmuError>;

    /// 映射一个未映射页；虚拟和物理地址须按页对齐；失败时撤销本次修改并归还借出的页表页。
    ///
    /// # Safety
    ///
    /// 新增执行映射前须关闭该物理页的全部写入路径，并完成代码同步。
    fn map(
        &self,
        tables: &mut dyn PageTableStorage,
        virtual_address: usize,
        mapping: PageMapping,
    ) -> Result<(), MmuError>;

    /// 撤销一个页对齐的映射并返回原描述，不释放目标物理页；失败时保留原映射。
    fn unmap(
        &self,
        tables: &mut dyn PageTableStorage,
        virtual_address: usize,
    ) -> Result<PageMapping, MmuError>;

    /// 修改一个页对齐映射的权限，不借出页表页或改变其他属性；失败时保留原权限。
    ///
    /// # Safety
    ///
    /// 权限不得超过原授权，新增执行权限前须关闭全部写入路径并完成代码同步。
    fn protect(
        &self,
        tables: &mut dyn PageTableStorage,
        virtual_address: usize,
        permissions: PagePermissions,
    ) -> Result<(), MmuError>;

    /// 查询 address 所在页，返回的物理地址为页首地址；未映射时返回 None。
    fn query(
        &self,
        tables: &dyn PageTableStorage,
        address: usize,
    ) -> Result<Option<PageMapping>, MmuError>;

    /// 切换页表根并清除本地旧翻译；失败时保留原根。
    ///
    /// # Safety
    ///
    /// 新页表须保留当前代码、栈和仍在使用的数据映射；存储及其后备页须在下次切换前保持有效。
    fn switch_root(&self, tables: &dyn PageTableStorage) -> Result<(), MmuError>;

    /// 使包含 address 的页在本地 TLB 失效，包括该地址的全局项。
    fn invalidate_page(&self, address: usize) -> Result<(), MmuError>;

    /// 清除本地全部地址空间的 TLB 翻译，包括全局项。
    fn invalidate_all(&self);

    /// 使 range 内已完成的数据写入对后续取指可见；空范围无操作，反向范围返回 InvalidRange。
    ///
    /// # Safety
    ///
    /// range 须对应保持有效的普通内存映射，且同步期间不得继续写入该范围。
    fn synchronize_code(&self, range: Range<usize>) -> Result<(), MmuError>;
}
