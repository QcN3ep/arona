// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use runtime::{
    Error, KernelLog, Rights,
    ipc::ReceivedMessage,
    logger::{Level, MAX_RECORD_SIZE},
};

pub fn run(mut startup: ReceivedMessage) {
    let log = startup
        .take_handle(0)
        .unwrap()
        .downcast::<KernelLog>()
        .unwrap();
    assert!(startup.handles().is_empty());
    let denied = log.try_clone(Rights::NONE).unwrap();
    let mut bytes = [0; MAX_RECORD_SIZE];
    assert_eq!(
        denied.read(0, &mut bytes),
        Err(Error::Kernel(abi::Error::Denied))
    );
    assert_eq!(
        log.read(0, &mut []),
        Err(Error::Kernel(abi::Error::BufferTooSmall))
    );
    let early = log.read(0, &mut bytes).unwrap().unwrap();
    assert_eq!(early.sequence, 1);
    assert_eq!(early.timestamp_ns, None);
    assert_eq!(early.target, "kernel::boot");
    assert_eq!(early.message, "Early kernel log");
    assert!(!early.truncated);
    let cursor = early.sequence + 1;
    let other = log.try_clone(Rights::READ).unwrap();
    let mut snapshot = [0; MAX_RECORD_SIZE];
    assert_eq!(other.read(0, &mut snapshot).unwrap(), Some(early));
    let next = log.read(cursor, &mut bytes).unwrap().unwrap();
    assert_eq!(next.sequence, cursor);
    assert_eq!(next.level, Level::Warn);
    assert!(next.timestamp_ns.is_some());
    assert_eq!(next.target, "kernel::test");
    assert_eq!(next.message, "Timestamped kernel log");
    assert_eq!(log.read(u64::MAX, &mut bytes), Ok(None));
}
