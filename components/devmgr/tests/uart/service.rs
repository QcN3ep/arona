// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;
use devmgr::{
    Error,
    uart::{self, Access, Configuration, DataBits, LineErrors, Session, Settings, StopBits, Uart},
};
use runtime::{
    Channel, DeviceSet, Handle, KernelError, ObjectType, Rights,
    ipc::{Message, WaitMode},
    process, thread,
};

const CHANNEL_RIGHTS: Rights = Rights::READ_WRITE.union(Rights::WAIT);

pub fn run(devices: &Handle<DeviceSet>, id: u32, group: u32, configuration: Configuration) {
    let (control, worker) = start(devices, id, group, configuration);
    let session = exercise(&control, id);
    let session = receive(session, b"UART service RX 2\n", b"reconnected-uart");
    session.write_all(b"UART service tests passed\n").unwrap();
    session.flush().unwrap();
    drop(control);
    worker.join().unwrap().unwrap();
    assert!(matches!(
        session.write(b"closed"),
        Err(Error::Call(runtime::ipc::CallError::NotSubmitted(
            runtime::Error::Kernel(KernelError::PeerClosed)
        )))
    ));
    drop(session);
    drop(
        devices
            .claim_group(group, ObjectType::Device.supported_rights())
            .unwrap(),
    );
}

fn start(
    devices: &Handle<DeviceSet>,
    id: u32,
    group: u32,
    configuration: Configuration,
) -> (Handle<Channel>, thread::JoinHandle<Result<(), Error>>) {
    let grant = devices
        .claim_group(group, ObjectType::Device.supported_rights())
        .unwrap();
    let event = process::task()
        .event(ObjectType::Event.supported_rights())
        .unwrap();
    let (control, server) = process::task()
        .channel(CHANNEL_RIGHTS, CHANNEL_RIGHTS)
        .unwrap();
    let worker = thread::spawn(move || {
        let deadline = thread::now_ns().unwrap() + 10_000_000_000;
        let mut uart = loop {
            match Uart::new(configuration, &grant, &event, 1, Settings::default()) {
                Ok(uart) => break uart,
                Err(Error::Busy) => {
                    assert!(thread::now_ns().unwrap() < deadline);
                    thread::yield_now().unwrap();
                }
                Err(error) => panic!("UART service initialization failed: {error:?}"),
            }
        };
        uart::service::run(&mut uart, &event, Some(&server), id)
    })
    .unwrap();
    (control, worker)
}

fn exercise(control: &Handle<Channel>, id: u32) -> Session {
    let invalid = control
        .call(&mut Message::new(u64::MAX, &[]).unwrap(), None)
        .unwrap();
    assert_eq!(invalid.tag, 1);
    assert!(matches!(
        Session::open(process::task(), control, id + 1, Access::InputOutput),
        Err(Error::Unsupported)
    ));
    let session = Session::open(process::task(), control, id, Access::InputOutput).unwrap();
    assert!(matches!(
        Session::open(process::task(), control, id, Access::InputOutput),
        Err(Error::Busy)
    ));
    assert_eq!(session.try_read(&mut [0; 16]).unwrap().bytes, 0);
    assert_eq!(
        session.configure(Settings {
            data_bits: DataBits::Five,
            stop_bits: StopBits::Two,
            ..Settings::default()
        }),
        Err(Error::InvalidConfiguration)
    );
    session.configure(Settings::default()).unwrap();
    session.write_all(b"UART buffered TX: ").unwrap();
    session.write_all(&[b'@'; 8192]).unwrap();
    session.write_all(b"\n").unwrap();
    session.flush().unwrap();
    shared_output(control, id);
    let session = receive(session, b"UART service RX 1\n", b"service-uart");
    let (sender, receiver) = process::task()
        .channel(Rights::WRITE, Rights::READ)
        .unwrap();
    let mut handoff = Message::new(0, &[]).unwrap();
    handoff
        .push(runtime::ipc::Transfer {
            handle: session.into_channel().into(),
            rights: CHANNEL_RIGHTS,
        })
        .unwrap();
    sender.send(&mut handoff, WaitMode::Poll).unwrap();
    let channel = receiver
        .receive(WaitMode::Poll)
        .unwrap()
        .take_handle(0)
        .unwrap()
        .downcast()
        .unwrap();
    let session = Session::from_channel(channel).unwrap();
    session.write_all(b"UART session transferred\n").unwrap();
    session.flush().unwrap();
    drop(session);
    let deadline = thread::now_ns().unwrap() + 1_000_000_000;
    loop {
        match Session::open(process::task(), control, id, Access::InputOutput) {
            Ok(session) => break session,
            Err(Error::Busy) => {
                assert!(thread::now_ns().unwrap() < deadline);
                thread::yield_now().unwrap();
            }
            Err(error) => panic!("UART session reconnect failed: {error:?}"),
        }
    }
}

fn receive(session: Session, marker: &[u8], expected: &[u8]) -> Session {
    let session = Arc::new(session);
    let reading = Arc::clone(&session);
    let length = expected.len();
    let reader = thread::spawn(move || {
        let mut bytes = [0; 32];
        let mut received = 0;
        while received < length {
            let result = reading.read(&mut bytes[received..length]).unwrap();
            assert!(result.bytes != 0);
            assert_eq!(result.errors, LineErrors::default());
            received += result.bytes;
        }
        bytes
    })
    .unwrap();
    let deadline = thread::now_ns().unwrap() + 1_000_000_000;
    while reader.thread().state().unwrap().0 != thread::ThreadState::Blocked {
        assert!(thread::now_ns().unwrap() < deadline);
        thread::yield_now().unwrap();
    }
    session.write_all(marker).unwrap();
    session.flush().unwrap();
    let mut bytes = reader.join().unwrap();
    assert_eq!(&bytes[..length], expected);
    assert_eq!(session.try_read(&mut bytes).unwrap().bytes, 0);
    session.write_all(expected).unwrap();
    session.write_all(b"\n").unwrap();
    session.flush().unwrap();
    Arc::try_unwrap(session).unwrap_or_else(|_| panic!("UART session is still shared"))
}

fn shared_output(control: &Handle<Channel>, id: u32) {
    let output = Session::open(process::task(), control, id, Access::Output).unwrap();
    assert_eq!(output.try_read(&mut [0]), Err(Error::Denied));
    assert_eq!(output.configure(Settings::default()), Err(Error::Denied));
    let output = output.into_channel();
    let response = output
        .call(&mut Message::new(7, &[1]).unwrap(), None)
        .unwrap();
    assert_eq!(response.tag, 7);
    assert!(response.words().is_empty());
    let output = Session::from_channel(output).unwrap();
    let second = Session::open(process::task(), control, id, Access::Output).unwrap();
    let writer = thread::spawn(move || {
        second.write_all(b"UART second output session\n").unwrap();
        second.flush().unwrap();
    })
    .unwrap();
    output.write_all(b"UART shared output session\n").unwrap();
    output.flush().unwrap();
    writer.join().unwrap();
    drop(output);
    assert!(matches!(
        Session::open(process::task(), control, id, Access::InputOutput),
        Err(Error::Busy)
    ));
}
