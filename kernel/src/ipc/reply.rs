// SPDX-License-Identifier: MPL-2.0

use super::{CallReference, ChannelError, Message};
use crate::task::{HandleEntry, Task};
use alloc::vec::Vec;

/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
pub struct Reply {
    reference: ReplyReference,
}

/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
#[derive(Clone)]
pub struct ReplyReference(CallReference);

pub fn active_replies() -> usize {
    super::call::active_calls()
}

impl Reply {
    pub(crate) fn new(reference: CallReference) -> Self {
        Self {
            reference: ReplyReference(reference),
        }
    }

    pub(crate) fn reference(&self) -> ReplyReference {
        self.reference.clone()
    }
}

impl ReplyReference {
    pub(crate) fn destination(&self) -> Result<Task, ChannelError> {
        self.0
            .destination_task()
            .map_err(|_| ChannelError::InvalidState)
    }

    pub(crate) fn complete(
        &self,
        commit: impl FnOnce(&mut Vec<HandleEntry>) -> Message,
    ) -> Result<(), ChannelError> {
        self.0.complete(commit)
    }

    pub(crate) fn notify(&self) {
        self.0.notify();
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        self.reference.0.fail(ChannelError::ServerGone);
    }
}
