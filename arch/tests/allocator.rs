// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(support::run)]
#![reexport_test_harness_main = "test_main"]

mod support;

use core::marker::PhantomPinned;
use core::sync::atomic::{AtomicUsize, Ordering};

use arch::device::uart::Uart;
use kernel::{
    hardware::mmu::PAGE_SIZE,
    memory::{
        self, Frame, FrameOwner, FramePurpose, HeapArc, HeapBox, MemoryError, MemoryManager,
        MemoryRegion, MemoryRegionState, PhysRange, VirtRange,
    },
};

#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0060_01e0;
#[cfg(target_arch = "riscv64")]
const RAM: u64 = 0x8200_0000;
#[cfg(target_arch = "loongarch64")]
const RAM: u64 = 0x0200_0000;

static DROPS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
}

struct Object {
    value: usize,
    _pinned: PhantomPinned,
}

#[repr(align(512))]
struct AlignedObject(u8);

impl Object {
    fn new(value: usize) -> Self {
        Self {
            value,
            _pinned: PhantomPinned,
        }
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

/// # Safety
///
/// The higher-half mapping of the boot image and the boot stack must be usable, and the test RAM must be exclusively available.
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native() -> ! {
    let mut cursor = core::ptr::addr_of!(_sbss) as usize;
    let end = core::ptr::addr_of!(_ebss) as usize;
    while cursor < end {
        // SAFETY: cursor must lie within BSS not yet used by the boot entry point.
        unsafe { (cursor as *mut u8).write_volatile(0) };
        cursor += 1;
    }
    // SAFETY: UART must be the boot UART mapping of the current ISA and must be used only by the test.
    let uart = unsafe { Uart::new(UART) };
    uart.init().unwrap();
    test_main();
    halt()
}

#[kernel_tests::ktest]
fn allocations() {
    // SAFETY: The test exclusively owns the mapped and initialized boot console.
    let uart = unsafe { Uart::new(UART) };
    uart.write(format_args!("Heap allocator tests started\n"))
        .unwrap();
    assert!(HeapBox::try_new(1u64).is_err());
    let manager = initialize();
    run_allocations(manager);
    uart.write(format_args!("Heap allocator tests passed\n"))
        .unwrap();
}

fn initialize() -> &'static MemoryManager {
    arch::hardware::bind();
    let regions = [
        (0, 1, MemoryRegionState::Available),
        (1, 3, MemoryRegionState::BootReserved),
        (3, 4, MemoryRegionState::Reserved),
        (4, 5, MemoryRegionState::NoMap),
        (
            6,
            64 * 1024 * 1024 / PAGE_SIZE,
            MemoryRegionState::Available,
        ),
    ]
    .map(|(start, end, state)| MemoryRegion {
        range: PhysRange {
            start: RAM + (start * PAGE_SIZE) as u64,
            end: RAM + (end * PAGE_SIZE) as u64,
        },
        state,
    });
    kernel_tests::validate_memory_regions(&regions);
    memory::initialize(&regions).unwrap();
    let manager = memory::manager();
    manager.initialize_kernel_address_space().unwrap();
    let space = manager.kernel_address_space();
    let mmu = kernel::hardware::mmu();
    let layout = mmu.layout();
    let shift = PAGE_SIZE.trailing_zeros() + layout.index_bits * (layout.levels - 1) as u32;
    let address = arch::boot::kernel_image().text.virtual_start;
    let start = address & !((1usize << shift) - 1);
    space
        .reserve_at(VirtRange {
            start,
            end: start + (1usize << shift),
        })
        .unwrap();
    let slot = (address >> shift) & ((1 << layout.index_bits) - 1);
    let boot_root = Frame::new(arch::boot::kernel_image().boot_tables.start).unwrap();
    let entry = mmu.read_entry(boot_root, slot, layout.levels - 1).unwrap();
    // The test keeps the entire boot branch; the image and UART continue to use the static boot page tables.
    mmu.write_entry(space.root(), slot, layout.levels - 1, entry)
        .unwrap();
    manager.initialize_direct_map().unwrap();
    kernel_tests::run_memory(&regions);
    manager
}

fn run_allocations(manager: &MemoryManager) {
    let initial = manager.stats();
    let mut exclusive = manager.allocate_box(Object::new(41)).unwrap();
    exclusive.value = 42;
    let shared = manager.allocate_arc(Object::new(73)).unwrap();
    let alias = shared.clone();
    assert!(HeapArc::ptr_eq(&shared, &alias));
    assert_eq!(manager.heap_stats().allocations, 2);
    drop(shared);
    assert_eq!(DROPS.load(Ordering::Relaxed), 0);
    assert_eq!(alias.value, 73);
    let pinned = manager.allocate_pinned(Object::new(19)).unwrap();
    let address = &*pinned as *const Object as usize;
    let moved = core::hint::black_box(pinned);
    assert_eq!(&*moved as *const Object as usize, address);
    assert_eq!(moved.value, 19);
    let mut large = manager.allocate_box([0u8; PAGE_SIZE + 17]).unwrap();
    large.fill(0xa5);
    assert!(large.iter().all(|&byte| byte == 0xa5));
    let aligned = manager.allocate_box(AlignedObject(7)).unwrap();
    assert!((&*aligned as *const AlignedObject as usize).is_multiple_of(512));
    assert_eq!(aligned.0, 7);
    assert_eq!(exclusive.value, 42);
    drop((exclusive, alias, moved, large, aligned));
    assert_eq!(DROPS.load(Ordering::Relaxed), 3);
    assert_eq!(manager.heap_stats().allocations, 0);
    manager.reclaim_heap();
    assert_eq!(manager.stats(), initial);

    let exhausted = manager
        .allocate(
            FrameOwner::Kernel,
            FramePurpose::Data,
            manager.stats().kernel.free_frames,
        )
        .unwrap();
    assert!(matches!(
        manager.allocate_box(Object::new(1)),
        Err(MemoryError::NoMemory)
    ));
    assert!(matches!(
        manager.allocate_arc(Object::new(2)),
        Err(MemoryError::NoMemory)
    ));
    assert!(matches!(
        manager.allocate_pinned(Object::new(3)),
        Err(MemoryError::NoMemory)
    ));
    assert_eq!(DROPS.load(Ordering::Relaxed), 6);
    assert_eq!(manager.heap_stats().allocations, 0);
    drop(exhausted);
    assert_eq!(manager.stats(), initial);
}

fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: UART must keep its boot mapping and must be used only by the test.
    let uart = unsafe { Uart::new(UART) };
    let _ = uart.emergency_write(format_args!("Heap allocator tests failed: {info}\n"));
    halt()
}
