// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

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
const UART: usize = 0xffff_8000_0040_01e0;
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
/// 启动映像的高半区映射与启动栈须可用，测试 RAM 须可独占使用。
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native() -> ! {
    let mut cursor = core::ptr::addr_of!(_sbss) as usize;
    let end = core::ptr::addr_of!(_ebss) as usize;
    while cursor < end {
        // SAFETY: cursor 须位于启动入口尚未使用的 BSS 内。
        unsafe { (cursor as *mut u8).write_volatile(0) };
        cursor += 1;
    }
    // SAFETY: UART 须为当前 ISA 的启动 UART 映射，且仅由测试使用。
    let uart = unsafe { Uart::new(UART) };
    uart.init().unwrap();
    uart.write(format_args!("Heap allocator tests started\n"))
        .unwrap();
    assert!(HeapBox::try_new(1u64).is_err());
    let manager = initialize();
    run_allocations(manager);
    uart.write(format_args!("Heap allocator tests passed\n"))
        .unwrap();
    halt()
}

fn initialize() -> &'static MemoryManager {
    arch::hardware::bind();
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
    // 测试保留整个启动分支，映像与 UART 继续使用静态启动页表。
    mmu.write_entry(space.root(), slot, layout.levels - 1, entry)
        .unwrap();
    manager.initialize_direct_map().unwrap();
    space.activate().unwrap();
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
    // SAFETY: UART 须保持启动映射且仅由测试使用。
    let uart = unsafe { Uart::new(UART) };
    let _ = uart.emergency_write(format_args!("Heap allocator tests failed: {info}\n"));
    halt()
}
