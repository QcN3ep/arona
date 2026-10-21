// SPDX-License-Identifier: MPL-2.0

use arrayvec::ArrayVec;
use spin::Mutex;

use super::{
    Frame, FrameAccess, FrameAllocation, FrameAllocationReference, FrameOwner, FramePurpose,
    MemoryError, ReferenceKind, manager,
};
use crate::hardware::{
    self,
    mmu::{
        MemoryType, MmuError, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions,
        PageTableEntry, PageTableLayout,
    },
};

const MAX_LEVELS: usize = 4;

/// # Safety
///
/// 操作及析构期间须满足 [`Mmu`](crate::hardware::mmu::Mmu) 的执行环境要求。
/// 修改前须停止其他处理器使用该页表并清除其 TLB 项，析构前须让所有处理器切离该页表。
pub struct PageTable {
    state: Mutex<PageTableState>,
}

struct PageTableState {
    root: Frame,
    backing: FrameAllocation,
}

struct PageWalk {
    tables: [Option<Frame>; MAX_LEVELS],
    level: usize,
    entry: PageTableEntry,
}

impl PageTable {
    pub fn new() -> Result<Self, MemoryError> {
        let layout = hardware::mmu().layout();
        if !(2..=MAX_LEVELS).contains(&layout.levels)
            || !(1..=9).contains(&layout.index_bits)
            || layout.window.level == 0
            || layout.window.level >= layout.levels
            || PAGE_SIZE.trailing_zeros() + layout.index_bits * layout.levels as u32 >= usize::BITS
        {
            return Err(MmuError::Unsupported.into());
        }
        let backing = manager().allocate(FrameOwner::Kernel, FramePurpose::PageTable, 1)?;
        let root = backing.frames().next().expect("empty root allocation");
        let mut state = PageTableState { root, backing };
        state.install_window(layout)?;
        Ok(Self {
            state: Mutex::new(state),
        })
    }

    pub fn root(&self) -> Frame {
        self.state.lock().root
    }

    pub fn map(
        &self,
        virtual_address: usize,
        allocation: &FrameAllocation,
        frame: Frame,
        attributes: PageAttributes,
    ) -> Result<(), MemoryError> {
        self.map_reference(
            virtual_address,
            allocation.reference(ReferenceKind::Mapping)?,
            frame,
            attributes,
        )
    }

    pub fn map_reference(
        &self,
        virtual_address: usize,
        reference: FrameAllocationReference,
        frame: Frame,
        attributes: PageAttributes,
    ) -> Result<(), MemoryError> {
        let info = reference.info();
        if info.purpose == FramePurpose::PageTable
            || attributes.memory_type != MemoryType::Normal
            || (attributes.user_accessible && info.owner == FrameOwner::Kernel)
            || !reference.contains(frame)
        {
            return Err(MemoryError::InvalidFrame);
        }
        let reference = if reference.kind == ReferenceKind::Mapping {
            reference
        } else {
            reference.reference(ReferenceKind::Mapping)?
        };
        let reference = reference.access(frame, attributes.permissions.into())?;
        let mut state = self.state.lock();
        state.map(
            virtual_address,
            PageMapping {
                physical_address: frame.address(),
                attributes,
            },
        )?;
        // 引用由已发布的页表项持有，撤销时通过页框记录释放。
        core::mem::forget(reference);
        Ok(())
    }

    pub fn unmap(&self, virtual_address: usize) -> Result<PageMapping, MemoryError> {
        validate_address(virtual_address, true)?;
        self.state.lock().unmap(virtual_address)
    }

    pub fn protect(
        &self,
        virtual_address: usize,
        permissions: PagePermissions,
    ) -> Result<(), MemoryError> {
        validate_address(virtual_address, true)?;
        self.state.lock().protect(virtual_address, permissions)
    }

    pub fn query(&self, address: usize) -> Result<Option<PageMapping>, MemoryError> {
        validate_address(address, false)?;
        match self.state.lock().walk(address)?.entry {
            PageTableEntry::Empty => Ok(None),
            PageTableEntry::Mapping(mapping) => Ok(Some(mapping)),
            PageTableEntry::Table(_) => Err(MmuError::InvalidTable.into()),
        }
    }

    /// # Safety
    ///
    /// 页表存续期间，后备存储须保持有效且不得被回收。
    pub(crate) fn map_permanent(
        &self,
        virtual_address: usize,
        mapping: PageMapping,
    ) -> Result<(), MemoryError> {
        if mapping.attributes.user_accessible || manager().is_managed(mapping.physical_address) {
            return Err(MmuError::InvalidAttributes.into());
        }
        self.state.lock().map(virtual_address, mapping)
    }
}

impl PageTableState {
    fn install_window(&mut self, layout: PageTableLayout) -> Result<(), MemoryError> {
        let mut table = self.root;
        for level in ((layout.window.level + 1)..layout.levels).rev() {
            let next = manager().allocate(FrameOwner::Kernel, FramePurpose::PageTable, 1)?;
            let frame = next.frames().next().expect("empty table allocation");
            hardware::mmu().write_entry(
                table,
                index(layout, layout.window.virtual_address, level),
                level,
                PageTableEntry::Table(frame),
            )?;
            self.backing.merge(next);
            table = frame;
        }
        hardware::mmu().write_entry(
            table,
            index(layout, layout.window.virtual_address, layout.window.level),
            layout.window.level,
            PageTableEntry::Table(layout.window.table),
        )?;
        Ok(())
    }

    fn walk(&self, address: usize) -> Result<PageWalk, MemoryError> {
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        let mut walk = PageWalk {
            tables: [None; MAX_LEVELS],
            level: layout.levels - 1,
            entry: PageTableEntry::Empty,
        };
        let mut table = self.root;
        loop {
            walk.tables[walk.level] = Some(table);
            walk.entry = mmu.read_entry(table, index(layout, address, walk.level), walk.level)?;
            match walk.entry {
                PageTableEntry::Table(next) if walk.level > 0 => {
                    table = next;
                    walk.level -= 1;
                }
                _ => return Ok(walk),
            }
        }
    }

    fn map(&mut self, address: usize, mapping: PageMapping) -> Result<(), MemoryError> {
        validate_address(address, true)?;
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        let walk = self.walk(address)?;
        if walk.entry != PageTableEntry::Empty {
            return Err(MemoryError::AlreadyMapped);
        }
        let mut entry = PageTableEntry::Mapping(mapping);
        let branch = if walk.level == 0 {
            None
        } else {
            let allocation = manager().allocate(
                FrameOwner::Kernel,
                FramePurpose::PageTable,
                walk.level as u64,
            )?;
            let frames: ArrayVec<Frame, MAX_LEVELS> = allocation.frames().collect();
            for (level, table) in frames.into_iter().enumerate() {
                mmu.write_entry(table, index(layout, address, level), level, entry)?;
                entry = PageTableEntry::Table(table);
            }
            Some(allocation)
        };
        mmu.write_entry(
            walk.tables[walk.level].expect("missing parent table"),
            index(layout, address, walk.level),
            walk.level,
            entry,
        )?;
        if let Some(branch) = branch {
            self.backing.merge(branch);
        }
        mmu.invalidate_all();
        Ok(())
    }

    fn unmap(&mut self, address: usize) -> Result<PageMapping, MemoryError> {
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        let walk = self.walk(address)?;
        let PageTableEntry::Mapping(mapping) = walk.entry else {
            return Err(MemoryError::NotMapped);
        };
        let frame = Frame::new(mapping.physical_address).ok_or(MemoryError::InvalidFrame)?;
        if !manager().is_managed(frame.address()) {
            return Err(MemoryError::InvalidFrame);
        }
        let mut detach = 0;
        while detach + 1 < layout.levels {
            let table = walk.tables[detach].expect("missing child table");
            let removed = index(layout, address, detach);
            let mut occupied = false;
            for slot in 0..(1 << layout.index_bits) {
                if slot != removed && mmu.read_entry(table, slot, detach)? != PageTableEntry::Empty
                {
                    occupied = true;
                    break;
                }
            }
            if occupied {
                break;
            }
            detach += 1;
        }
        mmu.write_entry(
            walk.tables[detach].expect("missing parent table"),
            index(layout, address, detach),
            detach,
            PageTableEntry::Empty,
        )?;
        mmu.invalidate_all();
        for frame in walk.tables[..detach].iter().flatten() {
            drop(self.backing.detach(*frame));
        }
        manager()
            .release_mapping(frame, mapping.attributes.permissions.into())
            .expect("lost mapping allocation");
        Ok(mapping)
    }

    fn protect(&mut self, address: usize, permissions: PagePermissions) -> Result<(), MemoryError> {
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        let walk = self.walk(address)?;
        let PageTableEntry::Mapping(mut mapping) = walk.entry else {
            return Err(MemoryError::NotMapped);
        };
        if !manager().is_managed(mapping.physical_address) {
            return Err(MemoryError::InvalidFrame);
        }
        let frame = Frame::new(mapping.physical_address).ok_or(MemoryError::InvalidFrame)?;
        let old = mapping.attributes.permissions.into();
        let new = permissions.into();
        let mut allocator = manager().allocator.lock();
        allocator.replace_mapping_access(frame, old, new)?;
        let result = (|| {
            if new == FrameAccess::Execute && old != new {
                mmu.synchronize_frame(frame)?;
            }
            mapping.attributes.permissions = permissions;
            mmu.write_entry(
                walk.tables[0].expect("missing leaf table"),
                index(layout, address, 0),
                0,
                PageTableEntry::Mapping(mapping),
            )?;
            mmu.invalidate_all();
            Ok(())
        })();
        if result.is_err() {
            allocator
                .replace_mapping_access(frame, new, old)
                .expect("failed to restore frame access");
        }
        result
    }

    fn release_mappings(&self, table: Frame, level: usize) -> Result<(), MemoryError> {
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        for slot in 0..(1 << layout.index_bits) {
            match mmu.read_entry(table, slot, level)? {
                PageTableEntry::Empty => {}
                PageTableEntry::Table(next) if next == layout.window.table => {}
                PageTableEntry::Table(next) => self.release_mappings(next, level - 1)?,
                PageTableEntry::Mapping(mapping) => {
                    let frame =
                        Frame::new(mapping.physical_address).ok_or(MemoryError::InvalidFrame)?;
                    if manager().is_managed(frame.address()) {
                        manager().release_mapping(frame, mapping.attributes.permissions.into())?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn index(layout: PageTableLayout, address: usize, level: usize) -> usize {
    (address >> (PAGE_SIZE.trailing_zeros() + layout.index_bits * level as u32))
        & ((1 << layout.index_bits) - 1)
}

fn validate_address(address: usize, aligned: bool) -> Result<(), MemoryError> {
    let mmu = hardware::mmu();
    let layout = mmu.layout();
    let window_bits = PAGE_SIZE.trailing_zeros() + layout.index_bits * layout.window.level as u32;
    let mask = !((1usize << window_bits) - 1);
    if !mmu.is_valid_virtual_address(address)
        || address & mask == layout.window.virtual_address & mask
    {
        return Err(MmuError::InvalidAddress.into());
    }
    if aligned && !address.is_multiple_of(PAGE_SIZE) {
        return Err(MmuError::InvalidAlignment.into());
    }
    Ok(())
}

impl Drop for PageTableState {
    fn drop(&mut self) {
        let mmu = hardware::mmu();
        mmu.invalidate_all();
        self.release_mappings(self.root, mmu.layout().levels - 1)
            .expect("failed to release page table mappings");
    }
}
