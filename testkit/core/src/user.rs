// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Run registered cases using the initial Task's device capability.

use crate::{Case, CaseKind, perf::Perf};
use core::fmt::{self, Write};
use runtime::{Channel, DeviceSet, Handle, Rights, device::Mmio, ipc::WaitMode};

static DEVICES: spin::Once<Handle<DeviceSet>> = spin::Once::new();

pub fn devices() -> &'static Handle<DeviceSet> {
    DEVICES.get().expect("test runner has not started")
}

pub fn run(cases: &[&Case], bootstrap: Handle<Channel>) {
    let devices = DEVICES.call_once(|| {
        let mut startup = bootstrap.receive(WaitMode::Forever).unwrap();
        assert_eq!(startup.tag, 0);
        startup.take_handle(1).unwrap().downcast().unwrap()
    });
    let mut cursor = 0;
    let id = loop {
        let id = devices.next(cursor).unwrap().expect("no boot console");
        if devices
            .property(id, "console", "selected", &mut [0; 1])
            .unwrap()
            .is_some()
        {
            break id;
        }
        cursor = id;
    };
    let group = devices.describe(id).unwrap().group.unwrap();
    let device = devices
        .claim_group(group, Rights::READ | Rights::MAP)
        .unwrap();
    let mut console = Console(device.map_mmio(id, 0).unwrap());
    for case in cases {
        let kind = match case.kind {
            CaseKind::Perf(_) => "perf",
            CaseKind::Test { .. } => panic!("user runner requires performance cases"),
        };
        writeln!(console, "KTEST CASE {}\t{kind}", case.name).unwrap();
    }
    writeln!(console, "KTEST READY").unwrap();
    let mut input = [0; 256];
    let mut length = 0;
    loop {
        let byte = console.read();
        if byte == b'\n' {
            break;
        }
        assert!(length < input.len(), "test selection is too long");
        input[length] = byte;
        length += 1;
    }
    let selection = core::str::from_utf8(&input[..length]).unwrap();
    if selection == "--list" {
        writeln!(console, "KTEST LISTED").unwrap();
        return;
    }
    let (name, count) = selection.split_once('\t').expect("missing iteration count");
    let iterations = count.parse().expect("invalid iteration count");
    let case = cases
        .iter()
        .find(|case| case.name == name)
        .expect("unknown performance case");
    let CaseKind::Perf(run) = case.kind else {
        unreachable!()
    };
    writeln!(console, "KTEST RUN {name}").unwrap();
    let mut perf = Perf::new(iterations, start, end);
    run(&mut perf);
    perf.finish();
    writeln!(console, "KPERF UNITS {iterations}\nKTEST PASS {name}").unwrap();
}

#[inline(never)]
pub extern "C" fn start() {
    core::hint::black_box(0x6b70_6572_6673_u64);
}

#[inline(never)]
pub extern "C" fn end() {
    core::hint::black_box(0x6b70_6572_6665_u64);
}

struct Console<'a>(Mmio<'a>);

impl Console<'_> {
    fn read(&mut self) -> u8 {
        while self.0.read_u8(5).unwrap() & 1 == 0 {
            core::hint::spin_loop();
        }
        self.0.read_u8(0).unwrap()
    }
}

impl Write for Console<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            while self.0.read_u8(5).map_err(|_| fmt::Error)? & 0x20 == 0 {
                core::hint::spin_loop();
            }
            self.0.write_u8(0, byte).map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}
