// SPDX-License-Identifier: MPL-2.0

use super::{Frame, FrameAllocation, FrameOwner, FramePurpose, MemoryError, manager};
use crate::hardware::{
    self,
    mmu::{MmuError, PageTableFrames},
};

/// # Safety
///
/// 操作及析构期间须满足 [`Mmu`](crate::hardware::mmu::Mmu) 的执行环境要求。
/// 修改前须停止其他处理器使用该页表并清除其 TLB 项，析构前须让所有处理器切离该页表。
/// 发布执行映射前须关闭该物理页的全部写入路径并完成代码同步。
pub struct PageTable {
    root: Frame,
    backing: FrameAllocation,
}

impl PageTable {
    pub fn new() -> Result<Self, MemoryError> {
        let backing = manager().allocate(FrameOwner::Kernel, FramePurpose::PageTable, 1)?;
        let root = backing.frames().next().expect("empty root allocation");
        let mut table = Self { root, backing };
        hardware::mmu().initialize(&mut table)?;
        Ok(table)
    }

    pub fn root(&self) -> Frame {
        self.root
    }
}

impl PageTableFrames for PageTable {
    fn root_address(&self) -> u64 {
        self.root().address()
    }

    fn take_table(&mut self) -> Result<u64, MmuError> {
        let allocation = manager()
            .allocate(FrameOwner::Kernel, FramePurpose::PageTable, 1)
            .map_err(|error| match error {
                MemoryError::NoMemory => MmuError::NoMemory,
                MemoryError::Mmu(error) => error,
                _ => MmuError::InvalidTable,
            })?;
        let frame = allocation.frames().next().expect("empty table allocation");
        self.backing.merge(allocation);
        Ok(frame.address())
    }

    fn return_table(&mut self, physical_address: u64) {
        let frame = Frame::new(physical_address).expect("unaligned page table frame");
        drop(self.backing.detach(frame));
    }
}
