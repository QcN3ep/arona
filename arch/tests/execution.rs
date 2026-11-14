// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(support::run)]
#![reexport_test_harness_main = "test_main"]

mod support;

use core::{
    alloc::Layout,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};

use ::kernel::{
    hardware::{
        self,
        cpu::InterruptState,
        execution::{Context, ContextStart, ExecutionError, UserEntry},
        mmu::{
            MemoryType, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions, PageTableEntry,
        },
        trap::TrapError,
    },
    memory::{
        self, Frame, HeapAllocation, HeapBox, MemoryManager, MemoryRegion, MemoryRegionState,
        PhysRange, VirtRange,
    },
    trap::{Trap, TrapEvent, TrapOrigin, TrapOutcome},
};
use arch::device::uart::Uart;

#[cfg(target_arch = "riscv64")]
#[path = "riscv64/execution.rs"]
mod isa;
#[cfg(target_arch = "loongarch64")]
#[path = "loongarch64/execution.rs"]
mod isa;

#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0060_01e0;
#[cfg(target_arch = "riscv64")]
const RAM: u64 = 0x8200_0000;
#[cfg(target_arch = "loongarch64")]
const RAM: u64 = 0x0200_0000;

const STACK_SIZE: usize = 16 * 1024;
const ROUNDS: usize = 8;
static CONTEXTS: AtomicPtr<Context> = AtomicPtr::new(core::ptr::null_mut());
static FIRMWARE: AtomicUsize = AtomicUsize::new(0);
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static RETIRED: AtomicUsize = AtomicUsize::new(0);
static KERNEL_ROUNDS: AtomicUsize = AtomicUsize::new(0);
static KERNEL_TESTS: AtomicBool = AtomicBool::new(false);
static USER_ROUNDS: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static USER_DONE: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
static STACKS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

#[repr(align(4096))]
struct Page {
    _bytes: [u8; PAGE_SIZE],
}
static mut USER_TABLES: [Page; 3] = [const {
    Page {
        _bytes: [0; PAGE_SIZE],
    }
}; 3];
static mut USER_STACKS: [Page; 2] = [const {
    Page {
        _bytes: [0; PAGE_SIZE],
    }
}; 2];

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
    static _boot_stack_bottom: u8;
    static _boot_stack_top: u8;
}

/// # Safety
///
/// The boot image and stack must already be mapped into the higher half, the test RAM must be exclusive, and local interrupts must be disabled.
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native(_cpu: usize, firmware: usize) -> ! {
    let start = core::ptr::addr_of!(_sbss) as *mut u8;
    let length = core::ptr::addr_of!(_ebss) as usize - start as usize;
    // SAFETY: The range must be exclusive BSS not yet used by the boot entry point.
    unsafe {
        start.write_bytes(0, length);
    }
    uart().init().unwrap();
    FIRMWARE.store(firmware, Ordering::Relaxed);
    test_main();
    loop {
        core::hint::spin_loop();
    }
}

fn prepare(kernel_tests: bool) {
    initialize();
    hardware::trap()
        .install(
            VirtRange {
                start: core::ptr::addr_of!(_boot_stack_bottom) as usize,
                end: core::ptr::addr_of!(_boot_stack_top) as usize,
            },
            handle,
        )
        .unwrap();
    if kernel_tests {
        hardware::timer()
            .initialize(isa::frequency(FIRMWARE.load(Ordering::Relaxed)))
            .unwrap();
        KERNEL_TESTS.store(true, Ordering::Relaxed);
    }
}

#[kernel_tests::ktest]
fn contexts() {
    prepare(false);
    run_execution(memory::manager());
}

mod kernel {
    use super::{Platform, prepare};

    fn initialize() {
        prepare(true);
        ::kernel::thread::scheduler::initialize().unwrap();
    }

    #[kernel_tests::ktest]
    fn threads() {
        prepare(true);
        kernel_tests::run_threads::<Platform>();
    }

    #[kernel_tests::ktest]
    fn memory() {
        initialize();
        kernel_tests::run_concurrent_memory();
    }

    #[kernel_tests::ktest]
    fn tasks() {
        initialize();
        kernel_tests::run_tasks();
    }

    #[kernel_tests::ktest]
    fn ipc() {
        initialize();
        kernel_tests::run_ipc();
    }

    #[kernel_tests::ktest]
    fn syscalls() {
        initialize();
        kernel_tests::run_syscalls(<Platform as kernel_tests::Platform>::syscall_code());
    }

    #[kernel_tests::ktest]
    fn user_threads() {
        initialize();
        kernel_tests::run_user_threads(
            <Platform as kernel_tests::Platform>::user_code(),
            <Platform as kernel_tests::Platform>::set_floating_enabled,
        );
    }

    #[kernel_tests::ktest]
    fn traps() {
        initialize();
        kernel_tests::run_traps::<Platform>();
    }
}

#[cfg(feature = "runtime-tests")]
#[kernel_tests::ktest]
fn runtime() {
    prepare(true);
    kernel_tests::run_program::<Platform>();
}

#[cfg(feature = "runtime-tests")]
#[kernel_tests::ktest]
fn process() {
    prepare(true);
    kernel_tests::run_process::<Platform>();
}

fn run_execution(manager: &'static MemoryManager) {
    let stacks: [HeapAllocation; 3] = core::array::from_fn(|index| {
        let stack = manager
            .allocate_heap(Layout::from_size_align(STACK_SIZE, PAGE_SIZE).unwrap())
            .unwrap();
        STACKS[index + 1].store(stack.address(), Ordering::Relaxed);
        stack
    });
    let range = |index: usize| VirtRange {
        start: stacks[index].address(),
        end: stacks[index].address() + STACK_SIZE,
    };
    let owned = HeapBox::try_new([
        Context::new(ContextStart::Current).unwrap(),
        Context::new(ContextStart::Kernel {
            stack: range(0),
            entry: worker,
            argument: 73,
        })
        .unwrap(),
        Context::new(ContextStart::User {
            stack: range(1),
            entry: user_entry(1),
        })
        .unwrap(),
        Context::new(ContextStart::User {
            stack: range(2),
            entry: user_entry(2),
        })
        .unwrap(),
    ])
    .unwrap();
    CONTEXTS.store(owned.as_ptr().cast_mut(), Ordering::Relaxed);
    for index in 2..4 {
        let mut state = context(index).user_state().unwrap();
        assert_eq!(state.floating, [0; 32]);
        isa::set_user_marker(&mut state, index - 1);
        context(index).set_user_state(&state).unwrap();
    }
    kernel_round_trips();
    map_user(manager);
    CURRENT.store(2, Ordering::Relaxed);
    context(0).switch_to(context(2)).unwrap();
    assert_eq!(CURRENT.load(Ordering::Relaxed), 0);
    assert!(USER_DONE.iter().all(|done| done.load(Ordering::Relaxed)));
    assert!(
        USER_ROUNDS
            .iter()
            .all(|rounds| rounds.load(Ordering::Relaxed) == ROUNDS)
    );
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
    assert!(!isa::floating_enabled());
    let retired = context(RETIRED.load(Ordering::Relaxed));
    assert_eq!(retired.user_state(), Err(ExecutionError::InvalidState));
    assert_eq!(
        retired.switch_to(context(1)),
        Err(ExecutionError::InvalidState)
    );
    assert_eq!(
        context(0).switch_to(retired),
        Err(ExecutionError::InvalidState)
    );
    uart()
        .write(format_args!("Execution tests passed\n"))
        .unwrap();
}

struct Platform;

impl kernel_tests::Platform for Platform {
    fn return_values(state: &mut ::kernel::hardware::execution::UserState, values: [usize; 3]) {
        isa::return_values(state, values);
    }
    #[cfg(feature = "runtime-tests")]
    fn program() -> Option<&'static [u8]> {
        Some(include_bytes!(env!("ARONA_RUNTIME_TEST_IMAGE")))
    }

    fn syscall_code() -> &'static [u64] {
        // SAFETY: The test code must exclusively occupy a complete, readable kernel code page.
        unsafe {
            core::slice::from_raw_parts(
                isa::syscall_user_code as *const () as *const u64,
                PAGE_SIZE / 8,
            )
        }
    }

    fn set_floating_enabled(enabled: bool) {
        isa::set_floating_enabled(enabled);
    }

    fn user_code() -> &'static [u64] {
        // SAFETY: The test code must exclusively occupy a complete, readable kernel code page.
        unsafe {
            core::slice::from_raw_parts(
                isa::task_user_code as *const () as *const u64,
                PAGE_SIZE / 8,
            )
        }
    }
}

fn kernel_round_trips() {
    assert_eq!(context(0).user_state(), Err(ExecutionError::InvalidState));
    assert_eq!(context(1).user_state(), Err(ExecutionError::InvalidState));
    assert_eq!(
        context(0).restore(context(0)),
        Err(ExecutionError::InvalidState)
    );
    assert_eq!(
        context(0).switch_to(context(0)),
        Err(ExecutionError::InvalidState)
    );
    hardware::cpu().restore_interrupts(InterruptState::Enabled);
    assert_eq!(
        context(2).user_state(),
        Err(ExecutionError::InterruptsEnabled)
    );
    assert_eq!(
        context(0).switch_to(context(1)),
        Err(ExecutionError::InterruptsEnabled)
    );
    hardware::cpu().disable_interrupts();
    for _ in 0..ROUNDS {
        assert_eq!(isa::switch(context(0), context(1)), 0);
        assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
        assert!(!isa::floating_enabled());
    }
    assert_eq!(KERNEL_ROUNDS.load(Ordering::Relaxed), ROUNDS);
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
    manager
}

extern "C" fn worker(argument: usize) -> ! {
    assert_eq!(argument, 73);
    for round in 0..ROUNDS {
        let canary = core::hint::black_box([round, 19, 42, 73]);
        KERNEL_ROUNDS.fetch_add(1, Ordering::Relaxed);
        assert_eq!(isa::switch(context(1), context(0)), 0);
        assert_eq!(core::hint::black_box(canary), [round, 19, 42, 73]);
    }
    panic!("completed worker resumed")
}

extern "C" fn switch_context(current: &Context, next: &Context) {
    current.switch_to(next).unwrap();
}

fn handle(trap: Trap) -> TrapOutcome {
    if KERNEL_TESTS.load(Ordering::Relaxed) {
        return kernel_tests::handle(trap);
    }
    assert_eq!(trap.origin, TrapOrigin::User);
    assert_eq!(hardware::cpu().interrupt_state(), InterruptState::Disabled);
    assert!(!isa::floating_enabled());
    let current = CURRENT.load(Ordering::Relaxed);
    let local = 0usize;
    let address = &local as *const usize as usize;
    let stack = STACKS[current].load(Ordering::Relaxed);
    assert!((stack..stack + STACK_SIZE).contains(&address));
    let TrapEvent::Syscall {
        handle, arguments, ..
    } = trap.event
    else {
        panic!("unexpected trap: {trap:?}");
    };
    let id = current - 1;
    assert_eq!(arguments[0], id);
    match handle {
        1 => {
            let round = USER_ROUNDS[id - 1].fetch_add(1, Ordering::Relaxed);
            assert_eq!(arguments[1], round);
            assert_eq!(arguments[2], id * 0x1000);
            assert_eq!(arguments[3], 0x20000 + id * PAGE_SIZE);
            user_state_round_trip(current, id, round, trap.pc);
            let next = 5 - current;
            CURRENT.store(next, Ordering::Relaxed);
            context(current).switch_to(context(next)).unwrap();
            assert_eq!(CURRENT.load(Ordering::Relaxed), current);
            assert_eq!(
                hardware::trap().set_stack(VirtRange {
                    start: stack,
                    end: stack + STACK_SIZE
                }),
                Err(TrapError::Busy)
            );
            TrapOutcome::Return {
                status: 0,
                values: [id, round],
            }
        }
        2 => {
            USER_DONE[id - 1].store(true, Ordering::Relaxed);
            if USER_DONE.iter().all(|done| done.load(Ordering::Relaxed)) {
                CURRENT.store(0, Ordering::Relaxed);
                RETIRED.store(current, Ordering::Relaxed);
                match context(current).restore(context(0)) {
                    Ok(never) => match never {},
                    Err(error) => panic!("failed to restore context: {error:?}"),
                }
            }
            let next = 5 - current;
            CURRENT.store(next, Ordering::Relaxed);
            context(current).switch_to(context(next)).unwrap();
            panic!("completed user resumed")
        }
        _ => panic!("invalid user register state: {arguments:?}"),
    }
}

fn user_state_round_trip(current: usize, id: usize, round: usize, pc: usize) {
    let context = context(current);
    let before = context.user_state().unwrap();
    assert_eq!(before.pc, pc);
    assert_eq!(
        before.floating,
        [if round == 0 {
            id as u64
        } else {
            id as u64 + 256
        }; 32]
    );
    let entry = hardware::execution().user_entry(&before).unwrap();
    assert_eq!(entry.stack_pointer, user_entry(id).stack_pointer);
    assert_eq!(entry.thread_pointer, user_entry(id).thread_pointer);
    let mut invalid = before.clone();
    invalid.registers[0] = 1;
    assert_eq!(
        context.set_user_state(&invalid),
        Err(ExecutionError::InvalidState)
    );
    invalid = before.clone();
    invalid.pc = usize::MAX;
    assert_eq!(
        context.set_user_state(&invalid),
        Err(ExecutionError::InvalidEntry)
    );
    invalid = before.clone();
    invalid.floating_control = u32::MAX;
    assert_eq!(
        context.set_user_state(&invalid),
        Err(ExecutionError::InvalidState)
    );
    invalid = before.clone();
    invalid.floating_conditions = [2; 8];
    assert_eq!(
        context.set_user_state(&invalid),
        Err(ExecutionError::InvalidState)
    );
    assert_eq!(context.user_state().unwrap(), before);
    let mut state = before;
    state.floating.fill(id as u64 + 256);
    isa::set_floating_control(&mut state, id);
    context.set_user_state(&state).unwrap();
    assert_eq!(context.user_state().unwrap(), state);
}

fn context(index: usize) -> &'static Context {
    // SAFETY: The test context must reside in an exclusively owned heap object that lives forever, and be accessed only by the current processor.
    unsafe { &*CONTEXTS.load(Ordering::Relaxed).add(index) }
}

fn user_entry(id: usize) -> UserEntry {
    UserEntry {
        pc: 0x10000,
        stack_pointer: 0x20000 + id * PAGE_SIZE,
        thread_pointer: id * 0x1000,
        argument: id,
    }
}

fn map_user(manager: &MemoryManager) {
    let mmu = hardware::mmu();
    let image = arch::boot::kernel_image();
    let physical =
        |address: usize| image.text.physical.start + (address - image.text.virtual_start) as u64;
    let base = physical(core::ptr::addr_of!(USER_TABLES) as usize);
    let layout = mmu.layout();
    let mut parent = manager.kernel_address_space().root();
    for level in (1..layout.levels).rev() {
        let child =
            Frame::new(base + (layout.levels - 1 - level) as u64 * PAGE_SIZE as u64).unwrap();
        mmu.zero_frame(child.address()).unwrap();
        mmu.write_entry(parent, 0, level, PageTableEntry::Table(child))
            .unwrap();
        parent = child;
    }
    for (address, source, permissions) in [
        (
            0x10000,
            isa::user_code as *const () as usize,
            PagePermissions::ReadExecute,
        ),
        (
            0x20000,
            core::ptr::addr_of!(USER_STACKS) as usize,
            PagePermissions::ReadWrite,
        ),
        (
            0x21000,
            core::ptr::addr_of!(USER_STACKS) as usize + PAGE_SIZE,
            PagePermissions::ReadWrite,
        ),
    ] {
        mmu.write_entry(
            parent,
            address / PAGE_SIZE,
            0,
            PageTableEntry::Mapping(PageMapping {
                physical_address: physical(source),
                attributes: PageAttributes {
                    permissions,
                    memory_type: MemoryType::Normal,
                    user_accessible: true,
                },
            }),
        )
        .unwrap();
    }
    mmu.invalidate_all();
}

fn uart() -> Uart {
    // SAFETY: UART must retain the boot mapping of the current ISA, and test accesses must be serialized.
    unsafe { Uart::new(UART) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = uart().emergency_write(format_args!("Execution tests failed: {info}\n"));
    loop {
        core::hint::spin_loop();
    }
}
