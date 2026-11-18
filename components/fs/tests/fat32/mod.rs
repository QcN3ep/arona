// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::rc::Rc;
use devmgr::{
    block::{self, Block, Session},
    device::DeviceDescription,
};
use fs::{Error, ReadAt, block::Disk, fat32::Fat32};
use runtime::{
    DeviceSet, ObjectType, Rights,
    ipc::{Message, WaitMode},
    process, thread,
};

const SIZE: u64 = 64 * 1024 * 1024;

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
        let description = DeviceDescription::new(&devices, id).unwrap();
        let Some(configuration) = block::probe(&description).unwrap() else {
            continue;
        };
        let grant = devices
            .claim_group(
                description.info().group.unwrap(),
                ObjectType::Device.supported_rights(),
            )
            .unwrap();
        let domain = grant
            .create_dma(id, ObjectType::DmaDomain.supported_rights())
            .unwrap();
        let event = process::task()
            .event(ObjectType::Event.supported_rights())
            .unwrap();
        let block = match Block::new(configuration, &grant, &domain, &event, process::task()) {
            Ok(block) => block,
            Err(devmgr::Error::Unsupported) => continue,
            Err(error) => panic!("block initialization failed: {error:?}"),
        };
        drop(block);
        let rights = Rights::READ_WRITE | Rights::WAIT;
        let (control, server) = process::task().channel(rights, rights).unwrap();
        let worker = thread::spawn(move || {
            let mut block =
                Block::new(configuration, &grant, &domain, &event, process::task()).unwrap();
            block::service::run(&mut block, &event, &server, id).unwrap();
            domain.revoke().unwrap();
        })
        .unwrap();
        let session = Session::open(process::task(), &control, id).unwrap();
        let disk = Rc::new(Disk::new(process::task(), session).unwrap());
        exercise(&disk);
        drop(disk);
        drop(control);
        worker.join().unwrap();
        tested += 1;
    }
    assert_eq!(tested, 2);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn exercise(disk: &Rc<Disk>) {
    let filesystem = Fat32::mount(Rc::clone(disk), 0, SIZE).unwrap();
    assert!(matches!(
        Fat32::mount(Rc::clone(disk), 0, SIZE),
        Err(Error::Busy)
    ));
    assert_eq!(
        filesystem.open_bytes(b"/data").err(),
        Some(Error::IsDirectory)
    );
    assert_eq!(
        filesystem.open_bytes(b"/missing").err(),
        Some(Error::NotFound)
    );
    for name in [
        b"/data/short.BIN".as_slice(),
        "/DATA/Long filename café.txt".as_bytes(),
    ] {
        let file = filesystem.open_bytes(name).unwrap();
        assert_eq!(file.len(), 12000);
        assert_eq!(filesystem.unmount(), Err(Error::Busy));
        let mut bytes = [0; 12000];
        file.read_exact_at(0, &mut bytes).unwrap();
        assert!(
            bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == (index % 251) as u8)
        );
        let mut edge = [0; 33];
        assert_eq!(file.read_at(11983, &mut edge).unwrap(), 17);
        assert_eq!(file.read_at(usize::MAX, &mut edge).unwrap(), 0);
        assert_eq!(
            filesystem.metadata(name).unwrap().inode,
            file.metadata().unwrap().inode
        );
    }
    let entries = filesystem.read_dir(b"/data").unwrap();
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "Long filename café.txt".as_bytes())
    );
    let stats = filesystem.statistics().unwrap();
    assert_eq!(stats.kind, 0x4d44);
    assert!(stats.free_blocks > 0 && stats.free_blocks < stats.blocks);
    filesystem.unmount().unwrap();
    drop(filesystem);
    if !disk.read_only() {
        for offset in [SIZE, 2 * SIZE] {
            assert!(matches!(
                Fat32::mount(Rc::clone(disk), offset, SIZE),
                Err(Error::InvalidData)
            ));
        }
    }
    let filesystem = Fat32::mount(Rc::clone(disk), 0, SIZE).unwrap();
    assert_eq!(filesystem.read_dir(b"/").unwrap().len(), 1);
    filesystem.unmount().unwrap();
}
