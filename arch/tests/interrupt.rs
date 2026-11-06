// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

#[cfg(feature = "runtime-tests")]
extern crate alloc;

#[cfg(feature = "runtime-tests")]
#[path = "device/pci.rs"]
mod pci;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use arch::device::uart::Uart;
use kernel::{
    HardwareInfo, HardwareInterruptInfo,
    hardware::{
        self,
        cpu::InterruptState,
        interrupt::InterruptError,
        mmio::{DeviceMapper, MmioMapper},
        mmu::{PAGE_SIZE, PageTableEntry},
    },
    memory::{self, AddressSpace, Frame, MemoryError, MemoryRegionState, VirtRange},
    trap::{Trap, TrapEvent, TrapOrigin, TrapOutcome},
};
use spin::Once;

#[cfg(target_arch = "riscv64")]
#[path = "riscv64/interrupt.rs"]
mod isa;
#[cfg(target_arch = "loongarch64")]
#[path = "loongarch64/interrupt.rs"]
mod isa;

#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0060_01e0;

const MILLISECOND: u64 = 1_000_000;
static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);
static DEVICE_TESTS: AtomicBool = AtomicBool::new(false);
static MODE: AtomicUsize = AtomicUsize::new(0);
static IRQ: Once<HardwareInterruptInfo> = Once::new();
#[cfg(feature = "runtime-tests")]
static PROGRAM: Once<alloc::vec::Vec<u8>> = Once::new();
static mut HARDWARE: HardwareInfo = HardwareInfo::new();

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
    static _boot_stack_bottom: u8;
    static _boot_stack_top: u8;
}

/// # Safety
///
/// 只能由测试入口在启动映射与栈有效且本地中断关闭时调用，参数须为固件交付值。
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native(first: usize, second: usize) -> ! {
    let start = core::ptr::addr_of!(_sbss) as *mut u8;
    let length = core::ptr::addr_of!(_ebss) as usize - start as usize;
    // SAFETY: 该范围须为启动入口尚未使用的独占可写 BSS。
    unsafe { start.write_bytes(0, length) };
    uart().init().unwrap();
    uart()
        .write(format_args!("Interrupt controller tests started\n"))
        .unwrap();
    #[cfg(feature = "runtime-tests")]
    let suite = select_program();
    arch::hardware::bind();
    hardware::trap()
        .install(
            VirtRange {
                start: core::ptr::addr_of!(_boot_stack_bottom) as usize,
                end: core::ptr::addr_of!(_boot_stack_top) as usize,
            },
            handle,
        )
        .unwrap();
    let (cpu_id, tree, program) = isa::boot(first, second);
    // SAFETY: 测试须独占该硬件描述，初始化后只读。
    let hardware = unsafe { &mut *core::ptr::addr_of_mut!(HARDWARE) };
    tree.dump(hardware).unwrap();
    let image = arch::boot::kernel_image();
    arch::boot::prepare_boot_info(cpu_id, image, hardware).unwrap();
    if let Some(range) = program {
        hardware
            .memory
            .reserve(range, MemoryRegionState::BootReserved)
            .unwrap();
    }
    initialize_memory(hardware);
    let device = hardware
        .devices
        .iter()
        .find(|device| device.instance_id == hardware.console.as_ref().unwrap().device)
        .unwrap();
    IRQ.call_once(|| device.interrupts[0]);
    hardware::timer()
        .initialize(hardware.timebase_frequency_hz)
        .unwrap();
    let controller = hardware::interrupt_controller();
    let mmio = DeviceMapper::new(hardware);
    mapping_lifecycle(&mmio);
    let user = AddressSpace::new().unwrap();
    assert_eq!(
        controller.claim().unwrap_err(),
        InterruptError::NotInitialized
    );
    controller.initialize(hardware, cpu_id, &mmio).unwrap();
    assert_eq!(
        controller.initialize(hardware, cpu_id, &mmio),
        Err(InterruptError::AlreadyInitialized)
    );
    user.activate().unwrap();
    lifecycle();
    memory::manager().kernel_address_space().activate().unwrap();
    drop(user);
    DEVICE_TESTS.store(true, Ordering::Relaxed);
    kernel::thread::scheduler::initialize().unwrap();
    #[cfg(feature = "runtime-tests")]
    let firmware = pci::prepare(suite, hardware);
    let devices = kernel::device::DeviceSet::initialize(hardware).unwrap();
    #[cfg(feature = "runtime-tests")]
    pci::verify(&devices, &firmware);
    let (pci_devices, behind_bridges) = kernel_tests::run_devices(devices.clone(), hardware);
    if pci_devices != 0 {
        uart()
            .write(format_args!(
                "PCI catalog tests passed: {pci_devices} devices, {behind_bridges} behind bridges\n"
            ))
            .unwrap();
    }
    #[cfg(feature = "runtime-tests")]
    run_program(suite, program, devices);
    uart()
        .write(format_args!("Interrupt controller tests passed\n"))
        .unwrap();
    hardware::cpu().halt()
}

#[cfg(feature = "runtime-tests")]
fn run_program(
    suite: &str,
    program: Option<kernel::memory::PhysRange>,
    devices: kernel::device::DeviceSet,
) {
    if matches!(
        suite,
        "dma" | "dma-drop" | "dma-revoke" | "block" | "fs" | "rng" | "rtc" | "service"
    ) {
        kernel::logger::init(kernel::log::LevelFilter::Info).unwrap();
        kernel::logger::LOGGER.add_sink(&TestLog).unwrap();
    }
    if matches!(suite, "dma-drop" | "dma-revoke") {
        kernel_tests::run_dma_failure(&devices, suite == "dma-revoke");
    }
    let data = kernel::BootData {
        physical: program.expect("device test image is missing"),
    };
    let bytes = PROGRAM.call_once(|| {
        let mut bytes = alloc::vec![0; data.len().unwrap()];
        data.read(0, &mut bytes).unwrap();
        bytes
    });
    if suite == "init" {
        let init = arch::boot::initramfs::find_init(
            arch::boot::initramfs::Image::from_bytes(bytes, 0).unwrap(),
        )
        .unwrap()
        .data()
        .physical;
        kernel_tests::run_init_program(
            &bytes[init.start as usize..init.end as usize],
            bytes,
            devices,
        );
        uart()
            .write(format_args!("Init service tests passed\n"))
            .unwrap();
    } else {
        let counts = kernel_tests::run_device_program(bytes, devices, suite == "service");
        if counts != 0 {
            uart()
                .write(format_args!(
                    "PCI resource tests passed: {} mapped, {} 64-bit, {} INTx\n",
                    counts >> 32,
                    (counts >> 16) & 0xffff,
                    counts & 0xffff
                ))
                .unwrap();
        }
    }
    if suite == "dma" {
        uart().write(format_args!("DMA tests passed\n")).unwrap();
    }
    if suite == "block" {
        uart()
            .write(format_args!("Block driver tests passed\n"))
            .unwrap();
    }
    if suite == "fs" {
        uart()
            .write(format_args!("Filesystem tests passed\n"))
            .unwrap();
    }
    if suite == "rng" {
        uart()
            .write(format_args!("RNG driver tests passed\n"))
            .unwrap();
    }
    if suite == "rtc" {
        uart()
            .write(format_args!("RTC driver tests passed\n"))
            .unwrap();
    }
    if suite == "service" {
        uart()
            .write(format_args!("Device service tests passed\n"))
            .unwrap();
    }
}

#[cfg(feature = "runtime-tests")]
fn select_program() -> &'static str {
    uart().write(format_args!("Test suite: ")).unwrap();
    let mut input = [0; 16];
    let mut length = 0;
    loop {
        // SAFETY: UART 须保持可读的启动映射且由测试独占。
        let byte = unsafe {
            while ((UART + 5) as *const u8).read_volatile() & 1 == 0 {
                core::hint::spin_loop();
            }
            (UART as *const u8).read_volatile()
        };
        if byte == b'\n' || byte == b'\r' {
            return match &input[..length] {
                b"device" => "device",
                b"service" => "service",
                b"init" => "init",
                b"dma" => "dma",
                b"block" => "block",
                b"rng" => "rng",
                b"rtc" => "rtc",
                b"fs" => "fs",
                b"dma-drop" => "dma-drop",
                b"dma-revoke" => "dma-revoke",
                b"pci-firmware" => "pci-firmware",
                b"pci-exhausted" => "pci-exhausted",
                b"pci-rollback" => "pci-rollback",
                _ => panic!("unknown device test suite"),
            };
        }
        assert!(length < input.len(), "test suite name is too long");
        input[length] = byte;
        length += 1;
    }
}

#[cfg(feature = "runtime-tests")]
struct TestLog;

#[cfg(feature = "runtime-tests")]
impl kernel::logger::Sink for TestLog {
    fn write(&self, record: &kernel::log::Record<'_>) -> core::fmt::Result {
        uart().write(format_args!("{}\n", record.args()))
    }
}

fn initialize_memory(hardware: &HardwareInfo) {
    memory::initialize(&hardware.memory.regions).unwrap();
    let manager = memory::manager();
    manager.initialize_kernel_address_space().unwrap();
    let space = manager.kernel_address_space();
    let mmu = hardware::mmu();
    let layout = mmu.layout();
    let shift = PAGE_SIZE.trailing_zeros() + layout.index_bits * (layout.levels - 1) as u32;
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
        .read_entry(
            Frame::new(image.boot_tables.start).unwrap(),
            slot,
            layout.levels - 1,
        )
        .unwrap();
    mmu.write_entry(space.root(), slot, layout.levels - 1, entry)
        .unwrap();
    manager.initialize_direct_map().unwrap();
    space.activate().unwrap();
}

fn mapping_lifecycle(mmio: &impl MmioMapper) {
    let device = IRQ.get().unwrap().controller;
    let mapping = mmio.map(device, 0).unwrap();
    let range = mapping.virtual_range();
    let space = memory::manager().kernel_address_space();
    assert_eq!(
        space.translate(range.start).unwrap(),
        Some(mapping.physical_range().start)
    );
    assert_eq!(
        space.translate(range.end - 1).unwrap(),
        Some(mapping.physical_range().end - 1)
    );
    let attributes = space
        .query(range.start)
        .unwrap()
        .unwrap()
        .mapping
        .unwrap()
        .attributes;
    assert_eq!(attributes.memory_type, hardware::mmu().device_memory_type());
    assert!(!attributes.user_accessible);
    assert!(matches!(mmio.map(device, 0), Err(MemoryError::InUse)));
    assert_eq!(
        space.unmap(space.query(range.start).unwrap().unwrap().range),
        Err(MemoryError::InUse)
    );
    drop(mapping);
    assert!(space.query(range.start).unwrap().is_none());
    let layout = hardware::mmu().layout();
    let slot = (range.start
        >> (PAGE_SIZE.trailing_zeros() + layout.index_bits * (layout.levels - 1) as u32))
        & ((1 << layout.index_bits) - 1);
    assert!(matches!(
        hardware::mmu()
            .read_entry(space.root(), slot, layout.levels - 1)
            .unwrap(),
        PageTableEntry::Table(_)
    ));
}

fn lifecycle() {
    let controller = hardware::interrupt_controller();
    let interrupt = *IRQ.get().unwrap();
    assert!(controller.claim().unwrap().is_none());
    uart().flush().unwrap();
    uart_interrupt(true);
    wait_quiet(0);
    controller.unmask(interrupt).unwrap();
    wait_interrupts(1);

    uart_interrupt(true);
    wait_interrupts(2);
    controller.mask(interrupt.into()).unwrap();
    uart_interrupt(true);
    wait_quiet(2);
    controller.unmask(interrupt).unwrap();
    wait_interrupts(3);

    MODE.store(1, Ordering::Relaxed);
    uart_interrupt(true);
    wait_interrupts(4);
    wait_quiet(4);
    assert!(controller.claim().unwrap().is_none());
    uart_interrupt(false);
    controller.unmask(interrupt).unwrap();
    uart_interrupt(true);
    wait_interrupts(5);

    #[cfg(target_arch = "loongarch64")]
    {
        MODE.store(2, Ordering::Relaxed);
        uart_interrupt(true);
        wait_interrupts(7);
    }

    let count = INTERRUPTS.load(Ordering::Relaxed);
    MODE.store(3, Ordering::Relaxed);
    uart_interrupt(true);
    wait_interrupts(count + 1);
    wait_quiet(count + 1);
    controller.unmask(interrupt).unwrap();
    uart_interrupt(false);
    uart_interrupt(true);
    wait_interrupts(count + 2);
    controller.mask(interrupt.into()).unwrap();
    #[cfg(target_arch = "loongarch64")]
    {
        let edge = HardwareInterruptInfo {
            trigger: Some(kernel::InterruptTrigger::Edge),
            ..interrupt
        };
        controller.unmask(edge).unwrap();
        uart_interrupt(true);
        wait_interrupts(count + 3);
        uart_interrupt(true);
        wait_interrupts(count + 4);
        controller.mask(interrupt.into()).unwrap();
    }
    assert!(controller.claim().unwrap().is_none());
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
}

fn wait_interrupts(count: usize) {
    hardware::timer()
        .set_deadline(hardware::timer().now_ns().unwrap() + 100 * MILLISECOND)
        .unwrap();
    while INTERRUPTS.load(Ordering::Relaxed) < count {
        hardware::cpu().idle_until_interrupt();
    }
    hardware::timer().cancel().unwrap();
    assert_eq!(INTERRUPTS.load(Ordering::Relaxed), count);
}

fn wait_quiet(count: usize) {
    let deadline = hardware::timer().now_ns().unwrap() + MILLISECOND;
    while hardware::timer().now_ns().unwrap() < deadline {
        hardware::cpu().restore_interrupts(InterruptState::Enabled);
        core::hint::spin_loop();
        hardware::cpu().disable_interrupts();
    }
    assert_eq!(INTERRUPTS.load(Ordering::Relaxed), count);
}

fn handle(trap: Trap) -> TrapOutcome {
    if DEVICE_TESTS.load(Ordering::Relaxed) {
        return kernel_tests::handle_program(trap);
    }
    assert_eq!(trap.origin, TrapOrigin::Kernel);
    assert_eq!(
        trap.event,
        TrapEvent::ExternalInterrupt,
        "external interrupt timed out after {} interrupts",
        INTERRUPTS.load(Ordering::Relaxed)
    );
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
    let controller = hardware::interrupt_controller();
    let claim = controller
        .claim()
        .unwrap()
        .expect("missing interrupt source");
    let interrupt = *IRQ.get().unwrap();
    assert_eq!(claim.source(), interrupt.into());
    assert_eq!(controller.claim().unwrap_err(), InterruptError::Busy);
    assert_eq!(controller.unmask(interrupt), Err(InterruptError::Busy));
    match MODE.swap(0, Ordering::Relaxed) {
        1 => drop(claim),
        2 => claim.complete().unwrap(),
        3 => {
            controller.mask(interrupt.into()).unwrap();
            claim.complete().unwrap();
        }
        _ => {
            uart_interrupt(false);
            claim.complete().unwrap();
        }
    }
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
    TrapOutcome::Resume
}

fn uart_interrupt(enabled: bool) {
    // SAFETY: 测试须独占启动 UART，访问须符合 16550 寄存器布局。
    unsafe {
        ((UART + 1) as *mut u8).write_volatile(if enabled { 2 } else { 0 });
    }
    #[cfg(target_arch = "riscv64")]
    // SAFETY: 执行环境须允许执行 I/O 屏障。
    unsafe {
        core::arch::asm!("fence iorw, iorw", options(nostack))
    };
    #[cfg(target_arch = "loongarch64")]
    // SAFETY: 执行环境须允许执行 I/O 屏障。
    unsafe {
        core::arch::asm!("dbar 0", options(nostack))
    };
}

fn uart() -> Uart {
    // SAFETY: UART 须为当前 ISA 的启动映射，测试须独占访问。
    unsafe { Uart::new(UART) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = uart().emergency_write(format_args!("Interrupt controller tests failed: {info}\n"));
    loop {
        core::hint::spin_loop();
    }
}
