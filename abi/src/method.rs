// SPDX-License-Identifier: MPL-2.0

use crate::{ObjectType, Rights};

pub const INFO: u64 = 0;
pub const CLOSE: u64 = 1;
pub const DUPLICATE: u64 = 2;

pub const fn required_rights(_kind: ObjectType, method: u64) -> Option<Rights> {
    match method {
        INFO | CLOSE => Some(Rights::NONE),
        DUPLICATE => Some(Rights::DUPLICATE),
        _ => None,
    }
}
