// SPDX-License-Identifier: MPL-2.0

use runtime::{
    Channel, DeviceSet, Error, Handle, KernelLog, Rights, Task,
    ipc::{self, CallError, Message, Transfer, WaitItem, WaitMode},
    process::{self, ProcessStart},
    thread,
};

use crate::{archive::Archive, program};

const START_TIMEOUT_NS: u64 = 10_000_000_000;
const STACK_SIZE: usize = 64 * 1024;

pub struct Service {
    task: Handle<Task>,
}

pub fn start_devmgr(
    archive: &Archive,
    devices: &Handle<DeviceSet>,
) -> Result<(Service, Handle<Channel>), Error> {
    let rights = ProcessStart::REQUIRED_RIGHTS
        .union(Rights::CREATE_EVENT)
        .union(Rights::CREATE_CHANNEL)
        .union(Rights::CREATE_THREAD);
    let (service, control) = launch(archive, b"bin/devmgr", rights)?;
    let rights = Rights::READ.union(Rights::RESOURCE_SPLIT);
    let mut startup = Message::new(0, &[])?;
    startup
        .push(Transfer {
            handle: devices.try_clone(rights.union(Rights::TRANSFER))?.into(),
            rights,
        })
        .map_err(|_| Error::Protocol)?;
    let deadline = thread::now_ns()?
        .checked_add(START_TIMEOUT_NS)
        .ok_or(Error::Protocol)?;
    let ready = control
        .call(&mut startup, Some(deadline))
        .map_err(|error| match error {
            CallError::NotSubmitted(error) | CallError::Submitted(error) => error,
        })?;
    if ready.tag != 0 || !ready.words().is_empty() || !ready.handles().is_empty() {
        return Err(Error::Protocol);
    }
    Ok((service, control))
}

pub fn start_logger(
    archive: &Archive,
    devices: &Handle<DeviceSet>,
    devmgr: &Handle<Channel>,
    kernel_log: &Handle<KernelLog>,
) -> Result<(Service, logger::Control<'static>, logger::Client<'static>), crate::InitError> {
    let mut cursor = 0;
    let console = loop {
        let id = devices.next(cursor)?.ok_or(Error::Protocol)?;
        if devices.describe(id)?.console {
            break id;
        }
        cursor = id;
    };
    let output = devmgr::uart::Session::open(process::task(), devmgr, console)?;
    let (service, control) = launch(archive, b"bin/logger", ProcessStart::REQUIRED_RIGHTS)?;
    let (control, client) = logger::Control::start(
        process::task(),
        control,
        output,
        kernel_log.try_clone(Rights::READ | Rights::TRANSFER)?,
    )?;
    Ok((service, control, client))
}

pub fn wait(services: &[&Service]) -> Result<usize, Error> {
    let items: alloc::vec::Vec<_> = services
        .iter()
        .map(|service| WaitItem::new(&service.task, ipc::EXITED))
        .collect();
    let ready = ipc::wait_many(&items, WaitMode::Forever)?;
    services[ready.index].task.wait(0)
}

fn launch(
    archive: &Archive,
    path: &[u8],
    rights: Rights,
) -> Result<(Service, Handle<Channel>), Error> {
    let (child, entry) = program::load(archive, path, rights)?;
    let control_rights = Rights::READ.union(Rights::WAIT);
    let (sender, receiver) =
        process::task().channel(Rights::READ_WRITE, control_rights.union(Rights::TRANSFER))?;
    let service = Service {
        task: child.start(entry, STACK_SIZE, Some((receiver, control_rights)))?,
    };
    Ok((service, sender))
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.task.terminate(1);
    }
}
