// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod archive;
mod program;
mod service;

use archive::Archive;
use runtime::{DeviceSet, Error, KernelLog, ipc::WaitMode, process};

enum InitError {
    Runtime(Error),
    Device(devmgr::Error),
    Logger(logger::Error),
}

impl core::fmt::Debug for InitError {
    fn fmt(&self, output: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Runtime(error) => output.debug_tuple("Runtime").field(error).finish(),
            Self::Device(error) => output.debug_tuple("Device").field(error).finish(),
            Self::Logger(error) => output.debug_tuple("Logger").field(error).finish(),
        }
    }
}

macro_rules! init_errors {
    ($($variant:ident($error:ty)),*) => { $(
        impl From<$error> for InitError {
            fn from(error: $error) -> Self { Self::$variant(error) }
        }
    )* };
}
init_errors!(Runtime(Error), Device(devmgr::Error), Logger(logger::Error));

fn main() -> Result<usize, InitError> {
    let bootstrap = process::bootstrap().ok_or(Error::Protocol)?;
    let mut startup = bootstrap.receive(WaitMode::Forever)?;
    if startup.tag != 0 || startup.words().len() != 1 || startup.handles().len() != 3 {
        return Err(Error::Protocol.into());
    }
    let length = usize::try_from(startup.words()[0]).map_err(|_| Error::Protocol)?;
    let frames = startup
        .take_handle(0)
        .ok_or(Error::Protocol)?
        .downcast()
        .map_err(|_| Error::Protocol)?;
    let devices = startup
        .take_handle(0)
        .ok_or(Error::Protocol)?
        .downcast::<DeviceSet>()
        .map_err(|_| Error::Protocol)?;
    let kernel_log = startup
        .take_handle(0)
        .ok_or(Error::Protocol)?
        .downcast::<KernelLog>()
        .map_err(|_| Error::Protocol)?;
    drop(bootstrap);
    let archive = Archive::new(frames, length)?;
    let (devmgr, device_control) = service::start_devmgr(&archive, &devices)?;
    let (logger, _logger_control, client) =
        service::start_logger(&archive, &devices, &device_control, &kernel_log)?;
    logger::init(client, logger::log::LevelFilter::Info).map_err(|_| Error::Protocol)?;
    logger::log::info!(target: "init", "System services ready");
    logger::log::logger().flush();
    Ok(service::wait(&[&devmgr, &logger])?.max(1))
}
