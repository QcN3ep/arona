// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod service;

use devmgr::{
    Error,
    device::DeviceDescription,
    net::{self, Info, Net},
};
use runtime::{
    DeviceSet, Event, Handle, ObjectType,
    dma::DmaState,
    ipc::{self, Message, WaitItem, WaitMode},
    process, thread,
};

const PEER: [u8; 6] = [0x52, 0x54, 0, 0x65, 0x43, 0x21];

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
        let Some(configuration) = net::probe(&device).unwrap() else {
            continue;
        };
        let grant = devices
            .claim_group(
                device.info().group.unwrap(),
                ObjectType::Device.supported_rights(),
            )
            .unwrap();
        let domain = grant
            .create_dma(id, ObjectType::DmaDomain.supported_rights())
            .unwrap();
        let event = process::task()
            .event(ObjectType::Event.supported_rights())
            .unwrap();
        let mut net = Net::new(configuration, &grant, &domain, &event, process::task()).unwrap();
        let info = net.info().unwrap();
        assert_eq!(&info.mac[..5], &[0x52, 0x54, 0, 0x12, 0x34]);
        assert!(info.link_up);
        driver(&mut net, &event);
        // Leave TX uncollected and every RX slot device-owned during reset.
        net.transmit(&frame(info, 0x10000, 60)).unwrap();
        net.stop().unwrap();
        assert_eq!(domain.state().unwrap(), DmaState::Quiesced);
        assert_eq!(net.transmit(&frame(info, 0, 60)), Err(Error::Unavailable));
        assert_eq!(
            net.receive(&mut [0; net::MAX_FRAME]),
            Err(Error::Unavailable)
        );
        drop(net);
        domain.revoke().unwrap();
        assert_eq!(domain.state().unwrap(), DmaState::Revoked);
        let domain = grant
            .create_dma(id, ObjectType::DmaDomain.supported_rights())
            .unwrap();
        service::run(configuration, id, grant, domain, event);
        tested += 1;
    }
    assert_eq!(tested, 1);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn driver(net: &mut Net<'_>, event: &Handle<Event>) {
    let info = net.info().unwrap();
    assert_eq!(net.transmit(&[0; 13]), Err(Error::InvalidRequest));
    assert_eq!(
        net.transmit(&[0; net::MAX_FRAME + 1]),
        Err(Error::InvalidRequest)
    );
    assert_eq!(net.receive(&mut [0; 1]), Err(Error::InvalidRequest));
    let capacity = net.transmit_capacity();
    let rounds = 260_usize.div_ceil(capacity);
    for round in 0..rounds {
        for index in 0..capacity {
            let id = (round * capacity + index) as u32;
            let length = if index == 0 {
                info.max_frame()
            } else {
                60 + index
            };
            net.transmit(&frame(info, id, length)).unwrap();
        }
        assert_eq!(net.transmit(&frame(info, 0xffff, 60)), Err(Error::Busy));
        let mut received = alloc::vec![false; capacity];
        let mut completed = 0;
        let mut count = 0;
        let until = deadline();
        loop {
            while net.poll_transmit().unwrap().is_some() {
                completed += 1;
            }
            let mut bytes = [0; net::MAX_FRAME];
            while let Some(length) = net.receive(&mut bytes).unwrap() {
                let id = u32::from_le_bytes(bytes[14..18].try_into().unwrap()) as usize;
                assert!((round * capacity..(round + 1) * capacity).contains(&id));
                let index = id % capacity;
                assert!(!received[index]);
                received[index] = true;
                let expected = frame(
                    info,
                    id as u32,
                    if index == 0 {
                        info.max_frame()
                    } else {
                        60 + index
                    },
                );
                check_frame(info, &bytes[..length], &expected);
                count += 1;
            }
            if completed == capacity && count == capacity {
                break;
            }
            ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Until(until)).unwrap();
            net.acknowledge_interrupt().unwrap();
        }
    }
    let short = frame(info, 0, net::MIN_FRAME);
    net.transmit(&short).unwrap();
    let mut completed = false;
    let mut received = false;
    let until = deadline();
    loop {
        completed |= net.poll_transmit().unwrap().is_some();
        let mut bytes = [0; net::MAX_FRAME];
        if let Some(length) = net.receive(&mut bytes).unwrap() {
            assert_eq!(length, 60);
            check_frame(info, &bytes[..length], &short);
            assert!(bytes[net::MIN_FRAME..length].iter().all(|byte| *byte == 0));
            received = true;
        }
        if completed && received {
            break;
        }
        ipc::wait_many(&[WaitItem::new(event, 1)], WaitMode::Until(until)).unwrap();
        net.acknowledge_interrupt().unwrap();
    }
}

fn frame(info: Info, id: u32, length: usize) -> alloc::vec::Vec<u8> {
    let mut frame = alloc::vec![0x5a; length];
    frame[..6].copy_from_slice(&PEER);
    frame[6..12].copy_from_slice(&info.mac);
    frame[12..14].copy_from_slice(&[0x88, 0xb5]);
    if length >= 18 {
        frame[14..18].copy_from_slice(&id.to_le_bytes());
    }
    frame
}

fn check_frame(info: Info, received: &[u8], sent: &[u8]) {
    assert_eq!(&received[..6], &info.mac);
    assert_eq!(&received[6..12], &PEER);
    assert_eq!(&received[12..sent.len()], &sent[12..]);
    assert_eq!(received.len(), sent.len().max(60));
}

fn deadline() -> u64 {
    thread::now_ns().unwrap() + 15_000_000_000
}
