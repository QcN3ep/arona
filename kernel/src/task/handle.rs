// SPDX-License-Identifier: MPL-2.0

use core::{mem, ops::BitOr, ptr};

use allocator_api2::vec::Vec;
use spin::Mutex;

use crate::{
    hardware::{self, cpu::InterruptState},
    memory::{FrameAllocation, HeapArc},
    thread::Thread,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct Handle(u64);

impl Handle {
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    fn index(self) -> Option<usize> {
        (self.0 as u32 as usize).checked_sub(1)
    }

    fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rights(u64);

impl Rights {
    pub const NONE: Self = Self(0);
    pub const DUPLICATE: Self = Self(1 << 0);
    pub const TRANSFER: Self = Self(1 << 1);
    pub const WAIT: Self = Self(1 << 2);
    pub const SIGNAL: Self = Self(1 << 3);
    pub const READ: Self = Self(1 << 4);
    pub const WRITE: Self = Self(1 << 5);
    pub const MAP: Self = Self(1 << 6);
    pub const EXECUTE: Self = Self(1 << 7);
    pub const CREATE_THREAD: Self = Self(1 << 8);
    pub const TERMINATE: Self = Self(1 << 9);
    pub const MAP_MEMORY: Self = Self(1 << 10);
    pub const RESOURCE_SPLIT: Self = Self(1 << 11);
    pub const DEVICE_CONTROL: Self = Self(1 << 12);
    pub const IRQ_ACK: Self = Self(1 << 13);
    pub const DMA_MAP: Self = Self(1 << 14);
    pub const REVOKE: Self = Self(1 << 16);
    pub const START: Self = Self(1 << 17);
    pub const FRAME_ALLOCATE: Self = Self(1 << 18);
    pub const INSTALL_HANDLE: Self = Self(1 << 19);
    pub const SET_TRAP: Self = Self(1 << 20);
    pub const INTERRUPT: Self = Self(1 << 21);
    pub const BIND_TRAP: Self = Self(1 << 22);
    pub const READ_MEMORY: Self = Self(1 << 23);
    pub const WRITE_MEMORY: Self = Self(1 << 24);
    pub const SET_STATE: Self = Self(1 << 25);

    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits & !0x03ff_7fff == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Rights {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectType {
    Thread,
    FrameAllocation,
}

/// # Safety
///
/// 析构期间须关闭本地中断并保留内存管理器所需的内核映射。
#[derive(Clone)]
pub enum Object {
    Thread(Thread),
    FrameAllocation(HeapArc<FrameAllocation>),
}

impl Object {
    pub fn kind(&self) -> ObjectType {
        match self {
            Self::Thread(_) => ObjectType::Thread,
            Self::FrameAllocation(_) => ObjectType::FrameAllocation,
        }
    }

    fn allowed_rights(&self) -> Rights {
        let common = Rights::DUPLICATE | Rights::TRANSFER;
        common
            | match self {
                Self::Thread(_) => {
                    Rights::WAIT
                        | Rights::TERMINATE
                        | Rights::START
                        | Rights::INTERRUPT
                        | Rights::SET_STATE
                }
                Self::FrameAllocation(_) => {
                    Rights::READ | Rights::WRITE | Rights::MAP | Rights::EXECUTE | Rights::DMA_MAP
                }
            }
    }
}

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
    Occupied(Entry),
    Retired,
}

struct Entry {
    object: Object,
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

struct InterruptGuard(InterruptState);

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
        let entry = Entry { object, rights };
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
    ) -> Result<Object, HandleError> {
        let _interrupts = InterruptGuard::new();
        let state = self.state.lock();
        let entry = state.entry(handle)?;
        if entry.object.kind() != kind {
            return Err(HandleError::WrongType);
        }
        entry.require(rights)?;
        Ok(entry.object.clone())
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
        let object = state.entry(handle)?.object.clone();
        let duplicate = state.reserve();
        state.publish(duplicate, Entry { object, rights });
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

impl Entry {
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
    fn entry(&self, handle: Handle) -> Result<&Entry, HandleError> {
        let slot = handle
            .index()
            .and_then(|index| self.slots.get(index))
            .ok_or(HandleError::BadHandle)?;
        match &slot.state {
            SlotState::Occupied(entry) if slot.generation == handle.generation() => Ok(entry),
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
        Handle((u64::from(slot.generation) << 32) | u64::from(index))
    }

    fn publish(&mut self, handle: Handle, entry: Entry) {
        self.slots[handle.index().expect("invalid reserved handle")].state =
            SlotState::Occupied(entry);
    }

    fn release(&mut self, handle: Handle) -> Option<Entry> {
        let slot = &mut self.slots[handle.index().expect("invalid released handle")];
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

impl InterruptGuard {
    fn new() -> Self {
        Self(hardware::cpu().disable_interrupts())
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        hardware::cpu().restore_interrupts(self.0);
    }
}
