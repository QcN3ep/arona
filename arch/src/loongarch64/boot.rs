// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

use kernel::hardware::cpu::Cpu;
use kernel::hardware::mmu::MemoryType;
use kernel::{
    BootConsoleMapping, HardwareInfo, log, logger,
    memory::{MemoryRegionState, PhysRange},
};
use uefi_raw::{
    Guid, guid,
    table::{configuration::ConfigurationTable, system::SystemTable},
};

use super::cpu::CPU;
use crate::boot::{console::EarlyConsole, dtb::Dtb, kernel_image, prepare_boot_info};
use crate::device::uart::Uart;

mod paging;

use paging::FIRMWARE_BASE;

#[repr(C)]
struct InitrdTable {
    base: u64,
    size: u64,
}

static mut HARDWARE_INFO: HardwareInfo = HardwareInfo::new();
// SAFETY: The address must correspond to the QEMU virt 16550 UART whose byte width and stride are both 1, and ordinary accesses must be exclusive to this instance.
pub(super) static EARLY_CONSOLE: EarlyConsole =
    EarlyConsole::new(unsafe { Uart::new(paging::UART_BASE + 0x1e0) });

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
}

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html

/// # Safety
///
/// Can only be started by firmware at a physical address following the ELF entry convention.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "csrwr $zero, 0x4",
        "csrwr $zero, 0x2",
        "li.w $t0, 4",
        "csrxchg $zero, $t0, 0x0",
        "la.local $sp, _boot_stack_phys_top",
        "move $tp, $zero",
        "move $a0, $a2",
        "b {enter}",
        enter = sym paging::enter,
    );
}

/// # Safety
///
/// Can only be entered after `_start` has set up the page tables and switched to the higher half.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start_virtual(_system_table: usize) -> ! {
    core::arch::naked_asm!(
        "csrwr $zero, 0x180",
        "la.local $sp, _boot_stack_top",
        "b _start_native",
    );
}

/// # Safety
///
/// Can only be called once by `_start` after establishing the higher-half mapping and stack during system boot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start_native(system_table: usize) -> ! {
    clear_bss();
    EARLY_CONSOLE.init();
    kernel::boot::register_early_console(&EARLY_CONSOLE).expect("failed to register early console");
    logger::init(log::LevelFilter::Debug).expect("failed to initialize logger");
    // REFERENCE: https://github.com/torvalds/linux/blob/v6.18/arch/loongarch/include/asm/loongarch.h
    let cpu_id: usize;
    // SAFETY: The execution environment must permit reading the CPUID register.
    unsafe {
        core::arch::asm!("csrrd {}, 0x20", out(reg) cpu_id, options(nomem, nostack));
    }
    let boot_cpu_id = (cpu_id & 0x1ff) as u64;
    log::info!(target: "arch::boot", "Booting kernel on CPU {boot_cpu_id}");

    // REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/loongarch/boot.c
    // SAFETY: `system_table` must point to a readable and unchanged SystemTable.
    let system = unsafe {
        &*(firmware_address(system_table, size_of::<SystemTable>()) as *const SystemTable)
    };
    assert_eq!(
        system.header.signature,
        SystemTable::SIGNATURE,
        "invalid QEMU system table signature"
    );
    let count = system.number_of_configuration_table_entries;
    let tables_address = firmware_address(
        system.configuration_table as usize,
        count
            .checked_mul(size_of::<ConfigurationTable>())
            .expect("configuration table size overflow"),
    );
    // SAFETY: `configuration_table` must cover `number_of_configuration_table_entries` readable and unchanged configuration table entries.
    let tables =
        unsafe { core::slice::from_raw_parts(tables_address as *const ConfigurationTable, count) };
    let dtb = configuration_table(tables, guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0"))
        .expect("QEMU configuration table is missing DTB");
    let external_initramfs = initramfs(tables);

    // SAFETY: HARDWARE_INFO must be exclusively accessible for the duration of the mutable borrow.
    let hardware = unsafe { &mut *core::ptr::addr_of_mut!(HARDWARE_INFO) };
    // SAFETY: `dtb` must point to a complete DTB that remains readable and unchanged during parsing.
    let tree = unsafe { Dtb::new(firmware_address(dtb, 1024 * 1024) as *const u8) }
        .unwrap_or_else(|error| panic!("failed to read DTB: {error}"));
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
    crate::hardware::bind();
    let image = kernel_image();
    let archive = crate::boot::initramfs::source(external_initramfs, image)
        .unwrap_or_else(|error| panic!("failed to locate initramfs: {error}"));
    let init = crate::boot::initramfs::prepare(archive, hardware, image, 258)
        .unwrap_or_else(|error| panic!("failed to parse init image: {error}"));
    let mut info = prepare_boot_info(boot_cpu_id, image, hardware)
        .unwrap_or_else(|error| panic!("failed to prepare boot information: {error}"));
    info.init = Some(&init);
    info.initramfs = Some(archive.data());
    info.early_console = Some(BootConsoleMapping {
        virtual_address: paging::UART_BASE,
        physical_address: paging::UART_PHYSICAL_BASE as u64,
        memory_type: MemoryType::Device,
    });

    log::debug!(target: "arch::boot", "Parsed DTB at {dtb:#x}");
    // SAFETY: Boot information must match the actual hardware, and the processor must be in the execution environment required to run the kernel.
    kernel::boot_kernel(info)
}

fn configuration_table(tables: &[ConfigurationTable], guid: Guid) -> Option<usize> {
    let mut matches = tables.iter().filter(|entry| entry.vendor_guid == guid);
    let address = matches.next()?.vendor_table as usize;
    assert!(
        matches.next().is_none(),
        "duplicate firmware configuration table"
    );
    Some(address)
}

fn initramfs(tables: &[ConfigurationTable]) -> Option<PhysRange> {
    let address = configuration_table(tables, guid!("5568e427-68fc-4f3d-ac74-ca555231cc68"))?;
    // REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/include/hw/loongarch/boot.h
    // SAFETY: The configuration table must point to a readable and unchanged QEMU initrd description.
    let table =
        unsafe { &*(firmware_address(address, size_of::<InitrdTable>()) as *const InitrdTable) };
    if table.base == 0 && table.size == 0 {
        return None;
    }
    let end = table
        .base
        .checked_add(table.size)
        .filter(|&end| table.base < end)
        .expect("invalid initramfs range");
    Some(PhysRange {
        start: table.base,
        end,
    })
}

fn clear_bss() {
    let mut cursor = core::ptr::addr_of!(_sbss) as usize;
    let end = core::ptr::addr_of!(_ebss) as usize;

    while cursor < end {
        // SAFETY: Must ensure this address lies in writable, 8-byte-aligned BSS.
        unsafe {
            (cursor as *mut u64).write_volatile(0);
        }
        cursor += size_of::<u64>();
    }
}

fn firmware_address(physical: usize, size: usize) -> usize {
    assert!(
        physical.checked_add(size).is_some_and(|end| end <= 1 << 48),
        "firmware data exceeds physical address space"
    );
    FIRMWARE_BASE + physical
}

/// # Safety
///
/// Can only be called by the processor while the higher-half mapping and exception entry are valid.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot.trap")]
unsafe extern "C" fn _boot_trap() -> ! {
    core::arch::naked_asm!("idle 0", "b _boot_trap");
}

pub fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    CPU.disable_interrupts();
    EARLY_CONSOLE.panic(info);
    CPU.halt()
}
