// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use devmgr::{
    device::DeviceDescription,
    rng::{self, Rng, Session},
};
use runtime::{DeviceSet, Handle, ObjectType, Rights, ipc::WaitMode, process, thread};

pub(super) fn read(devices: &Handle<DeviceSet>) -> [u8; 32] {
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let device = DeviceDescription::new(devices, id).unwrap();
        let Some(configuration) = rng::probe(&device).unwrap() else {
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
        let rights = Rights::READ_WRITE | Rights::WAIT;
        let (control, server) = process::task().channel(rights, rights).unwrap();
        let worker = thread::spawn(move || {
            let mut rng =
                Rng::new(configuration, &grant, &domain, &event, process::task()).unwrap();
            rng::service::run(&mut rng, &event, &server, id).unwrap();
            domain.revoke().unwrap();
        })
        .unwrap();
        let deadline = thread::now_ns().unwrap() + 5_000_000_000;
        let session = Session::start_open(process::task(), &control, id)
            .unwrap()
            .wait(WaitMode::Until(deadline))
            .unwrap();
        let mut seed = [0; 32];
        session.fill(&mut seed, deadline).unwrap();
        drop(session);
        drop(control);
        worker.join().unwrap();
        return seed;
    }
    panic!("no entropy device available");
}
