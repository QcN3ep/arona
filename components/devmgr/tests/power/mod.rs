// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod service;

use devmgr::{
    device::DeviceDescription,
    power::{self, Power},
};
use runtime::{DeviceSet, Handle, ObjectType, ipc::WaitMode, process};

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let mut startup = bootstrap.receive(WaitMode::Poll).unwrap();
    let devices = startup
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    let mut cursor = 0;
    let mut tested = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let device = DeviceDescription::new(&devices, id).unwrap();
        let Some(configuration) = power::probe(&device).unwrap() else {
            continue;
        };
        exercise(&devices, id, configuration);
        tested += 1;
    }
    assert_eq!(tested, 1, "QEMU virt must provide one system controller");
    unreachable!("QEMU did not stop after the shutdown request");
}

fn exercise(devices: &Handle<DeviceSet>, id: u32, configuration: power::Configuration) {
    let device = DeviceDescription::new(devices, id).unwrap();
    let grant = devices
        .claim_group(
            device.info().group.unwrap(),
            ObjectType::Device.supported_rights(),
        )
        .unwrap();
    let power = Power::new(configuration, &grant).unwrap();
    let capabilities = power.capabilities();
    assert!(capabilities.shutdown);
    assert!(capabilities.reboot);
    drop(power);
    service::exercise(configuration, id, grant);
}

fn deadline() -> u64 {
    runtime::thread::now_ns().unwrap() + 5_000_000_000
}
