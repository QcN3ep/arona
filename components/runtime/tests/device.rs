// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

use runtime::{
    DeviceSet, Error, ObjectType, Rights,
    device::DeviceField,
    ipc::{self, ReceivedMessage, WaitItem, WaitMode},
    process, thread,
};

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let startup = bootstrap.receive(WaitMode::Poll).unwrap();
    scenario(startup);
    bootstrap
        .send(
            &mut runtime::ipc::Message::new(73, &[2]).unwrap(),
            WaitMode::Forever,
        )
        .unwrap();
}

fn scenario(mut startup: ReceivedMessage) {
    let devices = startup
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    let console = catalog(&devices);
    let grant = devices
        .claim_group(
            console.group.unwrap(),
            ObjectType::Device.supported_rights(),
        )
        .unwrap();
    let denied = grant.try_clone(Rights::MAP).unwrap();
    let event = process::task()
        .event(ObjectType::Event.supported_rights())
        .unwrap();
    assert!(matches!(
        denied.bind_interrupt(console.id, 0, &event, 1),
        Err(Error::Kernel(abi::Error::Denied))
    ));
    let mut mmio = grant.map_mmio(console.id, 0).unwrap();
    assert_ne!(mmio.read_u8(5).unwrap() & 0x20, 0);
    assert_eq!(
        mmio.read_u16(1),
        Err(Error::Kernel(abi::Error::InvalidArgument))
    );
    assert_eq!(
        mmio.read_u32(mmio.len()),
        Err(Error::Kernel(abi::Error::InvalidArgument))
    );
    mmio.write_u8(1, 0).unwrap();
    mmio.read_u8(2).unwrap();
    interrupts(&grant, console.id, &event, &mut mmio);
    let previous = mmio.range().unwrap();
    grant.revoke().unwrap();
    assert!(
        process::task()
            .query(previous.start)
            .unwrap()
            .unwrap()
            .permissions
            .is_none()
    );
    assert!(matches!(
        grant.map_mmio(console.id, 0),
        Err(Error::Kernel(abi::Error::Closed))
    ));
    let replacement = devices
        .claim_group(
            console.group.unwrap(),
            ObjectType::Device.supported_rights(),
        )
        .unwrap();
    let mut current = replacement.map_mmio(console.id, 0).unwrap();
    let current_range = current.range().unwrap();
    assert_ne!(current_range, previous);
    drop(mmio);
    assert!(process::task().query(previous.start).unwrap().is_none());
    assert_ne!(current.read_u8(5).unwrap() & 0x20, 0);
    current.unmap().unwrap();
    assert!(
        process::task()
            .query(current_range.start)
            .unwrap()
            .is_none()
    );
    drop(current);
    drop(replacement);
    let _ = devices
        .claim_group(
            console.group.unwrap(),
            ObjectType::Device.supported_rights(),
        )
        .unwrap();
}

fn catalog(devices: &runtime::Handle<DeviceSet>) -> runtime::device::DeviceInfo {
    let reader = devices.try_clone(Rights::READ).unwrap();
    let mut cursor = 0;
    let mut console = None;
    while let Some(id) = devices.next(cursor).unwrap() {
        let info = devices.describe(id).unwrap();
        if info.console {
            console = Some(info);
        }
        if info.group.is_none() {
            assert!(matches!(
                devices.claim_group(id, Rights::MAP),
                Err(Error::Kernel(abi::Error::Denied))
            ));
        }
        cursor = id;
    }
    let console = console.unwrap();
    let mut short = [0xa5];
    assert_eq!(
        reader.read(console.id, DeviceField::Name, 0, &mut short),
        Err(Error::Kernel(abi::Error::BufferTooSmall))
    );
    assert_eq!(short, [0xa5]);
    assert_eq!(
        reader.read(console.id, DeviceField::Name, 1, &mut short),
        Err(Error::Kernel(abi::Error::InvalidArgument))
    );
    let mut bytes = [0; 128];
    assert!(
        !reader
            .read(console.id, DeviceField::Name, 0, &mut bytes)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        reader.claim_group(console.group.unwrap(), Rights::MAP),
        Err(Error::Kernel(abi::Error::Denied))
    ));
    for index in 0..console.identities {
        assert!(
            !devices
                .read(console.id, DeviceField::Identity, index, &mut bytes)
                .unwrap()
                .is_empty()
        );
    }
    for index in 0..console.properties {
        devices
            .read(console.id, DeviceField::PropertyName, index, &mut bytes)
            .unwrap();
        devices
            .read(console.id, DeviceField::PropertyValue, index, &mut bytes)
            .unwrap();
    }
    console
}

fn interrupts(
    grant: &runtime::Handle<runtime::Device>,
    id: u32,
    event: &runtime::Handle<runtime::Event>,
    mmio: &mut runtime::device::Mmio<'_>,
) {
    let mut interrupt = grant.bind_interrupt(id, 0, event, 1).unwrap();
    for _ in 0..2 {
        mmio.write_u8(1, 2).unwrap();
        let result = ipc::wait_many(
            &[WaitItem::new(event, 1)],
            WaitMode::Until(thread::now_ns().unwrap() + 1_000_000_000),
        )
        .unwrap();
        assert_eq!(result.ready, 1);
        mmio.write_u8(1, 0).unwrap();
        mmio.read_u8(2).unwrap();
        interrupt.acknowledge().unwrap();
    }
    interrupt.unbind().unwrap();
    mmio.write_u8(1, 2).unwrap();
    assert_eq!(
        ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Poll),
        Err(Error::Kernel(abi::Error::WouldBlock))
    );
    mmio.write_u8(1, 0).unwrap();
}
