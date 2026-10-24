// SPDX-License-Identifier: MPL-2.0

mod handle;
mod lifecycle;

pub(crate) use handle::HandleEntry;

pub use handle::{
    Handle, HandleError, HandleTable, Object, ObjectReference, ObjectType, PreparedTransfer,
    Rights, TransferRequest,
};
pub use lifecycle::{Task, TaskError, TaskState};
pub(crate) use lifecycle::{reclaim_finished, thread_exited};
