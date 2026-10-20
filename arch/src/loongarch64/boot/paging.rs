// SPDX-License-Identifier: MPL-2.0

//! 临时启动页表、处理器映射配置与高半区切换。

const KERNEL_BASE: usize = 0xffff_8000_0020_0000;
pub(super) const MMIO_BASE: usize = 0x8000_0000_0000_0000;
pub(super) const FIRMWARE_BASE: usize = 0x9000_0000_0000_0000;

// REFERENCE: https://loongson.github.io/LoongArch-Documentation/LoongArch-Vol1-EN.html
const VALID: usize = 1;
const DIRTY: usize = 1 << 1;
const CACHED: usize = 1 << 4;
const HUGE: usize = 1 << 6;

#[repr(C, align(4096))]
struct PageTable {
    entries: [usize; 512],
}

#[repr(C)]
struct PageTables {
    root: PageTable,
    kernel_l2: PageTable,
    kernel_l1: PageTable,
}

#[used]
#[unsafe(link_section = ".boot_tables")]
static mut TABLES: PageTables = PageTables {
    root: PageTable::new(),
    kernel_l2: PageTable::new(),
    kernel_l1: PageTable::new(),
};

unsafe extern "C" {
    static mut _boot_tables_phys: PageTables;
    static _kernel_phys_start: u8;
}

/// # Safety
///
/// 处理器须关闭中断并在物理地址执行，页表与启动栈须可独占读写。
#[unsafe(link_section = ".text.entry.rust")]
pub(super) unsafe extern "C" fn enter(system_table: usize) -> ! {
    let start = core::ptr::addr_of!(_kernel_phys_start) as usize;

    // SAFETY: 链接脚本定义的页表区域须按页对齐且可独占写入。
    let tables = unsafe { &mut *core::ptr::addr_of_mut!(_boot_tables_phys) };
    tables.init(start);

    let entry: usize;
    let trap: usize;
    // SAFETY: 目标符号须是链接脚本提供的高半区入口。
    unsafe {
        core::arch::asm!(
            "la.abs {entry}, {virtual_entry}",
            "la.abs {trap}, {trap_entry}",
            entry = out(reg) entry,
            trap = out(reg) trap,
            virtual_entry = sym super::_start_virtual,
            trap_entry = sym super::_boot_trap,
            options(nomem, nostack),
        );
    }

    // SAFETY: 处理器须关闭中断，页表与物理 refill 入口须可用。
    unsafe {
        write_csr::<0x1a>(&raw const tables.root as usize); // PGDH
        write_csr::<0x19>(0); // PGDL
        write_csr::<0x18>(0); // ASID
        write_csr::<0x1c>(12 | (9 << 5) | (21 << 10) | (9 << 15) | (30 << 20) | (9 << 25)); // PWCL
        write_csr::<0x1d>(39 | (9 << 6)); // PWCH
        write_csr::<0x1e>(12); // STLBPS
        write_csr::<0x8e>(12); // TLBREHI.PS
        write_csr::<0x88>(super::_boot_tlb_refill as *const () as usize); // TLBRENTRY
        write_csr::<0xc>(trap); // EENTRY
        write_csr::<0x180>(0x11); // 低地址 DMW 维持切换期间的取指。
        write_csr::<0x181>(MMIO_BASE | 1);
        write_csr::<0x182>(FIRMWARE_BASE | 0x11);
        write_csr::<0x183>(0);

        core::arch::asm!(
            "invtlb 0, $zero, $zero",
            "dbar 0",
            "csrwr $t0, 0x0",
            "jirl $zero, $t1, 0",
            in("$t0") 0xb0usize, // PG=1，DA=0，IE=0，缓存属性为 CC。
            in("$t1") entry,
            in("$a0") system_table,
            options(noreturn),
        );
    }
}

impl PageTables {
    #[inline(always)]
    fn init(&mut self, kernel: usize) {
        self.root.clear();
        self.kernel_l2.clear();
        self.kernel_l1.clear();

        self.root.link::<3>(KERNEL_BASE, &self.kernel_l2);
        self.kernel_l2.link::<2>(KERNEL_BASE, &self.kernel_l1);
        // 启动映像暂以一个可读写执行的 2 MiB 大页映射。
        self.kernel_l1.map(KERNEL_BASE, kernel);
    }
}

impl PageTable {
    const fn new() -> Self {
        Self { entries: [0; 512] }
    }

    #[inline(always)]
    fn clear(&mut self) {
        let mut index = 0usize;
        while index < self.entries.len() {
            self.write(index, 0);
            index = index.wrapping_add(1);
        }
    }

    #[inline(always)]
    fn link<const LEVEL: usize>(&mut self, virtual_address: usize, next: &Self) {
        self.write(
            virtual_address >> (12 + 9 * LEVEL),
            next as *const Self as usize,
        );
    }

    #[inline(always)]
    fn map(&mut self, virtual_address: usize, physical: usize) {
        self.write(
            virtual_address >> 21,
            physical | VALID | DIRTY | CACHED | HUGE,
        );
    }

    #[inline(always)]
    fn write(&mut self, index: usize, value: usize) {
        let entry = self.entries.as_mut_ptr().wrapping_add(index & 511);
        // SAFETY: entry 位于独占页表内；内联存储避免分页前调用高地址的 core 调试检查。
        unsafe {
            core::arch::asm!(
                "st.d {value}, {entry}, 0",
                entry = in(reg) entry,
                value = in(reg) value,
                options(nostack),
            );
        }
    }
}

/// # Safety
///
/// 处理器须允许访问 CSR，value 须符合该寄存器的配置约束。
#[inline(always)]
unsafe fn write_csr<const CSR: u32>(value: usize) {
    // SAFETY: CSR 须可写，value 须合法。
    unsafe {
        core::arch::asm!(
            "csrwr {value}, {csr}",
            value = inout(reg) value => _,
            csr = const CSR,
            options(nostack),
        );
    }
}
