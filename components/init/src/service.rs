// SPDX-License-Identifier: MPL-2.0

use runtime::{
    Channel, DeviceSet, Error, Handle, Rights, Task,
    ipc::{CallError, Message, Transfer},
    process::{self, ProcessStart},
    thread,
};

use crate::{archive::Archive, program};

const START_TIMEOUT_NS: u64 = 10_000_000_000;
const STACK_SIZE: usize = 64 * 1024;

pub struct Service {
    task: Handle<Task>,
    control: Handle<Channel>,
}

pub fn start_devmgr(archive: &Archive, devices: &Handle<DeviceSet>) -> Result<Service, Error> {
    let rights = ProcessStart::REQUIRED_RIGHTS.union(Rights::CREATE_EVENT);
    let (child, entry) = program::load(archive, b"bin/devmgr", rights)?;
    let control_rights = Rights::READ_WRITE.union(Rights::WAIT);
    let (sender, receiver) =
        process::task().channel(Rights::READ_WRITE, control_rights.union(Rights::TRANSFER))?;
    let service = Service {
        task: child.start(entry, STACK_SIZE, Some((receiver, control_rights)))?,
        control: sender,
    };
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
    let ready =
        service
            .control
            .call(&mut startup, Some(deadline))
            .map_err(|error| match error {
                CallError::NotSubmitted(error) | CallError::Submitted(error) => error,
            })?;
    if ready.tag != 0 || !ready.words().is_empty() || !ready.handles().is_empty() {
        return Err(Error::Protocol);
    }
    Ok(service)
}

impl Service {
    pub fn wait(&self) -> Result<usize, Error> {
        self.task.wait(u64::MAX)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.task.terminate(1);
    }
}
