// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::rc::Rc;

use devmgr::{
    block::{self, Block, Session},
    device::DeviceDescription,
};
use fs::{Error, FileSystem, ReadAt, block::Disk, ext4::Ext4};
use runtime::{
    DeviceSet, ObjectType, Rights,
    ipc::{Message, WaitMode},
    process, thread,
};

const SLICE: u64 = 16 * 1024 * 1024;

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let devices = bootstrap
        .receive(WaitMode::Poll)
        .unwrap()
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    archive_metadata();
    let mut cursor = 0;
    let mut tested = false;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let device = DeviceDescription::new(&devices, id).unwrap();
        let Some(configuration) = block::probe(&device).unwrap() else {
            continue;
        };
        let grant = devices
            .claim_group(
                device.info().group.unwrap(),
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
        let writable = !disk.read_only() && disk.capacity() >= 8 * SLICE;
        drop(disk);
        if writable {
            mount_service(&control, id);
        }
        drop(control);
        worker.join().unwrap();
        tested = true;
    }
    assert!(tested);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

fn exercise(disk: &Rc<Disk>) {
    if disk.read_only() {
        read_only(disk);
    } else if matches!(disk.capacity() / SLICE, 2 | 3 | 5) {
        failure(disk);
    } else if disk.capacity() == 4 * SLICE {
        assert!(matches!(
            Ext4::mount(Rc::clone(disk), 0, SLICE),
            Err(Error::InvalidData)
        ));
        let mut bytes = [0; 4096];
        disk.read(0, &mut bytes).unwrap();
        assert_eq!(&bytes[1082..1084], &[0, 0]);
    } else if disk.capacity() == 6 * SLICE {
        assert!(matches!(
            Ext4::mount(Rc::clone(disk), 0, SLICE),
            Err(Error::Io)
        ));
        failure(disk);
    } else {
        if !persisted(disk) {
            read_volume(disk);
            write_volume(disk);
            metadata_volume(disk);
            persistence(disk);
            dirty_volume(disk);
            recover_orphan(disk);
            no_journal(disk);
            large_write(disk);
        }
        for index in 1..7 {
            assert!(
                Ext4::mount(Rc::clone(disk), index * SLICE, SLICE).is_err(),
                "corrupt volume {index}"
            );
        }
    }
}

fn read_volume(disk: &Rc<Disk>) {
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    let file = filesystem.open("/data/message").unwrap();
    assert_eq!(file.len(), 12000);
    let mut bytes = alloc::vec![0; file.len()];
    assert_eq!(file.read_at(0, &mut bytes).unwrap(), bytes.len());
    for (index, byte) in bytes.iter().enumerate() {
        assert_eq!(*byte, (index % 251) as u8);
    }
    let mut tail = [0; 13];
    assert_eq!(file.read_at(11997, &mut tail).unwrap(), 3);
    assert_eq!(file.read_at(12000, &mut tail).unwrap(), 0);
    assert!(matches!(filesystem.open("/data"), Err(Error::IsDirectory)));
    assert!(matches!(filesystem.open("/missing"), Err(Error::NotFound)));
    assert!(matches!(
        filesystem.open("/../data/message"),
        Err(Error::InvalidRequest)
    ));
    let sparse = filesystem.open("/data/sparse").unwrap();
    let mut hole = [1; 4096];
    sparse.read_exact_at(4096, &mut hole).unwrap();
    assert_eq!(hole, [0; 4096]);
    let tree = filesystem.open("/data/tree").unwrap();
    let mut zeros = alloc::vec![1; tree.len()];
    tree.read_exact_at(0, &mut zeros).unwrap();
    assert!(zeros.iter().all(|byte| *byte == 0));
    let file = filesystem.open_bytes(b"/data/\xff").unwrap();
    file.read_exact_at(0, &mut tail[..3]).unwrap();
    assert_eq!(&tail[..3], b"raw");
}

fn start_service() -> (fs::Control<'static>, thread::JoinHandle<()>) {
    let (channel, server) = process::task()
        .channel(
            Rights::READ_WRITE | Rights::WAIT,
            Rights::READ | Rights::WAIT,
        )
        .unwrap();
    let worker = thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || fs::service::run(process::task(), server).unwrap())
        .unwrap();
    let mut archive = [b'0'; 124];
    archive[..6].copy_from_slice(b"070701");
    archive[94..102].copy_from_slice(b"0000000b");
    archive[110..121].copy_from_slice(b"TRAILER!!!\0");
    archive[121..].fill(0);
    let mut frames = process::task()
        .allocate(1, Rights::READ_WRITE | Rights::TRANSFER)
        .unwrap();
    frames.write(0, &archive).unwrap();
    let control =
        fs::Control::start(process::task(), channel, frames, archive.len(), deadline()).unwrap();
    (control, worker)
}

fn mount_service(device: &runtime::Handle<runtime::Channel>, id: u32) {
    let (control, worker) = start_service();
    let client = control.connect(deadline()).unwrap();
    control
        .mount(
            b"/var",
            Session::open(process::task(), device, id).unwrap(),
            0,
            SLICE,
            deadline(),
        )
        .unwrap();
    let file = client.open("/var/data/message", deadline()).unwrap();
    let mut bytes = [0; 32];
    assert_eq!(
        file.read_at(4091, &mut bytes, deadline()).unwrap(),
        bytes.len()
    );
    for (index, byte) in bytes.iter().enumerate() {
        assert_eq!(*byte, ((4091 + index) % 251) as u8);
    }
    assert!(matches!(
        control.unmount(b"/var", deadline()),
        Err(Error::Busy)
    ));
    let raw = client.open_bytes(b"/var/data/\xff", deadline()).unwrap();
    assert_eq!(raw.read_at(0, &mut bytes, deadline()).unwrap(), 3);
    assert_eq!(&bytes[..3], b"raw");
    service_metadata(&client);
    client.mkdir("/var/session", deadline()).unwrap();
    let writable = client.create("/var/session/file", deadline()).unwrap();
    assert_eq!(writable.write_at(5, b"hello", deadline()).unwrap(), 5);
    assert_eq!(writable.size(deadline()).unwrap(), 10);
    let mut output = [1; 10];
    assert_eq!(writable.read_at(0, &mut output, deadline()).unwrap(), 10);
    assert_eq!(&output, b"\0\0\0\0\0hello");
    writable.truncate(7, deadline()).unwrap();
    assert_eq!(writable.size(deadline()).unwrap(), 7);
    assert_eq!(
        client.read_dir("/var/session", deadline()).unwrap().len(),
        1
    );
    client
        .rename("/var/session/file", "/var/session/renamed", deadline())
        .unwrap();
    client.unlink("/var/session/renamed", deadline()).unwrap();
    writable.fsync(deadline()).unwrap();
    drop(writable);
    client.unlink("/var/session", deadline()).unwrap();
    assert!(matches!(
        client.create("/archive-file", deadline()),
        Err(Error::ReadOnly)
    ));
    drop(raw);
    drop(file);
    control.unmount(b"/var", deadline()).unwrap();
    assert!(matches!(
        client.open("/var/data/message", deadline()),
        Err(Error::NotFound)
    ));
    drop(client);
    drop(control);
    worker.join().unwrap();
}

fn service_metadata(client: &fs::Client<'_>) {
    client.mkdir_bytes(b"/var/\xfe", deadline()).unwrap();
    let directory = client.open_directory(b"/var/\xfe", deadline()).unwrap();
    let file = client.create_bytes(b"/var/\xfe/\xff", deadline()).unwrap();
    let mut attributes = file.metadata(deadline()).unwrap().attributes();
    attributes.permissions = 0o640;
    attributes.uid = 100_000;
    attributes.modified = fs::Timestamp {
        seconds: 4_000_000_000,
        nanoseconds: 987_654_321,
    };
    file.set_attributes(attributes, deadline()).unwrap();
    assert_eq!(
        client
            .metadata(b"/var/\xfe/\xff", deadline())
            .unwrap()
            .attributes(),
        attributes
    );
    client
        .hard_link(b"/var/\xfe/\xff", b"/var/\xfe/link", deadline())
        .unwrap();
    assert_eq!(file.metadata(deadline()).unwrap().links, 2);
    assert_eq!(file.append(b"first", deadline()).unwrap(), (0, 5));
    let other = client.open_bytes(b"/var/\xfe/link", deadline()).unwrap();
    assert_eq!(other.append(b"second", deadline()).unwrap(), (5, 6));
    client
        .symlink(b"../\xfe/link", b"/var/\xfe/symlink", deadline())
        .unwrap();
    assert_eq!(
        client.read_link(b"/var/\xfe/symlink", deadline()).unwrap(),
        b"../\xfe/link"
    );
    client
        .rename_bytes(b"/var/\xfe", b"/var/renamed", deadline())
        .unwrap();
    assert_eq!(directory.read_dir(deadline()).unwrap().len(), 3);
    assert_eq!(
        client
            .read_dir_bytes(b"/var/renamed", deadline())
            .unwrap()
            .len(),
        3
    );
    client
        .unlink_bytes(b"/var/renamed/\xff", deadline())
        .unwrap();
    client
        .unlink_bytes(b"/var/renamed/link", deadline())
        .unwrap();
    client
        .unlink_bytes(b"/var/renamed/symlink", deadline())
        .unwrap();
    assert_eq!(file.metadata(deadline()).unwrap().links, 0);
    let mut bytes = [0; 11];
    file.read_at(0, &mut bytes, deadline()).unwrap();
    assert_eq!(&bytes, b"firstsecond");
    drop(other);
    drop(file);
    client.unlink_bytes(b"/var/renamed", deadline()).unwrap();
    assert!(directory.read_dir(deadline()).unwrap().is_empty());
    drop(directory);
}

fn deadline() -> u64 {
    thread::now_ns().unwrap() + 10_000_000_000
}

fn write_volume(disk: &Rc<Disk>) {
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    directory_operations(&filesystem);
    filesystem.mkdir(b"/work").unwrap();
    filesystem.mkdir(b"/work/child").unwrap();
    let file = filesystem.create(b"/work/file").unwrap();
    let other = filesystem.open("/work/file").unwrap();
    let data: alloc::vec::Vec<u8> = (0..9000).map(|index| (index % 239) as u8).collect();
    assert_eq!(file.write_at(4090, &data).unwrap(), data.len());
    assert_eq!(other.len(), 13090);
    let mut bytes = alloc::vec![1; other.len()];
    other.read_exact_at(0, &mut bytes).unwrap();
    assert!(bytes[..4090].iter().all(|byte| *byte == 0));
    assert_eq!(&bytes[4090..], &data);
    file.truncate(5000).unwrap();
    assert_eq!(other.len(), 5000);
    file.truncate(9000).unwrap();
    bytes.resize(9000, 1);
    other.read_exact_at(0, &mut bytes).unwrap();
    assert!(bytes[5000..].iter().all(|byte| *byte == 0));
    for index in 0..9 {
        file.write_at(index * 8192, &[index as u8 + 1]).unwrap();
    }
    for index in 0..9 {
        let mut byte = [0];
        other.read_exact_at(index * 8192, &mut byte).unwrap();
        assert_eq!(byte, [index as u8 + 1]);
    }
    assert_eq!(
        file.write_at(0, &alloc::vec![0; SLICE as usize]),
        Err(Error::NoSpace)
    );
    filesystem
        .rename(b"/work/file", b"/work/child/moved")
        .unwrap();
    assert!(matches!(
        filesystem.open("/work/file"),
        Err(Error::NotFound)
    ));
    assert_eq!(filesystem.unlink(b"/work/child"), Err(Error::NotEmpty));
    assert_eq!(
        filesystem.rename(b"/work", b"/work/child/cycle"),
        Err(Error::InvalidRequest)
    );
    filesystem.unlink(b"/work/child/moved").unwrap();
    assert!(matches!(
        filesystem.open("/work/child/moved"),
        Err(Error::NotFound)
    ));
    file.write_at(0, b"open after unlink").unwrap();
    let mut head = [0; 17];
    other.read_exact_at(0, &mut head).unwrap();
    assert_eq!(&head, b"open after unlink");
    drop(file);
    drop(other);
    filesystem.unlink(b"/work/child").unwrap();
    filesystem.unlink(b"/work").unwrap();
    let tree = filesystem.open("/data/tree").unwrap();
    tree.write_at(4096 * 4 + 3, b"tree").unwrap();
    let mut text = [0; 4];
    tree.read_exact_at(4096 * 4 + 3, &mut text).unwrap();
    assert_eq!(&text, b"tree");
    drop(tree);
    filesystem.unmount().unwrap();
}

fn metadata_volume(disk: &Rc<Disk>) {
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    filesystem.mkdir(b"/links").unwrap();
    let file = filesystem.create(b"/links/\xff").unwrap();
    let attributes = fs::Attributes {
        permissions: 0o6751,
        uid: 0x1234_5678,
        gid: 0xabcd_0123,
        accessed: fs::Timestamp {
            seconds: -1,
            nanoseconds: 123_456_789,
        },
        modified: fs::Timestamp {
            seconds: 4_000_000_000,
            nanoseconds: 999_999_999,
        },
        changed: fs::Timestamp {
            seconds: 1_000_000,
            nanoseconds: 42,
        },
    };
    file.set_attributes(attributes).unwrap();
    let before = file.metadata().unwrap();
    assert_eq!(before.attributes(), attributes);
    let mut invalid = attributes;
    invalid.modified.seconds = i64::MAX;
    assert_eq!(file.set_attributes(invalid), Err(Error::Unsupported));
    assert_eq!(file.metadata().unwrap(), before);
    filesystem
        .hard_link(b"/links/\xff", b"/links/alias")
        .unwrap();
    let alias = filesystem.open_bytes(b"/links/alias").unwrap();
    assert_eq!(alias.metadata().unwrap().inode, before.inode);
    assert_eq!(alias.metadata().unwrap().links, 2);
    assert_eq!(file.append(b"first").unwrap(), (0, 5));
    assert_eq!(alias.append(b"second").unwrap(), (5, 6));
    filesystem.unlink(b"/links/\xff").unwrap();
    assert_eq!(file.metadata().unwrap().links, 1);
    let mut bytes = [0; 11];
    file.read_exact_at(0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"firstsecond");
    filesystem
        .symlink(b"../links/alias", b"/links/short")
        .unwrap();
    let long = alloc::vec![b'x'; 200];
    filesystem.symlink(&long, b"/links/long").unwrap();
    filesystem
        .hard_link(b"/links/short", b"/links/short-alias")
        .unwrap();
    assert_eq!(filesystem.metadata(b"/links/short").unwrap().links, 2);
    assert_eq!(
        filesystem.read_link(b"/links/short").unwrap(),
        b"../links/alias"
    );
    assert_eq!(filesystem.read_link(b"/links/long").unwrap(), long);
    file.fsync().unwrap();
    drop(file);
    drop(alias);
    filesystem.unmount().unwrap();
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    assert_eq!(
        filesystem.metadata(b"/links/alias").unwrap().attributes(),
        attributes
    );
    assert_eq!(
        filesystem.read_link(b"/links/short-alias").unwrap(),
        b"../links/alias"
    );
    assert_eq!(filesystem.read_link(b"/links/long").unwrap(), long);
    directory_session(&filesystem);
    filesystem.unmount().unwrap();
}

fn directory_session(filesystem: &Ext4) {
    filesystem.mkdir(b"/session").unwrap();
    filesystem.create(b"/session/entry").unwrap();
    let directory = filesystem.open_directory(b"/session").unwrap();
    let number = directory.metadata().unwrap().inode;
    filesystem.rename(b"/session", b"/renamed-session").unwrap();
    assert_eq!(directory.metadata().unwrap().inode, number);
    assert_eq!(directory.read_dir().unwrap().len(), 1);
    assert_eq!(filesystem.unmount(), Err(Error::Busy));
    filesystem.unlink(b"/renamed-session/entry").unwrap();
    filesystem.unlink(b"/renamed-session").unwrap();
    assert_eq!(directory.metadata().unwrap().links, 0);
    assert!(directory.read_dir().unwrap().is_empty());
    drop(directory);
}

fn persistence(disk: &Rc<Disk>) {
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    assert!(matches!(
        Ext4::mount(Rc::clone(disk), 0, SLICE),
        Err(Error::Busy)
    ));
    let file = filesystem.create(b"/persistent").unwrap();
    file.write_at(4093, b"survives reboot").unwrap();
    file.fsync().unwrap();
    assert_eq!(filesystem.unmount(), Err(Error::Busy));
    drop(file);
    filesystem.unmount().unwrap();
    assert_eq!(filesystem.mkdir(b"/closed"), Err(Error::Unavailable));
    assert!(persisted(disk));
}

fn persisted(disk: &Rc<Disk>) -> bool {
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    let file = match filesystem.open("/persistent") {
        Ok(file) => file,
        Err(Error::NotFound) => return false,
        Err(error) => panic!("persistent file failed: {error:?}"),
    };
    let mut bytes = [1; 4108];
    file.read_exact_at(0, &mut bytes).unwrap();
    assert!(bytes[..4093].iter().all(|byte| *byte == 0));
    assert_eq!(&bytes[4093..], b"survives reboot");
    drop(file);
    filesystem.unmount().unwrap();
    true
}

fn dirty_volume(disk: &Rc<Disk>) {
    let filesystem = Ext4::mount(Rc::clone(disk), 7 * SLICE, SLICE).unwrap();
    let file = filesystem.create(b"/unmounted").unwrap();
    file.write_at(0, b"dirty").unwrap();
    file.fsync().unwrap();
    drop(file);
    drop(filesystem);
    let recovered = Ext4::mount(Rc::clone(disk), 7 * SLICE, SLICE).unwrap();
    let file = recovered.open("/unmounted").unwrap();
    let mut bytes = [0; 5];
    file.read_exact_at(0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"dirty");
    drop(file);
    recovered.unmount().unwrap();
}

fn read_only(disk: &Rc<Disk>) {
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    let file = filesystem.open("/data/message").unwrap();
    assert_eq!(file.write_at(0, b"denied"), Err(Error::ReadOnly));
    assert_eq!(filesystem.mkdir(b"/denied"), Err(Error::ReadOnly));
    file.fsync().unwrap();
    drop(file);
    filesystem.unmount().unwrap();
}

fn failure(disk: &Rc<Disk>) {
    let mut superblock = [0; 4096];
    disk.read(0, &mut superblock).unwrap();
    let recovering = superblock[1082..1084] == [0, 0];
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    let file = filesystem.open("/data/message").unwrap();
    if recovering {
        let mut bytes = [0; 7];
        file.read_exact_at(0, &mut bytes).unwrap();
        if disk.capacity() == 5 * SLICE {
            assert_eq!(bytes, [0, 1, 2, 3, 4, 5, 6]);
        } else {
            assert_eq!(&bytes, b"\xc0\x3b\x39\x98log");
            if matches!(disk.capacity() / SLICE, 2 | 6) {
                let mut output = alloc::vec![0; 1024 * 1024 + 4096];
                file.read_exact_at(0, &mut output).unwrap();
                assert!(output[7..].iter().all(|byte| *byte == 0x45));
            }
        }
        file.fsync().unwrap();
        drop(file);
        filesystem.unmount().unwrap();
    } else {
        let mut input =
            alloc::vec![0x45; if disk.capacity() == 2 * SLICE { 1024 * 1024 + 4096 } else { 7 }];
        input[..7].copy_from_slice(b"\xc0\x3b\x39\x98log");
        assert_eq!(file.write_at(0, &input), Err(Error::Io));
        assert_eq!(file.fsync(), Err(Error::Io));
        drop(file);
        assert_eq!(filesystem.unmount(), Err(Error::Io));
    }
}

fn recover_orphan(disk: &Rc<Disk>) {
    let start = 8 * SLICE;
    let mut before = [0; 4096];
    disk.read(start, &mut before).unwrap();
    assert_ne!(&before[1256..1260], &[0; 4]);
    let filesystem = Ext4::mount(Rc::clone(disk), start, SLICE).unwrap();
    assert!(matches!(filesystem.open("/orphan"), Err(Error::NotFound)));
    filesystem.unmount().unwrap();
    let mut after = [0; 4096];
    disk.read(start, &mut after).unwrap();
    assert_eq!(&after[1256..1260], &[0; 4]);
    for offset in [1036, 1040] {
        let previous = u32::from_le_bytes(before[offset..offset + 4].try_into().unwrap());
        let current = u32::from_le_bytes(after[offset..offset + 4].try_into().unwrap());
        assert_eq!(current - previous, if offset == 1036 { 3 } else { 1 });
    }
}

fn no_journal(disk: &Rc<Disk>) {
    let filesystem = Ext4::mount(Rc::clone(disk), 9 * SLICE, SLICE).unwrap();
    let file = filesystem.open("/data/message").unwrap();
    assert_eq!(file.len(), 12000);
    assert_eq!(file.write_at(0, b"denied"), Err(Error::ReadOnly));
    assert_eq!(filesystem.mkdir(b"/denied"), Err(Error::ReadOnly));
    drop(file);
    filesystem.unmount().unwrap();
}

fn large_write(disk: &Rc<Disk>) {
    let filesystem = Ext4::mount(Rc::clone(disk), 0, SLICE).unwrap();
    let file = filesystem.create(b"/large").unwrap();
    let size = 4 * 1024 * 1024 + 4096;
    let bytes: alloc::vec::Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
    let first = file.write_at(0, &bytes).unwrap();
    assert!(first > 0);
    assert!(first < bytes.len());
    let mut done = first;
    while done < bytes.len() {
        let count = file.write_at(done, &bytes[done..]).unwrap();
        assert!(count > 0);
        done += count;
    }
    file.fsync().unwrap();
    for offset in [0, bytes.len() / 2, bytes.len() - 4096] {
        let mut output = [0; 4096];
        file.read_exact_at(offset, &mut output).unwrap();
        assert_eq!(output, bytes[offset..offset + output.len()]);
    }
    drop(file);
    filesystem.unlink(b"/large").unwrap();
    filesystem.unmount().unwrap();
}

fn directory_operations(filesystem: &Ext4) {
    filesystem.mkdir(b"/many").unwrap();
    for index in 0..20 {
        let mut path = alloc::vec![b'x'; 240];
        path[..6].copy_from_slice(b"/many/");
        path[6] = b'a' + index;
        drop(filesystem.create(&path).unwrap());
    }
    assert_eq!(filesystem.read_dir(b"/many").unwrap().len(), 20);
    for index in 0..20 {
        let mut path = alloc::vec![b'x'; 240];
        path[..6].copy_from_slice(b"/many/");
        path[6] = b'a' + index;
        filesystem.unlink(&path).unwrap();
    }
    filesystem.mkdir(b"/many/left").unwrap();
    filesystem.mkdir(b"/many/right").unwrap();
    filesystem.mkdir(b"/many/left/child").unwrap();
    filesystem.mkdir(b"/many/right/empty").unwrap();
    filesystem
        .rename(b"/many/left/child", b"/many/right/moved")
        .unwrap();
    filesystem
        .rename(b"/many/right/moved", b"/many/right/empty")
        .unwrap();
    assert!(filesystem.read_dir(b"/many/left").unwrap().is_empty());
    assert_eq!(filesystem.read_dir(b"/many/right").unwrap().len(), 1);
    filesystem.unlink(b"/many/right/empty").unwrap();
    filesystem.unlink(b"/many/left").unwrap();
    filesystem.unlink(b"/many/right").unwrap();
    let first = filesystem.create(b"/many/first").unwrap();
    first.write_at(0, b"first").unwrap();
    let second = filesystem.create(b"/many/second").unwrap();
    second.write_at(0, b"second").unwrap();
    filesystem.rename(b"/many/first", b"/many/second").unwrap();
    let mut text = [0; 6];
    second.read_exact_at(0, &mut text).unwrap();
    assert_eq!(&text, b"second");
    assert_eq!(filesystem.open("/many/second").unwrap().len(), 5);
    drop(first);
    drop(second);
    filesystem.unlink(b"/many/second").unwrap();
    filesystem.unlink(b"/many").unwrap();
}

fn archive_metadata() {
    let mut bytes = alloc::vec::Vec::new();
    for (name, data, inode, mode, links) in [
        (b"data/first".as_slice(), b"".as_slice(), 7, 0o100640, 2),
        (b"data/second", b"linked", 7, 0o100640, 2),
        (b"data/symlink", b"../data/first", 8, 0o120777, 1),
        (b"TRAILER!!!", b"", 0, 0, 0),
    ] {
        bytes.extend_from_slice(b"070701");
        for field in [
            inode,
            mode,
            123,
            456,
            links,
            12345,
            data.len() as u32,
            0,
            0,
            0,
            0,
            name.len() as u32 + 1,
            0,
        ] {
            bytes.extend_from_slice(alloc::format!("{field:08x}").as_bytes());
        }
        bytes.extend_from_slice(name);
        bytes.push(0);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        bytes.extend_from_slice(data);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
    }
    let mut frames = process::task()
        .allocate(bytes.len().div_ceil(4096) as u64, Rights::READ_WRITE)
        .unwrap();
    frames.write(0, &bytes).unwrap();
    let archive = fs::archive::Archive::new(frames, bytes.len()).unwrap();
    let first = archive.open_bytes(b"data/first").unwrap();
    let second = archive.open_bytes(b"data/second").unwrap();
    assert_eq!(first.metadata(), second.metadata());
    assert_eq!(first.metadata().uid, 123);
    assert_eq!(first.metadata().gid, 456);
    assert_eq!(first.metadata().mode, 0o100640);
    assert_eq!(first.metadata().modified.seconds, 12345);
    let mut output = [0; 6];
    first.read_exact_at(0, &mut output).unwrap();
    assert_eq!(&output, b"linked");
    assert_eq!(
        archive.read_link(b"data/symlink").unwrap(),
        b"../data/first"
    );
    assert_eq!(archive.open_directory(b"data").unwrap().read_dir().len(), 3);
    assert_eq!(archive.open_directory(b"/").unwrap().read_dir().len(), 1);
}
