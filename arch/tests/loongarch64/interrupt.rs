// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use arch::boot::dtb::Dtb;
use kernel::memory::PhysRange;
use uefi_raw::{
    guid,
    table::{configuration::ConfigurationTable, system::SystemTable},
};

#[repr(C)]
struct InitrdTable {
    base: u64,
    size: u64,
}

pub fn boot(system_table: usize, _unused: usize) -> (u64, Dtb<'static>, Option<PhysRange>) {
    const FIRMWARE: usize = 0x9000_0000_0000_0000;
    // SAFETY: The firmware must deliver a valid, immutable SystemTable and a valid, immutable ConfigurationTable array.
    let tables = unsafe {
        let system = &*((FIRMWARE | system_table) as *const SystemTable);
        core::slice::from_raw_parts(
            (FIRMWARE | system.configuration_table as usize) as *const ConfigurationTable,
            system.number_of_configuration_table_entries,
        )
    };
    let dtb = tables
        .iter()
        .find(|entry| entry.vendor_guid == guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0"))
        .unwrap()
        .vendor_table as usize;
    let program = tables
        .iter()
        .find(|entry| entry.vendor_guid == guid!("5568e427-68fc-4f3d-ac74-ca555231cc68"))
        .and_then(|entry| {
            // SAFETY: The configuration table must point to a readable and unchanging QEMU initrd description.
            let table =
                unsafe { &*((FIRMWARE | entry.vendor_table as usize) as *const InitrdTable) };
            (table.size != 0).then(|| PhysRange {
                start: table.base,
                end: table.base.checked_add(table.size).unwrap(),
            })
        });
    let cpu_id: usize;
    // SAFETY: The execution environment must permit reading CPUID.
    unsafe { core::arch::asm!("csrrd {}, 0x20", out(reg) cpu_id, options(nomem, nostack)) };
    // SAFETY: The firmware DTB must be fully readable within the firmware window and remain unchanged.
    (
        cpu_id as u64 & 0x1ff,
        unsafe { Dtb::new((FIRMWARE | dtb) as *const u8) }.unwrap(),
        program,
    )
}
