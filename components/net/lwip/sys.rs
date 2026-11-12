// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! lwIP's OS callbacks. C owns each opaque allocation until its matching free;
//! waits borrow it, and destruction requires all concurrent users to have left.
//! Only ordinary Arona threads may enter this adapter; no hardware IRQ calls it.

use crate::memory;
use core::{
    cell::Cell,
    ffi::{CStr, c_char, c_void},
    ptr,
};
use runtime::{
    Error, KernelError,
    ipc::WaitMode,
    process,
    sync::{Mailbox, Semaphore},
    thread,
};

const ERR_OK: i8 = 0;
const ERR_MEM: i8 = -1;
const TIMEOUT: u32 = u32::MAX;
static PROTECTION: Semaphore = Semaphore::new(1);
static CORE_STARTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[thread_local]
static ERRNO: Cell<i32> = Cell::new(0);
#[thread_local]
static DEPTH: Cell<u32> = Cell::new(0);
#[thread_local]
static IS_CORE: Cell<bool> = Cell::new(false);

struct Message(*mut c_void);
// SAFETY: lwIP transfers the pointed-to message to the receiving thread and
// retains its storage until protocol handling has completed.
unsafe impl Send for Message {}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_errno() -> *mut i32 {
    ERRNO.as_ptr()
}

/// # Safety
///
/// message and file must point to live null-terminated C strings for this call.
#[unsafe(no_mangle)]
unsafe extern "C" fn arona_lwip_assert(
    message: *const c_char,
    file: *const c_char,
    line: i32,
) -> ! {
    // SAFETY: lwIP provides static diagnostic strings from the failing assertion.
    let (message, file) = unsafe { (CStr::from_ptr(message), CStr::from_ptr(file)) };
    log::error!(
        "lwIP assertion: {} at {}:{line}",
        message.to_str().unwrap_or("<non-UTF8>"),
        file.to_str().unwrap_or("<non-UTF8>")
    );
    process::exit(1)
}

#[unsafe(no_mangle)]
extern "C" fn sys_init() {}

#[unsafe(no_mangle)]
extern "C" fn sys_now() -> u32 {
    (thread::now_ns().unwrap() / 1_000_000) as u32
}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_yield() {
    thread::yield_now().unwrap();
}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_mark_core() {
    IS_CORE.set(true);
    CORE_STARTED.store(true, core::sync::atomic::Ordering::Release);
}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_check_core() {
    assert!(
        !CORE_STARTED.load(core::sync::atomic::Ordering::Acquire) || IS_CORE.get(),
        "lwIP core accessed outside its protocol thread"
    );
}

#[unsafe(no_mangle)]
extern "C" fn sys_arch_protect() -> u32 {
    let previous = DEPTH.get();
    if previous == 0 {
        PROTECTION.wait(WaitMode::Forever).unwrap();
    }
    DEPTH.set(previous.checked_add(1).unwrap());
    previous
}

#[unsafe(no_mangle)]
extern "C" fn sys_arch_unprotect(previous: u32) {
    assert_eq!(DEPTH.get().checked_sub(1), Some(previous));
    DEPTH.set(previous);
    if previous == 0 {
        PROTECTION.post().unwrap();
    }
}

fn mode(timeout: u32) -> WaitMode {
    if timeout == 0 {
        WaitMode::Forever
    } else {
        WaitMode::Until(
            thread::now_ns()
                .unwrap()
                .checked_add(u64::from(timeout) * 1_000_000)
                .unwrap(),
        )
    }
}

fn elapsed(start: u64, result: Result<(), Error>) -> u32 {
    match result {
        Ok(()) => {
            ((thread::now_ns().unwrap() - start) / 1_000_000).min(u64::from(TIMEOUT - 1)) as u32
        }
        Err(Error::Kernel(KernelError::TimedOut | KernelError::WouldBlock)) => TIMEOUT,
        Err(error) => {
            log::error!("lwIP synchronization failed: {error:?}");
            process::exit(1)
        }
    }
}

/// # Safety
///
/// output must point to writable storage for a newly created semaphore handle.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_sem_new(output: *mut *mut Semaphore, count: u8) -> i8 {
    let value = memory::new(Semaphore::new(count.into()));
    // SAFETY: The caller exclusively provides the output slot.
    unsafe { output.write(value) };
    if value.is_null() { ERR_MEM } else { ERR_OK }
}

/// # Safety
///
/// sem must contain a live semaphore allocated by sys_sem_new.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_sem_signal(sem: *const *mut Semaphore) {
    // SAFETY: The C owner retains the semaphore throughout this operation.
    unsafe { (**sem).post().unwrap() };
}

/// # Safety
///
/// sem must contain a live semaphore retained until this wait finishes.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_arch_sem_wait(sem: *const *mut Semaphore, timeout: u32) -> u32 {
    let start = thread::now_ns().unwrap();
    // SAFETY: The C owner retains the semaphore throughout the wait.
    elapsed(start, unsafe { (**sem).wait(mode(timeout)) })
}

/// # Safety
///
/// sem must own a live semaphore with no remaining or concurrent users.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_sem_free(sem: *mut *mut Semaphore) {
    // SAFETY: The caller transfers the unique live allocation and output slot.
    unsafe {
        memory::delete(*sem);
        *sem = ptr::null_mut();
    }
}

/// # Safety
///
/// output must point to writable storage for a newly created mailbox handle.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_mbox_new(output: *mut *mut Mailbox<Message>, size: i32) -> i8 {
    let value = usize::try_from(size)
        .ok()
        .and_then(|size| Mailbox::new(size).ok())
        .map_or(ptr::null_mut(), memory::new);
    // SAFETY: The caller exclusively provides the output slot.
    unsafe { output.write(value) };
    if value.is_null() { ERR_MEM } else { ERR_OK }
}

/// # Safety
///
/// mbox must contain a live mailbox retained until this call finishes. Message
/// storage must outlive its handling, with ownership following lwIP's API.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_mbox_post(mbox: *const *mut Mailbox<Message>, message: *mut c_void) {
    // SAFETY: The C owner retains the mailbox and transfers the message.
    if unsafe { (**mbox).send(Message(message), WaitMode::Forever) }.is_err() {
        process::exit(1);
    }
}

/// # Safety
///
/// mbox must contain a live mailbox. Success transfers the message to its
/// receiver; failure leaves its ownership with the caller.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_mbox_trypost(
    mbox: *const *mut Mailbox<Message>,
    message: *mut c_void,
) -> i8 {
    // SAFETY: The C owner retains the mailbox and conditionally transfers the message.
    match unsafe { (**mbox).send(Message(message), WaitMode::Poll) } {
        Ok(()) => ERR_OK,
        Err(_) => ERR_MEM,
    }
}

#[unsafe(no_mangle)]
extern "C" fn sys_mbox_trypost_fromisr(_: *const *mut c_void, _: *mut c_void) -> i8 {
    panic!("lwIP IRQ entry is unavailable")
}

/// # Safety
///
/// mbox must contain a live mailbox retained through the wait. output must be
/// null or point to exclusive writable pointer storage.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_arch_mbox_fetch(
    mbox: *const *mut Mailbox<Message>,
    output: *mut *mut c_void,
    timeout: u32,
) -> u32 {
    // SAFETY: The caller retains the mailbox and output across the complete wait.
    unsafe { fetch(mbox, output, mode(timeout)) }
}

/// # Safety
///
/// mbox must contain a live mailbox. output must be null or point to exclusive
/// writable pointer storage.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_arch_mbox_tryfetch(
    mbox: *const *mut Mailbox<Message>,
    output: *mut *mut c_void,
) -> u32 {
    // SAFETY: The caller retains the mailbox and exclusive output for the call.
    unsafe { fetch(mbox, output, WaitMode::Poll) }
}

/// # Safety
///
/// mbox must retain a live mailbox for the operation; output must be null or an
/// exclusively writable message-pointer slot.
unsafe fn fetch(
    mbox: *const *mut Mailbox<Message>,
    output: *mut *mut c_void,
    mode: WaitMode,
) -> u32 {
    let start = thread::now_ns().unwrap();
    // SAFETY: The C owner retains the mailbox throughout receive.
    let result = unsafe { (**mbox).receive(mode) };
    elapsed(
        start,
        result.map(|message| {
            if !output.is_null() {
                // SAFETY: The caller provides a writable slot for the received pointer.
                unsafe { output.write(message.0) };
            }
        }),
    )
}

/// # Safety
///
/// mbox must own a live, drained mailbox with no remaining or concurrent users.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_mbox_free(mbox: *mut *mut Mailbox<Message>) {
    // SAFETY: The caller transfers the unique allocation and its handle slot.
    unsafe {
        memory::delete(*mbox);
        *mbox = ptr::null_mut();
    }
}

/// # Safety
///
/// function must follow lwIP's thread-entry ABI and remain callable until the
/// thread returns. argument and its reachable storage must remain valid for all
/// accesses made by function, with ownership transferred to the new thread.
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_thread_new(
    _: *const c_char,
    function: unsafe extern "C" fn(*mut c_void),
    argument: *mut c_void,
    stacksize: i32,
    _: i32,
) -> usize {
    let Ok(size) = usize::try_from(stacksize) else {
        return 0;
    };
    let argument = Message(argument);
    let worker = thread::Builder::new()
        .stack_size(size)
        .spawn(move || call(function, argument));
    match worker {
        Ok(worker) => {
            let id = worker.thread().borrow().raw().raw() as usize;
            drop(worker);
            id
        }
        Err(_) => 0,
    }
}

fn call(function: unsafe extern "C" fn(*mut c_void), argument: Message) {
    // SAFETY: sys_thread_new's caller transfers the callback and argument until
    // thread completion; this worker invokes them exactly once.
    unsafe { function(argument.0) };
}
