// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Run kernel integration tests on real hardware capabilities.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use kernel::trap::{Trap, TrapOrigin, TrapOutcome};

pub use ktest::ktest;

mod device;
mod ipc;
mod memory;
mod syscall;
mod task;
mod thread;
mod trap;

pub use device::{run as run_devices, run_dma_failure};
pub use ipc::run as run_ipc;
pub use memory::concurrent as run_concurrent_memory;
pub use memory::{run as run_memory, validate_regions as validate_memory_regions};
pub use syscall::run as run_syscalls;
pub use syscall::{handle_program, run_device_program, run_init_program, run_service_program};
pub use task::run as run_tasks;
pub use thread::{run as run_threads, run_user as run_user_threads};
pub use trap::run as run_traps;

pub trait Platform {
    fn return_values(state: &mut kernel::hardware::execution::UserState, values: [usize; 3]);
    fn set_floating_enabled(enabled: bool);
    fn syscall_code() -> &'static [u64];
    fn user_code() -> &'static [u64];
    fn program() -> Option<&'static [u8]> {
        None
    }
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
    // Test timeouts are controlled by the outer QEMU runner.
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
