// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

#[path = "device/uart.rs"]
mod uart_tests;

use core::sync::atomic::{AtomicUsize, Ordering};

use arch::device::uart::Uart;
use kernel::{
    hardware::mmu::{
        MemoryType, Mmu, MmuError, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions,
        PageTableEntry,
    },
    memory::Frame,
};

#[cfg(target_arch = "riscv64")]
const KERNEL_OFFSET: usize = 0xffff_ffbf_7fe0_0000;
#[cfg(target_arch = "loongarch64")]
const KERNEL_OFFSET: usize = 0xffff_8000_0000_0000;
#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0060_01e0;

const MAX_LEVELS: usize = 4;
const TEST_ADDRESS: usize = 0x1_0000;
const CODE_ADDRESS: usize = 0x2_0000;

#[repr(C, align(4096))]
struct FrameBytes([u8; PAGE_SIZE]);

static mut TABLES: [FrameBytes; 2 * MAX_LEVELS] =
    [const { FrameBytes([0; PAGE_SIZE]) }; 2 * MAX_LEVELS];
static mut DATA: [FrameBytes; 2] = [const { FrameBytes([0; PAGE_SIZE]) }; 2];
static mut CODE: FrameBytes = FrameBytes([0; PAGE_SIZE]);
static CONSOLE: AtomicUsize = AtomicUsize::new(UART);

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
}

struct Tables {
    root: Frame,
    leaf: Frame,
}

impl Tables {
    fn new(mmu: &dyn Mmu, base: u64) -> Self {
        let layout = mmu.layout();
        let root = Frame::new(base).unwrap();
        let mut words = [0; PAGE_SIZE / size_of::<u64>()];
        mmu.read_frame(arch::boot::kernel_image().boot_tables.start, 0, &mut words)
            .unwrap();
        mmu.write_frame(base, 0, &words).unwrap();
        let mut table = root;
        for level in (1..layout.levels).rev() {
            let child =
                Frame::new(base + (layout.levels - level) as u64 * PAGE_SIZE as u64).unwrap();
            mmu.zero_frame(child.address()).unwrap();
            assert_eq!(
                mmu.read_entry(table, 0, level).unwrap(),
                PageTableEntry::Empty
            );
            mmu.write_entry(table, 0, level, PageTableEntry::Table(child))
                .unwrap();
            table = child;
        }
        Self { root, leaf: table }
    }

    fn set(&self, mmu: &dyn Mmu, address: usize, entry: PageTableEntry) -> Result<(), MmuError> {
        mmu.write_entry(self.leaf, address / PAGE_SIZE, 0, entry)
    }

    fn get(&self, mmu: &dyn Mmu, address: usize) -> Result<PageTableEntry, MmuError> {
        mmu.read_entry(self.leaf, address / PAGE_SIZE, 0)
    }
}

fn physical(address: usize) -> u64 {
    address.wrapping_sub(KERNEL_OFFSET) as u64
}

fn mapping(physical_address: u64, permissions: PagePermissions) -> PageTableEntry {
    PageTableEntry::Mapping(PageMapping {
        physical_address,
        attributes: PageAttributes {
            permissions,
            memory_type: MemoryType::Normal,
            user_accessible: false,
        },
    })
}

/// # Safety
///
/// 仅由测试映像的启动入口在高半区映射和启动栈有效时调用。
#[unsafe(no_mangle)]
unsafe extern "C" fn __wrap__start_native() -> ! {
    let mut cursor = core::ptr::addr_of!(_sbss) as usize;
    let end = core::ptr::addr_of!(_ebss) as usize;
    while cursor < end {
        // SAFETY: cursor 须位于启动入口尚未使用的 BSS 内。
        unsafe { (cursor as *mut u8).write_volatile(0) };
        cursor += 1;
    }
    // SAFETY: 地址须为当前 ISA 的启动 UART 映射，且仅由测试使用。
    let uart = unsafe { Uart::new(UART) };
    uart.init().unwrap();
    uart.write(format_args!("MMU tests started\n")).unwrap();
    uart_tests::run(&uart);
    arch::hardware::bind();
    let mmu = kernel::hardware::mmu();
    let base = physical(core::ptr::addr_of!(TABLES) as usize);
    let first = Tables::new(mmu, base);
    let second = Tables::new(mmu, base + (MAX_LEVELS * PAGE_SIZE) as u64);
    let data = physical(core::ptr::addr_of!(DATA) as usize);
    run_entries(mmu, &first, data);
    run_frame_access(mmu, data);
    first
        .set(mmu, TEST_ADDRESS, mapping(data, PagePermissions::ReadWrite))
        .unwrap();
    second
        .set(
            mmu,
            TEST_ADDRESS,
            mapping(data + PAGE_SIZE as u64, PagePermissions::ReadWrite),
        )
        .unwrap();
    mmu.write_frame(data, 0, &[41]).unwrap();
    mmu.write_frame(data + PAGE_SIZE as u64, 0, &[73]).unwrap();
    mmu.switch_root(first.root).unwrap();
    assert_eq!(read_test(), 41);
    let mut word = [0];
    mmu.read_mapped_frame(TEST_ADDRESS, 0, &mut word).unwrap();
    assert_eq!(word, [41]);
    mmu.write_mapped_frame(TEST_ADDRESS, 0, &[42]).unwrap();
    assert_eq!(read_test(), 42);
    assert_eq!(
        mmu.write_mapped_frame(TEST_ADDRESS, PAGE_SIZE / 8, &[1]),
        Err(MmuError::InvalidRange)
    );
    assert_eq!(
        mmu.read_mapped_frame(TEST_ADDRESS + 1, 0, &mut word),
        Err(MmuError::InvalidAlignment)
    );
    mmu.switch_root(second.root).unwrap();
    assert_eq!(read_test(), 73);
    mmu.switch_root(first.root).unwrap();
    first
        .set(
            mmu,
            TEST_ADDRESS,
            mapping(data + PAGE_SIZE as u64, PagePermissions::ReadOnly),
        )
        .unwrap();
    mmu.invalidate_page(TEST_ADDRESS).unwrap();
    assert_eq!(read_test(), 73);
    mmu.read_mapped_frame(TEST_ADDRESS, 0, &mut word).unwrap();
    assert_eq!(word, [73]);
    first.set(mmu, TEST_ADDRESS, PageTableEntry::Empty).unwrap();
    mmu.invalidate_all();
    assert_eq!(first.get(mmu, TEST_ADDRESS).unwrap(), PageTableEntry::Empty);
    publish_code(mmu, &first);
    uart.write(format_args!("MMU tests passed\n")).unwrap();
    halt()
}

fn run_entries(mmu: &dyn Mmu, tables: &Tables, data: u64) {
    assert_eq!(
        tables.get(mmu, TEST_ADDRESS).unwrap(),
        PageTableEntry::Empty
    );
    for permissions in [
        PagePermissions::NoAccess,
        PagePermissions::ReadOnly,
        PagePermissions::ReadWrite,
        PagePermissions::ReadExecute,
        PagePermissions::ExecuteOnly,
    ] {
        let entry = mapping(data, permissions);
        tables.set(mmu, TEST_ADDRESS, entry).unwrap();
        assert_eq!(tables.get(mmu, TEST_ADDRESS).unwrap(), entry);
    }
    let original = tables.get(mmu, TEST_ADDRESS).unwrap();
    assert_eq!(
        tables.set(
            mmu,
            TEST_ADDRESS,
            mapping(data + 1, PagePermissions::ReadOnly)
        ),
        Err(MmuError::InvalidAlignment)
    );
    assert_eq!(
        tables.set(
            mmu,
            TEST_ADDRESS,
            mapping(!(PAGE_SIZE as u64 - 1), PagePermissions::ReadOnly)
        ),
        Err(MmuError::InvalidAddress)
    );
    assert_eq!(
        tables.set(mmu, TEST_ADDRESS, PageTableEntry::Table(tables.root)),
        Err(MmuError::InvalidTable)
    );
    assert_eq!(
        mmu.read_entry(tables.leaf, PAGE_SIZE / 8, 0),
        Err(MmuError::InvalidRange)
    );
    assert_eq!(
        mmu.read_entry(tables.root, 0, mmu.layout().levels),
        Err(MmuError::InvalidRange)
    );
    assert_eq!(tables.get(mmu, TEST_ADDRESS).unwrap(), original);
    run_device_entries(mmu, tables, data);
    tables
        .set(mmu, TEST_ADDRESS, PageTableEntry::Empty)
        .unwrap();
    assert_eq!(mmu.synchronize_code(19..19), Ok(()));
    assert_eq!(
        mmu.synchronize_code(core::ops::Range { start: 19, end: 0 }),
        Err(MmuError::InvalidRange)
    );
    assert_eq!(
        mmu.invalidate_page(1usize << 55),
        Err(MmuError::InvalidAddress)
    );
    assert_eq!(
        mmu.synchronize_frame(Frame::new(!(PAGE_SIZE as u64 - 1)).unwrap()),
        Err(MmuError::InvalidAddress)
    );
}

fn run_device_entries(mmu: &dyn Mmu, tables: &Tables, data: u64) {
    let mut device = PageMapping {
        physical_address: data,
        attributes: PageAttributes {
            permissions: PagePermissions::ReadOnly,
            memory_type: MemoryType::Device,
            user_accessible: false,
        },
    };
    #[cfg(target_arch = "riscv64")]
    assert_eq!(
        tables.set(mmu, TEST_ADDRESS, PageTableEntry::Mapping(device)),
        Err(MmuError::Unsupported)
    );
    #[cfg(target_arch = "loongarch64")]
    {
        tables
            .set(mmu, TEST_ADDRESS, PageTableEntry::Mapping(device))
            .unwrap();
        assert_eq!(
            tables.get(mmu, TEST_ADDRESS).unwrap(),
            PageTableEntry::Mapping(device)
        );
    }
    device.attributes.permissions = PagePermissions::ReadExecute;
    assert_eq!(
        tables.set(mmu, TEST_ADDRESS, PageTableEntry::Mapping(device)),
        Err(MmuError::InvalidAttributes)
    );
}

fn run_frame_access(mmu: &dyn Mmu, address: u64) {
    let mut words = [u64::MAX; PAGE_SIZE / size_of::<u64>()];
    mmu.zero_frame(address).unwrap();
    mmu.read_frame(address, 0, &mut words).unwrap();
    assert_eq!(words, [0; PAGE_SIZE / size_of::<u64>()]);
    let last = words.len() - 1;
    mmu.write_frame(address, last, &[41]).unwrap();
    assert_eq!(
        mmu.write_frame(address, last, &[73, 42]),
        Err(MmuError::InvalidRange)
    );
    assert_eq!(
        mmu.read_frame(address, usize::MAX, &mut []),
        Err(MmuError::InvalidRange)
    );
    assert_eq!(mmu.zero_frame(address + 1), Err(MmuError::InvalidAlignment));
    mmu.read_frame(address, 0, &mut words).unwrap();
    assert_eq!(words[last], 41);
    assert!(words[..last].iter().all(|word| *word == 0));
    mmu.zero_frame(address).unwrap();
    mmu.read_frame(address, 0, &mut words).unwrap();
    assert_eq!(words, [0; PAGE_SIZE / size_of::<u64>()]);
}

fn read_test() -> u8 {
    // SAFETY: TEST_ADDRESS 须由当前页表映射到有效数据页。
    unsafe { (TEST_ADDRESS as *const u8).read_volatile() }
}

fn publish_code(mmu: &dyn Mmu, tables: &Tables) {
    let address = core::ptr::addr_of!(CODE) as usize;
    for (source, expected) in [
        (answer_42 as *const u8, 42usize),
        (answer_43 as *const u8, 43usize),
    ] {
        // SAFETY: source 须指向两个指令的测试函数，CODE 须为独占可写页。
        unsafe {
            core::ptr::copy_nonoverlapping(source, core::ptr::addr_of_mut!(CODE).cast::<u8>(), 8)
        };
        if expected == 42 {
            mmu.synchronize_frame(Frame::new(physical(address)).unwrap())
                .unwrap();
        } else {
            mmu.synchronize_code(address..address + 8).unwrap();
        }
        tables
            .set(
                mmu,
                CODE_ADDRESS,
                mapping(physical(address), PagePermissions::ReadExecute),
            )
            .unwrap();
        mmu.invalidate_page(CODE_ADDRESS).unwrap();
        // SAFETY: CODE_ADDRESS 须包含已同步的无参数 C ABI 测试函数。
        let execute: extern "C" fn() -> usize = unsafe { core::mem::transmute(CODE_ADDRESS) };
        assert_eq!(execute(), expected);
        tables
            .set(mmu, CODE_ADDRESS, PageTableEntry::Empty)
            .unwrap();
        mmu.invalidate_page(CODE_ADDRESS).unwrap();
    }
}

#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
extern "C" fn answer_42() -> usize {
    core::arch::naked_asm!(".option norvc", "li a0, 42", "ret");
}

#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
extern "C" fn answer_43() -> usize {
    core::arch::naked_asm!(".option norvc", "li a0, 43", "ret");
}

#[cfg(target_arch = "loongarch64")]
#[unsafe(naked)]
extern "C" fn answer_42() -> usize {
    core::arch::naked_asm!("addi.d $a0, $zero, 42", "jirl $zero, $ra, 0");
}

#[cfg(target_arch = "loongarch64")]
#[unsafe(naked)]
extern "C" fn answer_43() -> usize {
    core::arch::naked_asm!("addi.d $a0, $zero, 43", "jirl $zero, $ra, 0");
}

fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: UART 须保持启动映射且仅由测试使用。
    let uart = unsafe { Uart::new(CONSOLE.load(Ordering::Relaxed)) };
    let _ = uart.emergency_write(format_args!("MMU tests failed: {info}\n"));
    halt()
}
