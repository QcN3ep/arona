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
    // SAFETY: 调用方须维持陷入期间的缓冲区有效性与输出独占性。
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
/// 缓冲区须在调用期间保持有效，输出范围须独占且可写。
#[cfg(target_arch = "riscv64")]
unsafe fn raw(handle: abi::Handle, mut method: u64, arguments: [u64; 5]) -> abi::Return {
    let mut first = arguments[0];
    let mut second = arguments[1];
    // SAFETY: 调用方须满足缓冲区有效性和别名要求，Trap 须保留其余寄存器。
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
/// 缓冲区须在调用期间保持有效，输出范围须独占且可写。
#[cfg(target_arch = "loongarch64")]
unsafe fn raw(handle: abi::Handle, mut method: u64, arguments: [u64; 5]) -> abi::Return {
    let mut first = arguments[0];
    let mut second = arguments[1];
    // SAFETY: 调用方须满足缓冲区有效性和别名要求，Trap 须保留其余寄存器。
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
/// 仅用于尚未实现原生陷入入口的构建目标。
#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
unsafe fn raw(_handle: abi::Handle, _method: u64, _arguments: [u64; 5]) -> abi::Return {
    abi::Return::failure(abi::Error::Unsupported)
}
