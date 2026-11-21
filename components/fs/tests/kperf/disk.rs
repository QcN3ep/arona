// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Serve a real QEMU VirtIO disk on a native thread for filesystem cases.

use alloc::rc::Rc;
use devmgr::{
    block::{self, Block, Session},
    device::DeviceDescription,
};
use fs::block::Disk;
use runtime::{Channel, Handle, ObjectType, Rights, process, thread};

pub struct Driver {
    control: Option<Handle<Channel>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Driver {
    pub fn start() -> (Rc<Disk>, Self) {
        let devices = testkit::user::devices();
        let mut cursor = 0;
        let (id, configuration, grant) = loop {
            let id = devices.next(cursor).unwrap().expect("no performance disk");
            cursor = id;
            let device = DeviceDescription::new(devices, id).unwrap();
            if !device.matches("pci", "1af4:1042").unwrap() {
                continue;
            }
            if let Some(configuration) = block::probe(&device).unwrap() {
                let grant = devices
                    .claim_group(
                        device.info().group.unwrap(),
                        ObjectType::Device.supported_rights(),
                    )
                    .unwrap();
                break (id, configuration, grant);
            }
        };
        let domain = grant
            .create_dma(id, ObjectType::DmaDomain.supported_rights())
            .unwrap();
        let event = process::task()
            .event(ObjectType::Event.supported_rights())
            .unwrap();
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
        (
            disk,
            Self {
                control: Some(control),
                worker: Some(worker),
            },
        )
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        drop(self.control.take());
        self.worker.take().unwrap().join().unwrap();
    }
}
