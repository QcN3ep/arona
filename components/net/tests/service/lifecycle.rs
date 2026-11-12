// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{launch, until};
use net::{CancelOutcome, Client, Completion, Endpoint, Error, Request, Shutdown, SocketKind};

pub fn run(client: &Client<'_>, output: &devmgr::uart::Session) {
    let mut listener = client.socket(SocketKind::Tcp, until()).unwrap();
    listener
        .perform(Request::Bind(Endpoint::loopback(0)), until())
        .unwrap();
    listener.perform(Request::Listen(1), until()).unwrap();
    let address = match listener.perform(Request::LocalEndpoint, until()).unwrap() {
        Completion::Endpoint(address) => address,
        _ => panic!("missing address"),
    };
    let mut peer = client.socket(SocketKind::Tcp, until()).unwrap();
    assert!(matches!(
        peer.perform(Request::Connect(address), until()),
        Ok(Completion::Done) | Err(Error::InProgress)
    ));
    listener.start(Request::Accept).unwrap();
    let _ = listener.call().unwrap().cancel();
    listener.forget_reply();
    let mut accepted = match listener.complete(until()).unwrap() {
        Completion::Accepted { socket, .. } => socket,
        _ => panic!("lost accepted socket"),
    };
    peer.perform(Request::Send(b"retained"), until()).unwrap();
    accepted.start(Request::Receive(8)).unwrap();
    accepted.forget_reply();
    let data = match accepted.complete(until()).unwrap() {
        Completion::Data(data) => data,
        _ => panic!("lost received bytes"),
    };
    let mut bytes = [0; 8];
    data.read(0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"retained");
    assert!(matches!(
        accepted.perform(Request::Receive(8), until()),
        Err(Error::WouldBlock)
    ));
    output
        .write_all(b"Network lifetime: retained receive passed\n")
        .unwrap();
    let notice = half_close(&mut peer, &mut accepted, output);
    for socket in [&mut listener, &mut accepted, &mut peer] {
        socket.perform(Request::Close, until()).unwrap();
    }
    assert!(notice.snapshot(launch::deadline()).unwrap().closed);
    let closed = notice.snapshot(launch::deadline()).unwrap();
    drop(accepted);
    assert!(notice.wait(closed, until()).is_err());
    reuse(client, address);
}

fn reuse(client: &Client<'_>, address: Endpoint) {
    // Session closure releases a bound socket without an explicit close request.
    let mut replacement = client.socket(SocketKind::Tcp, until()).unwrap();
    replacement
        .perform(Request::Bind(address), until())
        .unwrap();
    drop(replacement);
    let mut replacement = client.socket(SocketKind::Tcp, until()).unwrap();
    let deadline = launch::deadline();
    loop {
        match replacement.perform(Request::Bind(address), until()) {
            Ok(Completion::Done) => break,
            Err(Error::AddressInUse) => {
                assert!(runtime::thread::now_ns().unwrap() < deadline);
                runtime::thread::yield_now().unwrap();
            }
            _ => panic!("session cleanup failed"),
        }
    }
    replacement.perform(Request::Close, until()).unwrap();
}

fn half_close(
    peer: &mut net::Socket<'_>,
    accepted: &mut net::Socket<'_>,
    output: &devmgr::uart::Session,
) -> net::Subscription {
    peer.start(Request::Shutdown(Shutdown::Write)).unwrap();
    let canceled = peer.cancel(launch::deadline()).unwrap();
    let result = peer.complete(until());
    output
        .write_all(
            alloc::format!(
                "Network shutdown: {canceled:?} {:?}\n",
                result.as_ref().map(|_| ())
            )
            .as_bytes(),
        )
        .unwrap();
    if canceled == CancelOutcome::Canceled {
        assert!(matches!(result, Err(Error::Canceled)));
        peer.perform(Request::Shutdown(Shutdown::Write), until())
            .unwrap();
    } else {
        assert!(matches!(result, Ok(Completion::Done)));
    }
    let (notice, _) = accepted.subscribe(launch::deadline()).unwrap();
    loop {
        let observed = notice.snapshot(launch::deadline()).unwrap();
        match accepted.perform(Request::Receive(8), until()) {
            Ok(Completion::Data(data)) => {
                assert!(data.is_empty());
                break;
            }
            Err(Error::WouldBlock) => {
                notice.wait(observed, until()).unwrap();
            }
            _ => panic!("shutdown did not reach peer"),
        }
    }
    accepted.perform(Request::Send(b"reply"), until()).unwrap();
    let (peer_notice, _) = peer.subscribe(launch::deadline()).unwrap();
    loop {
        let observed = peer_notice.snapshot(launch::deadline()).unwrap();
        match peer.perform(Request::Receive(8), until()) {
            Ok(Completion::Data(data)) => {
                assert_eq!(data.len(), 5);
                break;
            }
            Err(Error::WouldBlock) => {
                peer_notice.wait(observed, until()).unwrap();
            }
            _ => panic!("half-close reply failed"),
        }
    }
    output
        .write_all(b"Network lifetime: half-close passed\n")
        .unwrap();
    notice
}
