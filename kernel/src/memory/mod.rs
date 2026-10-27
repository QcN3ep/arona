// SPDX-License-Identifier: MPL-2.0

mod address_space;
mod buffer;
mod direct_map;
mod frame;
mod frame_allocator;
mod heap_allocator;
mod mapping;
mod mmio;
mod paging;
mod region;
mod stack;

use core::alloc::Layout;
use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::{Mutex, Once};

use crate::hardware::mmu::MmuError;
use direct_map::DirectMap;
use frame_allocator::FrameAllocator;
use heap_allocator::HeapAllocator;

pub(crate) use buffer::{MemoryBuffer, UserMemoryError};

pub use address_space::{AddressSpace, USER_ADDRESS_RANGE};
pub use frame::{
    Frame, FrameAccess, FrameAccessReference, FrameAllocation, FrameAllocationInfo,
    FrameAllocationReference, FrameOwner, FramePool, FramePurpose, Frames, ReferenceKind,
    ReservedRangeReference,
};
pub use frame_allocator::{MemoryStats, PoolStats};
pub use heap_allocator::{HeapAllocation, HeapArc, HeapBox, HeapStats};
pub use mapping::{MappingInfo, MappingOptions, MappingRights};
pub use mmio::DeviceMapping;
pub use region::{MAX_MEMORY_REGIONS, MemoryRegion, MemoryRegionState, PhysRange, VirtRange};
pub use stack::KernelStack;

pub const MAX_USER_MAPPING_RECORDS: usize = 65536;

static MEMORY_MANAGER: MemoryManager = MemoryManager {
    allocator: Once::new(),
    kernel_space: Once::new(),
    mapping_records: AtomicUsize::new(0),
    direct_map: DirectMap::new(),
    heap_allocator: HeapAllocator::new(),
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryError {
    AccessConflict,
    AlreadyInitialized,
    AlreadyMapped,
    NotMapped,
    NotInitialized,
    InvalidFrame,
    InvalidRequest,
    InUse,
    NoMemory,
    ReferenceOverflow,
    Mmu(MmuError),
}

impl From<MmuError> for MemoryError {
    fn from(error: MmuError) -> Self {
        Self::Mmu(error)
    }
}

/// # Safety
///
/// 操作及句柄析构期间须关闭本地中断并保留物理访问窗口。
/// 启用 direct map 后，当前页表须保留共享内核映射。
/// 修改映射前须停止其他处理器访问对应范围并清除其 TLB 项。
pub struct MemoryManager {
    allocator: Once<Mutex<FrameAllocator>>,
    kernel_space: Once<AddressSpace>,
    mapping_records: AtomicUsize,
    direct_map: DirectMap,
    heap_allocator: HeapAllocator,
}

struct MappingQuota(usize);

impl MemoryManager {
    fn allocator(&self) -> &Mutex<FrameAllocator> {
        self.allocator.get().expect("memory is not initialized")
    }

    /// # Safety
    ///
    /// 初始化期间须停止其他处理器使用内存管理器。
    pub fn initialize_direct_map(&self) -> Result<(), MemoryError> {
        let space = self.kernel_space.get().ok_or(MemoryError::NotInitialized)?;
        self.direct_map.initialize(space)
    }

    pub fn direct_map_range(&self) -> Option<VirtRange> {
        self.direct_map.range()
    }

    pub fn initialize_kernel_address_space(&self) -> Result<(), MemoryError> {
        let mut initialized = false;
        self.kernel_space.try_call_once(|| {
            let space = AddressSpace::kernel()?;
            initialized = true;
            Ok::<_, MemoryError>(space)
        })?;
        if initialized {
            Ok(())
        } else {
            Err(MemoryError::AlreadyInitialized)
        }
    }

    pub fn kernel_address_space(&self) -> &AddressSpace {
        self.kernel_space
            .get()
            .expect("kernel address space is not initialized")
    }

    fn reserve_mapping_records(&self, count: usize) -> Result<MappingQuota, MemoryError> {
        self.mapping_records
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current
                    .checked_add(count)
                    .filter(|total| *total <= MAX_USER_MAPPING_RECORDS)
            })
            .map_err(|_| MemoryError::NoMemory)?;
        Ok(MappingQuota(count))
    }

    fn release_mapping_records(&self, count: usize) {
        self.mapping_records.fetch_sub(count, Ordering::Relaxed);
    }

    pub fn stats(&self) -> MemoryStats {
        self.allocator().lock().stats()
    }

    pub fn allocate_heap(&self, layout: Layout) -> Result<HeapAllocation, MemoryError> {
        self.heap_allocator.allocate(layout)
    }

    pub fn allocate_box<T>(&self, value: T) -> Result<HeapBox<T>, MemoryError> {
        HeapBox::try_new(value).map_err(|_| MemoryError::NoMemory)
    }

    pub fn allocate_arc<T>(&self, value: T) -> Result<HeapArc<T>, MemoryError> {
        HeapArc::try_new(value).map_err(|_| MemoryError::NoMemory)
    }

    pub fn allocate_pinned<T>(&self, value: T) -> Result<Pin<HeapBox<T>>, MemoryError> {
        self.allocate_box(value).map(HeapBox::into_pin)
    }

    pub fn heap_stats(&self) -> HeapStats {
        self.heap_allocator.stats()
    }

    /// 回收堆缓存，返回归还的页框数。
    pub fn reclaim_heap(&self) -> usize {
        self.heap_allocator.reclaim()
    }

    pub fn allocate(
        &self,
        owner: FrameOwner,
        purpose: FramePurpose,
        frames: u64,
    ) -> Result<FrameAllocation, MemoryError> {
        let head = self.allocator().lock().allocate(owner, purpose, frames)?;
        let allocation = FrameAllocation { head };
        for frame in allocation.frames() {
            crate::hardware::mmu().zero_frame(frame.address())?;
        }
        Ok(allocation)
    }

    fn reference(
        &self,
        head: u64,
        kind: ReferenceKind,
    ) -> Result<FrameAllocationReference, MemoryError> {
        self.allocator().lock().reference(head, kind)?;
        Ok(FrameAllocationReference { head, kind })
    }

    pub fn borrow_reserved(&self, range: PhysRange) -> Result<ReservedRangeReference, MemoryError> {
        self.allocator().lock().borrow_reserved(range)?;
        Ok(ReservedRangeReference { range })
    }

    /// # Safety
    ///
    /// 保留页须已脱离管理器之外的代码与硬件访问。
    pub fn reclaim_reserved(&self, range: PhysRange, pool: FramePool) -> Result<(), MemoryError> {
        self.allocator().lock().reclaim_reserved(range, pool)
    }

    fn info(&self, head: u64) -> FrameAllocationInfo {
        self.allocator().lock().info(head)
    }

    fn contains(&self, head: u64, frame: Frame) -> bool {
        self.allocator().lock().allocation_of(frame) == Ok(head)
    }

    fn is_managed(&self, address: u64) -> bool {
        self.allocator().lock().is_managed(address)
    }

    fn reserve_access(
        &self,
        allocator: &mut FrameAllocator,
        head: u64,
        frame: Frame,
        kind: ReferenceKind,
        access: FrameAccess,
    ) -> Result<bool, MemoryError> {
        allocator.reserve_access(head, frame, kind, access)?;
        if access == FrameAccess::Read {
            return Ok(false);
        }
        match self
            .direct_map
            .set_writable(frame, access == FrameAccess::Write)
        {
            Ok(previous) => Ok(previous),
            Err(error) => {
                allocator.release_access(frame, kind, access);
                Err(error)
            }
        }
    }

    fn restore_direct_access(&self, allocator: &FrameAllocator, frame: Frame, writable: bool) {
        let writable =
            !allocator.has_executors(frame) && (writable || allocator.has_writers(frame));
        self.direct_map
            .set_writable(frame, writable)
            .expect("failed to restore direct map permissions");
    }

    fn release_mapping(&self, frame: Frame, access: FrameAccess) -> Result<(), MemoryError> {
        let mut allocator = self.allocator().lock();
        let head = allocator.allocation_of(frame)?;
        allocator.release_access(frame, ReferenceKind::Mapping, access);
        allocator.release_reference(head, ReferenceKind::Mapping);
        Ok(())
    }

    fn frame(&self, index: u64) -> (u64, u64) {
        self.allocator().lock().frame(index)
    }

    fn release_owner(&self, head: u64) {
        self.allocator().lock().release_owner(head);
    }

    fn release_reference(&self, head: u64, kind: ReferenceKind) {
        self.allocator().lock().release_reference(head, kind);
    }

    fn release_reserved_reference(&self, range: PhysRange) {
        self.allocator().lock().release_reserved_reference(range);
    }
}

pub fn manager() -> &'static MemoryManager {
    try_manager().expect("memory is not initialized")
}

pub fn try_manager() -> Option<&'static MemoryManager> {
    MEMORY_MANAGER.allocator.get().map(|_| &MEMORY_MANAGER)
}

/// # Safety
///
/// 本地中断须关闭，物理访问窗口须可用，交接 RAM 须已脱离外部使用。
pub fn initialize(regions: &[MemoryRegion]) -> Result<(), MemoryError> {
    let mut initialized = false;
    MEMORY_MANAGER
        .allocator
        .try_call_once(|| -> Result<_, MemoryError> {
            let allocator = FrameAllocator::new(regions)?;
            MEMORY_MANAGER.direct_map.set_regions(regions);
            initialized = true;
            Ok(Mutex::new(allocator))
        })?;
    if initialized {
        Ok(())
    } else {
        Err(MemoryError::AlreadyInitialized)
    }
}

impl Drop for MappingQuota {
    fn drop(&mut self) {
        manager().release_mapping_records(self.0);
    }
}
