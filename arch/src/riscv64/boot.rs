// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

use kernel::{BootInfo, CpuInfo, MemoryInfo};

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_HEADER_SIZE: usize = 40;
const FDT_MAX_SIZE: usize = 16 * 1024 * 1024;

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
}

/// QEMU virt 上 RISC-V 64 固件入口。
///
/// # Safety
///
/// 只能被固件调用一次。
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "csrw sie, zero",
        "csrci sstatus, 2",
        "lla sp, _boot_stack_top",
        "call {entry}",
        "j .",
        entry = sym _start_native,
    );
}

/// # Safety
///
/// 只能由 [`_start`] 在系统启动时调用一次。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start_native(hartid: usize, dtb: usize) -> ! {
    if hartid != 0 {
        halt();
    }

    clear_bss();

    if dtb == 0 || !dtb.is_multiple_of(8) || dtb > usize::MAX - FDT_HEADER_SIZE {
        halt();
    }

    // SAFETY: 需要保证该 40 字节范围可读且不被并发修改。
    let header = unsafe { core::slice::from_raw_parts(dtb as *const u8, FDT_HEADER_SIZE) };
    if !validate_fdt_header(header) {
        halt();
    }

    let info = BootInfo {
        cpu: CpuInfo {
            hardware_id: hartid as u64,
        },
        memory: MemoryInfo { regions: &[] },
    };

    kernel::boot_kernel(info)
}

fn clear_bss() {
    let mut cursor = core::ptr::addr_of!(_sbss) as usize;
    let end = core::ptr::addr_of!(_ebss) as usize;

    while cursor < end {
        // SAFETY:需要保证该地址位于可写且 8 字节对齐的 BSS 中。
        unsafe {
            (cursor as *mut u64).write_volatile(0);
        }
        cursor += size_of::<u64>();
    }
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let field: [u8; 4] = bytes.get(offset..end)?.try_into().ok()?;
    Some(u32::from_be_bytes(field))
}

// REFERENCE: https://www.devicetree.org/specifications/
fn validate_fdt_header(bytes: &[u8]) -> bool {
    if bytes.len() < FDT_HEADER_SIZE {
        return false;
    }

    let header = (
        read_be_u32(bytes, 0),
        read_be_u32(bytes, 4),
        read_be_u32(bytes, 8),
        read_be_u32(bytes, 12),
        read_be_u32(bytes, 16),
        read_be_u32(bytes, 20),
        read_be_u32(bytes, 24),
        read_be_u32(bytes, 32),
        read_be_u32(bytes, 36),
    );
    let (
        Some(magic),
        Some(totalsize),
        Some(off_struct),
        Some(off_strings),
        Some(off_rsvmap),
        Some(version),
        Some(last_comp),
        Some(size_strings),
        Some(size_struct),
    ) = header
    else {
        return false;
    };

    if magic != FDT_MAGIC {
        return false;
    }

    let totalsize = totalsize as usize;
    if !(FDT_HEADER_SIZE..=FDT_MAX_SIZE).contains(&totalsize) {
        return false;
    }

    if version < 17 || last_comp > 17 {
        return false;
    }

    let off_struct = off_struct as usize;
    let off_strings = off_strings as usize;
    let off_rsvmap = off_rsvmap as usize;
    let size_strings = size_strings as usize;
    let size_struct = size_struct as usize;

    if !off_struct.is_multiple_of(4) || off_struct < FDT_HEADER_SIZE || off_struct >= totalsize {
        return false;
    }
    if !off_strings.is_multiple_of(4) || off_strings < FDT_HEADER_SIZE || off_strings >= totalsize {
        return false;
    }
    if !off_rsvmap.is_multiple_of(8) || off_rsvmap < FDT_HEADER_SIZE || off_rsvmap >= totalsize {
        return false;
    }

    let Some(struct_end) = off_struct.checked_add(size_struct) else {
        return false;
    };
    if struct_end > totalsize {
        return false;
    }

    let Some(strings_end) = off_strings.checked_add(size_strings) else {
        return false;
    };
    strings_end <= totalsize
}

pub fn halt() -> ! {
    loop {
        // SAFETY: 需要处理器位于 S-mode 且 mstatus.TW = 0
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
        }
    }
}
