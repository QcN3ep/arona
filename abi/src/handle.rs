// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::ops::BitOr;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct Handle(u64);

impl Handle {
    pub const CURRENT_THREAD: Self = Self(0);

    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rights(u64);

macro_rules! rights {
    ($($name:ident = $bit:literal;)*) => {
        impl Rights {
            $(pub const $name: Self = Self(1 << $bit);)*
            const VALID_BITS: u64 = 0 $(| Self::$name.0)*;
        }

        const _: () = {
            let mut used = 0u64;
            $(
                assert!($bit < 64);
                assert!(used & (1u64 << $bit) == 0);
                used |= 1u64 << $bit;
            )*
            assert!(used == Rights::VALID_BITS);
        };
    };
}

rights! {
    DUPLICATE = 0;
    TRANSFER = 1;
    WAIT = 2;
    SIGNAL = 3;
    READ = 4;
    WRITE = 5;
    MAP = 6;
    EXECUTE = 7;
    CREATE_THREAD = 8;
    TERMINATE = 9;
    MAP_MEMORY = 10;
    RESOURCE_SPLIT = 11;
    DEVICE_CONTROL = 12;
    IRQ_ACK = 13;
    DMA_MAP = 14;
    REVOKE = 16;
    START = 17;
    FRAME_ALLOCATE = 18;
    INSTALL_HANDLE = 19;
    SET_TRAP = 20;
    INTERRUPT = 21;
    READ_MEMORY = 23;
    WRITE_MEMORY = 24;
    SET_STATE = 25;
    CREATE_TASK = 26;
    CREATE_CHANNEL = 27;
    CREATE_EVENT = 28;
    CREATE_TIMER = 29;
}

impl Rights {
    pub const NONE: Self = Self(0);
    pub const READ_WRITE: Self = Self::READ.union(Self::WRITE);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits & !Self::VALID_BITS == 0 {
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
