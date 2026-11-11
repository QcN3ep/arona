# Program Runtime Environment

Creating a `Vec` from `main`, or launching a worker thread with `thread::spawn`, both depend on the program's own runtime environment.
With `runtime/program` enabled, the runtime manages the current Task, heap, TLS, and thread resources in each native program's address space.

## Current Program

`ProcessStart` is encoded as 16 bytes and contains handles for the current Task and an optional bootstrap channel.
Its own Task handle carries page frame allocation, memory mapping, and termination rights; during initialization the runtime checks the handoff record against the handle information.

| Interface | Purpose |
|---|---|
| `process::task()` | Borrow the saved handle of the program's own Task |
| `process::bootstrap()` | Take the optional bootstrap channel |
| `process::exit(status)` | Terminate the current Task |
| `thread::current()` | Borrow a special handle to the current thread |

The boot process establishes the initial thread's TLS before calling `main`.
When `main` returns normally, TLS destructors run first, and then the Task exits.
Panic and allocation failure use exit code 1.
Calling the exit interface directly ends control flow without unwinding the current Rust stack.

## User Heap

The global allocator uses `dlmalloc`, and its state is protected by a spin lock.
It allocates page frames through the current Task, establishes read-write user mappings, and hands the resulting virtual ranges to the allocator.
`alloc` types such as `Box`, `Vec`, `String`, and `Arc` use this allocation path.

For example, storage obtained by growing a `Vec` comes from a mapping range managed by `dlmalloc`, and on release it first returns to this user heap for reuse by later allocations.
When `dlmalloc` returns an entire underlying range, the runtime revokes the page-by-page mappings, and the kernel then releases the corresponding mapping references.

## Safe Thread Creation

`thread::spawn` accepts an `FnOnce() -> T`, and both the closure and the return value require `Send + 'static`.
A successful call returns a `JoinHandle<T>`.
The runtime first saves the closure, establishes the user stack, guard page, and TLS, and then creates the kernel thread, letting the new thread begin execution from this closure.

The default user stack is 2 MiB.
`thread::Builder::stack_size` can specify the stack size; the runtime rounds it up to a page boundary and checks for at least 16 KiB.
Callers such as device driver worker threads use Builder to choose their stack size.

When a worker thread returns normally, it runs TLS destructors, saves the return value, and ends the kernel thread.
`JoinHandle::join` waits for the kernel thread to exit, then takes the return value and reclaims resources.
`JoinError::Exited` indicates that the thread exited without delivering a return value normally, while `JoinError::Runtime` indicates that a runtime operation such as waiting failed.

Dropping a JoinHandle detaches the thread.
The runtime hands its resources to a reclamation thread to hold; the worker thread continues execution, and after it exits the reclamation thread frees the stack, TLS, and result storage.

## Thread Local Storage

The linker script places TLS initial data in `.tdata` and the zero-initialized part in `.tbss`.
The runtime copies the TLS initial image for each thread, zeroes the rest, and sets the thread pointer for the corresponding architecture.

`thread_local!` provides lazily initialized thread-local values:

```rust
use core::cell::Cell;

runtime::thread_local! {
    static COUNT: Cell<u64> = Cell::new(0);
}

fn next_count() -> u64 {
    COUNT.with(|count| {
        let next = count.get() + 1;
        count.set(next);
        next
    })
}
```

Suppose two threads each call `next_count`; each obtains 1 on its first call and thereafter accumulates independently in its own `Cell`.
`try_with` returns `AccessError` for initialization reentrancy and already-destructed states, and `with` panics in those states.
Values that are initialized and require destruction are registered in that thread's destruction list and processed when the thread returns normally.

## Child Task Construction

`ChildTask` is used to construct an independent native program environment.
It provides interfaces for allocation, mapping, reservation ranges, and installing handles; `start` establishes the user stack and ProcessStart, and then starts the entry point.
Dropping a ChildTask during construction terminates the child Task that has not yet been delivered.

`SupervisedTask` is used for execution environments supervised by a TrapPort.
It binds the supervision port at creation and provides address space and thread state configuration capabilities.
The Linux compatibility service establishes Linux programs through this type.

init uses ChildTask to load native services, and the Linux compatibility service uses SupervisedTask to establish supervised programs.
Rust closure threads in the same address space are created by `thread::spawn`, and their code and data share the program's mappings.
