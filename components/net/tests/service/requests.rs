// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{launch, protocol as p, until};
use net::{Client, Completion, Endpoint, Error, OptionName, Request, SocketKind};
use runtime::{
    Channel, Handle, Rights,
    ipc::{Message, WaitMode},
    process,
};

pub fn run(client: &Client<'_>, output: &devmgr::uart::Session) {
    let mut udp = client.socket(SocketKind::Udp, until()).unwrap();
    udp.perform(Request::Bind(Endpoint::loopback(0)), until())
        .unwrap();
    let target = match udp.perform(Request::LocalEndpoint, until()).unwrap() {
        Completion::Endpoint(value) => value,
        _ => panic!("missing local endpoint"),
    };
    let mut sender = client.socket(SocketKind::Udp, until()).unwrap();
    sender
        .start(Request::SendTo(b"one datagram", target))
        .unwrap();
    assert!(matches!(
        sender.start(Request::LocalEndpoint),
        Err(Error::Busy)
    ));
    output
        .write_all(b"Network request: lost send reply\n")
        .unwrap();
    sender.forget_reply();
    assert!(matches!(
        sender.complete(until()).unwrap(),
        Completion::Count(12)
    ));
    let data = match udp.perform(Request::ReceiveDatagram, until()).unwrap() {
        Completion::Data(data) => data,
        _ => panic!("missing datagram"),
    };
    let mut bytes = [0; 12];
    data.read(0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"one datagram");
    assert!(matches!(
        udp.perform(Request::ReceiveDatagram, until()),
        Err(Error::WouldBlock)
    ));
    options(client, &mut sender, output);
    output
        .write_all(b"Network request: raw protocol\n")
        .unwrap();
    raw(client, target);
    assert!(
        matches!(udp.perform(Request::ReceiveDatagram, until()).unwrap(), Completion::Data(data) if data.len() == 3)
    );
    assert!(matches!(
        udp.perform(Request::ReceiveDatagram, until()),
        Err(Error::WouldBlock)
    ));
    output.write_all(b"Network request: cleanup\n").unwrap();
    for socket in [&mut sender, &mut udp] {
        socket.perform(Request::Close, until()).unwrap();
    }
}

fn raw(client: &Client<'_>, target: Endpoint) {
    let root = client_channel(client);
    let (channel, server) = process::task()
        .channel(p::OWNED_CHANNEL, p::OWNED_CHANNEL)
        .unwrap();
    let mut attach = Message::new(p::ATTACH, &[]).unwrap();
    p::push(&mut attach, server, p::OWNED_CHANNEL).unwrap();
    root.send(&mut attach, WaitMode::Poll).unwrap();
    let call = |message: &mut Message| channel.call(message, Some(launch::deadline())).unwrap();
    let submit = |id, opcode, args: &[u64], frames: Option<Handle<runtime::FrameAllocation>>| {
        let mut words = alloc::vec![id, opcode];
        words.extend_from_slice(args);
        let mut request = Message::new(p::SUBMIT, &words).unwrap();
        if let Some(frames) = frames {
            p::push(&mut request, frames, Rights::READ).unwrap();
        }
        assert_eq!(call(&mut request).tag, 0);
    };
    submit(1, p::CREATE, &[1], None);
    assert_eq!(
        call(&mut Message::new(p::RESULT, &[1]).unwrap()).words(),
        [0, p::DONE]
    );
    acknowledge(&channel, 1);
    // A canceled reply wait does not cancel committed datagram transmission.
    let mut frames = process::task()
        .allocate(1, Rights::READ_WRITE | Rights::TRANSFER)
        .unwrap();
    frames.write(0, b"raw").unwrap();
    submit(2, p::SEND_TO, &[0, 3, p::endpoint(target)], Some(frames));
    submit(2, p::SEND_TO, &[0, 3, p::endpoint(target)], None);
    let first = call(&mut Message::new(p::RESULT, &[2]).unwrap());
    assert_eq!(first.words(), [0, p::COUNT, 3]);
    assert_eq!(
        call(&mut Message::new(p::RESULT, &[2]).unwrap()).words(),
        first.words()
    );
    assert_eq!(
        call(&mut Message::new(p::CANCEL, &[2]).unwrap()).words(),
        [1]
    );
    acknowledge(&channel, 2);
    acknowledge(&channel, 2);
    assert_ne!(call(&mut Message::new(p::RESULT, &[2]).unwrap()).tag, 0);
    for (id, offset, length) in [(3, u64::MAX, 4), (4, 4095, 2), (5, 0, 65508)] {
        let frames = process::task()
            .allocate(1, Rights::READ | Rights::TRANSFER)
            .unwrap();
        submit(
            id,
            p::SEND_TO,
            &[offset, length, p::endpoint(target)],
            Some(frames),
        );
        assert_eq!(
            call(&mut Message::new(p::RESULT, &[id]).unwrap()).words(),
            [5]
        );
        acknowledge(&channel, id);
    }
    // Cancel may observe either the unexecuted attempt or its retained result.
    submit(6, p::BIND, &[p::endpoint(Endpoint::loopback(0))], None);
    let canceled = call(&mut Message::new(p::CANCEL, &[6]).unwrap());
    assert!(matches!(canceled.words(), [0] | [1]));
    let result = call(&mut Message::new(p::RESULT, &[6]).unwrap());
    assert_eq!(
        result.words(),
        if canceled.words() == [0] {
            &[4][..]
        } else {
            &[0, p::DONE][..]
        }
    );
    acknowledge(&channel, 6);
}
fn client_channel(client: &Client<'_>) -> Handle<Channel> {
    client.channel().try_clone(p::OWNED_CHANNEL).unwrap()
}
fn acknowledge(channel: &Handle<Channel>, id: u64) {
    assert_eq!(
        channel
            .call(
                &mut Message::new(p::ACK, &[id]).unwrap(),
                Some(launch::deadline())
            )
            .unwrap()
            .tag,
        0
    );
}

fn options(client: &Client<'_>, sender: &mut net::Socket<'_>, output: &devmgr::uart::Session) {
    output.write_all(b"Network request: options\n").unwrap();
    sender
        .perform(Request::SetOption(OptionName::ReuseAddress, true), until())
        .unwrap();
    assert!(matches!(
        sender
            .perform(Request::GetOption(OptionName::ReuseAddress), until())
            .unwrap(),
        Completion::Value(1)
    ));
    let mut tcp = client.socket(SocketKind::Tcp, until()).unwrap();
    tcp.perform(Request::SetOption(OptionName::NoDelay, true), until())
        .unwrap();
    assert!(matches!(
        tcp.perform(Request::GetOption(OptionName::NoDelay), until())
            .unwrap(),
        Completion::Value(1)
    ));
    output
        .write_all(b"Network request: refused connection\n")
        .unwrap();
    tcp.start(Request::Connect(Endpoint::loopback(39999)))
        .unwrap();
    tcp.forget_reply();
    let connected = tcp.complete(until());
    output
        .write_all(
            alloc::format!(
                "Network connect result: {:?}\n",
                connected.as_ref().map(|_| ())
            )
            .as_bytes(),
        )
        .unwrap();
    assert!(matches!(
        connected,
        Err(Error::InProgress | Error::ConnectionRefused)
    ));
    let (notice, _) = tcp.subscribe(launch::deadline()).unwrap();
    loop {
        let state = notice.snapshot(launch::deadline()).unwrap();
        if state.error {
            break;
        }
        notice.wait(state, until()).unwrap();
    }
    assert!(tcp.readiness(launch::deadline()).unwrap().error);
    let pending_error = tcp.perform(Request::TakeError, until());
    output
        .write_all(
            alloc::format!(
                "Network pending error: {:?}\n",
                pending_error.as_ref().map(|_| ())
            )
            .as_bytes(),
        )
        .unwrap();
    assert!(matches!(pending_error, Err(Error::ConnectionRefused)));
    tcp.perform(Request::Close, until()).unwrap();
}
