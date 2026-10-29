// SPDX-License-Identifier: MPL-2.0

//! 在真实硬件能力上运行内核集成测试。

#![no_std]
#![forbid(unsafe_code)]

use kernel::trap::{Trap, TrapOrigin, TrapOutcome};

mod device;
mod ipc;
mod syscall;
mod task;
mod thread;

pub use device::run as run_devices;
pub use syscall::run_device_program;

pub trait Platform {
    fn set_floating_enabled(enabled: bool);
    fn syscall_code() -> &'static [u64];
    fn user_code() -> &'static [u64];
    fn program() -> Option<&'static [u8]> {
        None
    }
}

pub fn run<P: Platform>() {
    thread::run::<P>();
    task::run();
    ipc::run();
    syscall::run(P::syscall_code());
    thread::run_user(P::user_code(), P::set_floating_enabled);
}

pub fn run_program<P: Platform>() {
    kernel::thread::scheduler::initialize().unwrap();
    syscall::run_program(P::program().expect("runtime test image is unavailable"));
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

fn run_thread(entry: kernel::thread::ThreadEntry) {
    use kernel::{
        hardware,
        thread::{Thread, ThreadState, scheduler},
    };
    let owner = Thread::new(entry, 0).unwrap();
    owner.start().unwrap();
    let limit = hardware::timer().now_ns().unwrap() + 10_000_000_000;
    loop {
        scheduler::run_ready().unwrap();
        if owner.state() == ThreadState::Dead(0) {
            break;
        }
        assert!(
            hardware::timer().now_ns().unwrap() < limit,
            "Test thread stalled"
        );
        hardware::cpu().idle_until_interrupt();
    }
}
