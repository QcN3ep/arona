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
    let pci = scenario(startup);
    bootstrap
        .send(
            &mut runtime::ipc::Message::new(73, &[2, pci[0], pci[1], pci[2]]).unwrap(),
            WaitMode::Forever,
        )
        .unwrap();
}

fn scenario(mut startup: ReceivedMessage) -> [u64; 3] {
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
    pci_devices(&devices)
}

fn pci_devices(devices: &runtime::Handle<DeviceSet>) -> [u64; 3] {
    let mut counts = [0; 3];
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        if let Some(device_counts) = pci_device(devices, id) {
            for (count, device_count) in counts.iter_mut().zip(device_counts) {
                *count += device_count;
            }
        }
    }
    counts
}

fn pci_device(devices: &runtime::Handle<DeviceSet>, id: u32) -> Option<[u64; 3]> {
    let mut bytes = [0; 4096];
    devices.pci_config(id, &mut bytes).ok()?;
    let info = devices.describe(id).unwrap();
    let group = info.group?;
    let grant = devices
        .claim_group(group, ObjectType::Device.supported_rights())
        .unwrap();
    let denied = grant.try_clone(Rights::NONE).unwrap();
    assert!(matches!(
        denied.map_mmio(id, 0),
        Err(Error::Kernel(abi::Error::Denied))
    ));
    assert!(matches!(
        grant.map_mmio(info.parent.unwrap(), 0),
        Err(Error::Kernel(abi::Error::Denied))
    ));
    let counts = pci_bars(devices, id, &info, &grant, &bytes);
    pci_lifecycle(devices, id, group, info, &grant);
    Some(counts)
}

fn pci_bars(
    devices: &runtime::Handle<DeviceSet>,
    id: u32,
    info: &runtime::device::DeviceInfo,
    grant: &runtime::Handle<runtime::Device>,
    config: &[u8; 4096],
) -> [u64; 3] {
    let mut counts = [0; 3];
    for index in 0..6 {
        let Ok(bar) = devices.pci_bar(id, index) else {
            continue;
        };
        assert_eq!(usize::from(bar.index), index);
        let Some(region) = bar.region else {
            continue;
        };
        let mut mmio = grant.map_mmio(id, usize::from(region)).unwrap();
        assert_eq!(mmio.len() as u64, bar.size);
        counts[0] = 1;
        counts[1] += u64::from(bar.width == 64);
        verify_pci_bar(config, bar, info, devices, grant, &mut mmio, &mut counts);
    }
    counts
}

fn verify_pci_bar(
    config: &[u8; 4096],
    bar: abi::device::PciBar,
    info: &runtime::device::DeviceInfo,
    devices: &runtime::Handle<DeviceSet>,
    grant: &runtime::Handle<runtime::Device>,
    mmio: &mut runtime::device::Mmio<'_>,
    counts: &mut [u64; 3],
) {
    if config[..4] == [0x36, 0x1b, 5, 0] && bar.index == 0 {
        // REFERENCE: https://www.qemu.org/docs/master/specs/pci-testdev.html
        mmio.write_u8(0, 0).unwrap();
        assert_eq!(mmio.read_u8(1).unwrap(), 1);
        let offset = mmio.read_u32(4).unwrap() as usize;
        let value = mmio.read_u8(8).unwrap();
        mmio.write_u8(offset, value).unwrap();
        assert_eq!(mmio.read_u32(12).unwrap(), 1);
    }
    if config[..2] == [0xf4, 0x1a] && bar.width == 64 {
        pci_modern_config(config, bar.index, mmio);
    }
    if config[..4] == [0x86, 0x80, 0x0e, 0x10] && bar.index == 0 {
        assert_eq!(info.interrupts, 1);
        pci_interrupts(devices, grant, *info, mmio);
        counts[2] += 1;
    }
}

fn pci_lifecycle(
    devices: &runtime::Handle<DeviceSet>,
    id: u32,
    group: u32,
    info: runtime::device::DeviceInfo,
    grant: &runtime::Handle<runtime::Device>,
) {
    let mapped = grant.map_mmio(id, 0).unwrap();
    let range = mapped.range().unwrap();
    let event = process::task()
        .event(ObjectType::Event.supported_rights())
        .unwrap();
    let binding = (info.interrupts != 0).then(|| grant.bind_interrupt(id, 0, &event, 1).unwrap());
    grant.revoke().unwrap();
    assert_revoke(devices, id, group, grant, binding, range.start);
    drop(mapped);
    assert!(process::task().query(range.start).unwrap().is_none());
}

fn assert_revoke(
    devices: &runtime::Handle<DeviceSet>,
    id: u32,
    group: u32,
    grant: &runtime::Handle<runtime::Device>,
    mut binding: Option<runtime::device::InterruptBinding>,
    start: usize,
) {
    if let Some(binding) = binding.as_mut() {
        assert_eq!(
            binding.acknowledge(),
            Err(Error::Kernel(abi::Error::Closed))
        );
    }
    assert!(
        process::task()
            .query(start)
            .unwrap()
            .unwrap()
            .permissions
            .is_none()
    );
    assert!(matches!(
        grant.map_mmio(id, 0),
        Err(Error::Kernel(abi::Error::Closed))
    ));
    let replacement = devices
        .claim_group(group, ObjectType::Device.supported_rights())
        .unwrap();
    let mut current = replacement.map_mmio(id, 0).unwrap();
    assert_ne!(current.range().unwrap().start, start);
    grant.revoke().unwrap();
    assert_ne!(current.read_u32(0).unwrap(), u32::MAX);
}

fn pci_modern_config(bytes: &[u8; 4096], bar: u8, mmio: &mut runtime::device::Mmio<'_>) {
    let mut offset = usize::from(bytes[0x34]);
    while offset != 0 {
        if bytes[offset] == 9 && bytes[offset + 3] == 1 && bytes[offset + 4] == bar {
            let common =
                u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into().unwrap()) as usize;
            mmio.write_u32(common, 1).unwrap();
            assert_eq!(mmio.read_u32(common).unwrap(), 1);
            mmio.write_u32(common, 0).unwrap();
            assert_ne!(mmio.read_u32(common + 4).unwrap(), 0);
            assert_eq!(mmio.read_u8(common + 20).unwrap(), 0);
            return;
        }
        offset = usize::from(bytes[offset + 1]);
    }
    panic!("virtio common configuration is missing");
}

fn pci_interrupts(
    devices: &runtime::Handle<DeviceSet>,
    grant: &runtime::Handle<runtime::Device>,
    info: runtime::device::DeviceInfo,
    mmio: &mut runtime::device::Mmio<'_>,
) {
    // REFERENCE: https://gitlab.com/qemu-project/qemu/-/blob/master/hw/net/e1000.c
    const CAUSE: u32 = 4;
    mmio.write_u32(0xd8, u32::MAX).unwrap();
    mmio.read_u32(0xc0).unwrap();
    let event = process::task()
        .event(ObjectType::Event.supported_rights())
        .unwrap();
    let mut binding = grant.bind_interrupt(info.id, 0, &event, 1).unwrap();
    let mut cursor = 0;
    let mut shared = false;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let other = devices.describe(id).unwrap();
        if other.group == info.group && other.interrupts != 0 {
            shared |= id != info.id;
            assert!(matches!(
                grant.bind_interrupt(id, 0, &event, 2),
                Err(Error::Kernel(abi::Error::Busy))
            ));
        }
    }
    assert!(shared);
    for _ in 0..2 {
        mmio.write_u32(0xd0, CAUSE).unwrap();
        mmio.write_u32(0xc8, CAUSE).unwrap();
        assert_eq!(
            ipc::wait_many(
                &[WaitItem::new(&event, 1)],
                WaitMode::Until(thread::now_ns().unwrap() + 1_000_000_000)
            )
            .unwrap()
            .ready,
            1
        );
        assert_ne!(mmio.read_u32(0xc0).unwrap() & CAUSE, 0);
        binding.acknowledge().unwrap();
    }
    binding.unbind().unwrap();
    drain_interrupt(&event);
    mmio.write_u32(0xc8, CAUSE).unwrap();
    assert_eq!(
        ipc::wait_many(&[WaitItem::new(&event, 1)], WaitMode::Poll),
        Err(Error::Kernel(abi::Error::WouldBlock))
    );
    mmio.write_u32(0xd8, u32::MAX).unwrap();
    mmio.read_u32(0xc0).unwrap();
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
    drain_interrupt(event);
    mmio.write_u8(1, 2).unwrap();
    assert_eq!(
        ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Poll),
        Err(Error::Kernel(abi::Error::WouldBlock))
    );
    mmio.write_u8(1, 0).unwrap();
}

fn drain_interrupt(event: &runtime::Handle<runtime::Event>) {
    assert!(matches!(
        ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Poll),
        Ok(_) | Err(Error::Kernel(abi::Error::WouldBlock))
    ));
}
