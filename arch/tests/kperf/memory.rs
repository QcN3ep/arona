// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Measure page allocation with the kernel's normal direct mapping active.

use kernel::{
    hardware::{self, mmu::PAGE_SIZE},
    memory::{self, Frame, MemoryRegion, MemoryRegionState, PhysRange, VirtRange},
};
use testkit::perf::Perf;

#[cfg(target_arch = "riscv64")]
const RAM: u64 = 0x8200_0000;
#[cfg(target_arch = "loongarch64")]
const RAM: u64 = 0x0200_0000;

#[testkit::kperf]
fn allocate_free_frame(perf: &mut Perf) {
    initialize();
    testkit::perf_allocate_free_frame(perf);
}

fn initialize() {
    // SAFETY: The single-CPU image owns this RAM and retains the boot access window with interrupts disabled.
    memory::initialize(&[MemoryRegion {
        range: PhysRange {
            start: RAM,
            end: RAM + 64 * 1024 * 1024,
        },
        state: MemoryRegionState::Available,
    }])
    .unwrap();
    let manager = memory::manager();
    manager.initialize_kernel_address_space().unwrap();
    let space = manager.kernel_address_space();
    let mmu = hardware::mmu();
    let layout = mmu.layout();
    let level = layout.levels - 1;
    let shift = PAGE_SIZE.trailing_zeros() + layout.index_bits * level as u32;
    let image = arch::boot::kernel_image();
    let address = image.text.virtual_start;
    let start = address & !((1usize << shift) - 1);
    space
        .reserve_at(VirtRange {
            start,
            end: start + (1usize << shift),
        })
        .unwrap();
    let slot = (address >> shift) & ((1 << layout.index_bits) - 1);
    let entry = mmu
        .read_entry(Frame::new(image.boot_tables.start).unwrap(), slot, level)
        .unwrap();
    // Keep the image, stack, UART, and physical access window in the boot branch.
    mmu.write_entry(space.root(), slot, level, entry).unwrap();
    manager.initialize_direct_map().unwrap();
    space.activate().unwrap();
}
