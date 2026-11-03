// SPDX-License-Identifier: MPL-2.0

use devmgr::{
    Error,
    block::{self, Block, Configuration, Session},
};
use runtime::{DeviceSet, Handle, ObjectType, Rights, process, thread};

pub fn run(devices: &Handle<DeviceSet>, id: u32, group: u32, configuration: Configuration) {
    let grant = devices
        .claim_group(group, ObjectType::Device.supported_rights())
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
        let mut block =
            Block::new(configuration, &grant, &domain, &event, process::task()).unwrap();
        block::service::run(&mut block, &event, &server, id).unwrap();
        domain.revoke().unwrap();
    })
    .unwrap();
    assert!(matches!(
        Session::open(process::task(), &control, id + 1000),
        Err(Error::Unsupported)
    ));
    let mut session = Session::open(process::task(), &control, id).unwrap();
    assert!(matches!(
        Session::open(process::task(), &control, id),
        Err(Error::Busy)
    ));
    let info = session.info().unwrap();
    let mut input = super::frames();
    input.write(0, &[0x79; 4096]).unwrap();
    let output = super::frames();
    session.write(1, 4096, &input, 0, 4096).unwrap();
    session.flush(2).unwrap();
    session.read(3, 4096, &output, 0, 4096).unwrap();
    for (id, bytes) in [(3, 4096), (1, 4096), (2, 0)] {
        assert_eq!(
            session.acknowledge(id, deadline()).unwrap(),
            super::success(bytes)
        );
    }
    let mut bytes = [0; 4096];
    output.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x79; 4096]);
    assert_eq!(session.try_acknowledge(3), Err(Error::NotFound));
    assert_eq!(
        session.read(4, 1, &output, 0, 512),
        Err(Error::InvalidRequest)
    );
    for id in 0..info.max_requests as u64 {
        session.read(id, 0, &output, 0, 512).unwrap();
    }
    assert_eq!(session.read(100, 0, &output, 0, 512), Err(Error::Busy));
    for id in 0..info.max_requests as u64 {
        session.acknowledge(id, deadline()).unwrap();
    }
    for id in 0..info.max_requests as u64 {
        session.read(id, 0, &output, 0, 512).unwrap();
    }
    drop(session);
    session = reconnect(&control, id);
    assert_eq!(session.info().unwrap(), info);
    session.read(1, 0, &output, 0, 512).unwrap();
    assert_eq!(
        session.acknowledge(1, deadline()).unwrap(),
        super::success(512)
    );
    // 监督者关闭控制通道时，服务仍须排空当前会话的在途请求。
    session.read(2, 0, &output, 0, 512).unwrap();
    drop(control);
    worker.join().unwrap();
    assert!(session.acknowledge(2, deadline()).is_err());
    drop(session);
    drop(
        devices
            .claim_group(group, ObjectType::Device.supported_rights())
            .unwrap(),
    );
}

fn reconnect(control: &Handle<runtime::Channel>, id: u32) -> Session {
    let deadline = deadline().unwrap();
    loop {
        match Session::open(process::task(), control, id) {
            Ok(session) => return session,
            Err(Error::Busy) => {
                assert!(thread::now_ns().unwrap() < deadline);
                thread::yield_now().unwrap();
            }
            Err(error) => panic!("block session reconnect failed: {error:?}"),
        }
    }
}

fn deadline() -> Option<u64> {
    Some(thread::now_ns().unwrap() + 10 * super::SECOND)
}
