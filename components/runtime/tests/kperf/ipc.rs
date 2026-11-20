// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Measure message delivery, moved capabilities, and a scheduled server round trip.

use runtime::{
    ObjectType, Rights,
    ipc::{Message, Transfer, WaitMode},
    process, thread,
};
use testkit::{kperf, perf::Perf};

const WORDS: [u64; 8] = [3, 5, 8, 13, 21, 34, 55, 89];

fn roundtrip(perf: &mut Perf, handles: usize) {
    let rights = ObjectType::Channel.supported_rights();
    let (client, server) = process::task().channel(rights, rights).unwrap();
    let mut message = Message::new(1, &WORDS).unwrap();
    let rights = Rights::TRANSFER | Rights::WAIT;
    for _ in 0..handles {
        message
            .push(Transfer {
                handle: process::task().event(rights).unwrap().into(),
                rights,
            })
            .unwrap();
    }
    perf.iter(|| {
        client.send(&mut message, WaitMode::Poll).unwrap();
        let mut request = server.receive(WaitMode::Poll).unwrap();
        let mut response = Message::new(request.tag, request.words()).unwrap();
        while let Some(handle) = request.take_handle(0) {
            response.push(Transfer { handle, rights }).unwrap();
        }
        server.send(&mut response, WaitMode::Poll).unwrap();
        let mut received = client.receive(WaitMode::Poll).unwrap();
        message = Message::new(received.tag, received.words()).unwrap();
        while let Some(handle) = received.take_handle(0) {
            message.push(Transfer { handle, rights }).unwrap();
        }
    });
    assert_eq!(message.tag, 1);
    assert_eq!(message.words(), WORDS);
    assert_eq!(message.transfers().len(), handles);
}

#[kperf]
fn message_roundtrip(perf: &mut Perf) {
    roundtrip(perf, 0);
}
#[kperf]
fn transfer_1(perf: &mut Perf) {
    roundtrip(perf, 1);
}
#[kperf]
fn transfer_8(perf: &mut Perf) {
    roundtrip(perf, 8);
}

#[kperf]
fn call_roundtrip(perf: &mut Perf) {
    let rights = ObjectType::Channel.supported_rights();
    let (client, server) = process::task().channel(rights, rights).unwrap();
    let (ready, notification) = process::task().channel(rights, rights).unwrap();
    let worker = thread::spawn(move || {
        ready
            .send(&mut Message::new(1, &[]).unwrap(), WaitMode::Forever)
            .unwrap();
        let mut rounds = 0;
        loop {
            let mut request = server.receive(WaitMode::Forever).unwrap();
            if request.tag == 0 {
                return rounds;
            }
            let reply = request.take_reply().unwrap();
            reply
                .send(&mut Message::new(request.tag, request.words()).unwrap())
                .unwrap();
            rounds += 1;
        }
    })
    .unwrap();
    notification.receive(WaitMode::Forever).unwrap();
    let mut message = Message::new(1, &WORDS).unwrap();
    let mut response = None;
    let mut rounds = 0;
    perf.iter(|| {
        response = Some(client.call(&mut message, None).unwrap());
        rounds += 1;
    });
    let response = response.unwrap();
    assert_eq!(response.words(), WORDS);
    client
        .send(&mut Message::new(0, &[]).unwrap(), WaitMode::Forever)
        .unwrap();
    assert_eq!(worker.join().unwrap(), rounds);
}
