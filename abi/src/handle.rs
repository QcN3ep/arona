// SPDX-License-Identifier: MPL-2.0

use core::ops::BitOr;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct Handle(u64);

impl Handle {
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rights(u64);

impl Rights {
    pub const NONE: Self = Self(0);
    pub const DUPLICATE: Self = Self(1 << 0);
    pub const TRANSFER: Self = Self(1 << 1);
    pub const WAIT: Self = Self(1 << 2);
    pub const SIGNAL: Self = Self(1 << 3);
    pub const READ: Self = Self(1 << 4);
    pub const WRITE: Self = Self(1 << 5);
    pub const MAP: Self = Self(1 << 6);
    pub const EXECUTE: Self = Self(1 << 7);
    pub const CREATE_THREAD: Self = Self(1 << 8);
    pub const TERMINATE: Self = Self(1 << 9);
    pub const MAP_MEMORY: Self = Self(1 << 10);
    pub const RESOURCE_SPLIT: Self = Self(1 << 11);
    pub const DEVICE_CONTROL: Self = Self(1 << 12);
    pub const IRQ_ACK: Self = Self(1 << 13);
    pub const DMA_MAP: Self = Self(1 << 14);
    pub const REVOKE: Self = Self(1 << 16);
    pub const START: Self = Self(1 << 17);
    pub const FRAME_ALLOCATE: Self = Self(1 << 18);
    pub const INSTALL_HANDLE: Self = Self(1 << 19);
    pub const SET_TRAP: Self = Self(1 << 20);
    pub const INTERRUPT: Self = Self(1 << 21);
    pub const BIND_TRAP: Self = Self(1 << 22);
    pub const READ_MEMORY: Self = Self(1 << 23);
    pub const WRITE_MEMORY: Self = Self(1 << 24);
    pub const SET_STATE: Self = Self(1 << 25);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits & !0x03ff_7fff == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Rights {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
