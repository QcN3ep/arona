// SPDX-License-Identifier: MPL-2.0

macro_rules! errors {
    ($($name:ident = $value:literal,)*) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u64)]
        pub enum Error { $($name = $value,)* }

        impl Error {
            pub const fn from_raw(raw: u64) -> Option<Self> {
                match raw { $($value => Some(Self::$name),)* _ => None }
            }
        }
    };
}

errors! {
    BadHandle = 1, WrongType = 2, Denied = 3, InvalidArgument = 4,
    NoMemory = 5, InvalidState = 6, BadAddress = 7, WouldBlock = 8,
    TimedOut = 9, Closed = 10, PeerClosed = 11, EndOfStream = 12,
    BufferTooSmall = 13, ServerGone = 14, Unsupported = 15, Busy = 16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Return {
    pub status: u64,
    pub values: [u64; 2],
}

impl Return {
    pub const fn success(values: [u64; 2]) -> Self {
        Self { status: 0, values }
    }

    pub const fn failure(error: Error) -> Self {
        Self {
            status: error as u64,
            values: [0; 2],
        }
    }
}
