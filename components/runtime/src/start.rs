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
/// argument must point to a valid ProcessStart encoding, and the launcher must exclusively transfer the handles it contains and the current process.
unsafe extern "C" fn enter(argument: *const [u8; ProcessStart::SIZE]) -> ! {
    // SAFETY: argument must be a readable handover record provided by the launcher.
    let start = ProcessStart::decode(&unsafe { argument.read_unaligned() });
    // SAFETY: This entry point must be the sole executor of initialization for the current process.
    if unsafe { process::current::initialize(start) }.is_err() {
        process::exit(1);
    }
    let tls = TlsBlock::new().unwrap_or_else(|_| process::exit(1));
    // SAFETY: The current thread has not yet accessed TLS, and tls must remain alive until the thread stops accessing TLS.
    unsafe { tls.activate() };
    // SAFETY: The compiler-generated main must follow the C entry convention of the Rust start.
    let status = unsafe { main(0, core::ptr::null()) };
    crate::tls::destroy();
    drop(tls);
    process::exit(status as usize)
}

/// # Safety
///
/// SP must point to an exclusive valid user stack, and a0 must satisfy the startup handover convention of enter.
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
/// SP must point to an exclusive valid user stack, and a0 must satisfy the startup handover convention of enter.
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
