# Runtime Overview and Programming Model

Native services start executing from an ordinary Rust `main`, can create a `Vec`, spawn closure threads, and access other services through channels.
The program entry point, allocator, and thread environment are provided by `arona-runtime`, which also wraps kernel capabilities into Rust interfaces.
init, devmgr, logger, fs, and linux use this library under the crate name `runtime`.

The runtime is linked with the program into the user address space.
For example, when a `Vec` grows, it requests page frames from the kernel and establishes user mappings; when sending a message, it encodes arguments and executes native system calls.
The program's Rust objects and kernel resources are connected by these wrappers.

## The Two Runtime Builds

The runtime is a `no_std` library that uses `alloc`.
The default build provides kernel call interfaces, and the `program` feature adds an entry point and runtime environment for executables:

| Build | Provided content |
|---|---|
| Default features | Object and handle, low-level memory and thread interfaces, IPC, devices, DMA, Trap and kernel log interfaces, and child Task construction helper types |
| `program` with target system `none` | Additionally provides the program entry point, panic and allocation failure handling, global heap, current program environment, TLS, and safe thread creation and reclamation |

The `baremetal` feature of a system service enables `runtime/program`.
The `baremetal` feature in each service's Cargo configuration selects the corresponding binary's build mode.

## From Entry to main

The loader establishes the program's segment mappings, user stack, and initial thread, and passes the address of `ProcessStart` to `_start`.
This record contains the current Task handle and an optional bootstrap channel.
The runtime receives the record, establishes the initial thread's TLS, and then calls the parameterless `main` through the compiler's startup entry point.

A running program uses its own Task through `runtime::process::task()`, and takes the bootstrap channel through `runtime::process::bootstrap()`.
The bootstrap resource is delivered once; after it is taken, the program owns it.

The messages in the bootstrap channel are agreed upon by the starter and the started program.
init receives the startup archive, device set, and kernel log handle from it, while devmgr receives the device catalog and its management connection.
For the program entry point and Cargo configuration, see [Writing Native Programs](programs.md).

## Return, Panic, and Exit

When `main` returns, the runtime first runs the TLS destructors registered for the current thread, and then ends the Task.
The return type determines the exit status:

| Return type | Exit status |
|---|---|
| `()` | 0 |
| `usize` | The return value |
| `Result<T, E>` | `Ok` uses the exit status of `T`, `Err` uses 1 |

The `Result` implementation requires `E: Debug`, and the error value is converted to exit status 1.
Logging is recorded by the program through logger, and the return path is responsible for handing over the status.
Panic and global allocation failure likewise end the Task with status 1.

`process::exit` and `thread::exit` both end control flow directly without unwinding the Rust stack.
These two interfaces enter the termination path directly; ordinary destruction of the current stack and TLS destruction are handled by the normal return path.
When a worker thread returns normally, the runtime runs TLS destructors and saves the return value for `join` to retrieve.

## Heap and Threads

After the program runtime environment is enabled, containers such as `Box` and `Vec` in `alloc` use the global allocator provided by the runtime.
This allocator uses `dlmalloc`; when the heap needs to grow, it requests page frames from the current Task and establishes user mappings, and it revokes mappings when a complete underlying region is released.
The boot environment delivers the rights these operations require along with the program's own Task handle.

`runtime::thread::spawn` accepts a closure satisfying `Send + 'static` and returns a `JoinHandle`.
The runtime establishes the thread stack, guard page, and TLS, and reclaims them after the thread stops using these resources.
`JoinHandle::join` waits for the thread to finish and obtains the normal return value; dropping a `JoinHandle` detaches the thread, and the runtime reclaims resources after it exits.

These threads use the same Task's address space.
`JoinHandle` manages the kernel Thread handle together with the closure, return value, stack, and TLS; see [Program Runtime Environment](environment.md) for details.

## Object and Handle Interfaces

`Handle<T>` owns a handle to some object in the current Task.
`T` denotes the object type, such as `Task`, `Thread`, `Channel`, or `FrameAllocation`.
Dropping `Handle<T>` attempts to close the handle.

`BorrowedHandle<'a, T>` borrows an existing handle and is not responsible for closing it.
`AnyHandle` is used to receive handles whose type can only be determined after a message arrives; it can check the object type and convert to a concrete type through `downcast`.
Copying a handle uses `try_clone(rights)`, which requires the kernel to confirm that the source handle supports copying and that the new rights do not exceed the source rights.

`T` expresses the object type in Rust, while the rights in a handle are checked by the kernel each time.
See [Using Kernel Capabilities](capabilities.md) for this ownership and rights usage model.

## IPC and Service Interfaces

The runtime provides channel messages, handle passing, synchronous calls, asynchronous calls, events, timers, and multi-object waiting.
Callers can choose polling, infinite waiting, or an absolute deadline.

The meaning of the `tag` and data words in a message is agreed upon by the specific service.
The protocols and client wrappers for file, log, and device services live in their respective crates, and the runtime provides the transport mechanism they share.

`Channel::call` uses `CallError` to distinguish errors before and after request submission and to express whether the handles to transfer have already been handed over.
For examples of request/reply and event loops, see [IPC Programming](ipc.md).

Ordinary interfaces return errors using `runtime::Error`:

| Error | Meaning |
|---|---|
| `Error::Kernel` | An object, rights, address, resource, or wait error returned by the kernel |
| `Error::Protocol` | Kernel return values or exchanged data do not conform to the current interface conventions |

Service clients may also define their own error types to represent results such as file not found, device state, or service protocol errors.

## Native Rust and Linux Programs

Native services use `core`, `alloc`, and the interfaces provided by the runtime.
Linux static programs are loaded by the compatibility service, use the language runtime linked into the program itself, and request system functionality through the Linux ABI.
Both kinds of programs run in kernel Tasks, and their entry points and calling conventions are established by their respective loading paths.
