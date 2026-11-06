// SPDX-License-Identifier: MPL-2.0

use super::{check_time, deadline};
use devmgr::{
    Error,
    rtc::{self, Rtc, Session},
};
use runtime::{
    Device, Handle, Rights,
    ipc::{self, Message, Transfer, WaitItem, WaitMode},
    process, thread,
};

pub fn exercise(configuration: rtc::Configuration, id: u32, grant: Handle<Device>) {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (control, server) = process::task().channel(rights, rights).unwrap();
    let worker = thread::spawn(move || {
        let mut rtc = Rtc::new(configuration, &grant).unwrap();
        rtc::service::run(&mut rtc, &server, id).unwrap();
    })
    .unwrap();
    assert!(matches!(
        Session::open(process::task(), &control, u32::MAX, deadline()),
        Err(Error::Unsupported)
    ));
    let first = Session::start_open(process::task(), &control, id)
        .unwrap()
        .wait(WaitMode::Until(deadline()))
        .unwrap();
    assert_eq!(first.now(0), Err(Error::InvalidRequest));
    assert_eq!(first.now(u64::MAX), Err(Error::InvalidRequest));
    let second = Session::open(process::task(), &control, id, deadline()).unwrap();
    let mut before = first.start_now().unwrap();
    let mut other = second.start_now().unwrap();
    ipc::wait_many(
        &[
            WaitItem::new(before.call(), ipc::READABLE),
            WaitItem::new(other.call(), ipc::READABLE),
        ],
        WaitMode::Until(deadline()),
    )
    .unwrap();
    let before = before.wait(WaitMode::Until(deadline())).unwrap();
    let other = other.wait(WaitMode::Until(deadline())).unwrap();
    check_time(before);
    assert!(other >= before);
    drop(first);
    let reconnected = Session::open(process::task(), &control, id, deadline()).unwrap();
    assert!(reconnected.now(deadline()).unwrap() >= other);
    malformed_request(&control, id);
    assert!(reconnected.now(deadline()).unwrap() >= other);
    drop(control);
    worker.join().unwrap();
    assert!(matches!(reconnected.now(deadline()), Err(Error::Call(_))));
}

fn malformed_request(control: &Handle<runtime::Channel>, id: u32) {
    let rights = Rights::READ_WRITE | Rights::WAIT | Rights::TRANSFER;
    let (client, server) = process::task().channel(rights, rights).unwrap();
    let mut message = Message::new(1, &[id.into()]).unwrap();
    message
        .push(Transfer {
            handle: server.into(),
            rights,
        })
        .unwrap();
    assert_eq!(control.call(&mut message, Some(deadline())).unwrap().tag, 0);
    for (tag, words) in [(u64::MAX, &[][..]), (2, &[1][..])] {
        let response = client
            .call(&mut Message::new(tag, words).unwrap(), Some(deadline()))
            .unwrap();
        assert_eq!(response.tag, 1);
        assert!(response.words().is_empty());
        assert!(response.handles().is_empty());
    }
    let response = client
        .call(&mut Message::new(2, &[]).unwrap(), Some(deadline()))
        .unwrap();
    assert_eq!(response.tag, 0);
    assert_eq!(response.words().len(), 2);
}
