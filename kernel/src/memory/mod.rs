// SPDX-License-Identifier: MPL-2.0

mod address_space;
mod frame;
mod frame_allocator;
mod mapping;
mod paging;
mod region;

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::{Mutex, Once};

use crate::hardware::mmu::MmuError;
use frame_allocator::FrameAllocator;

pub use address_space::{AddressSpace, USER_ADDRESS_RANGE};
pub use frame::{
    Frame, FrameAccess, FrameAccessReference, FrameAllocation, FrameAllocationInfo,
    FrameAllocationReference, FrameOwner, FramePool, FramePurpose, Frames, ReferenceKind,
    ReservedRangeReference,
};
pub use frame_allocator::{MemoryStats, PoolStats};
pub use mapping::{MappingInfo, MappingOptions, MappingRights};
pub use region::{MAX_MEMORY_REGIONS, MemoryRegion, MemoryRegionState, PhysRange, VirtRange};

pub const MAX_USER_MAPPING_RECORDS: usize = 65536;

static MEMORY_MANAGER: Once<MemoryManager> = Once::new();

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
pub struct MemoryManager {
    allocator: Mutex<FrameAllocator>,
    kernel_space: Once<AddressSpace>,
    mapping_records: AtomicUsize,
}

struct MappingQuota(usize);

impl MemoryManager {
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
        self.allocator.lock().stats()
    }

    pub fn allocate(
        &self,
        owner: FrameOwner,
        purpose: FramePurpose,
        frames: u64,
    ) -> Result<FrameAllocation, MemoryError> {
        let head = self.allocator.lock().allocate(owner, purpose, frames)?;
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
        self.allocator.lock().reference(head, kind)?;
        Ok(FrameAllocationReference { head, kind })
    }

    pub fn borrow_reserved(&self, range: PhysRange) -> Result<ReservedRangeReference, MemoryError> {
        self.allocator.lock().borrow_reserved(range)?;
        Ok(ReservedRangeReference { range })
    }

    /// # Safety
    ///
    /// 保留页须已脱离管理器之外的代码与硬件访问。
    pub fn reclaim_reserved(&self, range: PhysRange, pool: FramePool) -> Result<(), MemoryError> {
        self.allocator.lock().reclaim_reserved(range, pool)
    }

    fn info(&self, head: u64) -> FrameAllocationInfo {
        self.allocator.lock().info(head)
    }

    fn contains(&self, head: u64, frame: Frame) -> bool {
        self.allocator.lock().allocation_of(frame) == Ok(head)
    }

    fn is_managed(&self, address: u64) -> bool {
        self.allocator.lock().is_managed(address)
    }

    fn release_mapping(&self, frame: Frame, access: FrameAccess) -> Result<(), MemoryError> {
        let mut allocator = self.allocator.lock();
        let head = allocator.allocation_of(frame)?;
        allocator.release_access(frame, ReferenceKind::Mapping, access);
        allocator.release_reference(head, ReferenceKind::Mapping);
        Ok(())
    }

    fn frame(&self, index: u64) -> (u64, u64) {
        self.allocator.lock().frame(index)
    }

    fn release_owner(&self, head: u64) {
        self.allocator.lock().release_owner(head);
    }

    fn release_reference(&self, head: u64, kind: ReferenceKind) {
        self.allocator.lock().release_reference(head, kind);
    }

    fn release_reserved_reference(&self, range: PhysRange) {
        self.allocator.lock().release_reserved_reference(range);
    }
}

pub fn manager() -> &'static MemoryManager {
    MEMORY_MANAGER.get().expect("memory is not initialized")
}

/// # Safety
///
/// 本地中断须关闭，物理访问窗口须可用，交接 RAM 须已脱离外部使用。
pub fn initialize(regions: &[MemoryRegion]) -> Result<(), MemoryError> {
    let mut initialized = false;
    MEMORY_MANAGER.try_call_once(|| -> Result<_, MemoryError> {
        let allocator = FrameAllocator::new(regions)?;
        initialized = true;
        Ok(MemoryManager {
            allocator: Mutex::new(allocator),
            kernel_space: Once::new(),
            mapping_records: AtomicUsize::new(0),
        })
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
