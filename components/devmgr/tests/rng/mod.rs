// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use devmgr::{
    Error,
    device::DeviceDescription,
    rng::{self, Rng, Session},
};
use runtime::{
    DeviceSet, Handle, ObjectType, Rights,
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
    assert!(tested > 0);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn exercise_device(devices: &Handle<DeviceSet>, id: u32) -> bool {
    let device = DeviceDescription::new(devices, id).unwrap();
    let Some(configuration) = rng::probe(&device).unwrap() else {
        return false;
    };
    let group = device.info().group.unwrap();
    let grant = devices
        .claim_group(group, ObjectType::Device.supported_rights())
        .unwrap();
    let domain = grant
        .create_dma(id, ObjectType::DmaDomain.supported_rights())
        .unwrap();
    let event = process::task()
        .event(ObjectType::Event.supported_rights())
        .unwrap();
    let mut rng = Rng::new(configuration, &grant, &domain, &event, process::task()).unwrap();
    exercise_driver(&mut rng, &event);
    rng.request(rng::MAX_BYTES).unwrap();
    rng.stop().unwrap();
    assert_eq!(rng.request(1), Err(Error::Unavailable));
    drop(rng);
    exercise_service(configuration, id, grant, domain, event);
    true
}

fn exercise_driver(rng: &mut Rng<'_>, event: &runtime::Handle<runtime::Event>) {
    let mut bytes = [0; 128];
    assert_eq!(rng.request(0), Err(Error::InvalidRequest));
    assert_eq!(rng.request(rng::MAX_BYTES + 1), Err(Error::InvalidRequest));
    // Cross ring wrap even when the device offers 256 entries.
    for _ in 0..260 {
        rng.request(bytes.len()).unwrap();
        assert_eq!(rng.request(1), Err(Error::Busy));
        loop {
            if let Some(length) = rng.poll(&mut bytes).unwrap() {
                assert!(length > 0 && length <= bytes.len());
                break;
            }
            runtime::ipc::wait_many(
                &[runtime::ipc::WaitItem::new(event, 1)],
                WaitMode::Until(deadline()),
            )
            .unwrap();
            rng.acknowledge_interrupt().unwrap();
        }
    }
    assert!(bytes.iter().any(|byte| *byte != 0));
}

fn exercise_service(
    configuration: rng::Configuration,
    id: u32,
    grant: runtime::Handle<runtime::Device>,
    domain: runtime::Handle<runtime::DmaDomain>,
    event: runtime::Handle<runtime::Event>,
) {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (control, server) = process::task().channel(rights, rights).unwrap();
    let worker = thread::spawn(move || {
        let mut rng = Rng::new(configuration, &grant, &domain, &event, process::task()).unwrap();
        rng::service::run(&mut rng, &event, &server, id).unwrap();
        domain.revoke().unwrap();
    })
    .unwrap();
    let first = Session::start_open(process::task(), &control, id)
        .unwrap()
        .wait(WaitMode::Until(deadline()))
        .unwrap();
    let second = Session::open(process::task(), &control, id, deadline()).unwrap();
    let mut first_read = first.start_read(128).unwrap();
    let mut second_read = second.start_read(128).unwrap();
    runtime::ipc::wait_many(
        &[
            runtime::ipc::WaitItem::new(first_read.call(), runtime::ipc::READABLE),
            runtime::ipc::WaitItem::new(second_read.call(), runtime::ipc::READABLE),
        ],
        WaitMode::Until(deadline()),
    )
    .unwrap();
    let first_bytes = first_read.wait(WaitMode::Until(deadline())).unwrap();
    let second_bytes = second_read.wait(WaitMode::Until(deadline())).unwrap();
    assert!(!first_bytes.is_empty());
    assert!(!second_bytes.is_empty());
    assert_ne!(first_bytes, second_bytes);
    let other = thread::spawn(move || {
        let mut bytes = alloc::vec![0; 8193];
        second.fill(&mut bytes, deadline()).unwrap();
        bytes
    })
    .unwrap();
    let mut current = alloc::vec![0; 8193];
    first.fill(&mut current, deadline()).unwrap();
    let other = other.join().unwrap();
    assert_ne!(current, other);
    assert!(current.iter().any(|byte| *byte != 0));
    drop(first);
    drop(control);
    worker.join().unwrap();
}

fn deadline() -> u64 {
    thread::now_ns().unwrap() + 5_000_000_000
}
