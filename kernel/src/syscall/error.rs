// SPDX-License-Identifier: MPL-2.0

use crate::{
    memory::MemoryError,
    task::{HandleError, TaskError},
    thread::ThreadError,
};
use abi::Error;

impl From<crate::trap::port::ForwardError> for Error {
    fn from(error: crate::trap::port::ForwardError) -> Self {
        use crate::trap::port::ForwardError;
        match error {
            ForwardError::Closed => Self::Closed,
            ForwardError::WouldBlock => Self::WouldBlock,
            ForwardError::InvalidState => Self::InvalidState,
            ForwardError::Memory(error) => error.into(),
            ForwardError::Thread(error) => error.into(),
            ForwardError::Execution(_) => Self::InvalidArgument,
        }
    }
}

impl From<HandleError> for Error {
    fn from(error: HandleError) -> Self {
        match error {
            HandleError::BadHandle => Self::BadHandle,
            HandleError::WrongType => Self::WrongType,
            HandleError::Denied => Self::Denied,
            HandleError::InvalidArgument => Self::InvalidArgument,
            HandleError::NoMemory => Self::NoMemory,
        }
    }
}

impl From<MemoryError> for Error {
    fn from(error: MemoryError) -> Self {
        match error {
            MemoryError::NoMemory | MemoryError::ReferenceOverflow => Self::NoMemory,
            MemoryError::AccessConflict => Self::Denied,
            MemoryError::InUse => Self::Busy,
            MemoryError::InvalidFrame | MemoryError::InvalidRequest => Self::InvalidArgument,
            MemoryError::NotMapped => Self::BadAddress,
            MemoryError::AlreadyMapped => Self::Busy,
            _ => Self::InvalidState,
        }
    }
}

impl From<ThreadError> for Error {
    fn from(error: ThreadError) -> Self {
        match error {
            ThreadError::Memory(error) => error.into(),
            ThreadError::Capacity | ThreadError::GenerationExhausted => Self::NoMemory,
            _ => Self::InvalidState,
        }
    }
}

impl From<TaskError> for Error {
    fn from(error: TaskError) -> Self {
        match error {
            TaskError::Memory(error) => error.into(),
            TaskError::Handle(error) => error.into(),
            TaskError::Thread(error) => error.into(),
            TaskError::Channel(error) => error.into(),
            TaskError::Wait(error) => error.into(),
            TaskError::InvalidState => Self::InvalidState,
        }
    }
}

impl From<crate::memory::UserMemoryError> for Error {
    fn from(error: crate::memory::UserMemoryError) -> Self {
        match error.0 {
            MemoryError::NoMemory | MemoryError::ReferenceOverflow => Self::NoMemory,
            MemoryError::Mmu(_) => Self::InvalidState,
            _ => Self::BadAddress,
        }
    }
}

impl From<crate::ipc::ChannelError> for Error {
    fn from(error: crate::ipc::ChannelError) -> Self {
        use crate::ipc::ChannelError as E;
        match error {
            E::InvalidMessage | E::CyclicTransfer => Self::InvalidArgument,
            E::BufferTooSmall => Self::BufferTooSmall,
            E::Closed => Self::Closed,
            E::WouldBlock => Self::WouldBlock,
            E::PeerClosed => Self::PeerClosed,
            E::EndOfStream => Self::EndOfStream,
            E::TimedOut => Self::TimedOut,
            E::InvalidState => Self::InvalidState,
            E::ServerGone => Self::ServerGone,
            E::Cancelled => Self::Cancelled,
            E::Memory(error) => error.into(),
            E::Thread(error) => error.into(),
        }
    }
}
impl From<crate::ipc::EventError> for Error {
    fn from(error: crate::ipc::EventError) -> Self {
        match error {
            crate::ipc::EventError::InvalidArgument => Self::InvalidArgument,
            crate::ipc::EventError::Closed => Self::Closed,
            crate::ipc::EventError::Memory(error) => error.into(),
        }
    }
}
impl From<crate::ipc::TimerError> for Error {
    fn from(error: crate::ipc::TimerError) -> Self {
        use crate::ipc::TimerError as E;
        match error {
            E::InvalidArgument => Self::InvalidArgument,
            E::Closed => Self::Closed,
            E::Event(error) => error.into(),
            E::Memory(error) => error.into(),
            E::Thread(error) => error.into(),
            E::Hardware(_) => Self::InvalidState,
        }
    }
}
impl From<crate::ipc::WaitManyError> for Error {
    fn from(error: crate::ipc::WaitManyError) -> Self {
        use crate::ipc::WaitManyError as E;
        match error {
            E::InvalidArgument => Self::InvalidArgument,
            E::WouldBlock => Self::WouldBlock,
            E::TimedOut => Self::TimedOut,
            E::Closed => Self::Closed,
            E::Handle(error) => error.into(),
            E::Channel(error) => error.into(),
            E::Event(error) => error.into(),
            E::Thread(error) => error.into(),
            E::Memory(error) => error.into(),
        }
    }
}

impl From<crate::device::DeviceError> for Error {
    fn from(error: crate::device::DeviceError) -> Self {
        use crate::device::DeviceError;
        match error {
            DeviceError::InvalidArgument => Self::InvalidArgument,
            DeviceError::Denied => Self::Denied,
            DeviceError::Busy => Self::Busy,
            DeviceError::Closed => Self::Closed,
            DeviceError::Unsupported => Self::Unsupported,
            DeviceError::Memory(error) => error.into(),
            _ => Self::InvalidState,
        }
    }
}
