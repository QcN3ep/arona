// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::{format, sync::Arc, vec::Vec};
use devmgr::{
    Error as DeviceError,
    block::{self, Block, Session as BlockSession},
    device::DeviceDescription,
    uart::{self, Access, Configuration, Session, Settings, Uart},
};
use logger::{Client, Control, Error, Level, MAX_RECORD_SIZE, log};
use runtime::{
    Channel, DeviceSet, Handle, KernelError, KernelLog, ObjectType, Rights,
    ipc::{CallError, Message, WaitMode},
    process, thread,
};

const CHANNEL_RIGHTS: Rights = Rights::READ_WRITE.union(Rights::WAIT);

struct TestFilesystem {
    control: fs::Control<'static>,
    files: fs::Client<'static>,
    worker: thread::JoinHandle<Result<(), fs::Error>>,
    block_control: Handle<Channel>,
    block_worker: thread::JoinHandle<Result<(), DeviceError>>,
}

struct RunningLogger {
    control: Control<'static>,
    client: Client<'static>,
    worker: thread::JoinHandle<Result<(), Error>>,
}

impl RunningLogger {
    fn stop(self) {
        drop(self.client);
        drop(self.control);
        self.worker.join().unwrap().unwrap();
    }
}

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
    let session = Session::open(process::task(), &uart_control, id, Access::Output).unwrap();
    let filesystem = start_filesystem(&devices);
    let restart_kernel_log = kernel_log
        .try_clone(Rights::READ | Rights::DUPLICATE | Rights::TRANSFER)
        .unwrap();
    filesystem.files.mkdir("/var/log", deadline()).unwrap();
    let persistent_file = filesystem
        .files
        .create("/var/log/system.log", deadline())
        .unwrap();
    let logger = start_logger(session, kernel_log, Some(persistent_file));
    backpressure();
    let cursor = exercise(&logger.control, &logger.client);
    logger::init(logger.client.try_clone().unwrap(), log::LevelFilter::Info).unwrap();
    log::debug!("filtered logger record");
    log::info!(target: "logger::test", "Logger service tests passed");
    log::logger().flush();
    assert_eq!(logger::failures(), 0);
    let mut bytes = [0; MAX_RECORD_SIZE];
    assert_eq!(
        logger
            .control
            .read(cursor, &mut bytes, deadline())
            .unwrap()
            .unwrap()
            .message,
        "Logger service tests passed"
    );
    shared_console(&uart_control, id, &logger);
    let logger = restart_persistent(
        &filesystem.files,
        &uart_control,
        id,
        logger,
        restart_kernel_log,
    );
    verify_uart_failure(logger, &filesystem.files, uart_control, uart_worker);
    filesystem.stop();
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn restart_persistent(
    files: &fs::Client<'static>,
    uart_control: &Handle<Channel>,
    id: u32,
    logger: RunningLogger,
    kernel_log: Handle<KernelLog>,
) -> RunningLogger {
    let failure_kernel_log = kernel_log
        .try_clone(Rights::READ | Rights::TRANSFER)
        .unwrap();
    let persisted = logger
        .client
        .write(
            log::Level::Warn,
            "logger::test",
            "persisted before restart",
            deadline(),
        )
        .unwrap();
    logger.client.flush(deadline()).unwrap();
    logger.stop();
    verify_file_failure(files, uart_control, id, failure_kernel_log);
    let file = files.open("/var/log/system.log", deadline()).unwrap();
    let size = file.size(deadline()).unwrap();
    assert_eq!(file.write_at(size, b"torn", deadline()).unwrap(), 4);
    file.fsync(deadline()).unwrap();
    drop(file);
    let output = Session::open(process::task(), uart_control, id, Access::Output).unwrap();
    let persistent_file = files.open("/var/log/system.log", deadline()).unwrap();
    let logger = start_logger(output, kernel_log, Some(persistent_file));
    assert_eq!(
        files
            .open("/var/log/system.log", deadline())
            .unwrap()
            .size(deadline())
            .unwrap(),
        size
    );
    let mut recovered = [0; MAX_RECORD_SIZE];
    let record = logger
        .control
        .read(persisted, &mut recovered, deadline())
        .unwrap()
        .unwrap();
    assert_eq!(record.message, "persisted before restart");
    let next = logger
        .client
        .write(
            log::Level::Info,
            "logger::test",
            "continued after restart",
            deadline(),
        )
        .unwrap();
    assert_eq!(next, persisted + 1);
    logger.client.flush(deadline()).unwrap();
    logger
}

fn verify_file_failure(
    files: &fs::Client<'static>,
    uart_control: &Handle<Channel>,
    id: u32,
    kernel_log: Handle<KernelLog>,
) {
    let file = files.open("/readonly.log", deadline()).unwrap();
    let output = Session::open(process::task(), uart_control, id, Access::Output).unwrap();
    let logger = start_logger(output, kernel_log, Some(file));
    let sequence = logger
        .client
        .write(
            log::Level::Warn,
            "logger::test",
            "read-only log file",
            deadline(),
        )
        .unwrap();
    assert_eq!(logger.client.flush(deadline()), Err(Error::Unavailable));
    assert_eq!(logger.client.flush(deadline()), Err(Error::Unavailable));
    let mut bytes = [0; MAX_RECORD_SIZE];
    assert_eq!(
        logger
            .control
            .read(sequence, &mut bytes, deadline())
            .unwrap()
            .unwrap()
            .message,
        "read-only log file"
    );
    logger.stop();
    assert_eq!(
        files
            .open("/readonly.log", deadline())
            .unwrap()
            .size(deadline())
            .unwrap(),
        0
    );
}

fn verify_uart_failure(
    logger: RunningLogger,
    files: &fs::Client<'static>,
    uart_control: Handle<Channel>,
    uart_worker: thread::JoinHandle<Result<(), DeviceError>>,
) {
    drop(uart_control);
    uart_worker.join().unwrap().unwrap();
    let sequence = logger
        .client
        .write(
            log::Level::Warn,
            "logger::test",
            "retained after UART shutdown",
            deadline(),
        )
        .unwrap();
    let mut bytes = [0; MAX_RECORD_SIZE];
    assert_eq!(
        logger
            .control
            .read(sequence, &mut bytes, deadline())
            .unwrap()
            .unwrap()
            .message,
        "retained after UART shutdown"
    );
    assert_eq!(logger.client.flush(deadline()), Err(Error::Unavailable));
    logger.stop();
    log::info!("logger is no longer running");
    assert_eq!(logger::failures(), 1);
    let file = files.open("/var/log/system.log", deadline()).unwrap();
    let size = file.size(deadline()).unwrap();
    let length =
        abi::logger::HEADER_SIZE + "logger::test".len() + "retained after UART shutdown".len();
    assert_eq!(
        file.read_at(size - length, &mut bytes[..length], deadline())
            .unwrap(),
        length
    );
    let record = logger::Record::decode(&bytes[..length]).unwrap();
    assert_eq!(record.message, "retained after UART shutdown");
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
    thread::now_ns().unwrap() + 30_000_000_000
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

fn start_filesystem(devices: &Handle<DeviceSet>) -> TestFilesystem {
    let mut cursor = 0;
    let (id, configuration, grant, domain, event) = loop {
        let id = devices.next(cursor).unwrap().expect("missing test disk");
        cursor = id;
        let device = DeviceDescription::new(devices, id).unwrap();
        let Some(configuration) = block::probe(&device).unwrap() else {
            continue;
        };
        let group = device.info().group.unwrap();
        let grant = devices
            .claim_group(
                group,
                Rights::MAP | Rights::DEVICE_CONTROL | Rights::IRQ_ACK,
            )
            .unwrap();
        let domain = grant
            .create_dma(id, ObjectType::DmaDomain.supported_rights())
            .unwrap();
        let event = process::task()
            .event(Rights::SIGNAL | Rights::WAIT)
            .unwrap();
        let supported = match Block::new(configuration, &grant, &domain, &event, process::task()) {
            Ok(block) => {
                drop(block);
                true
            }
            Err(DeviceError::Unsupported) => false,
            Err(error) => panic!("block initialization failed: {error:?}"),
        };
        if supported {
            break (id, configuration, grant, domain, event);
        }
    };
    let (block_control, block_server) = process::task()
        .channel(CHANNEL_RIGHTS, CHANNEL_RIGHTS)
        .unwrap();
    let block_worker = thread::spawn(move || {
        let mut block = Block::new(configuration, &grant, &domain, &event, process::task())?;
        block::service::run(&mut block, &event, &block_server, id)?;
        domain.revoke().unwrap();
        Ok(())
    })
    .unwrap();
    let session = BlockSession::open(process::task(), &block_control, id).unwrap();
    let size = session.info().unwrap().capacity;
    let (fs_channel, fs_server) = process::task()
        .channel(CHANNEL_RIGHTS, CHANNEL_RIGHTS)
        .unwrap();
    let fs_worker = thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || fs::service::run(process::task(), fs_server))
        .unwrap();
    let archive = archive();
    let mut frames = process::task()
        .allocate(1, Rights::READ_WRITE | Rights::TRANSFER)
        .unwrap();
    frames.write(0, &archive).unwrap();
    let filesystem = fs::Control::start(
        process::task(),
        fs_channel,
        frames,
        archive.len(),
        deadline(),
    )
    .unwrap();
    let files = filesystem.connect(deadline()).unwrap();
    filesystem
        .mount(b"/var", session, 0, size, deadline())
        .unwrap();
    TestFilesystem {
        control: filesystem,
        files,
        worker: fs_worker,
        block_control,
        block_worker,
    }
}

fn archive() -> Vec<u8> {
    let mut archive = Vec::new();
    for name in [b"readonly.log\0".as_slice(), b"TRAILER!!!\0"] {
        let mut header = [b'0'; 110];
        header[..6].copy_from_slice(b"070701");
        if name != b"TRAILER!!!\0" {
            header[14..22].copy_from_slice(b"00008124");
            header[38..46].copy_from_slice(b"00000001");
        }
        header[94..102].copy_from_slice(format!("{:08x}", name.len()).as_bytes());
        archive.extend_from_slice(&header);
        archive.extend_from_slice(name);
        archive.resize(archive.len().next_multiple_of(4), 0);
    }
    archive
}

impl TestFilesystem {
    fn stop(self) {
        drop(self.files);
        drop(self.control);
        self.worker.join().unwrap().unwrap();
        drop(self.block_control);
        self.block_worker.join().unwrap().unwrap();
    }
}

fn start_logger(
    output: Session,
    kernel_log: Handle<KernelLog>,
    persistent_file: Option<fs::File<'static>>,
) -> RunningLogger {
    let (channel, server) = process::task()
        .channel(CHANNEL_RIGHTS, CHANNEL_RIGHTS)
        .unwrap();
    let worker = thread::spawn(move || logger::service::run(process::task(), server)).unwrap();
    let (control, client) = Control::start(
        process::task(),
        channel,
        output,
        kernel_log,
        persistent_file,
    )
    .unwrap();
    RunningLogger {
        control,
        client,
        worker,
    }
}

fn shared_console(control: &Handle<Channel>, id: u32, logger: &RunningLogger) {
    let terminal =
        Arc::new(Session::open(process::task(), control, id, Access::InputOutput).unwrap());
    let input = Arc::clone(&terminal);
    let reader = thread::spawn(move || {
        let expected = b"shared-console-input";
        let mut bytes = [0; 20];
        let mut count = 0;
        while count < bytes.len() {
            let read = input.read(&mut bytes[count..]).unwrap();
            assert!(read.bytes > 0);
            count += read.bytes;
        }
        assert_eq!(&bytes, expected);
    })
    .unwrap();
    let end = deadline();
    while reader.thread().state().unwrap().0 != thread::ThreadState::Blocked {
        assert!(thread::now_ns().unwrap() < end);
        thread::yield_now().unwrap();
    }
    logger
        .client
        .write(
            log::Level::Info,
            "logger::test",
            "Shared console log",
            deadline(),
        )
        .unwrap();
    logger.client.flush(deadline()).unwrap();
    terminal.write_all(b"Logger shared console RX\n").unwrap();
    terminal.flush().unwrap();
    reader.join().unwrap();
    terminal
        .write_all(b"Logger shared console tests passed\n")
        .unwrap();
    terminal.flush().unwrap();
}
