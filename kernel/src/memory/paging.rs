// SPDX-License-Identifier: MPL-2.0

// TODO: 支持跨处理器 TLB 失效与完成确认。

use arrayvec::ArrayVec;
use spin::Mutex;

use super::{Frame, FrameAllocation, FrameOwner, FramePurpose, MemoryError, manager};
use crate::hardware::{
    self,
    mmu::{MmuError, PAGE_SIZE, PageMapping, PageTableEntry, PageTableLayout},
};

const MAX_LEVELS: usize = 4;

/// # Safety
///
/// 操作及析构期间须满足 [`Mmu`](crate::hardware::mmu::Mmu) 的执行环境要求。
/// 修改前须停止其他处理器使用该页表并清除其 TLB 项，析构前须让所有处理器切离该页表。
pub(super) struct PageTable {
    state: Mutex<PageTableState>,
}

pub(super) struct PageTableState {
    root: Frame,
    backing: FrameAllocation,
    shared_kernel: Option<&'static PageTable>,
}

pub(super) struct PageWalk {
    pub tables: [Option<Frame>; MAX_LEVELS],
    pub level: usize,
    pub entry: PageTableEntry,
}

impl PageTable {
    pub(super) fn kernel() -> Result<Self, MemoryError> {
        Self::create(None)
    }

    pub(super) fn user(kernel: &'static Self) -> Result<Self, MemoryError> {
        if kernel.is_user() {
            return Err(MemoryError::InvalidRequest);
        }
        Self::create(Some(kernel))
    }

    fn create(shared_kernel: Option<&'static Self>) -> Result<Self, MemoryError> {
        let layout = hardware::mmu().layout();
        if !(2..=MAX_LEVELS).contains(&layout.levels)
            || !(1..=9).contains(&layout.index_bits)
            || layout.window.level == 0
            || layout.window.level >= layout.levels
            || PAGE_SIZE.trailing_zeros() + layout.index_bits * layout.levels as u32 >= usize::BITS
        {
            return Err(MmuError::Unsupported.into());
        }
        let first_shared = 1 << (layout.index_bits - 1);
        let frames = (1 + if shared_kernel.is_none() {
            first_shared
        } else {
            0
        }) as u64;
        let backing = manager().allocate(FrameOwner::Kernel, FramePurpose::PageTable, frames)?;
        let root = backing.frames().next().expect("empty root allocation");
        let mut state = PageTableState {
            root,
            backing,
            shared_kernel,
        };
        if let Some(kernel) = shared_kernel {
            let kernel = kernel.state.lock();
            for slot in first_shared..2 * first_shared {
                let entry = hardware::mmu().read_entry(kernel.root, slot, layout.levels - 1)?;
                if !matches!(entry, PageTableEntry::Table(_)) {
                    return Err(MmuError::InvalidTable.into());
                }
                hardware::mmu().write_entry(root, slot, layout.levels - 1, entry)?;
            }
        } else {
            // 共享分支在用户根创建前全部建立，且不参与运行期剪枝。
            let mut branches = state.backing.frames().skip(1);
            for slot in first_shared..2 * first_shared {
                let frame = branches.next().expect("missing shared kernel branch");
                hardware::mmu().write_entry(
                    root,
                    slot,
                    layout.levels - 1,
                    PageTableEntry::Table(frame),
                )?;
            }
            state.install_window(layout)?;
        }
        Ok(Self {
            state: Mutex::new(state),
        })
    }

    pub(super) fn is_user(&self) -> bool {
        self.state.lock().shared_kernel.is_some()
    }

    pub(super) fn root(&self) -> Frame {
        self.state.lock().root
    }

    pub(super) fn edit<R>(&self, operation: impl FnOnce(&mut PageTableState) -> R) -> R {
        operation(&mut self.state.lock())
    }

    pub(super) fn query(&self, address: usize) -> Result<Option<PageMapping>, MemoryError> {
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
    pub(super) fn map_permanent(
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
            if let PageTableEntry::Table(next) = hardware::mmu().read_entry(
                table,
                index(layout, layout.window.virtual_address, level),
                level,
            )? {
                table = next;
                continue;
            }
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

    pub(super) fn walk(&self, address: usize) -> Result<PageWalk, MemoryError> {
        Self::walk_from(self.root, address)
    }

    pub(super) fn walk_from(root: Frame, address: usize) -> Result<PageWalk, MemoryError> {
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        let mut walk = PageWalk {
            tables: [None; MAX_LEVELS],
            level: layout.levels - 1,
            entry: PageTableEntry::Empty,
        };
        let mut table = root;
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
        let walk = self.prepare(address)?;
        if let Err(error) = Self::write_leaf(address, walk.tables[0].unwrap(), Some(mapping)) {
            self.prune_empty(address)
                .expect("failed to discard empty page tables");
            return Err(error);
        }
        hardware::mmu().invalidate_all();
        Ok(())
    }

    pub(super) fn prepare(&mut self, address: usize) -> Result<PageWalk, MemoryError> {
        validate_address(address, true)?;
        self.validate_edit(address)?;
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        let walk = self.walk(address)?;
        if walk.entry != PageTableEntry::Empty {
            return Err(MemoryError::AlreadyMapped);
        }
        if walk.level != 0 {
            let allocation = manager().allocate(
                FrameOwner::Kernel,
                FramePurpose::PageTable,
                walk.level as u64,
            )?;
            let frames: ArrayVec<Frame, MAX_LEVELS> = allocation.frames().collect();
            for level in 1..frames.len() {
                mmu.write_entry(
                    frames[level],
                    index(layout, address, level),
                    level,
                    PageTableEntry::Table(frames[level - 1]),
                )?;
            }
            mmu.write_entry(
                walk.tables[walk.level].expect("missing parent table"),
                index(layout, address, walk.level),
                walk.level,
                PageTableEntry::Table(*frames.last().unwrap()),
            )?;
            self.backing.merge(allocation);
        }
        self.walk(address)
    }

    pub(super) fn write_leaf(
        address: usize,
        table: Frame,
        mapping: Option<PageMapping>,
    ) -> Result<(), MemoryError> {
        hardware::mmu().write_entry(
            table,
            index(hardware::mmu().layout(), address, 0),
            0,
            mapping.map_or(PageTableEntry::Empty, PageTableEntry::Mapping),
        )?;
        Ok(())
    }

    fn validate_edit(&self, address: usize) -> Result<(), MemoryError> {
        if self.shared_kernel.is_some() && address > isize::MAX as usize {
            return Err(MemoryError::InvalidRequest);
        }
        Ok(())
    }

    fn owned_levels(&self, address: usize) -> usize {
        hardware::mmu().layout().levels
            - usize::from(self.shared_kernel.is_none() && address > isize::MAX as usize)
    }

    pub(super) fn detach_empty(
        &mut self,
        address: usize,
        tables: &[Option<Frame>; MAX_LEVELS],
        mut detached: impl FnMut(usize),
    ) -> Result<(), MemoryError> {
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        for level in 0..self.owned_levels(address) - 1 {
            let Some(table) = tables[level] else { continue };
            let parent = tables[level + 1].expect("missing parent table");
            let slot = index(layout, address, level + 1);
            if mmu.read_entry(parent, slot, level + 1)? != PageTableEntry::Table(table) {
                continue;
            }
            for slot in 0..1 << layout.index_bits {
                if mmu.read_entry(table, slot, level)? != PageTableEntry::Empty {
                    return Ok(());
                }
            }
            mmu.write_entry(parent, slot, level + 1, PageTableEntry::Empty)?;
            detached(level);
        }
        Ok(())
    }

    pub(super) fn restore_tables(
        address: usize,
        tables: &[Option<Frame>; MAX_LEVELS],
        detached: u64,
    ) {
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        for level in (0..layout.levels - 1).rev() {
            if detached & (1 << level) != 0 {
                mmu.write_entry(
                    tables[level + 1].unwrap(),
                    index(layout, address, level + 1),
                    level + 1,
                    PageTableEntry::Table(tables[level].unwrap()),
                )
                .expect("failed to restore page table branch");
            }
        }
    }

    pub(super) fn release_tables(&mut self, tables: &[Option<Frame>; MAX_LEVELS], detached: u64) {
        for (level, table) in tables.iter().enumerate() {
            if detached & (1 << level) != 0 {
                drop(self.backing.detach(table.unwrap()));
            }
        }
    }

    pub(super) fn prune_empty(&mut self, address: usize) -> Result<(), MemoryError> {
        let walk = self.walk(address)?;
        let mut detached = 0;
        let result = self.detach_empty(address, &walk.tables, |level| detached |= 1 << level);
        if result.is_err() {
            Self::restore_tables(address, &walk.tables, detached);
        }
        hardware::mmu().invalidate_all();
        if result.is_ok() {
            self.release_tables(&walk.tables, detached);
        }
        result
    }

    fn release_mappings(&self, table: Frame, level: usize) -> Result<(), MemoryError> {
        let mmu = hardware::mmu();
        let layout = mmu.layout();
        for slot in 0..(1 << layout.index_bits) {
            if level == layout.levels - 1
                && self.shared_kernel.is_some()
                && slot >= 1 << (layout.index_bits - 1)
            {
                continue;
            }
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

pub(super) fn validate_address(address: usize, aligned: bool) -> Result<(), MemoryError> {
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
