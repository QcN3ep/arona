// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

use devmgr::{
    Error,
    device::DeviceDescription,
    uart::{self, DataBits, LineErrors, Parity, Settings, StopBits, Uart},
};
use runtime::{
    Event, Handle, ObjectType,
    ipc::{self, Message, WaitItem, WaitMode},
    process, thread,
};

const SECOND: u64 = 1_000_000_000;

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let mut startup = bootstrap.receive(WaitMode::Poll).unwrap();
    let devices = startup
        .take_handle(0)
        .unwrap()
        .downcast::<runtime::DeviceSet>()
        .unwrap();
    let mut cursor = 0;
    let console = loop {
        let id = devices.next(cursor).unwrap().expect("console is missing");
        let description = DeviceDescription::new(&devices, id).unwrap();
        if description.info().console {
            break description;
        }
        cursor = id;
    };
    let configuration = uart::probe(&console).unwrap().expect("unsupported console");
    let grant = devices
        .claim_group(
            console.info().group.unwrap(),
            ObjectType::Device.supported_rights(),
        )
        .unwrap();
    let event = process::task()
        .event(ObjectType::Event.supported_rights())
        .unwrap();
    let deadline = now() + 10 * SECOND;
    let mut uart = loop {
        match Uart::new(configuration, &grant, &event, 1, Settings::default()) {
            Ok(uart) => break uart,
            Err(Error::Busy) => pause(deadline),
            Err(error) => panic!("UART initialization failed: {error:?}"),
        }
    };
    uart.configure(Settings {
        baud_rate: Some(57_600),
        data_bits: DataBits::Seven,
        parity: Parity::Even,
        stop_bits: StopBits::Two,
    })
    .unwrap();
    assert_eq!(
        uart.configure(Settings {
            baud_rate: Some(0),
            ..Settings::default()
        }),
        Err(Error::InvalidConfiguration)
    );
    uart.configure(Settings::default()).unwrap();
    assert!(matches!(
        Uart::new(configuration, &grant, &event, 1, Settings::default()),
        Err(Error::Runtime(_))
    ));
    send(
        &mut uart,
        &event,
        b"UART TX: 0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\n",
    );
    receive(&mut uart, &event, b"UART RX 1\n", b"first-uart");
    drop(uart);
    drop(event);
    let event = process::task()
        .event(ObjectType::Event.supported_rights())
        .unwrap();
    let mut uart = Uart::new(configuration, &grant, &event, 1, Settings::default()).unwrap();
    receive(&mut uart, &event, b"UART RX 2\n", b"second-uart");
    send(&mut uart, &event, b"UART driver tests passed\n");
    drop(uart);
    drop(event);
    drop(grant);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn send(uart: &mut Uart<'_>, event: &Handle<Event>, mut bytes: &[u8]) {
    let deadline = now() + 10 * SECOND;
    while !bytes.is_empty() {
        let count = uart.write(bytes).unwrap();
        bytes = &bytes[count..];
        if !bytes.is_empty() {
            wait(event, deadline);
            uart.acknowledge_interrupt().unwrap();
        }
    }
    while !uart.is_transmit_empty().unwrap() {
        pause(deadline);
    }
}

fn receive(uart: &mut Uart<'_>, event: &Handle<Event>, marker: &[u8], expected: &[u8]) {
    send(uart, event, marker);
    let deadline = now() + 10 * SECOND;
    let mut bytes = [0; 16];
    let mut received = 0;
    while received < expected.len() {
        wait(event, deadline);
        loop {
            let result = uart.read(&mut bytes[received..]).unwrap();
            assert_eq!(result.errors, LineErrors::default());
            received += result.bytes;
            assert!(received <= expected.len(), "unexpected UART input");
            match uart.acknowledge_interrupt() {
                Ok(()) => break,
                Err(Error::Busy) => assert!(now() < deadline, "UART receive timed out"),
                Err(error) => panic!("UART interrupt acknowledgement failed: {error:?}"),
            }
        }
    }
    assert_eq!(&bytes[..received], expected);
    send(uart, event, &bytes[..received]);
    send(uart, event, b"\n");
}

fn wait(event: &Handle<Event>, deadline: u64) {
    assert!(now() < deadline, "UART interrupt timed out");
    let result = ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Until(deadline)).unwrap();
    assert_eq!(result.ready, 1);
}

fn now() -> u64 {
    thread::now_ns().unwrap()
}

fn pause(deadline: u64) {
    let now = now();
    assert!(now < deadline, "UART transmission timed out");
    thread::sleep_until((now + SECOND / 1000).min(deadline)).unwrap();
}
