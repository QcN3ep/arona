// SPDX-License-Identifier: MPL-2.0

use crate::Rights;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessStart {
    pub task: crate::Handle,
    pub bootstrap: Option<crate::Handle>,
}

impl ProcessStart {
    pub const SIZE: usize = 16;
    pub const REQUIRED_RIGHTS: Rights = Rights::FRAME_ALLOCATE
        .union(Rights::MAP_MEMORY)
        .union(Rights::TERMINATE);

    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        bytes[..8].copy_from_slice(&self.task.raw().to_le_bytes());
        bytes[8..].copy_from_slice(&self.bootstrap.map_or(0, crate::Handle::raw).to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        let task = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let bootstrap = u64::from_le_bytes(bytes[8..].try_into().unwrap());
        Self {
            task: crate::Handle::from_raw(task),
            bootstrap: (bootstrap != 0).then_some(crate::Handle::from_raw(bootstrap)),
        }
    }
}
