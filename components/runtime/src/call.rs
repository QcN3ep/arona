// SPDX-License-Identifier: MPL-2.0

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Kernel(abi::Error),
    Protocol,
}

pub(crate) fn invoke(
    handle: abi::Handle,
    method: u64,
    arguments: [u64; 5],
) -> Result<[u64; 2], Error> {
    check(invoke_return(handle, method, arguments))
}

pub(crate) fn invoke_return(handle: abi::Handle, method: u64, arguments: [u64; 5]) -> abi::Return {
    // SAFETY: The caller must maintain buffer validity and output exclusivity during the trap.
    unsafe { raw(handle, method, arguments) }
}

pub(crate) fn check(result: abi::Return) -> Result<[u64; 2], Error> {
    if result.status == 0 {
        Ok(result.values)
    } else {
        Err(abi::Error::from_raw(result.status)
            .map(Error::Kernel)
            .unwrap_or(Error::Protocol))
    }
}

/// # Safety
///
/// The buffer must remain valid during the call, and the output range must be exclusive and writable.
#[cfg(target_arch = "riscv64")]
unsafe fn raw(handle: abi::Handle, mut method: u64, arguments: [u64; 5]) -> abi::Return {
    let mut first = arguments[0];
    let mut second = arguments[1];
    // SAFETY: The caller must satisfy the buffer validity and aliasing requirements, and the Trap must preserve the remaining registers.
    unsafe {
        core::arch::asm!("ecall",
            in("a7") handle.raw(), inlateout("a0") method,
            inlateout("a1") first, inlateout("a2") second,
            in("a3") arguments[2], in("a4") arguments[3], in("a5") arguments[4],
            options(nostack));
    }
    abi::Return {
        status: method,
        values: [first, second],
    }
}

/// # Safety
///
/// The buffer must remain valid during the call, and the output range must be exclusive and writable.
#[cfg(target_arch = "loongarch64")]
unsafe fn raw(handle: abi::Handle, mut method: u64, arguments: [u64; 5]) -> abi::Return {
    let mut first = arguments[0];
    let mut second = arguments[1];
    // SAFETY: The caller must satisfy the buffer validity and aliasing requirements, and the Trap must preserve the remaining registers.
    unsafe {
        core::arch::asm!("syscall 0",
            in("$a7") handle.raw(), inlateout("$a0") method,
            inlateout("$a1") first, inlateout("$a2") second,
            in("$a3") arguments[2], in("$a4") arguments[3], in("$a5") arguments[4],
            options(nostack));
    }
    abi::Return {
        status: method,
        values: [first, second],
    }
}

/// # Safety
///
/// Only used for build targets that do not yet implement a native trap entry.
#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
unsafe fn raw(_handle: abi::Handle, _method: u64, _arguments: [u64; 5]) -> abi::Return {
    abi::Return::failure(abi::Error::Unsupported)
}
