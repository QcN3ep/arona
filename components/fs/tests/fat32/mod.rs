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
    assert_eq!(tested, 4);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn exercise(disk: &Rc<Disk>) {
    if !disk.read_only() && disk.capacity() <= 2 * SIZE {
        failure(disk);
        return;
    }
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
        b"/data/LONGFI~1.TXT".as_slice(),
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
    if disk.read_only() {
        assert_eq!(filesystem.create(b"/denied").err(), Some(Error::ReadOnly));
        assert_eq!(
            filesystem
                .open_bytes(b"/data/Short.bin")
                .unwrap()
                .write_at(0, b"x"),
            Err(Error::ReadOnly)
        );
    } else {
        write_volume(&filesystem);
    }
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
    if disk.read_only() {
        assert_eq!(filesystem.read_dir(b"/").unwrap().len(), 1);
    } else {
        let file = filesystem.open_bytes(b"/persist.bin").unwrap();
        let mut data = [0; 12000];
        file.read_exact_at(0, &mut data).unwrap();
        assert!(
            data.iter()
                .enumerate()
                .all(|(index, byte)| *byte == (index % 251) as u8)
        );
    }
    filesystem.unmount().unwrap();
    drop(filesystem);
    if !disk.read_only() {
        no_space(disk);
        active_fat(disk);
        wide_sectors(disk);
    }
}

fn write_volume(filesystem: &Fat32) {
    let free = filesystem.statistics().unwrap().free_blocks;
    filesystem.mkdir(b"/work").unwrap();
    let file = filesystem.create("/work/source α.txt".as_bytes()).unwrap();
    assert_eq!(
        filesystem.create("/WORK/SOURCE α.TXT".as_bytes()).err(),
        Some(Error::AlreadyExists)
    );
    assert_eq!(
        filesystem.create(b"/work/bad:name").err(),
        Some(Error::InvalidRequest)
    );
    assert_eq!(filesystem.unlink(b"/work"), Err(Error::NotEmpty));
    let pattern = [0x6d; 8193];
    file.write_at(513, &pattern).unwrap();
    let mut bytes = [0xff; 8706];
    file.read_exact_at(0, &mut bytes).unwrap();
    assert!(bytes[..513].iter().all(|byte| *byte == 0));
    assert_eq!(&bytes[513..], &pattern);
    file.write_at(510, b"across").unwrap();
    assert_eq!(file.append(b"end").unwrap(), (8706, 3));
    file.truncate(1).unwrap();
    file.truncate(1025).unwrap();
    let mut bytes = [0xff; 1025];
    file.read_exact_at(0, &mut bytes).unwrap();
    assert!(bytes.iter().all(|byte| *byte == 0));
    let inode = file.metadata().unwrap().inode;
    filesystem
        .rename("/work/source α.txt".as_bytes(), b"/work/dest")
        .unwrap();
    let other = filesystem.open_bytes(b"/work/dest").unwrap();
    assert_eq!(other.metadata().unwrap().inode, inode);
    other.append(b"shared").unwrap();
    assert_eq!(file.len(), 1031);
    filesystem.unlink(b"/work/dest").unwrap();
    assert_eq!(
        filesystem.open_bytes(b"/work/dest").err(),
        Some(Error::NotFound)
    );
    assert_eq!(file.metadata().unwrap().links, 0);
    file.write_at(4097, b"open").unwrap();
    assert_eq!(other.len(), 4101);
    drop(other);
    drop(file);
    filesystem.unlink(b"/work").unwrap();
    assert_eq!(filesystem.statistics().unwrap().free_blocks, free);

    filesystem.mkdir(b"/a").unwrap();
    filesystem.mkdir(b"/a/b").unwrap();
    let child = filesystem.open_directory(b"/a/b").unwrap();
    filesystem.rename(b"/a", b"/moved").unwrap();
    assert_eq!(child.path().unwrap(), b"moved/b");
    assert_eq!(
        child.parent_metadata().unwrap().unwrap().inode,
        filesystem.metadata(b"/moved").unwrap().inode
    );
    assert_eq!(
        filesystem.rename(b"/moved", b"/moved/b/c"),
        Err(Error::InvalidRequest)
    );
    filesystem.mkdir(b"/other").unwrap();
    filesystem.rename(b"/moved/b", b"/other/child").unwrap();
    assert_eq!(child.path().unwrap(), b"other/child");
    filesystem.unlink(b"/other/child").unwrap();
    assert_eq!(child.path(), Err(Error::NotFound));
    drop(child);
    filesystem.unlink(b"/other").unwrap();
    filesystem.unlink(b"/moved").unwrap();

    let target = filesystem.create(b"/target").unwrap();
    target.write_at(0, b"old").unwrap();
    let source = filesystem.create(b"/source").unwrap();
    source.write_at(0, b"new").unwrap();
    filesystem.rename(b"/source", b"/target").unwrap();
    assert_eq!(target.metadata().unwrap().links, 0);
    let mut old = [0; 3];
    target.read_exact_at(0, &mut old).unwrap();
    assert_eq!(&old, b"old");
    drop(target);
    filesystem.rename(b"/target", b"/TARGET").unwrap();
    assert_eq!(
        source.metadata().unwrap().inode,
        filesystem.metadata(b"/TARGET").unwrap().inode
    );
    filesystem.unlink(b"/TARGET").unwrap();
    drop(source);

    filesystem.mkdir(b"/many").unwrap();
    for index in 0..20 {
        drop(
            filesystem
                .create(alloc::format!("/many/Long entry {index}.txt").as_bytes())
                .unwrap(),
        );
    }
    assert_eq!(filesystem.read_dir(b"/many").unwrap().len(), 20);
    for index in 0..20 {
        filesystem
            .unlink(alloc::format!("/many/Long entry {index}.txt").as_bytes())
            .unwrap();
    }
    let name = alloc::format!("/many/{}", "x".repeat(255));
    drop(filesystem.create(name.as_bytes()).unwrap());
    assert_eq!(filesystem.read_dir(b"/many").unwrap()[0].name.len(), 255);
    filesystem.unlink(name.as_bytes()).unwrap();
    filesystem.unlink(b"/many").unwrap();
    assert_eq!(filesystem.statistics().unwrap().free_blocks, free);

    let file = filesystem.create(b"/persist.bin").unwrap();
    let mut data = [0; 12000];
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    file.write_at(0, &data).unwrap();
    let mut attributes = file.metadata().unwrap().attributes();
    attributes.modified = fs::Timestamp {
        seconds: 946684800,
        nanoseconds: 0,
    };
    attributes.changed = attributes.modified;
    attributes.permissions = 0o444;
    file.set_attributes(attributes).unwrap();
    assert_eq!(file.write_at(0, b"x"), Err(Error::ReadOnly));
    assert_eq!(filesystem.unlink(b"/persist.bin"), Err(Error::ReadOnly));
    assert_eq!(file.metadata().unwrap().modified, attributes.modified);
    attributes.permissions = 0o644;
    file.set_attributes(attributes).unwrap();
    file.fsync().unwrap();
}

fn no_space(disk: &Rc<Disk>) {
    let filesystem = Fat32::mount(Rc::clone(disk), 3 * SIZE, SIZE).unwrap();
    let free = filesystem.statistics().unwrap().free_blocks;
    assert_eq!(free, 4);
    let file = filesystem.create(b"/small").unwrap();
    assert_eq!(file.write_at(0, &[0; 4096]), Err(Error::NoSpace));
    assert_eq!(file.len(), 0);
    assert_eq!(filesystem.statistics().unwrap().free_blocks, free);
    file.write_at(0, b"ok").unwrap();
    assert_eq!(file.len(), 2);
    drop(file);
    filesystem.unlink(b"/small").unwrap();
    assert_eq!(filesystem.statistics().unwrap().free_blocks, free);
    filesystem.unmount().unwrap();
}

fn active_fat(disk: &Rc<Disk>) {
    let filesystem = Fat32::mount(Rc::clone(disk), 4 * SIZE, SIZE).unwrap();
    let file = filesystem.create(b"/active.bin").unwrap();
    file.write_at(0, b"active FAT").unwrap();
    drop(file);
    filesystem.unmount().unwrap();
    drop(filesystem);
    let filesystem = Fat32::mount(Rc::clone(disk), 4 * SIZE, SIZE).unwrap();
    let file = filesystem.open_bytes(b"/active.bin").unwrap();
    let mut bytes = [0; 10];
    file.read_exact_at(0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"active FAT");
    drop(file);
    filesystem.unmount().unwrap();
}

fn wide_sectors(disk: &Rc<Disk>) {
    let filesystem = Fat32::mount(Rc::clone(disk), 5 * SIZE, 8 * SIZE).unwrap();
    let file = filesystem.open_bytes(b"/wide.bin").unwrap();
    let mut bytes = [0; 12000];
    file.read_exact_at(0, &mut bytes).unwrap();
    assert!(
        bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == (index % 251) as u8)
    );
    drop(file);
    let file = filesystem.create(b"/wide-write.bin").unwrap();
    file.write_at(0, b"wide sectors").unwrap();
    file.fsync().unwrap();
    drop(file);
    filesystem.unmount().unwrap();
}

fn failure(disk: &Rc<Disk>) {
    let filesystem = Fat32::mount(Rc::clone(disk), 0, SIZE).unwrap();
    let free = filesystem.statistics().unwrap().free_blocks;
    if disk.capacity() == SIZE {
        assert_eq!(filesystem.create(b"/failed").err(), Some(Error::Io));
    } else {
        let file = filesystem.create(b"/flush").unwrap();
        file.write_at(0, b"before flush").unwrap();
        assert_eq!(file.fsync(), Err(Error::Io));
        assert_eq!(file.write_at(0, b"after flush"), Err(Error::Unavailable));
    }
    assert_eq!(filesystem.create(b"/after").err(), Some(Error::Unavailable));
    drop(filesystem);
    let filesystem = Fat32::mount(Rc::clone(disk), 0, SIZE).unwrap();
    if disk.capacity() == SIZE {
        assert_eq!(
            filesystem.open_bytes(b"/failed").err(),
            Some(Error::NotFound)
        );
        assert_eq!(filesystem.statistics().unwrap().free_blocks, free);
    } else {
        assert_eq!(filesystem.open_bytes(b"/flush").unwrap().len(), 12);
    }
    filesystem.unmount().unwrap();
}
