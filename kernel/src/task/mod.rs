// SPDX-License-Identifier: MPL-2.0

mod handle;
mod lifecycle;
mod object;

pub(crate) use handle::HandleEntry;

pub use handle::{Handle, HandleError, HandleTable, PreparedTransfer, Rights, TransferRequest};
pub(crate) use lifecycle::ReservedHandles;
pub use lifecycle::allocated_task_records;
pub use lifecycle::allocated_task_resources;
pub use lifecycle::registered_tasks;
pub use lifecycle::{Task, TaskError, TaskState};
pub(crate) use lifecycle::{reclaim_finished, thread_exited};

pub use object::{Object, ObjectReference, ObjectType};
