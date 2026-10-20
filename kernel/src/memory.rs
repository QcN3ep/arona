// SPDX-License-Identifier: MPL-2.0

use arrayvec::ArrayVec;
use spin::Mutex;

use crate::{
    BootInfo,
    boot::{ImageSegment, MAX_MEMORY_REGIONS, MemoryInfo, MemoryRegionState},
    hardware::mmu::{
        self, MemoryType, Mmu, MmuError, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions,
        PageTableFrames,
    },
};

const PAGE_SIZE_U64: u64 = PAGE_SIZE as u64;

static MEMORY_MANAGER: Mutex<MemoryManager> = Mutex::new(MemoryManager::empty());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryError {
    AlreadyInitialized,
    InvalidFrame,
    NoMemory,
    NotInitialized,
    Mmu(MmuError),
}

impl From<MmuError> for MemoryError {
    fn from(error: MmuError) -> Self {
        Self::Mmu(error)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Frame {
    address: u64,
}

impl Frame {
    pub const fn physical_address(&self) -> u64 {
        self.address
    }
}

struct FrameRegion {
    start: u64,
    next: u64,
    end: u64,
}

struct MemoryManager {
    regions: ArrayVec<FrameRegion, MAX_MEMORY_REGIONS>,
    root: Option<Frame>,
    free: Option<Frame>,
    initialized: bool,
}

impl MemoryManager {
    const fn empty() -> Self {
        Self {
            regions: ArrayVec::new_const(),
            root: None,
            free: None,
            initialized: false,
        }
    }

    fn configure(&mut self, info: &MemoryInfo, mmu: &dyn Mmu) -> Result<(), MemoryError> {
        self.regions.clear();
        self.root = None;
        self.free = None;
        let physical_limit = 1u64 << mmu.physical_address_bits();
        for region in &info.regions {
            if region.state != MemoryRegionState::Available {
                continue;
            }
            let end = region.range.end.min(physical_limit);
            if region.range.start < end {
                self.regions.push(FrameRegion {
                    start: region.range.start,
                    next: region.range.start,
                    end,
                });
            }
        }
        if self.regions.is_empty() {
            return Err(MemoryError::NoMemory);
        }
        Ok(())
    }

    fn allocate_frame(&mut self, mmu: &dyn Mmu) -> Result<Frame, MemoryError> {
        if let Some(address) = self.free.as_ref().map(Frame::physical_address) {
            let frame = Frame { address };
            let next = mmu.read_frame_link(frame.address)?;
            let free = if next == 0 {
                None
            } else {
                Some(self.frame_at(next)?)
            };
            self.free = free;
            return Ok(frame);
        }

        let region = self
            .regions
            .iter_mut()
            .find(|region| region.next < region.end)
            .ok_or(MemoryError::NoMemory)?;
        let address = region.next;
        region.next += PAGE_SIZE_U64;
        Ok(Frame { address })
    }

    fn release_frame(&mut self, frame: Frame, mmu: &dyn Mmu) -> Result<(), MemoryError> {
        if !self.owns(frame.address) {
            return Err(MemoryError::InvalidFrame);
        }
        mmu.write_frame_link(
            frame.address,
            self.free.as_ref().map_or(0, Frame::physical_address),
        )?;
        self.free = Some(frame);
        Ok(())
    }

    fn frame_at(&self, address: u64) -> Result<Frame, MemoryError> {
        if !self.owns(address) {
            return Err(MemoryError::InvalidFrame);
        }
        Ok(Frame { address })
    }

    fn owns(&self, address: u64) -> bool {
        address.is_multiple_of(PAGE_SIZE_U64)
            && self
                .regions
                .iter()
                .any(|region| region.start <= address && address < region.end)
    }

    fn map_segment(
        &mut self,
        mmu: &dyn Mmu,
        segment: ImageSegment,
        permissions: PagePermissions,
    ) -> Result<(), MemoryError> {
        let length = segment.physical.end - segment.physical.start;
        for offset in (0..length).step_by(PAGE_SIZE) {
            let virtual_address = segment.virtual_start + offset as usize;
            mmu.map(
                self,
                virtual_address,
                PageMapping {
                    physical_address: segment.physical.start + offset,
                    attributes: PageAttributes {
                        permissions,
                        memory_type: MemoryType::Normal,
                        user_accessible: false,
                    },
                },
            )?;
        }
        Ok(())
    }
}

pub(crate) fn initialize(info: &BootInfo<'_>) -> Result<(), MemoryError> {
    let mmu = mmu::mmu();
    let mut manager = MEMORY_MANAGER.lock();
    if manager.initialized {
        return Err(MemoryError::AlreadyInitialized);
    }

    manager.configure(&info.hardware.memory, mmu)?;
    manager.root = Some(manager.allocate_frame(mmu)?);
    mmu.initialize(&mut *manager)?;
    manager.map_segment(mmu, info.kernel_image.text, PagePermissions::ReadExecute)?;
    manager.map_segment(mmu, info.kernel_image.rodata, PagePermissions::ReadOnly)?;
    manager.map_segment(mmu, info.kernel_image.data, PagePermissions::ReadWrite)?;
    if let Some(console) = info.early_console {
        mmu.map(
            &mut *manager,
            console.virtual_address,
            PageMapping {
                physical_address: console.physical_address,
                attributes: PageAttributes {
                    permissions: PagePermissions::ReadWrite,
                    memory_type: console.memory_type,
                    user_accessible: false,
                },
            },
        )?;
    }

    mmu.switch_root(&*manager)?;
    manager.initialized = true;
    Ok(())
}

pub fn allocate_frame() -> Result<Frame, MemoryError> {
    let mut manager = MEMORY_MANAGER.lock();
    if !manager.initialized {
        return Err(MemoryError::NotInitialized);
    }
    manager.allocate_frame(mmu::mmu())
}

/// # Safety
///
/// 该帧不得被活动映射或设备引用。
pub fn release_frame(frame: Frame) -> Result<(), MemoryError> {
    let mut manager = MEMORY_MANAGER.lock();
    if !manager.initialized {
        return Err(MemoryError::NotInitialized);
    }
    manager.release_frame(frame, mmu::mmu())
}

impl PageTableFrames for MemoryManager {
    fn root_address(&self) -> u64 {
        self.root
            .as_ref()
            .expect("page table root is not allocated")
            .physical_address()
    }

    fn take_table(&mut self) -> Result<u64, MmuError> {
        self.allocate_frame(mmu::mmu())
            .map(|frame| frame.physical_address())
            .map_err(|error| match error {
                MemoryError::NoMemory => MmuError::NoMemory,
                MemoryError::Mmu(error) => error,
                _ => MmuError::InvalidTable,
            })
    }

    fn return_table(&mut self, physical_address: u64) {
        let frame = self
            .frame_at(physical_address)
            .expect("page table returned a frame outside managed memory");
        self.release_frame(frame, mmu::mmu())
            .expect("failed to return page table frame");
    }
}
