// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Measure supervision from one illegal instruction to the next, including register access.

use runtime::{ObjectType, Rights, ipc::WaitMode, memory::Permissions, process, thread};
use testkit::{kperf, perf::Perf};

fn roundtrip(perf: &mut Perf, floating: bool) {
    let port = process::task()
        .trap_port(ObjectType::TrapPort.supported_rights())
        .unwrap();
    let child = process::SupervisedTask::new(process::task(), &port).unwrap();
    let code = process::task()
        .allocate(1, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)
        .unwrap();
    let stack = process::task()
        .allocate(2, Rights::READ_WRITE | Rights::MAP)
        .unwrap();
    let text = child
        .task()
        .map(&code, 0, 1, None, Permissions::ReadExecute)
        .unwrap();
    let stack = child
        .task()
        .map(&stack, 0, 2, None, Permissions::ReadWrite)
        .unwrap();
    let entry = text.range().unwrap().start;
    let worker = child
        .create_thread(
            thread::UserEntry {
                pc: entry as u64,
                stack_pointer: stack.range().unwrap().end as u64,
                thread_pointer: 0,
                argument: 0,
            },
            ObjectType::Thread.supported_rights(),
        )
        .unwrap();
    worker.start().unwrap();
    let mut trap = Some(port.receive(WaitMode::Forever).unwrap());
    perf.iter(|| {
        let current = trap.take().unwrap();
        let state = if floating {
            current.state().unwrap()
        } else {
            current.general_state().unwrap()
        };
        if floating {
            current.resume(&state).unwrap();
        } else {
            let bytes = state.general().encode();
            current
                .resume_registers(&[runtime::register::Write {
                    group: abi::register::GENERAL,
                    bytes: &bytes,
                }])
                .unwrap();
        }
        trap = Some(port.receive(WaitMode::Forever).unwrap());
    });
    let trap = trap.unwrap();
    assert_eq!(trap.info.kind, runtime::trap::TrapInfo::ILLEGAL_INSTRUCTION);
    assert_eq!(trap.general_state().unwrap().pc, entry);
    drop(trap);
    assert_eq!(
        worker
            .wait(thread::now_ns().unwrap() + 2_000_000_000)
            .unwrap(),
        usize::MAX
    );
}

#[kperf]
fn general_roundtrip(perf: &mut Perf) {
    roundtrip(perf, false);
}
#[kperf]
fn floating_roundtrip(perf: &mut Perf) {
    roundtrip(perf, true);
}
