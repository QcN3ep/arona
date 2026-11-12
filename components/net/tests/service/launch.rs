// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use alloc::{vec, vec::Vec};
use fs::{FileSystem, ReadAt, archive::Archive};
use linux::elf::{Image, Machine};
use runtime::{
    Channel, Handle, Rights, Task,
    process::{self, ChildTask},
};

pub struct Service(Handle<Task>);

impl Service {
    pub fn terminate(&self) {
        self.0.terminate(0).unwrap();
        self.0.wait(deadline()).unwrap();
    }
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
            Rights::READ_WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSFER,
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

pub fn deadline() -> u64 {
    runtime::thread::now_ns().unwrap() + 60_000_000_000
}

pub const RIGHTS: Rights = runtime::process::ProcessStart::REQUIRED_RIGHTS
    .union(Rights::CREATE_EVENT)
    .union(Rights::CREATE_CHANNEL)
    .union(Rights::CREATE_THREAD);
