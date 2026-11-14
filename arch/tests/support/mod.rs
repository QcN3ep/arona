// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Select and report one test case over the boot console.

use arch::device::uart::Uart;
use testkit::{Case, CaseKind, perf::Perf};

mod perf;

static CURRENT: spin::Once<&'static str> = spin::Once::new();

#[cfg(target_arch = "riscv64")]
const UART: usize = 0xffff_ffc0_0020_0000;
#[cfg(target_arch = "loongarch64")]
const UART: usize = 0xffff_8000_0060_01e0;

pub fn run(cases: &[&Case]) {
    let uart = uart();
    for case in cases {
        uart.write(format_args!("KTEST CASE {}", case.name))
            .unwrap();
        match case.kind {
            CaseKind::Test { expected, .. } => {
                uart.write(format_args!("\ttest")).unwrap();
                for message in expected {
                    uart.write(format_args!("\t{message}")).unwrap();
                }
            }
            CaseKind::Perf(_) => uart.write(format_args!("\tperf")).unwrap(),
        }
        uart.write(format_args!("\n")).unwrap();
    }
    uart.write(format_args!("KTEST READY\n")).unwrap();
    let mut input = [0; 256];
    let mut length = 0;
    loop {
        // SAFETY: The boot UART is mapped and exclusively read by the test harness.
        let byte = unsafe {
            while ((UART + 5) as *const u8).read_volatile() & 1 == 0 {
                core::hint::spin_loop();
            }
            (UART as *const u8).read_volatile()
        };
        if byte == b'\n' {
            break;
        }
        assert!(length < input.len(), "test selection is too long");
        input[length] = byte;
        length += 1;
    }
    let selection = core::str::from_utf8(&input[..length]).unwrap();
    let (name, iterations) =
        selection
            .split_once('\t')
            .map_or((selection, None), |(name, count)| {
                (
                    name,
                    Some(count.parse::<u64>().expect("invalid iteration count")),
                )
            });
    if name == "--list" {
        uart.write(format_args!("KTEST LISTED\n")).unwrap();
        return;
    }
    let case = cases
        .iter()
        .find(|case| case.name == name)
        .expect("unknown test case");
    uart.write(format_args!("KTEST RUN {}\n", case.name))
        .unwrap();
    CURRENT.call_once(|| case.name);
    match case.kind {
        CaseKind::Test { run, expected } => {
            assert!(
                iterations.is_none(),
                "functional tests do not take iterations"
            );
            run();
            assert!(
                expected.is_empty(),
                "expected fatal diagnostic was not raised"
            );
        }
        CaseKind::Perf(run) => {
            let iterations = iterations.expect("performance case requires iterations");
            let mut perf = Perf::new(iterations, perf::start, perf::end);
            run(&mut perf);
            perf.finish();
            uart.write(format_args!("KPERF UNITS {iterations}\n"))
                .unwrap();
        }
    }
    pass();
}

pub fn pass() {
    uart()
        .write(format_args!("KTEST PASS {}\n", CURRENT.get().unwrap()))
        .unwrap();
}

fn uart() -> Uart {
    // SAFETY: Every caller runs after boot console initialization on a single CPU.
    unsafe { Uart::new(UART) }
}
