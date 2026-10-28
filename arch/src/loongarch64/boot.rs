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
// SAFETY: 地址须对应 QEMU virt 的字节宽度与步长均为 1 的 16550 UART，且普通访问须由此实例独占。
pub(super) static EARLY_CONSOLE: EarlyConsole =
    EarlyConsole::new(unsafe { Uart::new(paging::UART_BASE + 0x1e0) });

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
}

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html

/// # Safety
///
/// 只能由固件按 ELF 入口约定在物理地址启动。
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
/// 只能在 `_start` 建立页表并切换至高半区后进入。
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
/// 只能由 `_start` 在系统启动时建立高半区映射与栈后调用一次。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start_native(system_table: usize) -> ! {
    clear_bss();
    EARLY_CONSOLE.init();
    logger::LOGGER
        .add_sink(&EARLY_CONSOLE)
        .expect("failed to register early console");
    logger::init(log::LevelFilter::Debug).expect("failed to initialize logger");
    // REFERENCE: https://github.com/torvalds/linux/blob/v6.18/arch/loongarch/include/asm/loongarch.h
    let cpu_id: usize;
    // SAFETY: 执行环境须允许读取 CPUID 寄存器。
    unsafe {
        core::arch::asm!("csrrd {}, 0x20", out(reg) cpu_id, options(nomem, nostack));
    }
    let boot_cpu_id = (cpu_id & 0x1ff) as u64;
    log::info!(target: "arch::boot", "Booting kernel on CPU {boot_cpu_id}");

    // REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/loongarch/boot.c
    // SAFETY: system_table 须指向可读且不变的 SystemTable。
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
    // SAFETY: configuration_table 须覆盖 number_of_configuration_table_entries 个可读且不变的配置表项。
    let tables =
        unsafe { core::slice::from_raw_parts(tables_address as *const ConfigurationTable, count) };
    let dtb = configuration_table(tables, guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0"))
        .expect("QEMU configuration table is missing DTB");
    let external_initramfs = initramfs(tables);

    // SAFETY: 可变借用存续期间须独占访问 HARDWARE_INFO。
    let hardware = unsafe { &mut *core::ptr::addr_of_mut!(HARDWARE_INFO) };
    // SAFETY: dtb 须指向解析期间可读且不变的完整 DTB。
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
    // SAFETY: 启动信息须与实际硬件一致，处理器须处于内核运行所需的执行环境。
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
    // SAFETY: 配置表须指向可读且不变的 QEMU initrd 描述。
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
        // SAFETY: 需要保证该地址位于可写且 8 字节对齐的 BSS 中。
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
/// 只能由处理器在高半区映射和异常入口有效时调用。
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
