// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{Error, ObjectType, Rights, call};
use core::marker::PhantomData;

pub trait Object: crate::sealed::Object {
    const TYPE: ObjectType;
}

pub struct Handle<T: Object> {
    raw: abi::Handle,
    _kind: PhantomData<T>,
}
pub struct BorrowedHandle<'a, T: Object> {
    raw: abi::Handle,
    _owner: PhantomData<&'a Handle<T>>,
}
impl<T: Object> Copy for BorrowedHandle<'_, T> {}
impl<T: Object> Clone for BorrowedHandle<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandleInfo {
    pub kind: u64,
    pub rights: Rights,
}

impl<T: Object> Handle<T> {
    /// # Safety
    ///
    /// raw must be an installed handle of type T held exclusively by the current Task.
    pub unsafe fn from_raw(raw: abi::Handle) -> Self {
        Self {
            raw,
            _kind: PhantomData,
        }
    }
    pub fn borrow(&self) -> BorrowedHandle<'_, T> {
        BorrowedHandle {
            raw: self.raw,
            _owner: PhantomData,
        }
    }
    pub fn info(&self) -> Result<HandleInfo, Error> {
        self.borrow().info()
    }
    pub fn try_clone(&self, rights: Rights) -> Result<Self, Error> {
        self.borrow().try_clone(rights)
    }
    pub fn into_raw(self) -> abi::Handle {
        let raw = self.raw;
        core::mem::forget(self);
        raw
    }
}
impl<T: Object> Drop for Handle<T> {
    fn drop(&mut self) {
        // SAFETY: CLOSE takes no buffer arguments.
        let _ = unsafe { call::invoke(self.raw, abi::method::CLOSE, [0; 5]) };
    }
}
impl BorrowedHandle<'static, crate::Thread> {
    pub(crate) const fn current() -> Self {
        Self {
            raw: abi::Handle::CURRENT_THREAD,
            _owner: PhantomData,
        }
    }
}
impl<T: Object> BorrowedHandle<'_, T> {
    pub fn raw(&self) -> abi::Handle {
        self.raw
    }
    pub fn info(&self) -> Result<HandleInfo, Error> {
        // SAFETY: Topic zero returns handle metadata in registers without accessing buffers.
        let [kind, rights] = unsafe { self.invoke(abi::method::INFO, [0; 5]) }?;
        Ok(HandleInfo {
            kind,
            rights: Rights::from_bits(rights).ok_or(Error::Protocol)?,
        })
    }
    pub fn try_clone(&self, rights: Rights) -> Result<Handle<T>, Error> {
        // SAFETY: DUPLICATE takes rights and no buffer arguments.
        let [raw, _] = unsafe { self.invoke(abi::method::DUPLICATE, [rights.bits(), 0, 0, 0, 0]) }?;
        // SAFETY: A successful DUPLICATE must deliver a new handle of the same type.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
    /// # Safety
    ///
    /// Any buffers encoded in arguments must remain valid for the call, and output buffers must be exclusively borrowed.
    pub(crate) unsafe fn invoke(
        &self,
        method: u64,
        arguments: [u64; 5],
    ) -> Result<[u64; 2], Error> {
        // SAFETY: The caller retains the encoded buffers and their required access.
        unsafe { call::invoke(self.raw, method, arguments) }
    }

    pub(crate) fn query(&self, topic: u64) -> Result<[u64; 2], Error> {
        // SAFETY: Query and output lengths are zero, so no buffers can be accessed.
        unsafe { self.invoke(abi::method::INFO, [topic, 0, 0, 0, 0]) }
    }
}

#[derive(Debug)]
pub struct AnyHandle {
    raw: abi::Handle,
    kind: u64,
}
impl AnyHandle {
    pub fn kind(&self) -> u64 {
        self.kind
    }
    pub fn downcast<T: Object>(self) -> Result<Handle<T>, Self> {
        if self.kind != T::TYPE as u64 {
            return Err(self);
        }
        let raw = self.into_raw();
        // SAFETY: The type is already matched, and ownership transfers from the dynamic handle.
        Ok(unsafe { Handle::from_raw(raw) })
    }
    pub(crate) fn raw(&self) -> abi::Handle {
        self.raw
    }
    pub(crate) fn into_raw(self) -> abi::Handle {
        let raw = self.raw;
        core::mem::forget(self);
        raw
    }
    pub(crate) fn received(descriptor: abi::ipc::HandleDescriptor) -> Self {
        Self {
            raw: descriptor.handle,
            kind: descriptor.kind,
        }
    }
}
impl<T: Object> From<Handle<T>> for AnyHandle {
    fn from(handle: Handle<T>) -> Self {
        Self {
            raw: handle.into_raw(),
            kind: T::TYPE as u64,
        }
    }
}
impl Drop for AnyHandle {
    fn drop(&mut self) {
        // SAFETY: CLOSE takes no buffer arguments.
        let _ = unsafe { call::invoke(self.raw, abi::method::CLOSE, [0; 5]) };
    }
}
impl<T: Object> core::fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Handle")
            .field("raw", &self.raw)
            .field("kind", &T::TYPE)
            .finish()
    }
}
