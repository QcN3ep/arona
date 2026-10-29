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

static DEVICES: Once<DeviceSet> = Once::new();

pub fn run(devices: DeviceSet, hardware: &HardwareInfo) {
    catalog(&devices, hardware);
    DEVICES.call_once(|| devices);
    crate::run_thread(scenario);
}

fn catalog(devices: &DeviceSet, hardware: &HardwareInfo) {
    for device in &hardware.devices {
        let id = device.instance_id.0;
        assert_eq!(
            devices.read(id, abi::device::DeviceField::Name, 0).unwrap(),
            device.name.as_bytes()
        );
        if device.use_policy == DeviceUsePolicy::KernelReserved
            || device.dma != DmaCapability::None
            || device.location != HardwareDeviceLocation::Platform
        {
            assert!(devices.describe(id).unwrap().group.is_none());
            assert!(matches!(devices.claim_group(id), Err(DeviceError::Denied)));
        }
    }
}

fn scenario(_: usize) -> usize {
    let devices = DEVICES.get().unwrap();
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
    let console = console.unwrap();
    let reserved = reserved.unwrap();
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
