// SPDX-License-Identifier: MPL-2.0

//! 在真实硬件能力上运行内核集成测试。

#![no_std]
#![forbid(unsafe_code)]

use kernel::trap::{Trap, TrapOrigin, TrapOutcome};

mod ipc;
mod syscall;
mod task;
mod thread;

pub trait Platform {
    fn set_floating_enabled(enabled: bool);
    fn syscall_code() -> &'static [u64];
    fn user_code() -> &'static [u64];
}

pub fn run<P: Platform>() {
    thread::run::<P>();
    task::run();
    ipc::run();
    syscall::run(P::syscall_code());
    thread::run_user(P::user_code(), P::set_floating_enabled);
}

pub fn handle(trap: Trap) -> TrapOutcome {
    if syscall::active() {
        return kernel::trap::dispatch(trap);
    }
    match trap.origin {
        TrapOrigin::User => thread::handle_user(trap),
        TrapOrigin::Kernel => thread::handle(trap),
    }
}
