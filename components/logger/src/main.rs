// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#![no_std]
#![forbid(unsafe_code)]

fn main() -> Result<(), logger::Error> {
    let bootstrap = runtime::process::bootstrap().ok_or(runtime::Error::Protocol)?;
    logger::service::run(runtime::process::task(), bootstrap)
}
