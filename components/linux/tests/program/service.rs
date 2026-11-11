// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{vec, vec::Vec};
use fs::{FileSystem, ReadAt, archive::Archive};
use linux::elf::{Image, Machine};
use runtime::{
    Channel, DeviceSet, Handle, Rights, Task,
    ipc::{Message, Transfer},
    process::{self, ChildTask, ProcessStart},
};

pub struct Service(Handle<Task>);

impl Service {
    pub fn wait(&self) {
        assert_eq!(self.0.wait(deadline()).unwrap(), 0);
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.0.terminate(0).unwrap();
        self.0.wait(deadline()).unwrap();
    }
}

pub fn launch(bytes: &[u8], machine: Machine, rights: Rights) -> (Service, Handle<Channel>) {
    let image = Image::parse(bytes, machine).unwrap();
    let mut child = ChildTask::new(process::task(), rights).unwrap();
    for segment in image.segments {
        let range = segment.pages();
        let count = (range.len() / 4096) as u64;
        let mut frames = child
            .allocate(count, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)
            .unwrap();
        frames
            .write(segment.memory.start - range.start, segment.data)
            .unwrap();
        child
            .map(&frames, 0, count, Some(range.start), segment.permissions)
            .unwrap();
    }
    let (sender, receiver) = process::task()
        .channel(
            Rights::READ_WRITE | Rights::WAIT,
            Rights::READ | Rights::WAIT | Rights::TRANSFER,
        )
        .unwrap();
    let task = child
        .start(
            image.entry,
            64 * 1024,
            Some((receiver, Rights::READ | Rights::WAIT)),
        )
        .unwrap();
    (Service(task), sender)
}

pub fn read(archive: &Archive, path: &str) -> Vec<u8> {
    let file = archive.open(path).unwrap();
    let mut bytes = vec![0; file.len()];
    file.read_exact_at(0, &mut bytes).unwrap();
    bytes
}

pub fn filesystem(
    archive: Archive,
    image: &[u8],
    machine: Machine,
) -> (Service, fs::Control<'static>, fs::Client<'static>) {
    let (service, channel) = launch(image, machine, ProcessStart::REQUIRED_RIGHTS);
    let (frames, length) = archive.into_parts().unwrap();
    let control = fs::Control::start(process::task(), channel, frames, length, deadline()).unwrap();
    let client = control.connect(deadline()).unwrap();
    (service, control, client)
}

pub fn devices(
    image: &[u8],
    machine: Machine,
    devices: &Handle<DeviceSet>,
) -> (Service, Handle<Channel>) {
    let rights = ProcessStart::REQUIRED_RIGHTS
        | Rights::CREATE_EVENT
        | Rights::CREATE_THREAD
        | Rights::CREATE_CHANNEL;
    let (service, channel) = launch(image, machine, rights);
    let rights = Rights::READ | Rights::RESOURCE_SPLIT;
    let mut request = Message::new(0, &[]).unwrap();
    request
        .push(Transfer {
            handle: devices.try_clone(rights | Rights::TRANSFER).unwrap().into(),
            rights,
        })
        .unwrap();
    assert_eq!(channel.call(&mut request, Some(deadline())).unwrap().tag, 0);
    (service, channel)
}

pub fn mount(control: &fs::Control<'_>, service: &Handle<Channel>) {
    let until = deadline();
    let id = loop {
        if let Some(id) =
            devmgr::device::next(service, devmgr::device::DriverKind::Block, 0, until).unwrap()
        {
            break id;
        }
        assert!(
            runtime::thread::now_ns().unwrap() < until,
            "missing test block device"
        );
        runtime::thread::yield_now().unwrap();
    };
    let session = devmgr::block::Session::open(process::task(), service, id).unwrap();
    control
        .mount(b"/var", session, 0, 16 * 1024 * 1024, deadline())
        .unwrap();
}

pub fn deadline() -> u64 {
    runtime::thread::now_ns().unwrap() + 30_000_000_000
}

pub fn resources(service: &Handle<Channel>) -> linux::Services<'static> {
    let until = deadline();
    let device = |kind| {
        loop {
            if let Some(id) = devmgr::device::next(service, kind, 0, until).unwrap() {
                break id;
            }
            assert!(runtime::thread::now_ns().unwrap() < until);
            runtime::thread::yield_now().unwrap();
        }
    };
    linux::Services {
        clock: Some(
            devmgr::rtc::Session::open(
                process::task(),
                service,
                device(devmgr::device::DriverKind::Rtc),
                until,
            )
            .unwrap(),
        ),
        entropy: Some(
            devmgr::rng::Session::open(
                process::task(),
                service,
                device(devmgr::device::DriverKind::Rng),
                until,
            )
            .unwrap(),
        ),
    }
}
