// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

use core::{
    alloc::Layout,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};

use arch::device::uart::Uart;
use kernel::{
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

#[path = "../../kernel/tests/task/mod.rs"]
mod task_tests;
#[path = "../../kernel/tests/thread/mod.rs"]
mod thread_tests;

#[cfg(target_arch = "riscv64")]
#[path = "riscv64/execution.rs"]
mod isa;
#[cfg(target_arch = "loongarch64")]
#[path = "loongarch64/execution.rs"]
mod isa;

#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0040_01e0;
#[cfg(target_arch = "riscv64")]
const RAM: u64 = 0x8200_0000;
#[cfg(target_arch = "loongarch64")]
const RAM: u64 = 0x0200_0000;

const STACK_SIZE: usize = 16 * 1024;
const ROUNDS: usize = 8;
static CONTEXTS: AtomicPtr<Context> = AtomicPtr::new(core::ptr::null_mut());
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static RETIRED: AtomicUsize = AtomicUsize::new(0);
static KERNEL_ROUNDS: AtomicUsize = AtomicUsize::new(0);
static THREAD_ORDER: AtomicUsize = AtomicUsize::new(0);
static THREAD_TARGET: spin::Mutex<Option<kernel::thread::Thread>> = spin::Mutex::new(None);
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
/// 启动映像及栈须已映射到高半区，测试 RAM 须独占且本地中断须关闭。
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native(_cpu: usize, firmware: usize) -> ! {
    let start = core::ptr::addr_of!(_sbss) as *mut u8;
    let length = core::ptr::addr_of!(_ebss) as usize - start as usize;
    // SAFETY: 该范围须为启动入口尚未使用的独占 BSS。
    unsafe {
        start.write_bytes(0, length);
    }
    uart().init().unwrap();
    uart()
        .write(format_args!("Execution tests started\n"))
        .unwrap();
    let manager = initialize();
    hardware::trap()
        .install(
            VirtRange {
                start: core::ptr::addr_of!(_boot_stack_bottom) as usize,
                end: core::ptr::addr_of!(_boot_stack_top) as usize,
            },
            handle,
        )
        .unwrap();
    if option_env!("ARONA_EXECUTION_TEST") == Some("thread") {
        hardware::timer()
            .initialize(isa::frequency(firmware))
            .unwrap();
        thread_round_trips();
    }
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
    hardware::cpu().halt()
}

fn thread_round_trips() -> ! {
    use kernel::thread::{self, Thread, ThreadError, ThreadState, scheduler};

    scheduler::initialize().unwrap();
    assert_eq!(
        scheduler::initialize(),
        Err(ThreadError::AlreadyInitialized)
    );
    let thread = Thread::new(
        |_| {
            assert_eq!(THREAD_ORDER.fetch_add(1, Ordering::Relaxed), 0);
            isa::set_floating_enabled(true);
            let result = thread::yield_now();
            isa::set_floating_enabled(false);
            assert_eq!(
                result,
                Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
            );
            assert_eq!(thread::current().unwrap().state(), ThreadState::Running);
            let wait = thread::prepare_wait().unwrap();
            let waker = wait.waker();
            isa::set_floating_enabled(true);
            let result = wait.wait_until(u64::MAX);
            isa::set_floating_enabled(false);
            assert_eq!(
                result,
                Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
            );
            assert!(!waker.wake());
            let target = THREAD_TARGET.lock().as_ref().unwrap().clone();
            isa::set_floating_enabled(true);
            let result = target.wait_until(u64::MAX);
            isa::set_floating_enabled(false);
            assert_eq!(
                result,
                Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
            );
            drop(thread::prepare_wait().unwrap());
            thread::yield_now().unwrap();
            19
        },
        0,
    )
    .unwrap();
    let next = Thread::new(
        |_| {
            assert_eq!(THREAD_ORDER.fetch_add(1, Ordering::Relaxed), 1);
            42
        },
        0,
    )
    .unwrap();
    *THREAD_TARGET.lock() = Some(next.clone());
    thread.start().unwrap();
    next.start().unwrap();
    isa::set_floating_enabled(true);
    let result = scheduler::run_ready();
    isa::set_floating_enabled(false);
    assert_eq!(
        result,
        Err(ThreadError::Execution(ExecutionError::ExtensionsEnabled))
    );
    assert_eq!(thread.state(), ThreadState::Ready);
    assert!(thread::current().is_none());
    scheduler::run_ready().unwrap();
    assert_eq!(thread.state(), ThreadState::Dead(19));
    assert_eq!(next.state(), ThreadState::Dead(42));
    *THREAD_TARGET.lock() = None;
    drop(thread);
    drop(next);
    thread_tests::run();
    task_tests::run();
    uart().write(format_args!("Thread tests passed\n")).unwrap();
    hardware::cpu().halt()
}

fn kernel_round_trips() {
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
    if option_env!("ARONA_EXECUTION_TEST") == Some("thread") {
        return thread_tests::handle(trap);
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
        number, arguments, ..
    } = trap.event
    else {
        panic!("unexpected trap: {trap:?}");
    };
    let id = current - 1;
    assert_eq!(arguments[0], id);
    match number {
        1 => {
            let round = USER_ROUNDS[id - 1].fetch_add(1, Ordering::Relaxed);
            assert_eq!(arguments[1], round);
            assert_eq!(arguments[2], id * 0x1000);
            assert_eq!(arguments[3], 0x20000 + id * PAGE_SIZE);
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
                context(current).restore(context(0)).unwrap();
            }
            let next = 5 - current;
            CURRENT.store(next, Ordering::Relaxed);
            context(current).switch_to(context(next)).unwrap();
            panic!("completed user resumed")
        }
        _ => panic!("invalid user register state: {arguments:?}"),
    }
}

fn context(index: usize) -> &'static Context {
    // SAFETY: 测试上下文须位于始终存活的独占堆对象中，且仅由当前处理器访问。
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
    // SAFETY: UART 须保留当前 ISA 的启动映射，测试访问须串行执行。
    unsafe { Uart::new(UART) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = uart().emergency_write(format_args!("Execution tests failed: {info}\n"));
    loop {
        core::hint::spin_loop();
    }
}
