// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{check_frame, deadline, frame};
use devmgr::{
    Error,
    net::{self, Net, Session},
};
use runtime::{
    Device, DmaDomain, Event, Handle, KernelError, Rights,
    dma::DmaState,
    ipc::{Message, Transfer, WaitMode},
    process, thread,
};

pub(super) fn run(
    configuration: net::Configuration,
    id: u32,
    grant: Handle<Device>,
    domain: Handle<DmaDomain>,
    event: Handle<Event>,
) {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (control, server) = process::task().channel(rights, rights).unwrap();
    let worker = thread::spawn(move || {
        let mut net = Net::new(configuration, &grant, &domain, &event, process::task()).unwrap();
        net::service::run(&mut net, &event, &server, id).unwrap();
        assert_eq!(domain.state().unwrap(), DmaState::Quiesced);
        drop(net);
        domain.revoke().unwrap();
        assert_eq!(domain.state().unwrap(), DmaState::Revoked);
    })
    .unwrap();
    let session = Session::open(process::task(), &control, id, deadline()).unwrap();
    assert!(matches!(
        Session::open(process::task(), &control, id, deadline()),
        Err(Error::Busy)
    ));
    let info = session
        .start_info()
        .unwrap()
        .wait(WaitMode::Until(deadline()))
        .unwrap();
    invalid_requests(&session);
    frames(&session, info);
    if info.mac[5] != 0x57 {
        link(&session, info);
    }
    let pending = session.start_receive().unwrap();
    let watch = session
        .start_watch(
            session
                .start_info()
                .unwrap()
                .wait(WaitMode::Until(deadline()))
                .unwrap()
                .sequence,
        )
        .unwrap();
    let sent = frame(info, 0x50000, 60);
    let transmit = session.start_transmit(&sent).unwrap();
    drop(session);
    drop(pending);
    drop(watch);
    drop(transmit);
    // The server observes peer closure before a new exclusive open can succeed.
    let session = loop {
        match Session::open(process::task(), &control, id, deadline()) {
            Ok(session) => break session,
            Err(Error::Busy) => thread::yield_now().unwrap(),
            Err(error) => panic!("reopen frame session: {error:?}"),
        }
    };
    let mut receive = session.start_receive().unwrap();
    drop(control);
    worker.join().unwrap();
    assert_eq!(
        receive.wait(WaitMode::Until(deadline())),
        Err(Error::Runtime(runtime::Error::Kernel(
            KernelError::ServerGone
        )))
    );
    assert!(session.start_info().is_err());
}

fn frames(session: &Session<'_>, info: net::Info) {
    for id in 0..260 {
        let sent = frame(
            info,
            0x20000 + id,
            if id % 2 == 0 { info.max_frame() } else { 60 },
        );
        let mut receive = session.start_receive().unwrap();
        session
            .start_transmit(&sent)
            .unwrap()
            .wait(WaitMode::Until(deadline()))
            .unwrap();
        let received = receive.wait(WaitMode::Until(deadline())).unwrap();
        check_frame(info, &received, &sent);
    }
    let mut canceled = session.start_receive().unwrap();
    canceled.cancel().unwrap();
    assert_eq!(
        canceled.wait(WaitMode::Until(deadline())),
        Err(Error::Runtime(runtime::Error::Kernel(
            KernelError::Cancelled
        )))
    );
    let sent = frame(info, 0x30000, 60);
    session
        .start_transmit(&sent)
        .unwrap()
        .wait(WaitMode::Until(deadline()))
        .unwrap();
    // The next request may race the canceled receive's retirement. Busy publishes no receive.
    let received = loop {
        match session
            .start_receive()
            .unwrap()
            .wait(WaitMode::Until(deadline()))
        {
            Ok(bytes) => break bytes,
            Err(Error::Busy) => thread::yield_now().unwrap(),
            Err(error) => panic!("receive after cancellation: {error:?}"),
        }
    };
    check_frame(info, &received, &sent);
}

fn link(session: &Session<'_>, info: net::Info) {
    let mut watch = session.start_watch(info.sequence).unwrap();
    assert_eq!(
        session
            .start_watch(info.sequence)
            .unwrap()
            .wait(WaitMode::Until(deadline())),
        Err(Error::Busy)
    );
    session
        .start_transmit(&frame(info, 0x40000, 60))
        .unwrap()
        .wait(WaitMode::Until(deadline()))
        .unwrap();
    let down = watch.wait(WaitMode::Until(deadline())).unwrap();
    assert!(!down.link_up);
    assert_ne!(down.sequence, info.sequence);
    assert_eq!(
        session
            .start_transmit(&frame(info, 0x40001, 60))
            .unwrap()
            .wait(WaitMode::Until(deadline())),
        Err(Error::Unavailable)
    );
    let up = session
        .start_watch(down.sequence)
        .unwrap()
        .wait(WaitMode::Until(deadline()))
        .unwrap();
    assert!(up.link_up);
    let sent = frame(info, 0x40002, 60);
    session
        .start_transmit(&sent)
        .unwrap()
        .wait(WaitMode::Until(deadline()))
        .unwrap();
    let received = session
        .start_receive()
        .unwrap()
        .wait(WaitMode::Until(deadline()))
        .unwrap();
    check_frame(info, &received, &sent);
}

fn invalid_requests(session: &Session<'_>) {
    assert!(matches!(
        session.start_transmit(&[0; 13]),
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        session.start_transmit(&[0; net::MAX_FRAME + 1]),
        Err(Error::InvalidRequest)
    ));
    for tag in [3, 4, 99] {
        let reply = session
            .channel()
            .call(&mut Message::new(tag, &[]).unwrap(), Some(deadline()))
            .unwrap();
        assert_eq!(reply.tag, 1);
    }
    let buffer = process::task()
        .allocate(1, Rights::READ | Rights::TRANSFER)
        .unwrap();
    let mut request = Message::new(3, &[]).unwrap();
    request
        .push(Transfer {
            handle: buffer.into(),
            rights: Rights::READ,
        })
        .unwrap();
    assert_eq!(
        session
            .channel()
            .call(&mut request, Some(deadline()))
            .unwrap()
            .tag,
        1
    );
}
