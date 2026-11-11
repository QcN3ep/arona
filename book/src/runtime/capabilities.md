# Using Kernel Capabilities

The runtime holds kernel objects and resources with Rust types.
An allocation handle can be handed to the mapping interface, an MMIO mapping can be handed to a device driver, and destruction ties cleanup such as closing handles and unmapping back into the object's lifetime.
The kernel checks permissions and state at each of these operations.

## Owning, Borrowing, and Checking Handles

`Handle<T>` owns a handle and calls CLOSE on destruction.
`BorrowedHandle<'a, T>` borrows an existing handle, and its lifetime is tied to the original owner.
`info` returns the object type and rights, and `try_clone(rights)` requests a new handle with reduced or identical rights.

An `AnyHandle` received from IPC also owns a handle.
A program can check `kind()` and then convert it to a concrete type through `downcast::<T>()`; if the type does not match, the original AnyHandle is returned.
The converted handle retains the original rights.

```rust
use runtime::{AnyHandle, DeviceSet, Error, Handle};

fn read_device_catalog(
    handle: AnyHandle,
) -> Result<Handle<DeviceSet>, Error> {
    handle.downcast().map_err(|_| runtime::Error::Protocol)
}
```

This code expects to receive a device catalog and, on a successful match, obtains `Handle<DeviceSet>`.
When the type does not match, `downcast` returns the original handle; the example releases it along the error path and returns `Protocol`.

## Page Frames and Mapping

Task's `allocate` returns a FrameAllocation handle.
`read` and `write` access the allocation contents byte by byte, while `map` places it into an address space by page frame range.
The following writes a string into a one-page allocation, establishes a mapping, and then reads back the same backing data:

```rust
use runtime::{Rights, memory::Permissions};

fn mapped_bytes() -> Result<(), runtime::Error> {
    let task = runtime::process::task();
    let mut frames =
        task.allocate(1, Rights::READ_WRITE | Rights::MAP)?;
    frames.write(0, b"Arona")?;
    let mut mapping =
        task.map(&frames, 0, 1, None, Permissions::ReadWrite)?;
    let _range =
        mapping.range().ok_or(runtime::Error::Protocol)?;
    let mut bytes = [0; 5];
    frames.read(0, &mut bytes)?;
    mapping.unmap()?;
    Ok(())
}
```

`Mapping` holds a borrow of the target Task and the virtual range.
Explicit `unmap` returns the result of unmapping, and destruction also attempts to unmap.
`protect` modifies mapping permissions, and `Reservation` holds a reserved range and performs UNRESERVE on release.
After the FrameAllocation handle is closed, an already-valid kernel mapping reference continues to keep the backing memory alive.
The end of the mapping and the release of allocation ownership together determine the final reclamation time.

## Threads and Task Control

The Task handle provides creating child Tasks, installing handles, termination, waiting, and address space operations.
`install` consumes the transferred Handle; on failure it returns the error together with the original Handle.

The Thread handle provides operations such as start, state, terminate, wait, and id.
The current thread can also query monotonic time, sleep, and yield the processor.
See [Program Runtime Environment](environment.md) for stack and result management of closure threads.

## Devices, MMIO, and DMA

DeviceSet supports enumeration, reading descriptions, and granting device groups.
Device's `map_mmio` returns `Mmio`, which provides register access with offset and width checks.
`InterruptBinding` holds borrows of a device and an Event, provides acknowledge and unbind, and handles unbinding on destruction.

`DmaMapping` and `DmaBuffer` hold the DMA range, bus address, and backing resources.
Programs read and write buffers through these wrappers, synchronize before and after submitting to a queue, and deactivate, unmap, and revoke according to object state.
The VirtIO driver in devmgr uses this layer to establish device queues and data buffers.

## Trap and Kernel Log

`trap_port` creates an event receiving endpoint, and `bind_trap` binds a supervised Task.
The receive result contains event information and a continuation.
The continuation reads, modifies, and restores user register state; on restore failure it returns the error together with the original object.

KernelLog's `read` accepts a cursor and an output slice and returns an optional borrowed record.
The record text borrows the output slice; after the reader has processed this record, it reuses this storage and obtains the next record with a new cursor.

## Error Handling

`runtime::Error::Kernel` preserves ABI error codes, while `Error::Protocol` indicates that handoff or kernel-returned data does not conform to the wrapper's conventions.
Error returns also preserve resource ownership.
For example, a failed `install` returns the original Handle, and a failed continuation restore returns the original continuation, so the caller can continue processing the objects it holds.
