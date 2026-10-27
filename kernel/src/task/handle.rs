// SPDX-License-Identifier: MPL-2.0

use core::{mem, ptr};

use super::{Object, ObjectReference, ObjectType};
pub use abi::{Handle, Rights};
use alloc::vec::Vec;
use arrayvec::ArrayVec;
use spin::Mutex;

use crate::{
    hardware::cpu::InterruptGuard,
    ipc::{MESSAGE_HANDLES, Message, ReceivedMessage, Reply, ReplyReference},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleError {
    BadHandle,
    WrongType,
    Denied,
    InvalidArgument,
    NoMemory,
}

/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
pub struct HandleTable {
    state: Mutex<TableState>,
}

#[derive(Default)]
struct TableState {
    slots: Vec<Slot>,
    free: Option<u32>,
    available: usize,
}

struct Slot {
    generation: u32,
    state: SlotState,
}

enum SlotState {
    Vacant(Option<u32>),
    Reserved,
    Occupied(HandleEntry),
    Retired,
}

pub(crate) struct HandleEntry {
    pub(crate) object: Object,
    rights: Rights,
}

#[derive(Clone, Copy, Debug)]
pub struct TransferRequest {
    pub handle: Handle,
    pub rights: Rights,
}

#[must_use]
pub struct PreparedTransfer<'table> {
    source: &'table HandleTable,
    target: &'table HandleTable,
    pending: Vec<PendingTransfer>,
    committed: bool,
}

struct PendingTransfer {
    request: TransferRequest,
    target: Handle,
    order: usize,
}

impl HandleTable {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(TableState {
                slots: Vec::new(),
                free: None,
                available: 0,
            }),
        }
    }

    pub fn insert(&self, object: Object, rights: Rights) -> Result<Handle, HandleError> {
        let _interrupts = InterruptGuard::new();
        let entry = HandleEntry { object, rights };
        if !entry.object.allowed_rights().contains(rights) {
            return Err(HandleError::InvalidArgument);
        }
        let mut state = self.state.lock();
        state.ensure_available(1)?;
        let handle = state.reserve();
        state.publish(handle, entry);
        Ok(handle)
    }

    pub fn get(
        &self,
        handle: Handle,
        kind: ObjectType,
        rights: Rights,
    ) -> Result<ObjectReference, HandleError> {
        let _interrupts = InterruptGuard::new();
        let state = self.state.lock();
        let entry = state.entry(handle)?;
        if entry.object.kind() != kind {
            return Err(HandleError::WrongType);
        }
        entry.require(rights)?;
        Ok(entry.object.reference())
    }

    pub fn close(&self, handle: Handle) -> Result<(), HandleError> {
        let _interrupts = InterruptGuard::new();
        let entry = {
            let mut state = self.state.lock();
            state.entry(handle)?;
            state.release(handle)
        };
        drop(entry);
        Ok(())
    }

    pub fn duplicate(&self, handle: Handle, rights: Rights) -> Result<Handle, HandleError> {
        let _interrupts = InterruptGuard::new();
        let mut state = self.state.lock();
        state.entry(handle)?.attenuate(Rights::DUPLICATE, rights)?;
        state.ensure_available(1)?;
        let object = state.entry(handle)?.object.try_clone()?;
        let duplicate = state.reserve();
        state.publish(duplicate, HandleEntry { object, rights });
        Ok(duplicate)
    }

    pub fn prepare_transfer<'table>(
        &'table self,
        target: &'table Self,
        requests: &[TransferRequest],
    ) -> Result<PreparedTransfer<'table>, HandleError> {
        let _interrupts = InterruptGuard::new();
        let mut pending = Vec::new();
        self.with_tables(target, |source, target| {
            for request in requests {
                if matches!(source.entry(request.handle)?.object, Object::Reply(_)) {
                    return Err(HandleError::Denied);
                }
                source
                    .entry(request.handle)?
                    .attenuate(Rights::TRANSFER, request.rights)?;
            }
            pending
                .try_reserve_exact(requests.len())
                .map_err(|_| HandleError::NoMemory)?;
            for (order, request) in requests.iter().enumerate() {
                pending.push(PendingTransfer {
                    request: *request,
                    target: Handle::from_raw(0),
                    order,
                });
            }
            pending.sort_unstable_by_key(|transfer| transfer.request.handle.raw());
            if pending
                .windows(2)
                .any(|pair| pair[0].request.handle == pair[1].request.handle)
            {
                return Err(HandleError::InvalidArgument);
            }
            pending.sort_unstable_by_key(|transfer| transfer.order);
            let target = target.unwrap_or(source);
            target.ensure_available(pending.len())?;
            for transfer in &mut pending {
                transfer.target = target.reserve();
            }
            Ok(())
        })?;
        Ok(PreparedTransfer {
            source: self,
            target,
            pending,
            committed: false,
        })
    }

    pub(crate) fn validate_transfer<const N: usize>(
        &self,
        requests: &[TransferRequest],
    ) -> Result<(), HandleError> {
        let _interrupts = InterruptGuard::new();
        self.state.lock().validate_transfer::<N>(requests)
    }

    pub fn info(&self, handle: Handle) -> Result<(ObjectType, Rights), HandleError> {
        let _interrupts = InterruptGuard::new();
        let state = self.state.lock();
        let entry = state.entry(handle)?;
        Ok((entry.object.kind(), entry.rights))
    }

    pub(crate) fn get_any(
        &self,
        handle: Handle,
        rights: Rights,
    ) -> Result<ObjectReference, HandleError> {
        let _interrupts = InterruptGuard::new();
        let state = self.state.lock();
        let entry = state.entry(handle)?;
        entry.require(rights)?;
        Ok(entry.object.reference())
    }

    pub(crate) fn transfer_out<const N: usize, E: From<HandleError>>(
        &self,
        requests: &[TransferRequest],
        prefix: Option<HandleEntry>,
        validate: impl FnOnce(&[&Object]) -> Result<(), E>,
    ) -> Result<Vec<HandleEntry>, E> {
        let _interrupts = InterruptGuard::new();
        let mut state = self.state.lock();
        state.validate_transfer::<N>(requests)?;
        if requests.is_empty() && prefix.is_none() {
            return Ok(Vec::new());
        }
        let mut objects = ArrayVec::<_, N>::new();
        for request in requests {
            objects.push(&state.entry(request.handle)?.object);
        }
        validate(&objects)?;
        drop(objects);
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(requests.len() + usize::from(prefix.is_some()))
            .map_err(|_| HandleError::NoMemory)?;
        entries.extend(prefix);
        for request in requests {
            let mut entry = state
                .release(request.handle)
                .expect("missing transfer source");
            entry.rights = request.rights;
            entries.push(entry);
        }
        Ok(entries)
    }

    pub(crate) fn reserve_reply_handles(
        &self,
    ) -> Result<ArrayVec<Handle, MESSAGE_HANDLES>, HandleError> {
        let mut state = self.state.lock();
        state.ensure_available(MESSAGE_HANDLES)?;
        Ok(core::array::from_fn::<_, MESSAGE_HANDLES, _>(|_| state.reserve()).into())
    }

    pub(crate) fn release_reply_handles(
        &self,
        handles: &[Handle],
    ) -> ArrayVec<HandleEntry, MESSAGE_HANDLES> {
        let mut state = self.state.lock();
        let mut entries = ArrayVec::new();
        for handle in handles {
            let slot = &state.slots[handle.raw_index().unwrap()];
            if slot.generation == handle.raw_generation()
                && matches!(slot.state, SlotState::Reserved | SlotState::Occupied(_))
                && let Some(entry) = state.release(*handle)
            {
                entries.push(entry);
            }
        }
        entries
    }

    pub(crate) fn reply_to(
        &self,
        handle: Handle,
        reply: &ReplyReference,
        target: &Self,
        message: Message,
        requests: &[TransferRequest],
    ) -> Result<(), super::TaskError> {
        self.with_tables(target, |source, mut target| {
            source.entry(handle)?.require(Rights::WRITE)?;
            source.validate_transfer::<MESSAGE_HANDLES>(requests)?;
            for request in requests {
                if matches!(source.entry(request.handle)?.object, Object::Reply(_)) {
                    return Err(HandleError::Denied.into());
                }
            }
            reply.complete(|slots| {
                let mut handles = ArrayVec::new();
                for (request, destination) in requests.iter().zip(slots) {
                    let mut entry = source.release(request.handle).unwrap();
                    entry.rights = request.rights;
                    target
                        .as_deref_mut()
                        .unwrap_or(source)
                        .publish(*destination, entry);
                    handles.push(*destination);
                }
                ReceivedMessage::new(message, handles, false)
            })?;
            Ok(())
        })
    }

    pub(crate) fn install<const N: usize>(
        &self,
        entries: &mut Vec<HandleEntry>,
    ) -> Result<ArrayVec<Handle, N>, HandleError> {
        let _interrupts = InterruptGuard::new();
        if entries.len() > N {
            return Err(HandleError::InvalidArgument);
        }
        let mut state = self.state.lock();
        state.ensure_available(entries.len())?;
        let mut handles = ArrayVec::new();
        for entry in entries.drain(..) {
            let handle = state.reserve();
            state.publish(handle, entry);
            handles.push(handle);
        }
        Ok(handles)
    }

    fn with_tables<T>(
        &self,
        target: &Self,
        operation: impl FnOnce(&mut TableState, Option<&mut TableState>) -> T,
    ) -> T {
        // 同表只取一次锁，跨表按地址顺序取锁。
        if ptr::eq(self, target) {
            operation(&mut self.state.lock(), None)
        } else if ptr::from_ref(self).addr() < ptr::from_ref(target).addr() {
            let mut source = self.state.lock();
            let mut target = target.state.lock();
            operation(&mut source, Some(&mut target))
        } else {
            let mut target = target.state.lock();
            let mut source = self.state.lock();
            operation(&mut source, Some(&mut target))
        }
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HandleTable {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        drop(mem::take(self.state.get_mut()));
    }
}

impl PreparedTransfer<'_> {
    /// 目标句柄仅在 `commit` 成功后可用，放弃事务会释放预留槽位。
    pub fn handles(&self) -> impl ExactSizeIterator<Item = Handle> + '_ {
        self.pending.iter().map(|transfer| transfer.target)
    }

    pub fn commit(mut self) -> Result<(), HandleError> {
        let _interrupts = InterruptGuard::new();
        self.source.with_tables(self.target, |source, mut target| {
            for transfer in &self.pending {
                source
                    .entry(transfer.request.handle)?
                    .attenuate(Rights::TRANSFER, transfer.request.rights)?;
            }
            for transfer in &self.pending {
                let mut entry = source
                    .release(transfer.request.handle)
                    .expect("missing transfer source");
                entry.rights = transfer.request.rights;
                target
                    .as_deref_mut()
                    .unwrap_or(source)
                    .publish(transfer.target, entry);
            }
            Ok(())
        })?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PreparedTransfer<'_> {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        if !self.committed {
            let mut target = self.target.state.lock();
            for transfer in &self.pending {
                target.release(transfer.target);
            }
        }
        drop(mem::take(&mut self.pending));
    }
}

impl HandleEntry {
    pub(crate) fn reply(reply: Reply) -> Self {
        Self {
            object: Object::Reply(reply),
            rights: Rights::TRANSFER | Rights::WRITE,
        }
    }

    fn require(&self, rights: Rights) -> Result<(), HandleError> {
        if self.rights.contains(rights) {
            Ok(())
        } else {
            Err(HandleError::Denied)
        }
    }

    fn attenuate(&self, operation: Rights, rights: Rights) -> Result<(), HandleError> {
        self.require(operation)?;
        self.require(rights)
    }
}

impl TableState {
    fn validate_transfer<const N: usize>(
        &self,
        requests: &[TransferRequest],
    ) -> Result<(), HandleError> {
        if requests.len() > N {
            return Err(HandleError::InvalidArgument);
        }
        for (index, request) in requests.iter().enumerate() {
            self.entry(request.handle)?
                .attenuate(Rights::TRANSFER, request.rights)?;
            if requests[..index]
                .iter()
                .any(|previous| previous.handle == request.handle)
            {
                return Err(HandleError::InvalidArgument);
            }
        }
        Ok(())
    }

    fn entry(&self, handle: Handle) -> Result<&HandleEntry, HandleError> {
        let slot = handle
            .raw_index()
            .and_then(|index| self.slots.get(index))
            .ok_or(HandleError::BadHandle)?;
        match &slot.state {
            SlotState::Occupied(entry) if slot.generation == handle.raw_generation() => Ok(entry),
            _ => Err(HandleError::BadHandle),
        }
    }

    fn ensure_available(&mut self, count: usize) -> Result<(), HandleError> {
        let additional = count.saturating_sub(self.available);
        self.slots
            .len()
            .checked_add(additional)
            .filter(|length| *length <= u32::MAX as usize)
            .ok_or(HandleError::NoMemory)?;
        self.slots
            .try_reserve(additional)
            .map_err(|_| HandleError::NoMemory)?;
        for _ in 0..additional {
            let index = self.slots.len() as u32 + 1;
            self.slots.push(Slot {
                generation: 1,
                state: SlotState::Vacant(self.free),
            });
            self.free = Some(index);
            self.available += 1;
        }
        Ok(())
    }

    fn reserve(&mut self) -> Handle {
        let index = self.free.expect("no reserved handle capacity");
        let slot = &mut self.slots[index as usize - 1];
        let SlotState::Vacant(next) = mem::replace(&mut slot.state, SlotState::Reserved) else {
            unreachable!("invalid handle free list");
        };
        self.free = next;
        self.available -= 1;
        Handle::from_raw((u64::from(slot.generation) << 32) | u64::from(index))
    }

    fn publish(&mut self, handle: Handle, entry: HandleEntry) {
        self.slots[handle.raw_index().expect("invalid reserved handle")].state =
            SlotState::Occupied(entry);
    }

    fn release(&mut self, handle: Handle) -> Option<HandleEntry> {
        let slot = &mut self.slots[handle.raw_index().expect("invalid released handle")];
        let previous = mem::replace(&mut slot.state, SlotState::Retired);
        if let Some(generation) = slot.generation.checked_add(1) {
            slot.generation = generation;
            slot.state = SlotState::Vacant(self.free);
            self.free = Some(handle.raw() as u32);
            self.available += 1;
        }
        match previous {
            SlotState::Occupied(entry) => Some(entry),
            _ => None,
        }
    }
}

trait SlotHandle {
    fn raw_index(self) -> Option<usize>;
    fn raw_generation(self) -> u32;
}

impl SlotHandle for Handle {
    fn raw_index(self) -> Option<usize> {
        (self.raw() as u32 as usize).checked_sub(1)
    }
    fn raw_generation(self) -> u32 {
        (self.raw() >> 32) as u32
    }
}
