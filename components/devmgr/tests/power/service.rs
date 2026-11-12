// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::deadline;
use devmgr::{
    Error,
    power::{self, Power, Session},
};
use runtime::{Device, Handle, Rights, ipc::Message, process, thread};

pub fn exercise(configuration: power::Configuration, id: u32, grant: Handle<Device>) {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (control, server) = process::task().channel(rights, rights).unwrap();
    let _worker = thread::spawn(move || {
        let mut power = Power::new(configuration, &grant).unwrap();
        power::service::run(&mut power, &server, id).unwrap();
    })
    .unwrap();
    assert!(matches!(
        Session::open(process::task(), &control, u32::MAX, deadline()),
        Err(Error::Unsupported)
    ));
    let session = Session::open(process::task(), &control, id, deadline()).unwrap();
    let capabilities = session.capabilities(deadline()).unwrap();
    assert!(capabilities.shutdown);
    assert!(capabilities.reboot);
    for (tag, words) in [(u64::MAX, &[][..]), (2, &[0][..]), (3, &[0][..])] {
        let response = session
            .channel()
            .call(&mut Message::new(tag, words).unwrap(), Some(deadline()))
            .unwrap();
        assert_eq!(response.tag, 1);
        assert!(response.words().is_empty());
        assert!(response.handles().is_empty());
    }
    assert!(session.capabilities(0).is_err());
    let _ = session.shutdown(deadline());
    // The write stops the virtual machine. If it does not, the caller must observe a timeout instead
    // of a successful test run.
    loop {
        thread::sleep_until(thread::now_ns().unwrap() + 1_000_000_000).unwrap();
    }
}
