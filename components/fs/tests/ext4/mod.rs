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
        read_volume(&disk);
        for index in 1..7 {
            assert!(
                Ext4::mount(Rc::clone(&disk), index * SLICE, SLICE).is_err(),
                "corrupt volume {index}"
            );
        }
        drop(disk);
        mount_service(&control, id);
        drop(control);
        worker.join().unwrap();
        tested = true;
    }
    assert!(tested);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
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

fn mount_service(device: &runtime::Handle<runtime::Channel>, id: u32) {
    let (channel, server) = process::task()
        .channel(
            Rights::READ_WRITE | Rights::WAIT,
            Rights::READ | Rights::WAIT,
        )
        .unwrap();
    let worker = thread::spawn(move || fs::service::run(process::task(), server).unwrap()).unwrap();
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

fn deadline() -> u64 {
    thread::now_ns().unwrap() + 10_000_000_000
}
