// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Authorized snapshots of object information.

use super::error::CallError;
use crate::{
    task::{ObjectReference, Rights, TaskState},
    thread::ThreadState,
};
use abi::{Error, info, thread::Lifecycle};

pub(super) fn invoke(
    target: &ObjectReference,
    rights: Rights,
    args: [u64; 5],
) -> Result<[u64; 2], CallError> {
    if args[1..] != [0; 4] {
        return Err(Error::InvalidArgument.into());
    }
    Ok(match (target, args[0]) {
        (_, info::HANDLE) => [target.kind() as u64, rights.bits()],
        (ObjectReference::Task(task), info::task::LIFECYCLE) => match task.state() {
            TaskState::Dead(status) => [Lifecycle::Exited as u64, status as u64],
            _ => [Lifecycle::Live as u64, 0],
        },
        (ObjectReference::Thread(thread), info::thread::LIFECYCLE) => match thread.state() {
            ThreadState::Dead(status) => [Lifecycle::Exited as u64, status as u64],
            _ => [Lifecycle::Live as u64, 0],
        },
        (ObjectReference::Thread(thread), info::thread::IDENTITY) => [thread.id(), 0],
        (ObjectReference::Thread(thread), info::thread::CPU_TIME) => [thread.cpu_time_ns()?, 0],
        (ObjectReference::FrameAllocation(frames), info::memory::SIZE) => [frames.info().frames, 0],
        (ObjectReference::FrameAllocation(frames), info::memory::USAGE) => {
            [frames.info().mappings, frames.handle_owners()]
        }
        (ObjectReference::KernelLog(_), info::kernel_log::CAPACITY) => {
            [crate::logger::CAPACITY as u64, 0]
        }
        (ObjectReference::DmaDomain(domain), info::dma::STATE) => [domain.state()? as u64, 0],
        _ => return Err(Error::Unsupported.into()),
    })
}
