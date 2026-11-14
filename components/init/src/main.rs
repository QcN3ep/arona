// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod config;
mod program;
mod service;
mod supervisor;

use fs::{FileSystem, ReadAt, archive::Archive};
use runtime::{DeviceSet, Error, KernelLog, ipc::WaitMode, process};

enum InitError {
    Runtime(Error),
    Device(devmgr::Error),
    Logger(logger::Error),
    Filesystem(fs::Error),
    Linux(linux::Error),
    Network(net::Error),
    Config(config::Error),
}

impl core::fmt::Debug for InitError {
    fn fmt(&self, output: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Runtime(error) => output.debug_tuple("Runtime").field(error).finish(),
            Self::Device(error) => output.debug_tuple("Device").field(error).finish(),
            Self::Logger(error) => output.debug_tuple("Logger").field(error).finish(),
            Self::Filesystem(error) => output.debug_tuple("Filesystem").field(error).finish(),
            Self::Network(error) => output.debug_tuple("Network").field(error).finish(),
            Self::Linux(error) => output.debug_tuple("Linux").field(error).finish(),
            Self::Config(error) => write!(output, "{error}"),
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
init_errors!(
    Runtime(Error),
    Device(devmgr::Error),
    Logger(logger::Error),
    Filesystem(fs::Error),
    Linux(linux::Error),
    Network(net::Error),
    Config(config::Error)
);

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
    let loader = program::Loader::new(archive.open("/bin/init")?)?;
    let (filesystem, mounts, files) = service::start_fs(&loader, archive)?;
    mounts.mount_tmpfs(b"/tmp", 0, service::deadline()?)?;
    let (devmgr, device_control) = service::start_devmgr(&loader, &files, &devices)?;
    let persistent_log = if let Some(id) = devmgr::device::next(
        &device_control,
        devmgr::device::DriverKind::Block,
        0,
        service::deadline()?,
    )? {
        let session = devmgr::block::Session::open(process::task(), &device_control, id)?;
        let size = session.info()?.capacity;
        mounts.mount(b"/var", session, 0, size, service::deadline()?)?;
        Some(service::persistent_log_file(&files)?)
    } else {
        None
    };
    let mounted = persistent_log.is_some();
    let (logger, logger_control, client) = service::start_logger(
        &loader,
        &files,
        &devices,
        &device_control,
        &kernel_log,
        persistent_log,
    )?;
    logger::init(client, logger::log::LevelFilter::Info).map_err(|_| Error::Protocol)?;
    let (net, net_control) = service::start_net(&loader, &files, &device_control)?;
    let (linux, linux_control) =
        service::start_linux(&loader, &files, &mounts, &device_control, &net_control)?;
    if mounted {
        logger::log::info!(target: "init", "Mounted block device at /var");
    }
    logger::log::info!(target: "init", "Mounted tmpfs at /tmp");
    logger::log::info!(target: "init", "System services ready");
    logger::log::logger().flush();
    let config = read_config(&files).inspect_err(|error| {
        logger::log::error!(target: "init", "Failed to load /etc/init.toml: {error:?}");
        logger::log::logger().flush();
    })?;
    let console = service::console(&devices)?;
    let system = supervisor::System {
        filesystem: &filesystem,
        devmgr: &devmgr,
        logger: &logger,
        linux: &linux,
        net: &net,
        net_control,
        linux_control,
        logger_control,
        mounts,
        devices: device_control,
        console,
        mounted,
    };
    if let Some(action) = config.power.action {
        return supervisor::shutdown(system, action);
    }
    supervisor::run(system, config)
}

fn read_config(files: &fs::Client<'_>) -> Result<config::Config, InitError> {
    let file = match files.open("/etc/init.toml", service::deadline()?) {
        Ok(file) => file,
        Err(fs::Error::NotFound) => return Ok(config::Config::default()),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = alloc::vec![0; file.len()];
    file.read_exact_at(0, &mut bytes)?;
    let text = core::str::from_utf8(&bytes).map_err(|_| Error::Protocol)?;
    Ok(config::Config::parse(text)?)
}
