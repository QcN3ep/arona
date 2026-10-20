// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

use kernel::{HardwareInfo, MemoryRegionState, PhysRange};
use uefi_raw::{guid, table::system::SystemTable};

use crate::boot::{dtb::Dtb, prepare_boot_info};

static mut HARDWARE_INFO: HardwareInfo = HardwareInfo::new();

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
    static _kernel_start: u8;
    static _kernel_end: u8;
}

/// QEMU virt 上 LoongArch 64 直启入口。
///
/// # Safety
///
/// 只能由 QEMU 调用一次。
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "csrwr $zero, 0x4",
        "li.d $t0, 4",
        "csrxchg $zero, $t0, 0x0",
        "la.local $sp, _boot_stack_top",
        "bl {entry}",
        "b .",
        entry = sym _start_native,
    );
}

/// # Safety
///
/// 只能由 [`_start`] 在系统启动时调用一次。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start_native(_argc: usize, _argv: usize, system_table: usize) -> ! {
    clear_bss();

    // REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/loongarch/boot.c
    // SAFETY: system_table 须指向可读且不变的 SystemTable。
    let system = unsafe { &*(system_table as *const SystemTable) };
    assert_eq!(
        system.header.signature,
        SystemTable::SIGNATURE,
        "invalid QEMU system table signature"
    );
    // SAFETY: configuration_table 须覆盖 number_of_configuration_table_entries 个可读且不变的配置表项。
    let tables = unsafe {
        core::slice::from_raw_parts(
            system.configuration_table,
            system.number_of_configuration_table_entries,
        )
    };
    let mut matches = tables
        .iter()
        .filter(|entry| entry.vendor_guid == guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0"));
    let dtb = matches
        .next()
        .expect("QEMU configuration table is missing DTB")
        .vendor_table
        .cast::<u8>();
    assert!(
        matches.next().is_none(),
        "duplicate DTB configuration table"
    );

    // SAFETY: 可变借用存续期间须独占访问 HARDWARE_INFO。
    let hardware = unsafe { &mut *core::ptr::addr_of_mut!(HARDWARE_INFO) };
    // SAFETY: dtb 须指向解析期间可读且不变的完整 DTB。
    let tree =
        unsafe { Dtb::new(dtb) }.unwrap_or_else(|error| panic!("failed to read DTB: {error}"));
    let dtb_range = PhysRange {
        start: dtb as u64,
        end: (dtb as u64)
            .checked_add(tree.total_size() as u64)
            .expect("DTB address range overflow"),
    };
    tree.dump(hardware)
        .unwrap_or_else(|error| panic!("failed to parse DTB: {error}"));
    for range in [
        dtb_range,
        PhysRange {
            start: 0,
            end: 0x00100000,
        },
    ] {
        hardware
            .memory
            .reserve(range, MemoryRegionState::BootReserved)
            .unwrap_or_else(|error| panic!("failed to reserve firmware data: {error}"));
    }
    let cpu_id: usize;
    // REFERENCE: https://github.com/torvalds/linux/blob/v6.18/arch/loongarch/include/asm/loongarch.h
    unsafe {
        core::arch::asm!("csrrd {}, 0x20", out(reg) cpu_id, options(nomem, nostack));
    }
    let info = prepare_boot_info(
        (cpu_id & 0x1ff) as u64,
        PhysRange {
            start: core::ptr::addr_of!(_kernel_start) as u64,
            end: core::ptr::addr_of!(_kernel_end) as u64,
        },
        hardware,
    )
    .unwrap_or_else(|error| panic!("failed to prepare boot information: {error}"));

    // SAFETY: 启动信息须与实际硬件一致，处理器须处于内核运行所需的执行环境。
    kernel::boot_kernel(info)
}

fn clear_bss() {
    let mut cursor = core::ptr::addr_of!(_sbss) as usize;
    let end = core::ptr::addr_of!(_ebss) as usize;

    while cursor < end {
        // SAFETY: 需要保证该地址位于可写且 8 字节对齐的 BSS 中。
        unsafe {
            (cursor as *mut u64).write_volatile(0);
        }
        cursor += size_of::<u64>();
    }
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("idle 0", options(nomem, nostack));
        }
    }
}
