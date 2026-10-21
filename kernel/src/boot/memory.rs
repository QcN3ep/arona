// SPDX-License-Identifier: MPL-2.0

use spin::Once;

use super::{BootInfo, ImageSegment};
use crate::{
    hardware::{
        self,
        mmu::{MemoryType, Mmu, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions},
    },
    memory::{self, MemoryError, PageTable},
};

static KERNEL_PAGE_TABLE: Once<PageTable> = Once::new();

pub(super) fn initialize(info: &BootInfo<'_>) -> Result<(), MemoryError> {
    memory::initialize(&info.hardware.memory.regions)?;
    let mmu = hardware::mmu();
    let mut table = PageTable::new()?;
    map_segment(
        mmu,
        &mut table,
        info.kernel_image.text,
        PagePermissions::ReadExecute,
    )?;
    map_segment(
        mmu,
        &mut table,
        info.kernel_image.rodata,
        PagePermissions::ReadOnly,
    )?;
    map_segment(
        mmu,
        &mut table,
        info.kernel_image.data,
        PagePermissions::ReadWrite,
    )?;
    if let Some(console) = info.early_console {
        mmu.map(
            &mut table,
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
    let table = KERNEL_PAGE_TABLE.call_once(|| table);
    mmu.switch_root(table)?;
    Ok(())
}

fn map_segment(
    mmu: &dyn Mmu,
    table: &mut PageTable,
    segment: ImageSegment,
    permissions: PagePermissions,
) -> Result<(), MemoryError> {
    let length = segment.physical.end - segment.physical.start;
    for offset in (0..length).step_by(PAGE_SIZE) {
        mmu.map(
            table,
            segment.virtual_start + offset as usize,
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
