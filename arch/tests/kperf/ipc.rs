// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register IPC measurements on the real scheduler and architecture backends.

use kernel::hardware;
use testkit::{ipc::perf as cases, perf::Perf};

fn run(perf: &mut Perf, case: fn(&mut Perf)) {
    super::setup::initialize_memory();
    unsafe extern "C" {
        static _boot_stack_bottom: u8;
        static _boot_stack_top: u8;
    }
    hardware::trap()
        .install(
            kernel::memory::VirtRange {
                start: core::ptr::addr_of!(_boot_stack_bottom) as usize,
                end: core::ptr::addr_of!(_boot_stack_top) as usize,
            },
            kernel::trap::dispatch,
        )
        .unwrap();
    #[cfg(target_arch = "riscv64")]
    let frequency = {
        let firmware = super::FIRMWARE.load(core::sync::atomic::Ordering::Relaxed);
        let pointer = (0xffff_ffc0_0060_0000 + (firmware & 0x1f_ffff)) as *const u32;
        // SAFETY: The boot branch retains the firmware's immutable DTB mapping.
        let tree = unsafe { fdt::Fdt::from_ptr_fallible(pointer) }.unwrap();
        tree.root()
            .unwrap()
            .cpus()
            .unwrap()
            .common_timebase_frequency()
            .unwrap()
    };
    #[cfg(target_arch = "loongarch64")]
    let frequency = None;
    hardware::timer().initialize(frequency).unwrap();
    kernel::thread::scheduler::initialize().unwrap();
    cases::run(perf, case);
}

mod channel {
    use super::*;
    #[testkit::kperf]
    fn roundtrip(perf: &mut Perf) {
        run(perf, cases::channel_roundtrip);
    }
}

mod task {
    use super::*;
    #[testkit::kperf]
    fn roundtrip(perf: &mut Perf) {
        run(perf, cases::task_roundtrip);
    }
}

mod transfer {
    use super::*;
    #[testkit::kperf]
    fn handles_1(perf: &mut Perf) {
        run(perf, cases::transfer::<1>);
    }
    #[testkit::kperf]
    fn handles_8(perf: &mut Perf) {
        run(perf, cases::transfer::<8>);
    }
}

mod call {
    use super::*;
    #[testkit::kperf]
    fn ready(perf: &mut Perf) {
        run(perf, cases::call_ready);
    }
    #[testkit::kperf]
    fn blocked(perf: &mut Perf) {
        run(perf, cases::call_blocked);
    }
}

mod scheduler {
    use super::*;
    #[testkit::kperf]
    fn yield_roundtrip(perf: &mut Perf) {
        run(perf, cases::yield_roundtrip);
    }
}

mod wait_many {
    use super::*;
    #[testkit::kperf]
    fn ready_1(perf: &mut Perf) {
        run(perf, cases::wait_ready::<1>);
    }
    #[testkit::kperf]
    fn ready_16(perf: &mut Perf) {
        run(perf, cases::wait_ready::<16>);
    }
    #[testkit::kperf]
    fn ready_256(perf: &mut Perf) {
        run(perf, cases::wait_ready::<256>);
    }
    #[testkit::kperf]
    fn blocked_1(perf: &mut Perf) {
        run(perf, cases::wait_blocked::<1>);
    }
    #[testkit::kperf]
    fn blocked_16(perf: &mut Perf) {
        run(perf, cases::wait_blocked::<16>);
    }
}
