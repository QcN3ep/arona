// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod service;

use fs::archive::Archive;
use linux::{Control, Error, ExitStatus, Program, elf::Machine};
use runtime::{
    DeviceSet, Handle,
    ipc::{Message, WaitMode},
    process,
};

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let mut startup = bootstrap.receive(WaitMode::Forever).unwrap();
    assert_eq!(startup.tag, 0);
    assert_eq!(startup.handles().len(), 3);
    let length = startup.words()[0] as usize;
    let archive =
        Archive::new(startup.take_handle(0).unwrap().downcast().unwrap(), length).unwrap();
    let devices = startup
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    drop(startup);
    let native = service::read(&archive, "/bin/linux");
    let machine = Machine::from_header(&native).unwrap();
    let fs = service::read(&archive, "/bin/fs");
    let devmgr = service::read(&archive, "/bin/devmgr");
    let (_filesystem, mounts, _files) = service::filesystem(archive, &fs, machine);
    let (_devices, device_service) = service::devices(&devmgr, machine, &devices);
    let (_linux, channel) = service::launch(&native, machine, linux::service::REQUIRED_RIGHTS);
    let linux = Control::start(
        process::task(),
        channel,
        mounts.connect(service::deadline()).unwrap(),
        service::deadline(),
    )
    .unwrap();
    let console = console(&devices);
    let output = || {
        devmgr::uart::Session::open(
            process::task(),
            &device_service,
            console,
            devmgr::uart::Access::Output,
        )
        .unwrap()
    };
    execution(&linux, &output);
    layouts(&linux, &output);
    malformed(&linux, &output);
    let output = output();
    output.write_all(b"Linux program tests passed\n").unwrap();
    output.flush().unwrap();
    bootstrap
        .send(&mut Message::new(73, &[37, 41]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn until() -> WaitMode {
    WaitMode::Until(service::deadline())
}

fn console(devices: &Handle<DeviceSet>) -> u32 {
    let mut cursor = 0;
    loop {
        let id = devices.next(cursor).unwrap().unwrap();
        if devices.describe(id).unwrap().console {
            return id;
        }
        cursor = id;
    }
}

fn execution(linux: &Control<'_>, output: &impl Fn() -> devmgr::uart::Session) {
    let start = |mode: &[u8]| {
        linux
            .start_run(
                Program {
                    path: b"/bin/linux-test",
                    arguments: &[b"/bin/linux-test", mode],
                    environment: &[b"ARONA_TEST=yes"],
                },
                output(),
            )
            .unwrap()
    };
    let mut first = start(b"normal");
    let mut second = start(b"normal");
    assert_eq!(
        first.wait(until()).unwrap(),
        ExitStatus {
            code: 42,
            signal: 0
        }
    );
    assert_eq!(
        second.wait(until()).unwrap(),
        ExitStatus {
            code: 42,
            signal: 0
        }
    );
    let mut fault = start(b"fault");
    let mut survivor = start(b"normal");
    assert_eq!(
        fault.wait(until()).unwrap(),
        ExitStatus {
            code: 0,
            signal: 11
        }
    );
    assert_eq!(
        survivor.wait(until()).unwrap(),
        ExitStatus {
            code: 42,
            signal: 0
        }
    );
    assert_eq!(
        start(b"readonly").wait(until()).unwrap(),
        ExitStatus {
            code: 0,
            signal: 11
        }
    );
    assert_eq!(
        start(b"illegal").wait(until()).unwrap(),
        ExitStatus { code: 0, signal: 4 }
    );
    assert_eq!(
        start(b"group").wait(until()).unwrap(),
        ExitStatus {
            code: 43,
            signal: 0
        }
    );
    for mode in [
        b"shrunk".as_slice(),
        b"xheap",
        b"0-none-read",
        b"1-unmapped",
        b"2-readonly",
        b"3-nonexec",
        b"4-none-write",
        b"5-none-execute",
    ] {
        assert_eq!(
            start(mode).wait(until()).unwrap(),
            ExitStatus {
                code: 0,
                signal: 11
            }
        );
    }
}

fn layouts(linux: &Control<'_>, output: &impl Fn() -> devmgr::uart::Session) {
    for path in [
        b"/bin/linux-compact".as_slice(),
        b"/bin/linux-separate-headers",
    ] {
        assert_eq!(
            linux
                .run(
                    Program {
                        path,
                        arguments: &[path, b"normal"],
                        environment: &[b"ARONA_TEST=yes"],
                    },
                    output(),
                    until()
                )
                .unwrap(),
            ExitStatus {
                code: 42,
                signal: 0
            }
        );
    }
}

fn malformed(linux: &Control<'_>, output: &impl Fn() -> devmgr::uart::Session) {
    let paths: &[&[u8]] = &[
        b"/bin/linux-truncated",
        b"/bin/linux-foreign",
        b"/bin/linux-writable-code",
    ];
    for path in paths {
        assert_eq!(
            linux.run(
                Program {
                    path,
                    arguments: &[path, b"normal"],
                    environment: &[b"ARONA_TEST=yes"],
                },
                output(),
                until()
            ),
            Err(Error::InvalidExecutable)
        );
    }
    assert_eq!(
        linux
            .run(
                Program {
                    path: b"/bin/linux-test",
                    arguments: &[b"/bin/linux-test", b"normal"],
                    environment: &[b"ARONA_TEST=yes"],
                },
                output(),
                until()
            )
            .unwrap(),
        ExitStatus {
            code: 42,
            signal: 0
        }
    );
}
