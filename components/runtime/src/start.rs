// SPDX-License-Identifier: MPL-2.0

use crate::{
    process::{self, ProcessStart},
    tls::TlsBlock,
};

#[lang = "termination"]
pub trait Termination {
    fn report(self) -> usize;
}

impl Termination for () {
    fn report(self) -> usize {
        0
    }
}

impl Termination for usize {
    fn report(self) -> usize {
        self
    }
}

impl<T: Termination, E: core::fmt::Debug> Termination for Result<T, E> {
    fn report(self) -> usize {
        self.map_or(1, Termination::report)
    }
}

#[lang = "start"]
fn lang_start<T: Termination + 'static>(
    main: fn() -> T,
    _argc: isize,
    _argv: *const *const u8,
    _sigpipe: u8,
) -> isize {
    main().report() as isize
}

unsafe extern "C" {
    fn main(argc: isize, argv: *const *const u8) -> isize;
}

/// # Safety
///
/// argument 须指向有效的 ProcessStart 编码，启动方须独占移交其中的句柄及当前进程。
unsafe extern "C" fn enter(argument: *const [u8; ProcessStart::SIZE]) -> ! {
    // SAFETY: argument 须为启动方提供的可读交接记录。
    let start = ProcessStart::decode(&unsafe { argument.read_unaligned() });
    // SAFETY: 本入口须是当前进程唯一的初始化执行者。
    if unsafe { process::current::initialize(start) }.is_err() {
        process::exit(1);
    }
    let tls = TlsBlock::new().unwrap_or_else(|_| process::exit(1));
    // SAFETY: 当前线程尚未访问 TLS，tls 须存活至线程停止访问 TLS。
    unsafe { tls.activate() };
    // SAFETY: 编译器生成的 main 须遵循 Rust start 的 C 入口约定。
    let status = unsafe { main(0, core::ptr::null()) };
    crate::tls::destroy();
    drop(tls);
    process::exit(status as usize)
}

/// # Safety
///
/// SP 须指向独占的有效用户栈，a0 须满足 enter 的启动交接约定。
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        ".option push",
        ".option norelax",
        "lla gp, __global_pointer$",
        ".option pop",
        "tail {enter}",
        enter = sym enter,
    );
}

/// # Safety
///
/// SP 须指向独占的有效用户栈，a0 须满足 enter 的启动交接约定。
#[cfg(target_arch = "loongarch64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!("b {enter}", enter = sym enter);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    process::exit(1)
}

#[alloc_error_handler]
fn allocation_failed(_: core::alloc::Layout) -> ! {
    process::exit(1)
}
