// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

mod service;

use devmgr::{
    Error,
    device::DeviceDescription,
    rtc::{self, Calendar, Rtc, Timestamp},
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
    exercise_set(&mut rtc, second);
    drop(rtc);
    service::exercise(configuration, id, grant);
    true
}

fn exercise_set(rtc: &mut Rtc<'_>, baseline: Timestamp) {
    assert_eq!(
        Timestamp::from_calendar(2024, 2, 29, 0, 0, 0),
        Ok(Timestamp {
            seconds: 1_709_164_800,
            nanoseconds: 0,
        })
    );
    assert_eq!(
        Timestamp {
            seconds: 951_868_799,
            nanoseconds: 0,
        }
        .calendar(),
        Ok(Calendar {
            year: 2000,
            month: 2,
            day: 29,
            hour: 23,
            minute: 59,
            second: 59,
            weekday: 2,
            year_day: 59,
        })
    );
    for fields in [
        (2025, 13, 1),
        (2025, 0, 1),
        (2025, 2, 29),
        (2024, 2, 30),
        (2025, 4, 31),
        (2025, 1, 1),
    ] {
        let (year, month, day) = fields;
        let hour = 0;
        let minute = 0;
        let second = 0;
        let expected = if fields == (2025, 1, 1) {
            Ok(Timestamp {
                seconds: 1_735_689_600,
                nanoseconds: 0,
            })
        } else {
            Err(Error::InvalidRequest)
        };
        assert_eq!(
            Timestamp::from_calendar(year, month, day, hour, minute, second),
            expected
        );
    }
    let target = Timestamp {
        seconds: baseline.seconds + 120,
        nanoseconds: 0,
    };
    rtc.set(target).unwrap();
    let changed = rtc.now().unwrap();
    assert!(
        (target.seconds..target.seconds + 2).contains(&changed.seconds),
        "unexpected written time: {changed:?}"
    );
    for invalid in [
        Timestamp {
            seconds: -1,
            nanoseconds: 0,
        },
        Timestamp {
            seconds: i64::MAX,
            nanoseconds: 0,
        },
        Timestamp {
            seconds: baseline.seconds,
            nanoseconds: 1_000_000_000,
        },
    ] {
        assert_eq!(rtc.set(invalid), Err(Error::InvalidRequest));
    }
    assert_eq!(
        rtc.set(Timestamp {
            seconds: baseline.seconds,
            nanoseconds: 0,
        }),
        Ok(())
    );
}

fn check_time(time: Timestamp) {
    // QEMU starts at the end of February in a leap year and in a century non-leap year, respectively.
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
