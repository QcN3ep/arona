// SPDX-License-Identifier: MPL-2.0

//! 在真实硬件能力上运行内核集成测试。

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use kernel::trap::{Trap, TrapOrigin, TrapOutcome};

mod device;
mod ipc;
mod syscall;
mod task;
mod thread;
mod trap;

pub use device::{run as run_devices, run_dma_failure};
pub use syscall::{handle_program, run_device_program, run_init_program, run_linux_program};

pub trait Platform {
    fn return_values(state: &mut kernel::hardware::execution::UserState, values: [usize; 3]);
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
    trap::run::<P>();
}

pub fn run_program<P: Platform>() {
    kernel::thread::scheduler::initialize().unwrap();
    syscall::run_program(P::program().expect("runtime test image is unavailable"));
}

pub fn run_process<P: Platform>() {
    kernel::thread::scheduler::initialize().unwrap();
    syscall::run_process(P::program().expect("runtime test image is unavailable"));
}

pub fn handle(trap: Trap) -> TrapOutcome {
    if syscall::active() || trap::active() {
        return kernel::trap::dispatch(trap);
    }
    match trap.origin {
        TrapOrigin::User => thread::handle_user(trap),
        TrapOrigin::Kernel => thread::handle(trap),
    }
}

fn run_thread(entry: kernel::thread::ThreadEntry) {
    use kernel::thread::{Thread, ThreadState, scheduler};
    let owner = Thread::new(entry, 0).unwrap();
    owner.start().unwrap();
    // 测试超时由外层 QEMU runner 控制。
    loop {
        scheduler::run_ready().unwrap();
        if let ThreadState::Dead(status) = owner.state() {
            assert_eq!(status, 0);
            assert!(!owner.is_waiting_for_exit());
            break;
        }
        kernel::hardware::cpu().idle_until_interrupt();
    }
}
