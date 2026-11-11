# Native System Calls

When a native program calls `event.signal(mask)`, the runtime places the Event handle, the SIGNAL method, and the bitmask in registers, then traps into the kernel.
The kernel obtains the object from the handle, checks the method permission, performs the operation, and returns the result.
Other native system calls use this same "handle, method, arguments" structure.

`abi` defines the encoding shared by both sides, and the runtime provides Rust wrappers.
The file, log, and device services provide operations on top of this mechanism through their respective IPC protocols.

## Register Convention

Both architectures use the same argument layout:

| Register | Input | Output |
|---|---|---|
| `a7` | Target handle | — |
| `a0` | Object method | Status code, 0 means success |
| `a1` | Argument 1 | Result word 1 |
| `a2` | Argument 2 | Result word 2 |
| `a3` to `a5` | Arguments 3 to 5 | — |

RISC-V uses `ecall`, and LoongArch uses `syscall 0`.
Larger structures are passed via a user address in an argument, and the kernel reads or writes them according to ABI records.
Records use explicit field encodings, and call return values are expressed by `abi::Return`.

## Dispatch Path

1. The architecture Trap entry saves state and converts the handle and arguments into `TrapEvent::Syscall`.
2. The kernel obtains the current thread and its owning Task.
3. It obtains the object type and permissions from the handle, or resolves `CURRENT_THREAD`.
4. It queries `method::required_rights` and checks the method and permissions.
5. It executes the common handle methods, or enters the handler module corresponding to the object type.
6. It returns the status and two result words, and resumes at the address following the syscall.

Native method dispatch is centralized in `kernel/src/syscall/mod.rs`.
Linux program syscalls are forwarded through a TrapPort to the compatibility service, where they are interpreted according to Linux conventions.

## Method Groups

All objects share `INFO`, `CLOSE`, and `DUPLICATE`.
Object-specific methods start at 16 and reuse numbers by type.

| Type | Dedicated operations |
|---|---|
| Task | Allocate page frames, map, reserve, and query addresses, read or write target memory, create Task, Thread, Channel, Event, Timer, install handles, set Traps, terminate and wait |
| Thread | Status, start, terminate, wait, exit, yield, sleep, time, identity, user state setup, interrupt and multi-object wait |
| FrameAllocation | Size, read, and write |
| Channel | Send, receive, synchronous call, start asynchronous call |
| Call, Reply | Receive result, cancel call, send one-shot reply |
| Event, Timer | Merged bit notification, set or cancel deadline |
| DeviceSet | Enumerate, describe, read fields, grant device groups |
| Device | Map, unmap, bind interrupt, unbind, acknowledge, revoke, create DMA object |
| DmaDomain | Status, map, unmap, enable, quiesce, and revoke |
| TrapPort, TrapContinuation | Receive event, read user state, resume |
| KernelLog | Read records by cursor |

Method constants and required permissions are centralized in `abi/src/method.rs`.
Concrete input records reside in modules such as `abi/src/memory.rs`, `thread.rs`, and `device.rs`.

## Arguments, User Memory, and Errors

Handlers verify argument ranges, record lengths, object types, and target state.
User memory is checked through the caller's address space and the backing page frame references are retained.
Reading or writing memory across Tasks also checks the memory permissions corresponding to the target Task.

| Error | Common meaning |
|---|---|
| `BadHandle`, `WrongType` | Invalid handle or type mismatch |
| `Denied` | Insufficient permission |
| `InvalidArgument`, `InvalidState` | The argument or object state does not allow the operation |
| `BadAddress` | The user input/output address cannot be accessed as requested |
| `NoMemory` | Resource allocation failed |
| `WouldBlock`, `TimedOut` | Cannot complete now, or the wait expired |
| `Closed`, `PeerClosed`, `ServerGone` | The object or communicating party is closed |
| `BufferTooSmall` | Insufficient output space |
| `Unsupported` | The method or request is not an operation accepted by this interface |
| `Cancelled`, `Busy` | The request was cancelled or the resource is in use |

Internal errors are converted to `abi::Error` at the syscall boundary.
User memory access uses a separate error wrapper that converts invalid user addresses into `BadAddress`.
Operations with commit semantics such as CALL also return phase information; see [IPC and Waiting](ipc.md).
