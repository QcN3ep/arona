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
    /// storage 须始终返回当前线程独占且存活至线程退出的同一个存储。
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
        // SAFETY: getter 须返回当前线程存活的独占 TLS 存储。
        let storage = unsafe { &*(self.storage)() };
        match storage.state.get() {
            State::Initializing => return Err(AccessError::Initializing),
            State::Destroyed => return Err(AccessError::Destroyed),
            State::Empty => {
                storage.state.set(State::Initializing);
                let value = (self.initialize)();
                // SAFETY: 当前线程须独占尚未初始化的存储。
                unsafe { (*storage.value.get()).write(value) };
                DESTRUCTORS.borrow_mut().push(Destructor {
                    value: core::ptr::from_ref(storage).cast(),
                    run: drop_value::<T>,
                });
                storage.state.set(State::Ready);
            }
            State::Ready => {}
        }
        // SAFETY: Ready 存储须已初始化，借用期间不执行线程退出析构。
        Ok(function(unsafe {
            (*storage.value.get()).assume_init_ref()
        }))
    }
}

/// # Safety
///
/// pointer 须指向当前线程已初始化且未析构的 LocalStorage<T>。
unsafe fn drop_value<T>(pointer: *const ()) {
    // SAFETY: 析构登记须保存对应类型的线程本地存储地址。
    let storage = unsafe { &*pointer.cast::<LocalStorage<T>>() };
    storage.state.set(State::Destroyed);
    // SAFETY: 线程退出时须已结束对该值的所有借用。
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
            // SAFETY: STORAGE 须由编译器为每个线程提供独立的静态存储。
            unsafe { $crate::tls::LocalKey::new(|| core::ptr::addr_of!(STORAGE), || $initialize) }
        };
    )* };
}

pub(crate) fn destroy() {
    loop {
        let Some(destructor) = DESTRUCTORS.borrow_mut().pop() else {
            break;
        };
        // SAFETY: 析构列表须仅包含当前线程的存活 TLS 值。
        unsafe { (destructor.run)(destructor.value) };
    }
    *DESTRUCTORS.borrow_mut() = Vec::new();
}

pub(crate) struct TlsBlock {
    pointer: NonNull<u8>,
    layout: Layout,
}

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
        // SAFETY: layout 须为有效且非空的 TLS 模板布局。
        let pointer = NonNull::new(unsafe { alloc_zeroed(layout) }).ok_or(())?;
        // SAFETY: 模板须可读，目标须为布局足够且独占的新分配。
        unsafe { core::ptr::copy_nonoverlapping(template, pointer.as_ptr(), file_size) };
        Ok(Self { pointer, layout })
    }

    /// # Safety
    ///
    /// 当前线程不得再访问旧 TLS，self 须存活至线程停止访问新 TLS。
    pub(crate) unsafe fn activate(&self) {
        // SAFETY: 当前线程须独占其 TP，且新 TLS 须已完整初始化。
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("mv tp, {}", in(reg) self.pointer.as_ptr(), options(nostack))
        };
        // SAFETY: 当前线程须独占其 TP，且新 TLS 须已完整初始化。
        #[cfg(target_arch = "loongarch64")]
        unsafe {
            core::arch::asm!("move $tp, {}", in(reg) self.pointer.as_ptr(), options(nostack))
        };
    }
}

impl Drop for TlsBlock {
    fn drop(&mut self) {
        // SAFETY: 线程须已停止访问此 TLS，分配须保持原始布局。
        unsafe { dealloc(self.pointer.as_ptr(), self.layout) };
    }
}
