// SPDX-License-Identifier: MPL-2.0

#[macro_export]
macro_rules! objects {
    ($consumer:ident) => {
        $consumer! {
            Task = 1 => [DUPLICATE, TRANSFER, WAIT, CREATE_THREAD, TERMINATE,
                MAP_MEMORY, INSTALL_HANDLE, READ_MEMORY, WRITE_MEMORY,
                SET_TRAP, FRAME_ALLOCATE, CREATE_TASK, CREATE_CHANNEL, CREATE_EVENT, CREATE_TIMER];
            Thread = 2 => [DUPLICATE, TRANSFER, WAIT, TERMINATE, START, INTERRUPT, SET_STATE];
            FrameAllocation = 3 => [DUPLICATE, TRANSFER, READ, WRITE, MAP, EXECUTE, DMA_MAP];
            Channel = 4 => [DUPLICATE, TRANSFER, WAIT, READ, WRITE];
            Event = 5 => [DUPLICATE, TRANSFER, WAIT, SIGNAL];
            Timer = 6 => [DUPLICATE, TRANSFER, WRITE];
            Reply = 7 => [TRANSFER, WRITE];
        }
    };
}

macro_rules! define_types {
    ($($kind:ident = $id:literal => [$($right:ident),*];)*) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u64)]
        pub enum ObjectType { $($kind = $id,)* }

        impl ObjectType {
            pub const fn from_raw(raw: u64) -> Option<Self> {
                match raw { $($id => Some(Self::$kind),)* _ => None }
            }

            pub const fn supported_rights(self) -> crate::Rights {
                match self {
                    $(Self::$kind => crate::Rights::NONE$(.union(crate::Rights::$right))*,)*
                }
            }
        }
    };
}

objects!(define_types);
