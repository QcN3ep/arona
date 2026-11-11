// SPDX-License-Identifier: MPL-2.0

use alloc::{
    alloc::{alloc_zeroed, dealloc},
    vec::Vec,
};
use core::{
    alloc::Layout,
    cell::{Cell, RefCell, UnsafeCell},
    mem::MaybeUninit,
    ptr::NonNull,
};

#[thread_local]
static DESTRUCTORS: RefCell<Vec<Destructor>> = RefCell::new(Vec::new());

struct Destructor {
    value: *const (),
    run: unsafe fn(*const ()),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessError {
    Initializing,
    Destroyed,
}

#[derive(Clone, Copy)]
enum State {
    Empty,
    Initializing,
    Ready,
    Destroyed,
}

pub struct LocalKey<T: 'static> {
    storage: fn() -> *const LocalStorage<T>,
    initialize: fn() -> T,
}

#[doc(hidden)]
pub struct LocalStorage<T> {
    state: Cell<State>,
    value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> LocalStorage<T> {
    pub const fn new() -> Self {
        Self {
            state: Cell::new(State::Empty),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

impl<T> Default for LocalStorage<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LocalKey<T> {
    /// # Safety
    ///
    /// storage must always return the same storage that is exclusive to the current thread and alive until the thread exits.
    #[doc(hidden)]
    pub const unsafe fn new(
        storage: fn() -> *const LocalStorage<T>,
        initialize: fn() -> T,
    ) -> Self {
        Self {
            storage,
            initialize,
        }
    }

    pub fn with<R>(&'static self, function: impl FnOnce(&T) -> R) -> R {
        self.try_with(function)
            .expect("thread local value is unavailable")
    }

    pub fn try_with<R>(&'static self, function: impl FnOnce(&T) -> R) -> Result<R, AccessError> {
        // SAFETY: getter must return the exclusive TLS storage that is alive for the current thread.
        let storage = unsafe { &*(self.storage)() };
        match storage.state.get() {
            State::Initializing => return Err(AccessError::Initializing),
            State::Destroyed => return Err(AccessError::Destroyed),
            State::Empty => {
                storage.state.set(State::Initializing);
                let value = (self.initialize)();
                // SAFETY: The current thread must exclusively own the not-yet-initialized storage.
                unsafe { (*storage.value.get()).write(value) };
                DESTRUCTORS.borrow_mut().push(Destructor {
                    value: core::ptr::from_ref(storage).cast(),
                    run: drop_value::<T>,
                });
                storage.state.set(State::Ready);
            }
            State::Ready => {}
        }
        // SAFETY: The Ready storage must already be initialized, and thread-exit destruction must not run during the borrow.
        Ok(function(unsafe {
            (*storage.value.get()).assume_init_ref()
        }))
    }
}

/// # Safety
///
/// pointer must point to a LocalStorage<T> that is initialized and not yet destroyed for the current thread.
unsafe fn drop_value<T>(pointer: *const ()) {
    // SAFETY: The destructor registration must store the thread-local storage address of the corresponding type.
    let storage = unsafe { &*pointer.cast::<LocalStorage<T>>() };
    storage.state.set(State::Destroyed);
    // SAFETY: All borrows of this value must have ended when the thread exits.
    unsafe { (*storage.value.get()).assume_init_drop() };
}

#[macro_export]
#[allow_internal_unstable(thread_local)]
macro_rules! thread_local {
    ($( $(#[$attribute:meta])* $visibility:vis static $name:ident: $kind:ty = $initialize:expr; )*) => { $(
        $(#[$attribute])*
        $visibility static $name: $crate::tls::LocalKey<$kind> = {
            #[thread_local]
            static STORAGE: $crate::tls::LocalStorage<$kind> = $crate::tls::LocalStorage::new();
            // SAFETY: STORAGE must be provided by the compiler as separate static storage for each thread.
            unsafe { $crate::tls::LocalKey::new(|| core::ptr::addr_of!(STORAGE), || $initialize) }
        };
    )* };
}

pub(crate) fn destroy() {
    loop {
        let Some(destructor) = DESTRUCTORS.borrow_mut().pop() else {
            break;
        };
        // SAFETY: The destructor list must contain only live TLS values of the current thread.
        unsafe { (destructor.run)(destructor.value) };
    }
    *DESTRUCTORS.borrow_mut() = Vec::new();
}

pub(crate) struct TlsBlock {
    pointer: NonNull<u8>,
    layout: Layout,
}

// SAFETY: The TLS allocation must be transferred exclusively and freed after the thread stops accessing it.
unsafe impl Send for TlsBlock {}

unsafe extern "C" {
    static __tls_start: u8;
    static __tls_file_end: u8;
    static __tls_end: u8;
    static __tls_align: u8;
}

impl TlsBlock {
    pub(crate) fn new() -> Result<Self, ()> {
        let template = core::ptr::addr_of!(__tls_start);
        let size = core::ptr::addr_of!(__tls_end) as usize - template as usize;
        let file_size = core::ptr::addr_of!(__tls_file_end) as usize - template as usize;
        let alignment = (core::ptr::addr_of!(__tls_align) as usize).max(16);
        let layout = Layout::from_size_align(size.max(1), alignment).map_err(|_| ())?;
        // SAFETY: layout must be a valid, non-empty layout of the TLS template.
        let pointer = NonNull::new(unsafe { alloc_zeroed(layout) }).ok_or(())?;
        // SAFETY: The template must be readable, and the destination must be a new exclusive allocation with sufficient layout.
        unsafe { core::ptr::copy_nonoverlapping(template, pointer.as_ptr(), file_size) };
        Ok(Self { pointer, layout })
    }

    pub(crate) fn thread_pointer(&self) -> usize {
        self.pointer.as_ptr() as usize
    }

    /// # Safety
    ///
    /// The current thread must no longer access the old TLS, self must not have been activated by another thread, and it must remain alive until the thread stops accessing the new TLS.
    pub(crate) unsafe fn activate(&self) {
        // SAFETY: The current thread must exclusively own its TP, and the new TLS must be fully initialized.
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("mv tp, {}", in(reg) self.pointer.as_ptr(), options(nostack))
        };
        // SAFETY: The current thread must exclusively own its TP, and the new TLS must be fully initialized.
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            core::arch::asm!("move $tp, {}", in(reg) self.pointer.as_ptr(), options(nostack))
        };
    }
}

impl Drop for TlsBlock {
    fn drop(&mut self) {
        // SAFETY: The thread must have stopped accessing this TLS, and the allocation must retain its original layout.
        unsafe { dealloc(self.pointer.as_ptr(), self.layout) };
    }
}
