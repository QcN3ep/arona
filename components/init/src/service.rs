// SPDX-License-Identifier: MPL-2.0

// TODO: Integrate network services providing the protocol stack, address configuration, and DNS.

use runtime::{
    Channel, DeviceSet, Error, Handle, KernelLog, Rights, Task,
    ipc::{self, CallError, Message, Transfer, WaitItem},
    process::{self, ProcessStart},
    thread,
};

use fs::{FileSystem, ReadAt, archive::Archive};

use crate::{InitError, program::Loader};

const START_TIMEOUT_NS: u64 = 10_000_000_000;
const STACK_SIZE: usize = 64 * 1024;

pub struct Service {
    task: Handle<Task>,
}

pub fn start_fs(
    loader: &Loader,
    archive: Archive,
) -> Result<(Service, fs::Control<'static>, fs::Client<'static>), InitError> {
    let (service, channel) = launch(
        loader,
        archive.open("/bin/fs")?,
        ProcessStart::REQUIRED_RIGHTS,
    )?;
    let (frames, length) = archive.into_parts()?;
    let control = fs::Control::start(process::task(), channel, frames, length, deadline()?)?;
    let client = control.connect(deadline()?)?;
    Ok((service, control, client))
}

pub fn start_devmgr(
    loader: &Loader,
    files: &fs::Client<'_>,
    devices: &Handle<DeviceSet>,
) -> Result<(Service, Handle<Channel>), InitError> {
    let rights = ProcessStart::REQUIRED_RIGHTS
        .union(Rights::CREATE_EVENT)
        .union(Rights::CREATE_CHANNEL)
        .union(Rights::CREATE_THREAD);
    let (service, control) = launch(loader, files.open("/bin/devmgr", deadline()?)?, rights)?;
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
        return Err(Error::Protocol.into());
    }
    Ok((service, control))
}

pub fn start_logger(
    loader: &Loader,
    files: &fs::Client<'_>,
    devices: &Handle<DeviceSet>,
    devmgr: &Handle<Channel>,
    kernel_log: &Handle<KernelLog>,
    persistent_log: Option<fs::File<'static>>,
) -> Result<(Service, logger::Control<'static>, logger::Client<'static>), crate::InitError> {
    let console = console(devices)?;
    let output = devmgr::uart::Session::open(
        process::task(),
        devmgr,
        console,
        devmgr::uart::Access::Output,
    )?;
    let (service, control) = launch(
        loader,
        files.open("/bin/logger", deadline()?)?,
        ProcessStart::REQUIRED_RIGHTS,
    )?;
    let (control, client) = logger::Control::start(
        process::task(),
        control,
        output,
        kernel_log.try_clone(Rights::READ | Rights::TRANSFER)?,
        persistent_log,
    )?;
    Ok((service, control, client))
}

pub fn persistent_log_file(files: &fs::Client<'static>) -> Result<fs::File<'static>, InitError> {
    match files.mkdir("/var/log", deadline()?) {
        Ok(()) | Err(fs::Error::AlreadyExists) => {}
        Err(error) => return Err(error.into()),
    }
    match files.open("/var/log/system.log", deadline()?) {
        Ok(file) => Ok(file),
        Err(fs::Error::NotFound) => Ok(files.create("/var/log/system.log", deadline()?)?),
        Err(error) => Err(error.into()),
    }
}

pub fn start_linux(
    loader: &Loader,
    files: &fs::Client<'_>,
    filesystem: &fs::Control<'static>,
    devices: &Handle<Channel>,
) -> Result<(Service, linux::Control<'static>), InitError> {
    let (service, channel) = launch(
        loader,
        files.open("/bin/linux", deadline()?)?,
        linux::service::REQUIRED_RIGHTS,
    )?;
    let until = deadline()?;
    let clock = devmgr::device::next(devices, devmgr::device::DriverKind::Rtc, 0, deadline()?)?
        .map(|id| devmgr::rtc::Session::open(process::task(), devices, id, until))
        .transpose()?;
    let entropy = devmgr::device::next(devices, devmgr::device::DriverKind::Rng, 0, deadline()?)?
        .map(|id| devmgr::rng::Session::open(process::task(), devices, id, until))
        .transpose()?;
    let control = linux::Control::start_with_services(
        process::task(),
        channel,
        filesystem.connect(deadline()?)?,
        linux::Services { clock, entropy },
        deadline()?,
    )?;
    Ok((service, control))
}

impl Service {
    pub fn wait_item(&self) -> WaitItem<'_> {
        WaitItem::new(&self.task, ipc::EXITED)
    }
    pub fn wait(&self) -> Result<usize, Error> {
        self.task.wait(0)
    }
}

pub fn console(devices: &Handle<DeviceSet>) -> Result<u32, Error> {
    let mut cursor = 0;
    loop {
        let id = devices.next(cursor)?.ok_or(Error::Protocol)?;
        if devices.describe(id)?.console {
            return Ok(id);
        }
        cursor = id;
    }
}

fn launch(
    loader: &Loader,
    file: impl ReadAt,
    rights: Rights,
) -> Result<(Service, Handle<Channel>), InitError> {
    let (child, entry) = loader.load(file, rights)?;
    let control_rights = Rights::READ.union(Rights::WAIT);
    let (sender, receiver) = process::task().channel(
        Rights::READ_WRITE | Rights::WAIT,
        control_rights.union(Rights::TRANSFER),
    )?;
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

pub fn deadline() -> Result<u64, Error> {
    thread::now_ns()?
        .checked_add(START_TIMEOUT_NS)
        .ok_or(Error::Protocol)
}
