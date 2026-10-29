// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::{sync::Arc, vec::Vec};
use devmgr::{
    Error as DeviceError,
    device::DeviceDescription,
    uart::{self, Configuration, Session, Settings, Uart},
};
use logger::{Client, Control, Error, Level, MAX_RECORD_SIZE, log};
use runtime::{
    Channel, DeviceSet, Handle, KernelError, KernelLog, Rights,
    ipc::{CallError, Message, WaitMode},
    process, thread,
};

const CHANNEL_RIGHTS: Rights = Rights::READ_WRITE.union(Rights::WAIT);

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let mut startup = bootstrap.receive(WaitMode::Poll).unwrap();
    let devices = startup
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    let kernel_log = startup
        .take_handle(0)
        .unwrap()
        .downcast::<KernelLog>()
        .unwrap();
    let (id, group, configuration) = console(&devices);
    let (uart_control, uart_worker) = start_uart(&devices, id, group, configuration);
    let session = Session::open(process::task(), &uart_control, id).unwrap();
    let (control, server) = process::task()
        .channel(CHANNEL_RIGHTS, CHANNEL_RIGHTS)
        .unwrap();
    let worker = thread::spawn(move || logger::service::run(server)).unwrap();
    let (control, client) = Control::start(process::task(), control, session, kernel_log).unwrap();
    backpressure();
    let cursor = exercise(&control, &client);
    logger::init(client.try_clone().unwrap(), log::LevelFilter::Info).unwrap();
    log::debug!("filtered logger record");
    log::info!(target: "logger::test", "Logger service tests passed");
    log::logger().flush();
    assert_eq!(logger::failures(), 0);
    let mut bytes = [0; MAX_RECORD_SIZE];
    assert_eq!(
        control
            .read(cursor, &mut bytes, deadline())
            .unwrap()
            .unwrap()
            .message,
        "Logger service tests passed"
    );
    drop(uart_control);
    uart_worker.join().unwrap().unwrap();
    let sequence = client
        .write(
            log::Level::Warn,
            "logger::test",
            "retained after UART shutdown",
            deadline(),
        )
        .unwrap();
    assert_eq!(
        control
            .read(sequence, &mut bytes, deadline())
            .unwrap()
            .unwrap()
            .message,
        "retained after UART shutdown"
    );
    assert_eq!(client.flush(deadline()), Err(Error::Unavailable));
    drop(client);
    drop(control);
    worker.join().unwrap().unwrap();
    log::info!("logger is no longer running");
    assert_eq!(logger::failures(), 1);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn exercise(control: &Control<'_>, client: &Client<'static>) -> u64 {
    let mut bytes = [0; MAX_RECORD_SIZE];
    assert_eq!(control.read(0, &mut bytes, deadline()), Ok(None));
    assert_eq!(control.read_kernel(0, &mut bytes, deadline()), Ok(None));
    assert_eq!(control.print_kernel(0, deadline()).unwrap(), None);
    assert_eq!(control.print_kernel(u64::MAX, deadline()).unwrap(), None);
    let first = client
        .write(log::Level::Info, "logger::test", "first record", deadline())
        .unwrap();
    assert_eq!(
        control.read(0, &mut [], deadline()),
        Err(Error::BufferTooSmall)
    );
    let record = control.read(0, &mut bytes, deadline()).unwrap().unwrap();
    assert_eq!(record.sequence, first);
    assert_eq!(record.target, "logger::test");
    assert_eq!(record.message, "first record");
    assert_eq!(record.level, Level::Info);
    assert!(record.timestamp_ns.is_some());
    assert!(!record.truncated);
    let mut snapshot = [0; MAX_RECORD_SIZE];
    assert_eq!(
        control.read(0, &mut snapshot, deadline()).unwrap(),
        Some(record)
    );
    let long = "日志".repeat(1024);
    let next = client
        .write(log::Level::Warn, "long", &long, deadline())
        .unwrap();
    let record = control.read(next, &mut bytes, deadline()).unwrap().unwrap();
    assert_eq!(record.sequence, next);
    assert_eq!(record.level, Level::Warn);
    assert!(record.truncated);
    assert!(record.message.len() > 900);
    assert!(long.starts_with(record.message));
    let channel = client.try_clone().unwrap().into_channel();
    for (tag, words) in [
        (16, &[0, 1024][..]),
        (u64::MAX, &[][..]),
        (1, &[u64::MAX][..]),
    ] {
        let response = channel
            .call(&mut Message::new(tag, words).unwrap(), Some(deadline()))
            .unwrap();
        assert_eq!(response.tag, 1);
        assert!(response.words().is_empty());
        assert!(response.handles().is_empty());
    }
    let client = Arc::new(client.try_clone().unwrap());
    let mut workers = Vec::new();
    for _ in 0..2 {
        let client = Arc::clone(&client);
        workers.push(
            thread::spawn(move || {
                client
                    .write(log::Level::Info, "worker", "concurrent record", deadline())
                    .unwrap()
            })
            .unwrap(),
        );
    }
    let mut sequences = Vec::new();
    for worker in workers {
        sequences.push(worker.join().unwrap());
    }
    sequences.sort_unstable();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    let mut cursor = next + 1;
    for sequence in sequences {
        let record = control
            .read(cursor, &mut bytes, deadline())
            .unwrap()
            .unwrap();
        assert_eq!(record.sequence, sequence);
        assert_eq!(record.message, "concurrent record");
        cursor = record.sequence + 1;
    }
    assert_eq!(control.read(cursor, &mut bytes, deadline()), Ok(None));
    client.flush(deadline()).unwrap();
    cursor
}

fn backpressure() {
    let (channel, receiver) = process::task()
        .channel(CHANNEL_RIGHTS, CHANNEL_RIGHTS)
        .unwrap();
    let client = Client::from_channel(process::task(), channel).unwrap();
    let text = "x".repeat(128);
    assert_eq!(
        client.write(log::Level::Info, "timeout", &text, u64::MAX),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        client.write(
            log::Level::Info,
            "timeout",
            &text,
            thread::now_ns().unwrap() + 1_000_000_000
        ),
        Err(Error::Call(CallError::Submitted(runtime::Error::Kernel(
            KernelError::TimedOut
        ))))
    );
    drop(receiver.receive(WaitMode::Poll).unwrap());
    let mut sent = 0;
    loop {
        match client.try_write(log::Level::Info, "queue", "queued record") {
            Ok(()) => {
                sent += 1;
                assert!(sent < 1024);
            }
            Err(Error::Runtime(runtime::Error::Kernel(KernelError::WouldBlock))) => break,
            Err(error) => panic!("unexpected logger error: {error:?}"),
        }
    }
    assert!(sent > 0);
    assert_eq!(
        client.try_write(log::Level::Info, "queue", &text),
        Err(Error::Runtime(runtime::Error::Kernel(
            KernelError::WouldBlock
        )))
    );
    assert_eq!(
        client.flush(thread::now_ns().unwrap() + 1_000_000),
        Err(Error::Call(CallError::NotSubmitted(
            runtime::Error::Kernel(KernelError::TimedOut)
        )))
    );
    drop(receiver.receive(WaitMode::Poll).unwrap());
    client.try_write(log::Level::Info, "queue", &text).unwrap();
    drop(receiver);
    assert!(
        client
            .try_write(log::Level::Info, "queue", "closed")
            .is_err()
    );
}

fn deadline() -> u64 {
    thread::now_ns().unwrap() + 5_000_000_000
}

fn console(devices: &Handle<DeviceSet>) -> (u32, u32, Configuration) {
    let mut cursor = 0;
    loop {
        let id = devices.next(cursor).unwrap().unwrap();
        let device = DeviceDescription::new(devices, id).unwrap();
        if device.info().console {
            return (
                id,
                device.info().group.unwrap(),
                uart::probe(&device).unwrap().unwrap(),
            );
        }
        cursor = id;
    }
}

fn start_uart(
    devices: &Handle<DeviceSet>,
    id: u32,
    group: u32,
    configuration: Configuration,
) -> (Handle<Channel>, thread::JoinHandle<Result<(), DeviceError>>) {
    let grant = devices
        .claim_group(
            group,
            Rights::MAP | Rights::DEVICE_CONTROL | Rights::IRQ_ACK,
        )
        .unwrap();
    let event = process::task()
        .event(Rights::SIGNAL | Rights::WAIT)
        .unwrap();
    let (control, server) = process::task()
        .channel(CHANNEL_RIGHTS, CHANNEL_RIGHTS)
        .unwrap();
    let worker = thread::spawn(move || {
        let deadline = thread::now_ns().unwrap() + 1_000_000_000;
        let mut uart = loop {
            match Uart::new(configuration, &grant, &event, 1, Settings::default()) {
                Ok(uart) => break uart,
                Err(DeviceError::Busy) => {
                    assert!(thread::now_ns().unwrap() < deadline);
                    thread::yield_now().unwrap();
                }
                Err(error) => return Err(error),
            }
        };
        uart::service::run(&mut uart, &event, Some(&server), id)
    })
    .unwrap();
    (control, worker)
}
