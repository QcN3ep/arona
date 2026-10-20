// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use arch::device::uart::Uart;
use kernel::hardware::mmu::{
    MemoryType, Mmu, MmuError, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions,
    PageTableStorage,
};

#[cfg(target_arch = "loongarch64")]
use arch::loongarch64::paging::LoongArchMmu;
#[cfg(target_arch = "riscv64")]
use arch::riscv64::paging::Sv39;

#[cfg(target_arch = "riscv64")]
const KERNEL_OFFSET: usize = 0xffff_ffbf_7fe0_0000;
#[cfg(target_arch = "loongarch64")]
const KERNEL_OFFSET: usize = 0xffff_8000_0000_0000;
#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_4000_01e0;
#[cfg(target_arch = "riscv64")]
const BOOT_UART: usize = UART;
#[cfg(target_arch = "loongarch64")]
const BOOT_UART: usize = 0x8000_0000_1fe0_01e0;
#[cfg(target_arch = "riscv64")]
const UART_PHYSICAL: u64 = 0x1000_0000;
#[cfg(target_arch = "loongarch64")]
const UART_PHYSICAL: u64 = 0x1fe0_0000;

const TABLE_COUNT: usize = 16;
const TEST_ADDRESS: usize = 0x1_0000;
const CODE_ADDRESS: usize = 0x2_0000;

#[repr(C, align(4096))]
struct Page([u8; PAGE_SIZE]);

static mut TABLES: [Page; 2 * TABLE_COUNT] = [const { Page([0; PAGE_SIZE]) }; 2 * TABLE_COUNT];
static mut DATA: [Page; 2] = [const { Page([0; PAGE_SIZE]) }; 2];
static mut CODE: Page = Page([0; PAGE_SIZE]);
static CONSOLE: AtomicUsize = AtomicUsize::new(BOOT_UART);

unsafe extern "C" {
    static _sbss: u8;
    static _ebss: u8;
    static _kernel_image: u8;
}

struct Storage<'a> {
    pages: &'a mut [Page],
    used: [bool; TABLE_COUNT],
    remaining: usize,
}

impl<'a> Storage<'a> {
    fn new(pages: &'a mut [Page]) -> Self {
        let mut used = [false; TABLE_COUNT];
        used[0] = true;
        Self {
            pages,
            used,
            remaining: usize::MAX,
        }
    }

    fn index(&self, address: u64) -> Result<usize, MmuError> {
        let offset = address
            .checked_sub(self.root_address())
            .ok_or(MmuError::InvalidTable)?;
        if !offset.is_multiple_of(PAGE_SIZE as u64) {
            return Err(MmuError::InvalidTable);
        }
        let index = (offset / PAGE_SIZE as u64) as usize;
        if !self.used.get(index).copied().unwrap_or(false) {
            return Err(MmuError::InvalidTable);
        }
        Ok(index)
    }

    fn count(&self) -> usize {
        self.used.iter().filter(|used| **used).count()
    }
}

impl PageTableStorage for Storage<'_> {
    fn root_address(&self) -> u64 {
        physical(self.pages.as_ptr() as usize)
    }

    fn table(&self, address: u64) -> Result<&[u8; PAGE_SIZE], MmuError> {
        Ok(&self.pages[self.index(address)?].0)
    }

    fn table_mut(&mut self, address: u64) -> Result<&mut [u8; PAGE_SIZE], MmuError> {
        let index = self.index(address)?;
        Ok(&mut self.pages[index].0)
    }

    fn take_table(&mut self) -> Result<u64, MmuError> {
        if self.remaining == 0 {
            return Err(MmuError::NoMemory);
        }
        let index = self
            .used
            .iter()
            .position(|used| !used)
            .ok_or(MmuError::NoMemory)?;
        self.remaining -= 1;
        self.pages[index].0.fill(0);
        self.used[index] = true;
        Ok(physical(&raw const self.pages[index] as usize))
    }

    fn return_table(&mut self, address: u64) {
        let index = self.index(address).unwrap();
        assert_ne!(index, 0);
        self.used[index] = false;
    }
}

fn physical(address: usize) -> u64 {
    address.wrapping_sub(KERNEL_OFFSET) as u64
}

fn mapping(physical_address: u64, permissions: PagePermissions) -> PageMapping {
    PageMapping {
        physical_address,
        attributes: PageAttributes {
            permissions,
            memory_type: MemoryType::Normal,
            user_accessible: false,
        },
    }
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
    let uart = unsafe { Uart::new(BOOT_UART) };
    uart.init().unwrap();
    uart.write(format_args!("MMU tests started\n")).unwrap();

    #[cfg(target_arch = "riscv64")]
    let mmu = Sv39;
    #[cfg(target_arch = "loongarch64")]
    let mmu = LoongArchMmu;

    // SAFETY: 测试须独占静态后备页，且页表启用后这些页不得移动或释放。
    let pages = unsafe { &mut *core::ptr::addr_of_mut!(TABLES) };
    let (first, second) = pages.split_at_mut(TABLE_COUNT);
    let mut first = Storage::new(first);
    let mut second = Storage::new(second);
    let data = physical((&raw const DATA) as usize);

    arch::hardware::bind();
    let bound = kernel::hardware::mmu();
    bound.initialize(&mut first).unwrap();
    let page = mapping(data, PagePermissions::ReadWrite);
    bound.map(&mut first, TEST_ADDRESS, page).unwrap();
    assert_eq!(bound.query(&first, TEST_ADDRESS).unwrap(), Some(page));
    assert_eq!(bound.unmap(&mut first, TEST_ADDRESS).unwrap(), page);

    run_operations(&mmu, &mut first, data);
    install_kernel(&mmu, &mut first);
    install_kernel(&mmu, &mut second);
    mmu.map(
        &mut first,
        TEST_ADDRESS,
        mapping(data, PagePermissions::ReadWrite),
    )
    .unwrap();
    mmu.map(
        &mut second,
        TEST_ADDRESS,
        mapping(data + PAGE_SIZE as u64, PagePermissions::ReadWrite),
    )
    .unwrap();

    // SAFETY: DATA 须由测试独占且持续映射。
    unsafe {
        (&raw mut DATA[0].0[0]).write_volatile(41);
        (&raw mut DATA[1].0[0]).write_volatile(73);
    }
    mmu.switch_root(&first).unwrap();
    CONSOLE.store(UART, Ordering::Relaxed);
    assert_eq!(read_test(), 41);
    mmu.switch_root(&second).unwrap();
    assert_eq!(read_test(), 73);
    mmu.switch_root(&first).unwrap();
    mmu.unmap(&mut first, TEST_ADDRESS).unwrap();
    mmu.map(
        &mut first,
        TEST_ADDRESS,
        mapping(data + PAGE_SIZE as u64, PagePermissions::ReadWrite),
    )
    .unwrap();
    assert_eq!(read_test(), 73);
    mmu.protect(&mut first, TEST_ADDRESS, PagePermissions::ReadOnly)
        .unwrap();
    assert_eq!(read_test(), 73);
    mmu.invalidate_page(TEST_ADDRESS + 1).unwrap();
    mmu.invalidate_all();
    publish_code(&mmu, &mut first);

    // SAFETY: UART 须在两个测试地址空间中保持设备映射。
    let uart = unsafe { Uart::new(UART) };
    uart.write(format_args!("MMU tests passed\n")).unwrap();
    halt()
}

fn run_operations(mmu: &dyn Mmu, storage: &mut Storage<'_>, data: u64) {
    mmu.initialize(storage).unwrap();
    assert_eq!(mmu.query(storage, TEST_ADDRESS).unwrap(), None);
    let page = mapping(data, PagePermissions::ReadWrite);
    mmu.map(storage, TEST_ADDRESS, page).unwrap();
    assert_eq!(mmu.query(storage, TEST_ADDRESS + 19).unwrap(), Some(page));
    assert_eq!(
        mmu.map(storage, TEST_ADDRESS, page),
        Err(MmuError::AlreadyMapped)
    );
    assert_eq!(
        mmu.map(storage, TEST_ADDRESS + 1, page),
        Err(MmuError::InvalidAlignment)
    );
    assert_eq!(
        mmu.map(storage, 1usize << 55, page),
        Err(MmuError::InvalidAddress)
    );
    let mut unaligned = page;
    unaligned.physical_address += 1;
    assert_eq!(
        mmu.map(storage, TEST_ADDRESS + PAGE_SIZE, unaligned),
        Err(MmuError::InvalidAlignment)
    );
    let mut invalid = page;
    invalid.physical_address = !(PAGE_SIZE as u64 - 1);
    assert_eq!(
        mmu.map(storage, TEST_ADDRESS + PAGE_SIZE, invalid),
        Err(MmuError::InvalidAddress)
    );

    let mut user = page;
    user.attributes.user_accessible = true;
    mmu.map(storage, TEST_ADDRESS + PAGE_SIZE, user).unwrap();
    assert_eq!(
        mmu.query(storage, TEST_ADDRESS + PAGE_SIZE).unwrap(),
        Some(user)
    );
    storage.remaining = 0;
    mmu.protect(storage, TEST_ADDRESS, PagePermissions::ReadOnly)
        .unwrap();
    assert_eq!(
        mmu.query(storage, TEST_ADDRESS)
            .unwrap()
            .unwrap()
            .attributes
            .permissions,
        PagePermissions::ReadOnly
    );
    mmu.unmap(storage, TEST_ADDRESS).unwrap();
    assert_eq!(mmu.query(storage, TEST_ADDRESS).unwrap(), None);
    assert_eq!(
        mmu.query(storage, TEST_ADDRESS + PAGE_SIZE).unwrap(),
        Some(user)
    );
    assert_eq!(mmu.unmap(storage, TEST_ADDRESS), Err(MmuError::NotMapped));
    mmu.unmap(storage, TEST_ADDRESS + PAGE_SIZE).unwrap();
    assert_eq!(storage.count(), 1);

    storage.remaining = 1;
    assert_eq!(
        mmu.map(storage, TEST_ADDRESS, page),
        Err(MmuError::NoMemory)
    );
    assert_eq!(storage.count(), 1);
    assert_eq!(mmu.query(storage, TEST_ADDRESS).unwrap(), None);
    storage.remaining = usize::MAX;
    mmu.map(storage, TEST_ADDRESS, page).unwrap();
    mmu.unmap(storage, TEST_ADDRESS).unwrap();
    assert_eq!(storage.count(), 1);

    let mut device = page;
    device.physical_address = UART_PHYSICAL;
    device.attributes.memory_type = MemoryType::Device;
    #[cfg(target_arch = "riscv64")]
    assert_eq!(
        mmu.map(storage, TEST_ADDRESS, device),
        Err(MmuError::Unsupported)
    );
    #[cfg(target_arch = "riscv64")]
    {
        device.attributes.permissions = PagePermissions::ReadExecute;
        assert_eq!(
            mmu.map(storage, TEST_ADDRESS, device),
            Err(MmuError::InvalidAttributes)
        );
    }
    #[cfg(target_arch = "loongarch64")]
    {
        mmu.map(storage, TEST_ADDRESS, device).unwrap();
        assert_eq!(mmu.query(storage, TEST_ADDRESS).unwrap(), Some(device));
        assert_eq!(
            mmu.protect(storage, TEST_ADDRESS, PagePermissions::ReadExecute),
            Err(MmuError::InvalidAttributes)
        );
        assert_eq!(mmu.query(storage, TEST_ADDRESS).unwrap(), Some(device));
        mmu.unmap(storage, TEST_ADDRESS).unwrap();
    }
    assert_eq!(mmu.synchronize_code(19..19), Ok(()));
    let reversed = core::ops::Range { start: 19, end: 0 };
    assert_eq!(mmu.synchronize_code(reversed), Err(MmuError::InvalidRange));
    assert_eq!(
        mmu.invalidate_page(1usize << 55),
        Err(MmuError::InvalidAddress)
    );
}

fn install_kernel(mmu: &dyn Mmu, storage: &mut Storage<'_>) {
    mmu.initialize(storage).unwrap();
    // SAFETY: 链接脚本须在 _kernel_image 存放内核映像物理范围的两个 u64。
    let image =
        unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(_kernel_image).cast::<u64>(), 2) };
    let start = image[0] as usize;
    let end = image[1] as usize;
    let text_end = core::ptr::addr_of!(_kernel_image) as usize;
    for physical_address in (start..end).step_by(PAGE_SIZE) {
        let virtual_address = physical_address.wrapping_add(KERNEL_OFFSET);
        let permissions = if virtual_address < text_end {
            PagePermissions::ReadExecute
        } else {
            PagePermissions::ReadWrite
        };
        mmu.map(
            storage,
            virtual_address,
            mapping(physical_address as u64, permissions),
        )
        .unwrap();
    }
    mmu.map(
        storage,
        UART & !(PAGE_SIZE - 1),
        PageMapping {
            physical_address: UART_PHYSICAL,
            attributes: PageAttributes {
                permissions: PagePermissions::ReadWrite,
                #[cfg(target_arch = "riscv64")]
                memory_type: MemoryType::Normal,
                #[cfg(target_arch = "loongarch64")]
                memory_type: MemoryType::Device,
                user_accessible: false,
            },
        },
    )
    .unwrap();
}

fn read_test() -> u8 {
    // SAFETY: TEST_ADDRESS 须由当前页表映射到有效数据页。
    unsafe { (TEST_ADDRESS as *const u8).read_volatile() }
}

fn publish_code(mmu: &dyn Mmu, storage: &mut Storage<'_>) {
    let address = (&raw const CODE) as usize;
    for (source, expected) in [
        (answer_42 as *const u8, 42usize),
        (answer_43 as *const u8, 43usize),
    ] {
        mmu.protect(storage, address, PagePermissions::ReadWrite)
            .unwrap();
        // SAFETY: source 须指向两个指令的测试函数，CODE 须为独占可写页。
        unsafe { core::ptr::copy_nonoverlapping(source, (&raw mut CODE).cast::<u8>(), 8) };
        mmu.protect(storage, address, PagePermissions::ReadOnly)
            .unwrap();
        mmu.synchronize_code(address..address + 8).unwrap();
        mmu.map(
            storage,
            CODE_ADDRESS,
            mapping(physical(address), PagePermissions::ReadExecute),
        )
        .unwrap();
        // SAFETY: CODE_ADDRESS 须包含已同步的无参数 C ABI 测试函数。
        let execute: extern "C" fn() -> usize = unsafe { core::mem::transmute(CODE_ADDRESS) };
        assert_eq!(execute(), expected);
        mmu.unmap(storage, CODE_ADDRESS).unwrap();
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
