# Objects and Capabilities

Programs use kernel objects through handles.
For example, a Channel handle both identifies a channel endpoint and records whether this handle permits sending, receiving, and transferring.
A Task's handle table stores these object references and permissions, and the kernel uses it to process each call.

## Object Types

`abi::objects!` declares object numbers and the permissions supported by each type in one place.
The ABI's type enumerations, along with the kernel's object and reference types, are generated from this declaration.

| Type | Resource represented |
|---|---|
| `Task` | Address space, handle table, and thread container |
| `Thread` | Object for observing execution and exit |
| `FrameAllocation` | One physical page frame allocation |
| `Channel` | One endpoint of a bidirectional message channel |
| `Event` | Bit notifications that can be coalesced |
| `Timer` | Timer object that sends notifications to an event |
| `Reply` | Reply permission for one call |
| `DeviceSet` | Entry point for the device directory and device group authorization |
| `Device` | Device group authorization |
| `KernelLog` | Entry point for reading kernel logs |
| `DmaDomain` | DMA mappings and start/stop state for a device |
| `TrapPort` | Receiving end for forwarded execution events |
| `TrapContinuation` | Permission to resume a suspended execution |
| `Call` | Result and cancellation entry point for an asynchronous call |

Objects control their own lifetime, and `ObjectReference` keeps a storage reference alive during kernel operations.
Some types also count open references separately so that waiters can be notified promptly when an object is closed.

## Handle Tables

Handles are interpreted within their owning Task.
A handle value encodes a slot and a generation; reusing a slot changes the generation so that identifiers pointing at old contents are rejected.
Slot states are vacant, reserved, occupied, and retired from reuse.

`INFO` returns the object type and the current handle permissions, and `CLOSE` releases a handle.
An object can have multiple handles, and `CLOSE` releases one of those holdings.
When handing a handle across Tasks, the kernel installs the object in the target handle table, and the receiver gets a slot and generation in its own table.

## Permission Checks

`Rights` is a set of permission bits.
The object declaration gives the acceptable permissions, and the method table gives the permissions required by each operation.
For example, sending on a Channel requires `WRITE`, receiving requires `READ`, and calling requires `READ_WRITE`; mapping operations on a Task require `MAP_MEMORY`.

The system call dispatcher first obtains the object type and handle permissions, then looks up the method table and checks permissions, and only then enters the handler for the type.
An unknown method returns `Unsupported`, and insufficient permissions return `Denied`.
The handler goes on to check arguments, target state, and the permissions of other objects involved.

When a new Task handle is created, the creation capability is also constrained by the caller's existing creation permissions.
The child Task's subsequent creation capabilities are determined by this authorization.

## Duplication and Transfer

`DUPLICATE` creates a new handle based on the source handle's duplication permission, with new permissions that are a subset of the source permissions.
For example, when a service holds a readable and writable channel, it can duplicate a writable handle and give it to a log producer so the producer can submit records to the channel.
One-shot objects such as Reply use separate ownership semantics.

IPC transfer requires `TRANSFER`.
The caller specifies receive permissions for each transferred handle; the kernel checks the type and permissions and reserves a slot for the target.
After commit, the sender's handle is moved away, and the receiver gets a handle in the target table.
Duplicating and then transferring one of the copies creates a relationship where both sides hold the object.

`PreparedTransfer` holds the transfer record for the preparation stage.
Dropping an uncommitted prepared object releases the reserved slot; after a successful commit, handles are published in message order.
Object creation also goes through the unified path of reserving a handle, creating the resource, and installing the object, which handles failure cleanup.

## Current Thread Identity

The native ABI reserves handle value 0 for `CURRENT_THREAD`.
At call time the kernel resolves it to the calling thread, which can query time, identity, and state, and can also yield, wait, or exit.
The duplication permissions of this special entry include `DUPLICATE`, `TRANSFER`, `WAIT`, and `TERMINATE`.
Duplicating it produces an ordinary Thread handle; close operations require an ordinary handle.

The current Task, by contrast, is handed over through the program start record.
The runtime keeps this handle for operations such as allocating memory, creating threads, and creating IPC objects.
