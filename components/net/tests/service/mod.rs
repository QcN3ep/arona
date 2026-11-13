// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

mod launch;
mod lifecycle;
mod memory;
#[path = "../../src/protocol.rs"]
pub mod protocol;
mod requests;

use net::{Client, Control, Endpoint, Error};
use runtime::{
    DeviceSet, Rights,
    ipc::{self, Message, Transfer, WaitItem, WaitMode},
    process,
};

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let mut startup = bootstrap.receive(WaitMode::Forever).unwrap();
    let length = startup.words()[0] as usize;
    let archive =
        fs::archive::Archive::new(startup.take_handle(0).unwrap().downcast().unwrap(), length)
            .unwrap();
    let devices = startup
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    let native = launch::read(&archive, "/bin/net");
    let client = launch::read(&archive, "/bin/net-native");
    let devmgr = launch::read(&archive, "/bin/devmgr");
    let machine = linux::elf::Machine::from_header(&native).unwrap();
    let rights = launch::RIGHTS;
    let (driver, channel) = launch::launch(&devmgr, machine, rights);
    let mut request = Message::new(0, &[]).unwrap();
    request
        .push(Transfer {
            handle: devices
                .try_clone(Rights::READ | Rights::RESOURCE_SPLIT | Rights::TRANSFER)
                .unwrap()
                .into(),
            rights: Rights::READ | Rights::RESOURCE_SPLIT,
        })
        .unwrap();
    assert_eq!(
        channel
            .call(&mut request, Some(launch::deadline()))
            .unwrap()
            .tag,
        0
    );
    let mut cursor = 0;
    let console = loop {
        let id = devices.next(cursor).unwrap().unwrap();
        if devices.describe(id).unwrap().console {
            break id;
        }
        cursor = id;
    };
    let output = devmgr::uart::Session::open(
        process::task(),
        &channel,
        console,
        devmgr::uart::Access::Output,
    )
    .unwrap();
    output.write_all(b"Network test: devices ready\n").unwrap();
    let (service, net_channel) = launch::launch(&native, machine, rights);
    let control = start_network(net_channel, &channel);
    for family in [0, 2] {
        native_clients(&control, &client, machine, &output, family);
    }
    requests::run(&control.connect().unwrap(), &output);
    output
        .write_all(b"Network test: request recovery passed\n")
        .unwrap();
    lifecycle::run(&control.connect().unwrap(), &output);
    output
        .write_all(b"Network test: lifetime checks passed\n")
        .unwrap();
    memory::run(&control.connect().unwrap());
    output
        .write_all(b"Network test: allocation recovery passed\n")
        .unwrap();
    let native_client = control.connect().unwrap();
    failure(&native_client, &service);
    drop(native_client);
    drop(control);
    drop(output);
    drop(channel);
    driver.wait();
    bootstrap
        .send(&mut Message::new(73, &[37, 41]).unwrap(), WaitMode::Forever)
        .unwrap();
}
fn until() -> WaitMode {
    WaitMode::Until(launch::deadline())
}
fn failure(client: &Client<'_>, service: &launch::Service) {
    let mut socket = client.socket(net::SocketKind::Udp, until()).unwrap();
    let (notice, state) = socket.subscribe(launch::deadline()).unwrap();
    socket.start(net::Request::LocalEndpoint).unwrap();
    service.terminate();
    assert!(socket.complete(until()).is_err());
    assert!(notice.wait(state, until()).is_err());
}

fn native_clients(
    control: &Control<'_>,
    client: &[u8],
    machine: linux::elf::Machine,
    output: &devmgr::uart::Session,
    family: u64,
) {
    let rights = launch::RIGHTS;
    let ready = process::task()
        .event(Rights::WAIT | Rights::SIGNAL | Rights::DUPLICATE | Rights::TRANSFER)
        .unwrap();
    let (server, server_channel) = launch::launch(client, machine, rights);
    let begin = |role, endpoint: &runtime::Handle<runtime::Channel>| {
        let mut request = Message::new(0, &[role]).unwrap();
        request
            .push(Transfer {
                handle: control.connect().unwrap().into_channel().into(),
                rights: protocol::OWNED_CHANNEL,
            })
            .unwrap();
        request
            .push(Transfer {
                handle: ready
                    .try_clone(Rights::SIGNAL | Rights::TRANSFER)
                    .unwrap()
                    .into(),
                rights: Rights::SIGNAL,
            })
            .unwrap();
        endpoint.start_call(&mut request).unwrap()
    };
    let server_call = begin(family, &server_channel);
    output
        .write_all(b"Network test: listener starting\n")
        .unwrap();
    ipc::wait_many(&[WaitItem::new(&ready, 1)], until()).unwrap();
    output.write_all(b"Network test: listener ready\n").unwrap();
    let (peer, peer_channel) = launch::launch(client, machine, rights);
    let peer_call = begin(family + 1, &peer_channel);
    for call in [&server_call, &peer_call] {
        ipc::wait_many(&[WaitItem::new(call, ipc::READABLE)], until()).unwrap();
        assert_eq!(call.try_receive().unwrap().tag, 0);
    }
    output
        .write_all(b"Network test: native clients passed\n")
        .unwrap();
    server.wait();
    peer.wait();
}

fn start_network(
    net_channel: runtime::Handle<runtime::Channel>,
    channel: &runtime::Handle<runtime::Channel>,
) -> Control<'static> {
    let rng = loop {
        if let Some(id) = devmgr::device::next(
            channel,
            devmgr::device::DriverKind::Rng,
            0,
            launch::deadline(),
        )
        .unwrap()
        {
            break id;
        }
        runtime::thread::yield_now().unwrap();
    };
    let entropy =
        devmgr::rng::Session::open(process::task(), channel, rng, launch::deadline()).unwrap();
    Control::start(process::task(), net_channel, entropy, launch::deadline()).unwrap()
}
