// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod service;

use devmgr::{
    Error,
    block::{self, Block, CancelResult, Completion, Status},
    device::DeviceDescription,
};
use runtime::{
    DeviceSet, Event, FrameAllocation, Handle, ObjectType, Rights,
    ipc::{self, Message, WaitItem, WaitMode},
    process, thread,
};

const SECOND: u64 = 1_000_000_000;
const FRAME_RIGHTS: Rights = Rights::READ_WRITE
    .union(Rights::TRANSFER)
    .union(Rights::DUPLICATE);

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
        let device = DeviceDescription::new(&devices, id).unwrap();
        let Some(configuration) = block::probe(&device).unwrap() else {
            continue;
        };
        let group = device.info().group.unwrap();
        if driver(&devices, id, group, configuration) {
            tested += 1;
        }
    }
    assert_eq!(tested, 3);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn driver(
    devices: &Handle<DeviceSet>,
    id: u32,
    group: u32,
    configuration: block::Configuration,
) -> bool {
    let grant = devices
        .claim_group(group, ObjectType::Device.supported_rights())
        .unwrap();
    let domain = grant
        .create_dma(id, ObjectType::DmaDomain.supported_rights())
        .unwrap();
    let event = process::task()
        .event(ObjectType::Event.supported_rights())
        .unwrap();
    let mut block = match Block::new(configuration, &grant, &domain, &event, process::task()) {
        Ok(block) => block,
        Err(Error::Unsupported) => return false,
        Err(error) => panic!("block initialization failed: {error:?}"),
    };
    let info = block.info();
    if info.read_only {
        read_only(&mut block, &event);
    } else if info.capacity == 2 * 1024 * 1024 {
        failed_io(&mut block, &event);
    } else {
        requests(&mut block, &event);
    }
    block.stop().unwrap();
    assert_eq!(block.flush(1000), Err(Error::Unavailable));
    drop(block);
    let mut block = Block::new(configuration, &grant, &domain, &event, process::task()).unwrap();
    if !info.read_only && info.capacity != 2 * 1024 * 1024 {
        let output = frames();
        block.read(1, 0, copy(&output), 0, 4096).unwrap();
        block.submit().unwrap();
        assert_eq!(finish(&mut block, &event, 1), success(4096));
        let mut bytes = [0; 4096];
        output.read(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0x5a; 4096]);
    }
    block.stop().unwrap();
    drop(block);
    domain.revoke().unwrap();
    drop(domain);
    drop(event);
    drop(grant);
    if !info.read_only && info.capacity != 2 * 1024 * 1024 {
        service::run(devices, id, group, configuration);
    }
    true
}

fn requests(block: &mut Block<'_>, event: &Handle<Event>) {
    let mut input = frames();
    input.write(0, &[0x5a; 4096]).unwrap();
    let output = frames();
    assert_eq!(
        block.read(1, 1, copy(&output), 0, 512),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        block.read(1, 0, copy(&output), 0, 513),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        block.read(1, block.info().capacity, copy(&output), 0, 512),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        block.read(1, u64::MAX - 511, copy(&output), 0, 512),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        block.read(1, 0, copy(&output), 4096, 512),
        Err(Error::InvalidRequest)
    );
    let denied = output.try_clone(Rights::READ).unwrap();
    assert_eq!(block.read(1, 0, denied, 0, 512), Err(Error::InvalidRequest));
    block.write(1, 0, &input, 0, 4096).unwrap();
    input.write(0, &[0xa5; 4096]).unwrap();
    block.flush(2).unwrap();
    block.read(3, 0, copy(&output), 0, 4096).unwrap();
    block.submit().unwrap();
    assert_eq!(finish(block, event, 3), success(4096));
    assert_eq!(finish(block, event, 1), success(4096));
    assert_eq!(finish(block, event, 2), success(0));
    let mut bytes = [0; 4096];
    output.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x5a; 4096]);

    for id in 0..block.info().max_requests as u64 {
        block.read(id, 0, copy(&output), 0, 512).unwrap();
    }
    assert_eq!(block.read(100, 0, copy(&output), 0, 512), Err(Error::Busy));
    assert_eq!(
        block.read(0, 0, copy(&output), 0, 512),
        Err(Error::InvalidRequest)
    );
    assert_eq!(block.acknowledge(0), Err(Error::Busy));
    assert_eq!(block.cancel(0).unwrap(), CancelResult::Cancelled);
    block.submit().unwrap();
    assert_eq!(block.cancel(1).unwrap(), CancelResult::Submitted);
    let count = block.info().max_requests as u64;
    wait_complete(block, event, count - 1);
    assert_eq!(block.read(100, 0, copy(&output), 0, 512), Err(Error::Busy));
    for id in 0..count {
        let completion = finish(block, event, id);
        assert_eq!(
            completion,
            if id <= 1 {
                Completion {
                    status: Status::Cancelled,
                    bytes: 0,
                }
            } else {
                success(512)
            }
        );
    }
    assert_eq!(block.acknowledge(0), Err(Error::NotFound));
    // Cross the ring wraparound point and reuse already acknowledged request slots.
    for round in 0..20 {
        for id in 0..count {
            block.read(id, 0, copy(&output), 0, 512).unwrap();
        }
        block.submit().unwrap();
        for id in 0..count {
            assert_eq!(finish(block, event, id), success(512), "round {round}");
        }
    }
    block.read(999, 0, copy(&output), 0, 512).unwrap();
    block.submit().unwrap();
}

fn read_only(block: &mut Block<'_>, event: &Handle<Event>) {
    let output = frames();
    assert_eq!(block.info().block_size, 4096);
    assert_eq!(block.write(1, 0, &output, 0, 4096), Err(Error::ReadOnly));
    block.read(1, 0, copy(&output), 0, 4096).unwrap();
    block.submit().unwrap();
    assert_eq!(finish(block, event, 1), success(4096));
    let mut bytes = [0; 4096];
    output.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x39; 4096]);
}

fn failed_io(block: &mut Block<'_>, event: &Handle<Event>) {
    let output = frames();
    block.read(1, 0, copy(&output), 0, 512).unwrap();
    block.submit().unwrap();
    assert_eq!(
        finish(block, event, 1),
        Completion {
            status: Status::Io,
            bytes: 0
        }
    );
}

fn finish(block: &mut Block<'_>, event: &Handle<Event>, id: u64) -> Completion {
    wait_complete(block, event, id);
    block.acknowledge(id).unwrap()
}

fn wait_complete(block: &mut Block<'_>, event: &Handle<Event>, id: u64) {
    let deadline = thread::now_ns().unwrap() + 10 * SECOND;
    loop {
        block.poll().unwrap();
        if block.completion(id).unwrap().is_some() {
            return;
        }
        block.submit().unwrap();
        ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Until(deadline)).unwrap();
        block.acknowledge_interrupt().unwrap();
    }
}

fn frames() -> Handle<FrameAllocation> {
    process::task().allocate(1, FRAME_RIGHTS).unwrap()
}

fn copy(frames: &Handle<FrameAllocation>) -> Handle<FrameAllocation> {
    frames.try_clone(FRAME_RIGHTS).unwrap()
}

fn success(bytes: usize) -> Completion {
    Completion {
        status: Status::Success,
        bytes,
    }
}
