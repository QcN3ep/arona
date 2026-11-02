// SPDX-License-Identifier: MPL-2.0

use kernel::{
    DeviceUsePolicy, DmaCapability, HardwareDeviceLocation, HardwareInfo,
    device::{DeviceError, DeviceSet},
    hardware::{self, mmu::PagePermissions},
    ipc::Event,
    memory::MemoryError,
    task::{Task, TaskError},
};
use spin::Once;

mod dma;
pub use dma::run_failure as run_dma_failure;

static DEVICES: Once<DeviceSet> = Once::new();

pub fn run(devices: DeviceSet, hardware: &HardwareInfo) -> (usize, usize) {
    let pci_devices = catalog(&devices, hardware);
    DEVICES.call_once(|| devices);
    crate::run_thread(scenario);
    pci_lifecycle(DEVICES.get().unwrap());
    dma::run(DEVICES.get().unwrap());
    pci_devices
}

fn pci_lifecycle(devices: &DeviceSet) {
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let info = devices.describe(id).unwrap();
        if info.group.is_none() || info.interrupts == 0 || !is_pci_device(devices, id) {
            continue;
        }
        let grant = devices.claim_group(info.group.unwrap()).unwrap();
        let reference = grant.reference();
        let event = Event::new().unwrap();
        reference.bind(id, 0, event.reference(), 1).unwrap();
        drop(event);
        let event = Event::new().unwrap();
        reference.bind(id, 0, event.reference(), 1).unwrap();
        reference.unbind(id, 0).unwrap();
    }
}

fn catalog(devices: &DeviceSet, hardware: &HardwareInfo) -> (usize, usize) {
    for device in &hardware.devices {
        let id = device.instance_id.0;
        assert_eq!(
            devices.read(id, abi::device::DeviceField::Name, 0).unwrap(),
            device.name.as_bytes()
        );
        if device.use_policy == DeviceUsePolicy::KernelReserved
            || ((device.dma != DmaCapability::None
                || device.location != HardwareDeviceLocation::Platform)
                && devices.describe(id).unwrap().dma.address_bits == 0)
        {
            assert!(devices.describe(id).unwrap().group.is_none());
            assert!(matches!(devices.claim_group(id), Err(DeviceError::Denied)));
        }
    }
    pci_catalog(devices, hardware)
}

fn pci_catalog(devices: &DeviceSet, hardware: &HardwareInfo) -> (usize, usize) {
    let mut cursor = 0;
    let mut pci_devices = 0;
    let mut behind_bridges = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        let info = devices.describe(id).unwrap();
        if is_pci_device(devices, id) {
            behind_bridges += usize::from(assert_pci_device(devices, hardware, id, info));
            pci_devices += 1;
        }
        cursor = id;
    }
    (pci_devices, behind_bridges)
}

fn is_pci_device(devices: &DeviceSet, id: u32) -> bool {
    devices
        .read(id, abi::device::DeviceField::IdentityNamespace, 0)
        .is_ok_and(|namespace| namespace == b"pci")
}

fn assert_pci_device(
    devices: &DeviceSet,
    hardware: &HardwareInfo,
    id: u32,
    info: abi::device::DeviceInfo,
) -> bool {
    assert_eq!(info.identities, 2);
    assert_eq!(info.properties, 1);
    if info.group.is_none() {
        assert!(matches!(devices.claim_group(id), Err(DeviceError::Denied)));
    }
    let config = devices
        .read(id, abi::device::DeviceField::PciConfig, 0)
        .unwrap();
    assert_eq!(config.len(), 4096);
    assert_eq!(config[4] & 5, 0);
    assert_eq!(config[5] & 4, 4);
    assert_pci_msi_disabled(config);
    assert_pci_bars(devices, id, info, config);
    assert_pci_identity(devices, id);
    let bdf = assert_pci_properties(devices, id);
    let bridged = assert_pci_parent(devices, hardware, info);
    assert_eq!(
        bdf,
        &devices.read(id, abi::device::DeviceField::Name, 0).unwrap()[4..]
    );
    bridged
}

fn assert_pci_bars(devices: &DeviceSet, id: u32, info: abi::device::DeviceInfo, config: &[u8]) {
    let mut regions = 0;
    for index in 0..6 {
        let Ok(bytes) = devices.read(id, abi::device::DeviceField::PciBar, index) else {
            continue;
        };
        let bar = abi::device::PciBar::decode(bytes).unwrap();
        assert_eq!(usize::from(bar.index), index);
        let offset = 0x10 + usize::from(bar.index) * 4;
        let value = u32::from_le_bytes(config[offset..offset + 4].try_into().unwrap());
        assert_eq!(value & 1, 0);
        assert_eq!(value & !15, bar.bus_address as u32);
        if bar.width == 64 {
            assert_eq!(
                u32::from_le_bytes(config[offset + 4..offset + 8].try_into().unwrap()),
                (bar.bus_address >> 32) as u32
            );
        }
        if let Some(region) = bar.region {
            assert_eq!(usize::from(region), regions);
            assert!(bar.size.is_multiple_of(4096));
            regions += 1;
        }
    }
    assert_eq!(regions, info.mmio_regions);
}

fn assert_pci_identity(devices: &DeviceSet, id: u32) {
    assert!(
        devices
            .read(id, abi::device::DeviceField::Name, 0)
            .unwrap()
            .starts_with(b"PCI ")
    );
    let vendor_device = devices
        .read(id, abi::device::DeviceField::Identity, 0)
        .unwrap();
    assert_eq!(vendor_device.len(), 9);
    assert_eq!(vendor_device[4], b':');
    assert!(
        vendor_device
            .iter()
            .enumerate()
            .all(|(index, byte)| { index == 4 || byte.is_ascii_hexdigit() })
    );
    assert_eq!(
        devices
            .read(id, abi::device::DeviceField::IdentityNamespace, 1)
            .unwrap(),
        b"pci-class"
    );
    let class = devices
        .read(id, abi::device::DeviceField::Identity, 1)
        .unwrap();
    assert_eq!(class.len(), 8);
    assert_eq!((class[2], class[5]), (b':', b':'));
    assert!(
        class
            .iter()
            .enumerate()
            .all(|(index, byte)| { index == 2 || index == 5 || byte.is_ascii_hexdigit() })
    );
}

fn assert_pci_properties(devices: &DeviceSet, id: u32) -> &'static [u8] {
    assert_eq!(
        devices
            .read(id, abi::device::DeviceField::PropertyNamespace, 0)
            .unwrap(),
        b"pci"
    );
    assert_eq!(
        devices
            .read(id, abi::device::DeviceField::PropertyName, 0)
            .unwrap(),
        b"bdf"
    );
    let bdf = devices
        .read(id, abi::device::DeviceField::PropertyValue, 0)
        .unwrap();
    assert_eq!(bdf.len(), 12);
    assert_eq!((bdf[4], bdf[7], bdf[10]), (b':', b':', b'.'));
    assert!(
        bdf.iter()
            .enumerate()
            .all(|(index, byte)| { [4, 7, 10].contains(&index) || byte.is_ascii_hexdigit() })
    );
    bdf
}

fn assert_pci_msi_disabled(config: &[u8]) {
    if config[6] & 0x10 == 0 {
        return;
    }
    let mut offset = usize::from(config[0x34]);
    let mut seen = [false; 64];
    while offset != 0 {
        assert!((0x40..=0xfc).contains(&offset) && offset.is_multiple_of(4));
        assert!(!seen[offset / 4]);
        seen[offset / 4] = true;
        let control = u16::from_le_bytes(config[offset + 2..offset + 4].try_into().unwrap());
        match config[offset] {
            5 => assert_eq!(control & 1, 0),
            0x11 => assert_eq!(control & 0xc000, 0x4000),
            _ => {}
        }
        offset = usize::from(config[offset + 1]);
    }
}

fn assert_pci_parent(
    devices: &DeviceSet,
    hardware: &HardwareInfo,
    info: abi::device::DeviceInfo,
) -> bool {
    let parent = info.parent.unwrap();
    let root_parent = hardware.devices.iter().any(|host| {
        host.instance_id.0 == parent
            && matches!(host.location, HardwareDeviceLocation::PciHostBridge { .. })
    });
    if root_parent {
        false
    } else {
        assert_eq!(
            devices
                .read(parent, abi::device::DeviceField::IdentityNamespace, 0)
                .unwrap(),
            b"pci"
        );
        true
    }
}

fn scenario(_: usize) -> usize {
    let devices = DEVICES.get().unwrap();
    let (console, reserved) = scenario_devices(devices);
    assert!(matches!(
        devices.claim_group(reserved),
        Err(DeviceError::Denied)
    ));
    let grant = devices.claim_group(console.group.unwrap()).unwrap();
    assert!(matches!(
        devices.claim_group(console.group.unwrap()),
        Err(DeviceError::Busy)
    ));
    let reference = grant.reference();
    let task = Task::new().unwrap();
    let other = Task::new().unwrap();
    let range = reference.map(&task, console.id, 0).unwrap();
    let info = task.query(range.start).unwrap().unwrap();
    let mapping = info.mapping.unwrap();
    assert!(mapping.attributes.user_accessible);
    assert_eq!(
        mapping.attributes.memory_type,
        hardware::mmu().device_memory_type()
    );
    assert_eq!(mapping.attributes.permissions, PagePermissions::ReadWrite);
    assert_eq!(
        task.protect(info.range, PagePermissions::ReadExecute),
        Err(TaskError::Memory(MemoryError::InUse))
    );
    assert_eq!(
        task.unmap(info.range),
        Err(TaskError::Memory(MemoryError::InUse))
    );
    assert_eq!(reference.map(&task, reserved, 0), Err(DeviceError::Denied));
    assert_eq!(
        reference.map(&task, console.id, console.mmio_regions),
        Err(DeviceError::InvalidArgument)
    );
    let alias = reference.map(&task, console.id, 0).unwrap();
    assert_eq!(alias, range);
    reference.unmap(&task, alias.start).unwrap();
    assert!(task.query(range.start).unwrap().is_some());
    shared_pages(devices, console, &reference, &task, info.range);
    let peer_range = reference.map(&other, console.id, 0).unwrap();
    let clone = grant.try_clone().unwrap();
    drop(grant);
    let event = Event::new().unwrap();
    reference.bind(console.id, 0, event.reference(), 1).unwrap();
    assert_eq!(reference.acknowledge(console.id, 0), Err(DeviceError::Busy));
    drop(event);
    let event = Event::new().unwrap();
    reference.bind(console.id, 0, event.reference(), 1).unwrap();
    reference.unbind(console.id, 0).unwrap();
    task.terminate(0);
    assert!(other.query(peer_range.start).unwrap().is_some());
    reference.revoke().unwrap();
    let revoked = other.query(peer_range.start).unwrap().unwrap();
    assert!(revoked.mapping.is_none());
    assert_eq!(
        other.unreserve(revoked.range),
        Err(TaskError::Memory(MemoryError::InUse))
    );
    assert_eq!(
        reference.map(&other, console.id, 0),
        Err(DeviceError::Closed)
    );
    let replacement = devices.claim_group(console.group.unwrap()).unwrap();
    let range = replacement.reference().map(&other, console.id, 0).unwrap();
    assert_ne!(range, peer_range);
    drop(clone);
    assert!(other.query(peer_range.start).unwrap().is_none());
    assert!(other.query(range.start).unwrap().is_some());
    drop(replacement);
    assert!(other.query(range.start).unwrap().is_none());
    other.terminate(0);
    0
}

fn scenario_devices(devices: &DeviceSet) -> (abi::device::DeviceInfo, u32) {
    let mut cursor = 0;
    let mut console = None;
    let mut reserved = None;
    while let Some(id) = devices.next(cursor).unwrap() {
        let info = devices.describe(id).unwrap();
        if info.console {
            console = Some(info);
        }
        if info.group.is_none() && info.mmio_regions != 0 {
            reserved = Some(id);
        }
        cursor = id;
    }
    (console.unwrap(), reserved.unwrap())
}

fn shared_pages(
    devices: &DeviceSet,
    console: abi::device::DeviceInfo,
    reference: &kernel::device::DeviceReference,
    task: &Task,
    pages: kernel::memory::VirtRange,
) {
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        if id != console.id && devices.describe(id).unwrap().group == console.group {
            assert!(matches!(
                devices.claim_group(devices.describe(id).unwrap().group.unwrap()),
                Err(DeviceError::Busy)
            ));
            let shared = reference.map(task, id, 0).unwrap();
            assert_eq!(task.query(shared.start).unwrap().unwrap().range, pages);
            reference.unmap(task, shared.start).unwrap();
        }
        cursor = id;
    }
}
