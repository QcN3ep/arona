// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use arch::device::uart::Uart;
use kernel::{
    hardware::{
        self,
        cpu::InterruptState,
        mmu::{
            MemoryType, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions, PageTableEntry,
        },
    },
    memory::{Frame, VirtRange},
    trap::{FaultAccess, Trap, TrapEvent, TrapOrigin, TrapOutcome},
};

#[cfg(target_arch = "riscv64")]
#[path = "riscv64/trap.rs"]
mod isa;
#[cfg(target_arch = "loongarch64")]
#[path = "loongarch64/trap.rs"]
mod isa;

#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0040_01e0;

const CASE: &str = match option_env!("ARONA_TRAP_TEST") {
    Some(case) => case,
    None => "interrupt",
};

static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);
static FLOATING: AtomicBool = AtomicBool::new(false);
static PAGE_FAULTS: AtomicUsize = AtomicUsize::new(0);
static USER_LEAF: AtomicUsize = AtomicUsize::new(0);

#[repr(align(4096))]
struct Page {
    _bytes: [u8; PAGE_SIZE],
}

static mut USER_TABLES: [Page; 3] = [const {
    Page {
        _bytes: [0; PAGE_SIZE],
    }
}; 3];
static mut USER_STACK: Page = Page {
    _bytes: [0; PAGE_SIZE],
};

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
    static _boot_stack_bottom: u8;
    static _boot_stack_top: u8;
}

/// # Safety
///
/// 只能由测试启动入口在高半区映射和启动栈有效且本地中断关闭时调用。
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native() -> ! {
    let start = core::ptr::addr_of!(_sbss) as *mut u8;
    let length = core::ptr::addr_of!(_ebss) as usize - start as usize;
    // SAFETY: 该范围须为启动入口尚未使用的独占可写 BSS。
    unsafe { start.write_bytes(0, length) };
    uart().init().unwrap();
    uart()
        .write(format_args!("Trap tests started: {CASE}\n"))
        .unwrap();
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
    match CASE {
        "interrupt" => {
            for floating in [false, true] {
                FLOATING.store(floating, Ordering::Relaxed);
                isa::floating(floating);
                assert_eq!(isa::round_trip(), 0);
                assert_eq!(isa::floating_enabled(), floating);
                assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
            }
            assert_eq!(INTERRUPTS.load(Ordering::Relaxed), 2);
            uart()
                .write(format_args!("Trap tests passed: interrupt\n"))
                .unwrap();
        }
        "user" => enter_user(),
        "fault" => isa::fault(),
        #[cfg(target_arch = "loongarch64")]
        "bounds" => isa::bounds_fault(),
        #[cfg(target_arch = "loongarch64")]
        "arithmetic" => isa::arithmetic_fault(),
        "bad-stack" => isa::bad_stack(),
        "reentry" => {
            isa::round_trip();
        }
        _ => panic!("unknown trap test case"),
    }
    hardware::cpu().halt()
}

fn handle(trap: Trap) -> TrapOutcome {
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
    assert!(!isa::floating_enabled());
    match trap.event {
        TrapEvent::SoftwareInterrupt => {
            assert_eq!(trap.origin, TrapOrigin::Kernel);
            isa::acknowledge();
            if CASE == "reentry" {
                isa::fault();
            }
            if FLOATING.load(Ordering::Relaxed) {
                isa::floating(true);
                isa::clobber_floating();
            }
            isa::floating(!FLOATING.load(Ordering::Relaxed));
            INTERRUPTS.fetch_add(1, Ordering::Relaxed);
            TrapOutcome::Resume
        }
        TrapEvent::Syscall {
            number,
            arguments,
            successor,
        } => {
            assert_eq!(trap.origin, TrapOrigin::User);
            assert_eq!(successor, trap.pc + 4);
            match number {
                17 => {
                    assert_eq!(successor, 0x10000 + isa::syscall_successor());
                    assert_eq!(arguments, [1, 2, 3, 4, 5, 6]);
                    TrapOutcome::Return {
                        status: 42,
                        values: [73, 19],
                    }
                }
                18 => {
                    assert_eq!(PAGE_FAULTS.load(Ordering::Relaxed), 3);
                    uart()
                        .write(format_args!("Trap tests passed: user\n"))
                        .unwrap();
                    hardware::cpu().halt()
                }
                _ => panic!("invalid user register state"),
            }
        }
        TrapEvent::PageFault { address, access } => {
            assert_eq!(trap.origin, TrapOrigin::User);
            let expected = match address {
                0x30000 => FaultAccess::Read,
                0x31000 => FaultAccess::Write,
                0x32000 => FaultAccess::Execute,
                _ => panic!("unexpected fault address: {address:#x}"),
            };
            assert_eq!(access, expected);
            let mmu = hardware::mmu();
            let leaf = Frame::new(USER_LEAF.load(Ordering::Relaxed) as u64).unwrap();
            let PageTableEntry::Mapping(mut mapping) =
                mmu.read_entry(leaf, address / PAGE_SIZE, 0).unwrap()
            else {
                panic!("missing test mapping");
            };
            mapping.attributes.user_accessible = true;
            if access == FaultAccess::Execute {
                mapping.attributes.permissions = PagePermissions::ReadExecute;
            }
            mmu.write_entry(
                leaf,
                address / PAGE_SIZE,
                0,
                PageTableEntry::Mapping(mapping),
            )
            .unwrap();
            mmu.invalidate_page(address).unwrap();
            PAGE_FAULTS.fetch_add(1, Ordering::Relaxed);
            TrapOutcome::Resume
        }
        _ => panic!("unexpected trap: {trap:?}"),
    }
}

fn enter_user() -> ! {
    let mmu = hardware::mmu();
    let image = arch::boot::kernel_image();
    let physical =
        |address: usize| image.text.physical.start + (address - image.text.virtual_start) as u64;
    let layout = mmu.layout();
    let base = physical(core::ptr::addr_of!(USER_TABLES) as usize);
    let mut parent = Frame::new(image.boot_tables.start).unwrap();
    for level in (1..layout.levels).rev() {
        let child =
            Frame::new(base + (layout.levels - 1 - level) as u64 * PAGE_SIZE as u64).unwrap();
        mmu.zero_frame(child.address()).unwrap();
        mmu.write_entry(parent, 0, level, PageTableEntry::Table(child))
            .unwrap();
        parent = child;
    }
    USER_LEAF.store(parent.address() as usize, Ordering::Relaxed);
    for (address, physical_address, permissions, user_accessible) in [
        (
            0x10000,
            physical(isa::user_code as *const () as usize),
            PagePermissions::ReadExecute,
            true,
        ),
        (
            0x20000,
            physical(core::ptr::addr_of!(USER_STACK) as usize),
            PagePermissions::ReadWrite,
            true,
        ),
        (
            0x30000,
            physical(core::ptr::addr_of!(USER_STACK) as usize),
            PagePermissions::ReadWrite,
            false,
        ),
        (
            0x31000,
            physical(core::ptr::addr_of!(USER_STACK) as usize),
            PagePermissions::ReadWrite,
            false,
        ),
        (
            0x32000,
            physical(isa::user_finish as *const () as usize),
            PagePermissions::ReadOnly,
            true,
        ),
    ] {
        mmu.write_entry(
            parent,
            address / PAGE_SIZE,
            0,
            PageTableEntry::Mapping(PageMapping {
                physical_address,
                attributes: PageAttributes {
                    permissions,
                    memory_type: MemoryType::Normal,
                    user_accessible,
                },
            }),
        )
        .unwrap();
    }
    isa::user_root(Frame::new(image.boot_tables.start).unwrap());
    mmu.invalidate_all();
    isa::enter_user()
}

fn uart() -> Uart {
    // SAFETY: UART 须为当前 ISA 的启动 UART 映射，测试访问须串行执行。
    unsafe { Uart::new(UART) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = uart().emergency_write(format_args!("Trap tests failed: {info}\n"));
    loop {
        core::hint::spin_loop();
    }
}
