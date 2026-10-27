// SPDX-License-Identifier: MPL-2.0

use crate::{
    memory::MemoryError,
    task::{HandleError, TaskError},
    thread::ThreadError,
};
use abi::Error;

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
            _ => Self::InvalidState,
        }
    }
}
