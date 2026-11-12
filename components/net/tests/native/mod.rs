// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

mod socket;
use net::{Client, Completion, Endpoint, Request, Shutdown, SocketKind};
use runtime::{
    Event,
    ipc::{Message, WaitMode},
    process,
};

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let mut startup = bootstrap.receive(WaitMode::Forever).unwrap();
    let reply = startup.take_reply().unwrap();
    let server = startup.words() == [0];
    let client = Client::from_channel(
        process::task(),
        startup.take_handle(0).unwrap().downcast().unwrap(),
    )
    .unwrap();
    let ready = startup.take_handle(0).unwrap().downcast::<Event>().unwrap();
    if server {
        serve(&client, &ready);
    } else {
        connect(&client);
    }
    reply.send(&mut Message::new(0, &[]).unwrap()).unwrap();
}
fn serve(client: &Client<'_>, ready: &runtime::Handle<Event>) {
    let mut listener = client.socket(SocketKind::Tcp, socket::until()).unwrap();
    let mut udp = client.socket(SocketKind::Udp, socket::until()).unwrap();
    listener
        .perform(Request::Bind(Endpoint::loopback(31000)), socket::until())
        .unwrap();
    listener
        .perform(Request::Listen(4), socket::until())
        .unwrap();
    udp.perform(Request::Bind(Endpoint::loopback(31001)), socket::until())
        .unwrap();
    let (listen, _) = listener.subscribe(socket::deadline()).unwrap();
    ready.signal(1).unwrap();
    let mut stream = loop {
        let observed = listen.snapshot(socket::deadline()).unwrap();
        match listener.perform(Request::Accept, socket::until()) {
            Ok(Completion::Accepted { socket, .. }) => break socket,
            Err(net::Error::WouldBlock) => {
                listen.wait(observed, socket::until()).unwrap();
            }
            _ => panic!("accept failed"),
        }
    };
    let (readiness, _) = stream.subscribe(socket::deadline()).unwrap();
    socket::receive_pattern(&mut stream, &readiness, 130_000);
    socket::send_pattern(&mut stream, &readiness, 130_000);
    stream
        .perform(Request::Shutdown(Shutdown::Write), socket::until())
        .unwrap();
    let (notice, _) = udp.subscribe(socket::deadline()).unwrap();
    let (other, _) = udp.subscribe(socket::deadline()).unwrap();
    for length in [0, 1, 65507] {
        let observed = other.snapshot(socket::deadline()).unwrap();
        let data = socket::receive(&mut udp, &notice, true);
        assert_eq!(data.len(), length);
        let mut bytes = alloc::vec![0; length];
        data.read(0, &mut bytes).unwrap();
        assert!(
            bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == (index % 251) as u8)
        );
        // Independent waiters retain their own wake notification after one receives.
        runtime::ipc::wait_many(
            &[runtime::ipc::WaitItem::new(other.event(), 1)],
            socket::until(),
        )
        .unwrap();
        assert!(
            other.snapshot(socket::deadline()).unwrap().sequence != observed.sequence
                || length == 0
        );
        assert!(
            matches!(udp.perform(Request::SendTo(&bytes, data.peer.unwrap()), socket::until()).unwrap(), Completion::Count(count) if count == length)
        );
    }
    stream.perform(Request::Close, socket::until()).unwrap();
    udp.perform(Request::Close, socket::until()).unwrap();
    listener.perform(Request::Close, socket::until()).unwrap();
}
fn connect(client: &Client<'_>) {
    let mut stream = client.socket(SocketKind::Tcp, socket::until()).unwrap();
    let (notice, _) = stream.subscribe(socket::deadline()).unwrap();
    match stream.perform(Request::Connect(Endpoint::loopback(31000)), socket::until()) {
        Ok(Completion::Done) | Err(net::Error::InProgress) => {}
        _ => panic!("connect failed"),
    }
    socket::send_pattern(&mut stream, &notice, 130_000);
    socket::receive_pattern(&mut stream, &notice, 130_000);
    assert!(socket::receive(&mut stream, &notice, false).is_empty());
    let mut udp = client.socket(SocketKind::Udp, socket::until()).unwrap();
    let (notice, _) = udp.subscribe(socket::deadline()).unwrap();
    for length in [0, 1, 65507] {
        let bytes: alloc::vec::Vec<_> = (0..length).map(|index| (index % 251) as u8).collect();
        assert!(
            matches!(udp.perform(Request::SendTo(&bytes, Endpoint::loopback(31001)), socket::until()).unwrap(), Completion::Count(count) if count == length)
        );
        let data = socket::receive(&mut udp, &notice, true);
        let mut received = alloc::vec![0; data.len()];
        data.read(0, &mut received).unwrap();
        assert_eq!(received, bytes);
        assert_eq!(data.peer, Some(Endpoint::loopback(31001)));
    }
    for socket in [&mut stream, &mut udp] {
        socket.perform(Request::Close, socket::until()).unwrap();
    }
}
