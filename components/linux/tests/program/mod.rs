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

const WRITE_TIMEOUT_NS: u64 = 120_000_000_000;

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
    let (_devices, device_service) = service::devices(&devmgr, machine, &devices);
    let (_filesystem, mounts, files) = service::filesystem(archive, &fs, machine);
    service::mount(&mounts, &device_service);
    let (_linux, channel) = service::launch(&native, machine, linux::service::REQUIRED_RIGHTS);
    let services = service::resources(&device_service);
    let linux = Control::start_with_services(
        process::task(),
        channel,
        mounts.connect(service::deadline()).unwrap(),
        services,
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
    writable(&linux, &files, &output);
    terminal(&linux, &output, &device_service, console);
    mounts.unmount(b"/var", service::deadline()).unwrap();
    let output = output();
    output.write_all(b"Linux program tests passed\n").unwrap();
    output.flush().unwrap();
    drop(output);
    drop(linux);
    _linux.wait();
    drop(files);
    drop(mounts);
    _filesystem.wait();
    drop(device_service);
    _devices.wait();
    bootstrap
        .send(&mut Message::new(73, &[37, 41]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn writable(
    linux: &Control<'_>,
    files: &fs::Client<'_>,
    output: &impl Fn() -> devmgr::uart::Session,
) {
    for path in [
        b"/var/linux-write".as_slice(),
        b"/var/linux-new",
        b"/var/linux-append",
        b"/var/linux-vector",
        b"/var/linux-stat-link",
        b"/var/linux-stat-symlink",
        b"/var/linux-stat",
    ] {
        match files.unlink_bytes(path, service::deadline()) {
            Ok(()) | Err(fs::Error::NotFound) => {}
            Err(error) => panic!("test cleanup failed: {error:?}"),
        }
    }
    let run = |mode: &[u8]| {
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
    metadata_fixture(files);
    for mode in [
        b"metadata".as_slice(),
        b"directory",
        b"path",
        b"FileMappings",
        b"Pipe",
        b"Readiness",
        b"System",
        b"HardwareThreads",
        b"Join",
        b"Kin",
        b"Exec",
        b"writable",
        b"vector",
    ] {
        let result = run(mode).wait(WaitMode::Until(
            runtime::thread::now_ns().unwrap() + WRITE_TIMEOUT_NS,
        ));
        let expected = ExitStatus {
            code: 42,
            signal: 0,
        };
        if result != Ok(expected) {
            let output = output();
            output
                .write_all(alloc::format!("Linux scenario {mode:?}: {result:?}\n").as_bytes())
                .unwrap();
            output.flush().unwrap();
        }
        assert_eq!(result.unwrap(), expected);
    }
    assert_eq!(
        run(b"MappingBus")
            .wait(WaitMode::Until(
                runtime::thread::now_ns().unwrap() + WRITE_TIMEOUT_NS
            ))
            .unwrap(),
        ExitStatus { code: 0, signal: 7 }
    );
    assert_eq!(
        files
            .metadata(b"/var/linux-new", service::deadline())
            .unwrap()
            .mode
            & 0o777,
        0o600
    );
    append_records(files, &run);
}

fn append_records(
    files: &fs::Client<'_>,
    run: &impl Fn(&[u8]) -> runtime::ipc::PendingCall<ExitStatus, (), Error>,
) {
    let append = files
        .create("/var/linux-append", service::deadline())
        .unwrap();
    let mut first = run(b"A");
    let mut second = run(b"B");
    for guest in [&mut first, &mut second] {
        assert_eq!(
            guest.wait(until()).unwrap(),
            ExitStatus {
                code: 42,
                signal: 0
            }
        );
    }
    let mut bytes = [0; 1024];
    assert_eq!(
        append.read_at(0, &mut bytes, service::deadline()).unwrap(),
        bytes.len()
    );
    let mut counts = [0; 2];
    for record in bytes.as_chunks::<64>().0 {
        assert!(matches!(record[0], b'A' | b'B'));
        assert!(record.iter().all(|byte| *byte == record[0]));
        counts[usize::from(record[0] - b'A')] += 1;
    }
    assert_eq!(counts, [8, 8]);
}
fn metadata_fixture(files: &fs::Client<'_>) {
    let file = files
        .create("/var/linux-stat", service::deadline())
        .unwrap();
    file.write_at(0, b"test", service::deadline()).unwrap();
    file.set_attributes(
        fs::Attributes {
            permissions: 0o600,
            uid: 123,
            gid: 456,
            accessed: fs::Timestamp {
                seconds: -5,
                nanoseconds: 99,
            },
            modified: fs::Timestamp {
                seconds: 7,
                nanoseconds: 123,
            },
            changed: fs::Timestamp {
                seconds: 9,
                nanoseconds: 321,
            },
        },
        service::deadline(),
    )
    .unwrap();
    files
        .hard_link(
            b"/var/linux-stat",
            b"/var/linux-stat-link",
            service::deadline(),
        )
        .unwrap();
    files
        .symlink(
            b"/var/linux-stat",
            b"/var/linux-stat-symlink",
            service::deadline(),
        )
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

fn terminal(
    linux: &Control<'_>,
    output: &impl Fn() -> devmgr::uart::Session,
    service: &runtime::Handle<runtime::Channel>,
    console: u32,
) {
    let input = devmgr::uart::Session::open(
        process::task(),
        service,
        console,
        devmgr::uart::Access::InputOutput,
    )
    .unwrap();
    let status = linux
        .start_run_with_input(
            Program {
                path: b"/bin/linux-test",
                arguments: &[b"/bin/linux-test", b"Terminal"],
                environment: &[b"ARONA_TEST=yes"],
            },
            output(),
            input,
        )
        .unwrap()
        .wait(WaitMode::Until(
            runtime::thread::now_ns().unwrap() + WRITE_TIMEOUT_NS,
        ))
        .unwrap();
    assert_eq!(
        status,
        ExitStatus {
            code: 42,
            signal: 0
        }
    );
}
