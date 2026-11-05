// SPDX-License-Identifier: MPL-2.0

use runtime::{
    Error, FrameAllocation, Handle, ObjectType, Rights,
    ipc::{Message, ReceivedMessage, WaitMode},
    memory::Permissions,
    process::{self, ChildTask, ProcessStart},
    thread,
};

const PAGE_SIZE: usize = 4096;
const STACK_SIZE: usize = 64 * 1024;

pub fn run(mut startup: ReceivedMessage) {
    let image = startup
        .take_handle(0)
        .unwrap()
        .downcast::<FrameAllocation>()
        .unwrap();
    let mut header = [0; 64];
    image.read(0, &mut header).unwrap();
    let entry = word(&header, 24) as usize;
    frame_io();
    remote_memory();
    rollback();
    let mut child = ChildTask::new(process::task(), ProcessStart::REQUIRED_RIGHTS).unwrap();
    let count = u16::from_le_bytes(header[56..58].try_into().unwrap());
    for index in 0..count {
        load(
            &mut child,
            &image,
            word(&header, 32) as usize + index as usize * 56,
        );
    }
    let reservation = child.reserve(PAGE_SIZE, PAGE_SIZE, None).unwrap();
    let (sender, receiver) = process::task()
        .channel(
            ObjectType::Channel.supported_rights(),
            ObjectType::Channel.supported_rights(),
        )
        .unwrap();
    sender
        .send(
            &mut Message::new(8, &[reservation.start as u64]).unwrap(),
            WaitMode::Forever,
        )
        .unwrap();
    let task = child
        .start(entry, STACK_SIZE, Some((receiver, Rights::READ_WRITE)))
        .unwrap();
    let deadline = thread::now_ns().unwrap() + 2_000_000_000;
    let report = sender.receive(WaitMode::Until(deadline)).unwrap();
    assert_eq!(report.tag, 73);
    assert_eq!(report.words(), &[37, 41]);
    assert_eq!(task.wait(deadline).unwrap(), 0);
    assert_eq!(super::INITIALIZED.get(), 37);
    assert_eq!(super::ZERO.get(), 41);
}

fn remote_memory() {
    let task = process::task()
        .create_task(
            Rights::MAP_MEMORY
                | Rights::READ_MEMORY
                | Rights::WRITE_MEMORY
                | Rights::TERMINATE
                | Rights::DUPLICATE,
        )
        .unwrap();
    let mut frames = process::task()
        .allocate(3, Rights::READ_WRITE | Rights::MAP)
        .unwrap();
    let mapping = task
        .map(&frames, 0, 3, None, Permissions::ReadWrite)
        .unwrap();
    let range = mapping.range().unwrap();
    let expected: alloc::vec::Vec<u8> = (0..PAGE_SIZE + 17)
        .map(|index| (index % 251) as u8)
        .collect();
    task.write_memory(range.start + PAGE_SIZE - 1, &expected)
        .unwrap();
    let mut actual = alloc::vec![0; expected.len()];
    task.read_memory(range.start + PAGE_SIZE - 1, &mut actual)
        .unwrap();
    assert_eq!(actual, expected);
    frames.read(PAGE_SIZE - 1, &mut actual).unwrap();
    assert_eq!(actual, expected);
    let restricted = task.try_clone(Rights::NONE).unwrap();
    assert_eq!(
        restricted.read_memory(range.start, &mut actual),
        Err(Error::Kernel(abi::Error::Denied))
    );
    assert_eq!(
        restricted.write_memory(range.start, &actual),
        Err(Error::Kernel(abi::Error::Denied))
    );
    assert_eq!(
        task.write_memory(range.end - 1, &[1, 2]),
        Err(Error::Kernel(abi::Error::BadAddress))
    );
    assert_eq!(
        task.read_memory(usize::MAX, &mut actual),
        Err(Error::Kernel(abi::Error::BadAddress))
    );
    assert_eq!(actual, expected);
    let mut tail = [0; 1];
    frames.read(3 * PAGE_SIZE - 1, &mut tail).unwrap();
    assert_eq!(tail, [0]);
    task.write_memory(0, &[]).unwrap();
    task.read_memory(0, &mut []).unwrap();
    drop(mapping);
    task.terminate(0).unwrap();
    assert_eq!(
        task.read_memory(range.start, &mut actual),
        Err(Error::Kernel(abi::Error::BadAddress))
    );
    assert_eq!(
        process::task().write_memory(actual.as_ptr() as usize, &[0]),
        Err(Error::Kernel(abi::Error::Denied))
    );
    frames.write(0, b"still owned").unwrap();
}

fn frame_io() {
    let mut frames = process::task().allocate(3, Rights::READ_WRITE).unwrap();
    let mut expected = alloc::vec![0xa5; 3 * PAGE_SIZE];
    frames.write(0, &expected).unwrap();
    let input: alloc::vec::Vec<u8> = (0..PAGE_SIZE + 17)
        .map(|index| (index % 251) as u8)
        .collect();
    let mut output = alloc::vec![0; expected.len() + 2];
    for offset in [0, 1, 7, 8, PAGE_SIZE - 1, PAGE_SIZE, PAGE_SIZE + 1] {
        frames.write(offset, &input[1..]).unwrap();
        expected[offset..offset + input.len() - 1].copy_from_slice(&input[1..]);
        frames.read(0, &mut output[1..expected.len() + 1]).unwrap();
        assert_eq!(&output[1..expected.len() + 1], expected);
        assert_eq!(output[0], 0);
        assert_eq!(output[expected.len() + 1], 0);
    }
    frames.read(expected.len(), &mut []).unwrap();
    frames.write(expected.len(), &[]).unwrap();
}

pub fn child(startup: ReceivedMessage) {
    assert_eq!(
        process::task().info().unwrap().rights,
        ProcessStart::REQUIRED_RIGHTS
    );
    assert!(matches!(
        process::task().event(Rights::SIGNAL),
        Err(Error::Kernel(abi::Error::Denied))
    ));
    assert_eq!(
        process::task()
            .query(startup.words()[0] as usize)
            .unwrap()
            .unwrap()
            .permissions,
        None
    );
    let local = 0;
    let mut range = process::task()
        .query(core::ptr::addr_of!(local) as usize)
        .unwrap()
        .unwrap()
        .range;
    while process::task()
        .query(range.start - PAGE_SIZE)
        .unwrap()
        .unwrap()
        .permissions
        .is_some()
    {
        range.start -= PAGE_SIZE;
    }
    assert!(
        (range.start..range.start + STACK_SIZE).contains(&(core::ptr::addr_of!(local) as usize))
    );
    for offset in (0..STACK_SIZE).step_by(PAGE_SIZE) {
        assert_eq!(
            process::task()
                .query(range.start + offset)
                .unwrap()
                .unwrap()
                .permissions,
            Some(Permissions::ReadWrite)
        );
    }
    let guard = process::task()
        .query(range.start - PAGE_SIZE)
        .unwrap()
        .unwrap();
    assert_eq!(guard.range, range.start - PAGE_SIZE..range.start);
    assert_eq!(guard.permissions, None);
    thread::yield_now().unwrap();
}

fn rollback() {
    assert!(matches!(
        ChildTask::new(process::task(), Rights::NONE),
        Err(Error::Kernel(abi::Error::InvalidArgument))
    ));
    let parent = process::task().try_clone(Rights::CREATE_TASK).unwrap();
    assert!(matches!(
        ChildTask::new(&parent, ProcessStart::REQUIRED_RIGHTS),
        Err(Error::Kernel(abi::Error::Denied))
    ));
    for stack_size in [0, usize::MAX, STACK_SIZE] {
        let mut child = ChildTask::new(process::task(), ProcessStart::REQUIRED_RIGHTS).unwrap();
        let mut frames = child
            .allocate(1, Rights::READ_WRITE.union(Rights::MAP))
            .unwrap();
        frames.write(0, b"child storage").unwrap();
        child
            .map(&frames, 0, 1, None, Permissions::ReadWrite)
            .unwrap();
        let (observer, installed) = process::task()
            .channel(
                ObjectType::Channel.supported_rights(),
                ObjectType::Channel.supported_rights(),
            )
            .unwrap();
        child.install(installed, Rights::READ).unwrap();
        assert!(matches!(
            observer.receive(WaitMode::Poll),
            Err(Error::Kernel(abi::Error::WouldBlock))
        ));
        let (sender, receiver) = process::task()
            .channel(
                ObjectType::Channel.supported_rights(),
                ObjectType::Channel.supported_rights(),
            )
            .unwrap();
        assert!(
            child
                .start(0, stack_size, Some((receiver, Rights::READ)))
                .is_err()
        );
        assert!(matches!(
            observer.receive(WaitMode::Poll),
            Err(Error::Kernel(abi::Error::EndOfStream))
        ));
        assert!(matches!(
            sender.receive(WaitMode::Poll),
            Err(Error::Kernel(abi::Error::EndOfStream))
        ));
        let mut output = [0; 13];
        frames.read(0, &mut output).unwrap();
        assert_eq!(&output, b"child storage");
    }
    let mut child = ChildTask::new(process::task(), ProcessStart::REQUIRED_RIGHTS).unwrap();
    let (observer, installed) = process::task()
        .channel(
            ObjectType::Channel.supported_rights(),
            Rights::READ.union(Rights::TRANSFER),
        )
        .unwrap();
    let raw = installed.borrow().raw();
    let (error, installed) = child.install(installed, Rights::READ_WRITE).unwrap_err();
    assert_eq!(error, Error::Kernel(abi::Error::Denied));
    assert_eq!(installed.borrow().raw(), raw);
    child.install(installed, Rights::READ).unwrap();
    drop(child);
    assert!(matches!(
        observer.receive(WaitMode::Poll),
        Err(Error::Kernel(abi::Error::EndOfStream))
    ));
}

fn load(child: &mut ChildTask, image: &Handle<FrameAllocation>, offset: usize) {
    let mut header = [0; 56];
    image.read(offset, &mut header).unwrap();
    if u32::from_le_bytes(header[..4].try_into().unwrap()) != 1 || word(&header, 40) == 0 {
        return;
    }
    let start = word(&header, 16) as usize;
    let base = start & !(PAGE_SIZE - 1);
    let count = (start - base + word(&header, 40) as usize).div_ceil(PAGE_SIZE) as u64;
    let mut frames = child
        .allocate(
            count,
            Rights::READ_WRITE.union(Rights::MAP).union(Rights::EXECUTE),
        )
        .unwrap();
    let mut bytes = [0; PAGE_SIZE];
    let length = word(&header, 32) as usize;
    for offset in (0..length).step_by(bytes.len()) {
        let size = bytes.len().min(length - offset);
        image
            .read(word(&header, 8) as usize + offset, &mut bytes[..size])
            .unwrap();
        frames.write(start - base + offset, &bytes[..size]).unwrap();
    }
    let flags = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let permissions = match flags {
        4 => Permissions::ReadOnly,
        5 => Permissions::ReadExecute,
        6 => Permissions::ReadWrite,
        _ => panic!("unsupported test segment permissions"),
    };
    child
        .map(&frames, 0, count, Some(base), permissions)
        .unwrap();
}

fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
