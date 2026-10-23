// SPDX-License-Identifier: MPL-2.0

mod handle;
mod lifecycle;

pub use handle::{
    Handle, HandleError, HandleTable, Object, ObjectType, PreparedTransfer, Rights, TransferRequest,
};
pub(crate) use lifecycle::creator_exited;
pub use lifecycle::{Task, TaskError, TaskState};
