// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

mod service;

use devmgr::{
    device::DeviceDescription,
    rtc::{self, Rtc, Timestamp},
};
use runtime::{
    DeviceSet, Handle, ObjectType,
    ipc::{Message, WaitMode},
    process, thread,
};

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let devices = bootstrap
        .receive(WaitMode::Poll)
        .unwrap()
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    let mut cursor = 0;
    let mut tested = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        tested += usize::from(exercise_device(&devices, id));
    }
    assert_eq!(tested, 1);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn exercise_device(devices: &Handle<DeviceSet>, id: u32) -> bool {
    let device = DeviceDescription::new(devices, id).unwrap();
    let Some(configuration) = rtc::probe(&device).unwrap() else {
        return false;
    };
    let grant = devices
        .claim_group(
            device.info().group.unwrap(),
            ObjectType::Device.supported_rights(),
        )
        .unwrap();
    let mut rtc = Rtc::new(configuration, &grant).unwrap();
    let first = rtc.now().unwrap();
    check_time(first);
    thread::sleep_until(thread::now_ns().unwrap() + 1_100_000_000).unwrap();
    let second = rtc.now().unwrap();
    check_time(second);
    assert!(second.seconds > first.seconds);
    drop(rtc);
    service::exercise(configuration, id, grant);
    true
}

fn check_time(time: Timestamp) {
    // QEMU 分别从闰年与世纪平年的二月末启动。
    assert!(
        [951_868_799, 4_107_542_399]
            .into_iter()
            .any(|start| (start..start + 600).contains(&time.seconds)),
        "unexpected RTC time: {time:?}"
    );
    assert!(time.nanoseconds < 1_000_000_000);
}

fn deadline() -> u64 {
    thread::now_ns().unwrap() + 5_000_000_000
}
