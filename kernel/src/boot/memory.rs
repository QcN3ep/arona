// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{BootInfo, ImageSegment};
use crate::{
    hardware::mmu::{MemoryType, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions},
    memory::{self, AddressSpace, MemoryError},
};

pub(super) fn initialize(info: &BootInfo<'_>) -> Result<(), MemoryError> {
    memory::initialize(&info.hardware.memory.regions)?;
    memory::manager().initialize_kernel_address_space()?;
    let table = memory::manager().kernel_address_space();
    map_segment(table, info.kernel_image.text, PagePermissions::ReadExecute)?;
    map_segment(table, info.kernel_image.rodata, PagePermissions::ReadOnly)?;
    map_segment(table, info.kernel_image.data, PagePermissions::ReadWrite)?;
    if let Some(console) = info.early_console {
        table.map_permanent(
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
    memory::manager().initialize_direct_map()?;
    table.activate()?;
    Ok(())
}

fn map_segment(
    table: &AddressSpace,
    segment: ImageSegment,
    permissions: PagePermissions,
) -> Result<(), MemoryError> {
    let length = segment.physical.end - segment.physical.start;
    for offset in (0..length).step_by(PAGE_SIZE) {
        table.map_permanent(
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
