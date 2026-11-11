// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{HandleError, Rights, Task};
use crate::{
    device::dma::DmaDomain,
    device::{Device, DeviceReference, DeviceSet},
    ipc::{
        Call, CallReference, Channel, ChannelReference, Event, EventReference, Reply,
        ReplyReference, Timer, TimerReference,
    },
    logger::KernelLog,
    memory::{self, HeapArc},
    thread::Thread,
    trap::port::{TrapContinuation, TrapContinuationReference, TrapPort, TrapPortReference},
};
pub use abi::ObjectType;

type FrameAllocation = HeapArc<memory::FrameAllocation>;

pub trait ObjectPayload: Sized {
    type Reference: Clone;
    fn reference(&self) -> Self::Reference;
    fn try_clone(&self) -> Result<Self, HandleError>;
}

macro_rules! define_objects {
    ($($kind:ident = $id:literal => [$($right:ident),*];)*) => {
        /// # Safety
        ///
        /// Local interrupts must be disabled and kernel mappings retained during drop.
        pub enum Object { $($kind($kind),)* }
        /// # Safety
        ///
        /// Local interrupts must be disabled and kernel mappings retained during drop.
        #[derive(Clone)]
        pub enum ObjectReference { $($kind(<$kind as ObjectPayload>::Reference),)* }

        impl Object {
            pub fn kind(&self) -> ObjectType {
                match self { $(Self::$kind(_) => ObjectType::$kind,)* }
            }
            pub(super) fn allowed_rights(&self) -> Rights {
                self.kind().supported_rights()
            }
            pub(super) fn reference(&self) -> ObjectReference {
                match self { $(Self::$kind(value) => ObjectReference::$kind(ObjectPayload::reference(value)),)* }
            }
            pub fn try_clone(&self) -> Result<Self, HandleError> {
                Ok(match self { $(Self::$kind(value) => Self::$kind(ObjectPayload::try_clone(value)?),)* })
            }
        }
    };
}
abi::objects!(define_objects);

macro_rules! cloned_payload {
    ($($kind:ty),*) => { $(
        impl ObjectPayload for $kind {
            type Reference = Self;
            fn reference(&self) -> Self { self.clone() }
            fn try_clone(&self) -> Result<Self, HandleError> { Ok(self.clone()) }
        }
    )* };
}
cloned_payload!(
    Task,
    Thread,
    FrameAllocation,
    DeviceSet,
    KernelLog,
    DmaDomain
);

macro_rules! referenced_payload {
    ($($kind:ty => $reference:ty),*) => { $(
        impl ObjectPayload for $kind {
            type Reference = $reference;
            fn reference(&self) -> Self::Reference { self.reference() }
            fn try_clone(&self) -> Result<Self, HandleError> { Ok(self.clone()) }
        }
    )* };
}
referenced_payload!(Channel => ChannelReference, Event => EventReference, Timer => TimerReference, TrapPort => TrapPortReference);

impl ObjectPayload for TrapContinuation {
    type Reference = TrapContinuationReference;
    fn reference(&self) -> Self::Reference {
        self.reference()
    }
    fn try_clone(&self) -> Result<Self, HandleError> {
        Err(HandleError::Denied)
    }
}

impl ObjectPayload for Reply {
    type Reference = ReplyReference;
    fn reference(&self) -> Self::Reference {
        self.reference()
    }
    fn try_clone(&self) -> Result<Self, HandleError> {
        Err(HandleError::Denied)
    }
}

impl ObjectPayload for Call {
    type Reference = CallReference;
    fn reference(&self) -> Self::Reference {
        self.reference()
    }
    fn try_clone(&self) -> Result<Self, HandleError> {
        Err(HandleError::Denied)
    }
}

impl ObjectPayload for Device {
    type Reference = DeviceReference;
    fn reference(&self) -> DeviceReference {
        self.reference()
    }
    fn try_clone(&self) -> Result<Self, HandleError> {
        self.try_clone().map_err(|error| match error {
            crate::device::DeviceError::Memory(_) => HandleError::NoMemory,
            _ => HandleError::Denied,
        })
    }
}
