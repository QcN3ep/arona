// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

mod archive;
mod program;
mod service;

use archive::Archive;
use runtime::{DeviceSet, Error, KernelLog, ipc::WaitMode, process};

fn main() -> Result<usize, Error> {
    let bootstrap = process::bootstrap().ok_or(Error::Protocol)?;
    let mut startup = bootstrap.receive(WaitMode::Forever)?;
    if startup.tag != 0 || startup.words().len() != 1 || startup.handles().len() != 3 {
        return Err(Error::Protocol);
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
    // TODO: 向日志服务交付内核日志句柄。
    let _kernel_log = startup
        .take_handle(0)
        .ok_or(Error::Protocol)?
        .downcast::<KernelLog>()
        .map_err(|_| Error::Protocol)?;
    drop(bootstrap);
    let archive = Archive::new(frames, length)?;
    let service = service::start_devmgr(&archive, &devices)?;
    service.wait().map(|status| status.max(1))
}
