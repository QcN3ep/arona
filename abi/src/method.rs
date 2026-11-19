// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Object call numbers, parameter counts, and handle rights.
//!
//! Methods execute synchronously. Only Channel SEND/RECEIVE/CALL and WAIT_MANY
//! may block. A deadline is absolute monotonic nanoseconds: zero polls and
//! u64::MAX waits indefinitely. Waiting thread termination abandons the call.
//! Unknown methods return Unsupported. Invalid inputs return InvalidArgument,
//! missing rights return Denied, and invalid lifecycle states return InvalidState.
//! Errors carry zero values except BufferTooSmall and CALL's submission flag.

use crate::{ObjectType, Rights};

macro_rules! methods {
    ($name:ident { $($(#[$meta:meta])* $method:ident = $number:literal => $right:ident ($($argument:ident),*);)* }) => {
        pub mod $name {
            use crate::Rights;
            $(
                $(#[$meta])*
                #[doc = concat!("Required handle rights: `", stringify!($right), "`.")]
                #[doc = concat!("Arguments: `", stringify!($($argument),*), "`. Unused registers must be zero.")]
                pub const $method: u64 = $number;
            )*
            pub(super) const fn required(method: u64) -> Option<Rights> {
                match method { $($method => Some(Rights::$right),)* _ => None }
            }
            pub(super) const fn argument_count(method: u64) -> Option<usize> {
                match method { $($method => Some((&[$(stringify!($argument)),*] as &[&str]).len()),)* _ => None }
            }
        }
    };
}
methods!(task {
    /// Returns a new allocation handle. Zero alignment allows scattered frames.
    /// Nonzero alignment requests contiguous frames. Failure retains no frames or handle.
    ALLOCATE = 16 => FRAME_ALLOCATE (frame_count, handle_rights, alignment);
    /// Maps an allocation range into the target Task.
    /// MapMode encodes access in bits 0..3 and the leaf level in bits 8..15.
    /// Levels zero and one select base pages and HUGE_PAGE_SIZE respectively.
    /// Other levels return InvalidArgument. Requested leaves require their own
    /// alignment and physical contiguity and never fall back to smaller pages.
    /// The allocation additionally requires MAP and the requested access rights.
    /// Success returns `[start, length]`; failure leaves the address space unchanged.
    MAP = 17 => MAP_MEMORY (allocation, frame_offset, frame_count, requested_address, mode);
    /// Removes the selected mapping range and releases its mapping references.
    /// Unmapped or invalid ranges fail without changing the remaining address space.
    UNMAP = 18 => MAP_MEMORY (start, length);
    /// Changes access permissions for the selected range.
    /// Device mappings retain their grant and accept their complete mapped range.
    /// Partial device ranges return Unsupported; access above the grant returns Denied.
    PROTECT = 19 => MAP_MEMORY (start, length, permissions);
    /// Reserves an aligned address range in the target Task.
    RESERVE = 20 => MAP_MEMORY (length, alignment, requested_address);
    /// Releases a reserved range. Existing mappings are not removed.
    UNRESERVE = 21 => MAP_MEMORY (start, length);
    /// Returns a new child Task handle. Requested creation rights cannot exceed
    /// the authority of the target handle. Failure publishes no child.
    CREATE = 24 => CREATE_TASK (handle_rights);
    /// Reads a 32-byte UserEntry and returns an unstarted Thread handle.
    /// The entry requires executable code and a writable aligned stack. Failure
    /// publishes no handle or registered thread. New threads have weight 1024.
    CREATE_THREAD = 25 => CREATE_THREAD (entry_address, handle_rights);
    /// Moves the source capability into the target Task with attenuated rights.
    /// Success returns the target handle. Failure leaves the source owned by the caller.
    INSTALL = 26 => INSTALL_HANDLE (source_handle, rights);
    /// Requests termination with exit_status. Resources are reclaimed after threads
    /// stop. WAIT_MANY and LIFECYCLE expose the completed exit.
    TERMINATE = 27 => TERMINATE (exit_status);
    /// Returns both ends of a new Channel. Failure publishes neither handle.
    CHANNEL = 29 => CREATE_CHANNEL (first_rights, second_rights);
    /// Returns an initially unsignaled Event handle. Failure publishes no handle.
    EVENT = 30 => CREATE_EVENT (handle_rights);
    /// Returns a disarmed Timer bound to the Event, which additionally requires
    /// SIGNAL. The mask must be nonzero. The Timer retains the Event reference.
    TIMER = 31 => CREATE_TIMER (event_handle, signal_mask, handle_rights);
    /// Copies bytes from the target Task to the caller and returns the copied length.
    /// Address and access failures return BadAddress. No capability is transferred.
    READ_MEMORY = 32 => READ_MEMORY (remote_address, local_address, length);
    /// Copies bytes from the caller to another Task and returns the copied length.
    /// Writing the calling Task is Denied. Address and access failures return BadAddress.
    WRITE_MEMORY = 33 => WRITE_MEMORY (remote_address, local_address, length);
    /// Returns an empty TrapPort owned by the target Task. A supervised Task
    /// cannot own a port. Failure publishes no handle.
    TRAP_PORT = 34 => SET_TRAP (handle_rights);
    /// Binds a created Task to a different Task's port before it owns ports or starts.
    /// Binding is single-use. Forwarded user events suspend the source thread.
    /// Failed delivery or an abandoned continuation terminates the source thread.
    /// Closing the last port owner terminates all Tasks still bound to it.
    SET_TRAP = 35 => SET_TRAP (port_handle);
    /// Compares and exchanges one aligned writable u32, returning [previous, exchanged].
    /// The calling Task is Denied. Expected and desired must fit in u32. A mismatch is successful with exchanged=0.
    ATOMIC_COMPARE_EXCHANGE = 40 => WRITE_MEMORY (address, expected, desired);
});
methods!(memory {
    /// Copies bytes from the allocation to caller memory without changing ownership.
    READ = 17 => READ (offset, buffer_address, length);
    /// Copies caller bytes into the allocation without changing ownership.
    WRITE = 18 => WRITE (offset, buffer_address, length);
});

methods!(thread {
    /// Makes an unstarted thread runnable. Repeated starts return InvalidState.
    START = 17 => START ();
    /// Requests thread termination with exit_status. Lifecycle queries retain the
    /// final status after execution resources have been reclaimed.
    TERMINATE = 18 => TERMINATE (exit_status);
    /// Yields the calling thread to other runnable threads. Other targets return Denied.
    YIELD = 21 => NONE ();
    /// Waits on distinct sources until one is ready or the deadline expires.
    /// Each target requires WAIT. Events consume selected signal bits; other
    /// sources remain ready until their contents or lifecycle change.
    /// TrapPort is readable while its queue is nonempty. Empty waits require
    /// a zero items address: polling returns WouldBlock, finite deadlines
    /// return TimedOut, and an infinite wait lasts until termination.
    /// Count must not exceed the caller's handle count plus CURRENT_THREAD.
    WAIT_MANY = 24 => NONE (items_address, item_count, deadline);
    /// Writes register groups before START.
    /// Entries are register::WriteRequest records. The batch is applied atomically.
    SET_REGISTERS = 25 => SET_STATE (groups_address, groups_count);
    /// Suspends a supervised runnable thread and delivers Interrupted. A running
    /// thread receives it at the user return boundary. An unbound thread returns InvalidState.
    INTERRUPT = 26 => INTERRUPT ();
    /// Sets the relative CPU weight, accepting 1..=u32::MAX.
    /// The default weight is 1024. Failure preserves the previous weight.
    SET_SCHEDULING = 32 => SET_STATE (weight);
});

methods!(trap_port {
    /// Returns `[continuation_handle, 48]` without waiting. WouldBlock leaves the
    /// queue unchanged. The output holds one TrapInfo record. Failure leaves the
    /// event queued and publishes no continuation handle.
    RECEIVE = 16 => READ (output_address);
});
methods!(trap_continuation {
    /// Updates register groups and resumes atomically.
    /// An empty batch preserves registers. Failure retains the continuation.
    RESUME = 17 => SET_STATE (groups_address, groups_count);
});

methods!(channel {
    /// Submits a message and transfers its capabilities when writable. Before
    /// submission, timeout or failure leaves the message and handles with the caller.
    SEND = 16 => WRITE (message_address, deadline);
    /// Receives a message and publishes up to max_handles capabilities. BufferTooSmall
    /// reports the needed handle count without consuming the message. Success returns [288, 0].
    RECEIVE = 17 => READ (output_address, max_handles, deadline);
    /// Submits a request and waits for its reply on the calling thread. Success returns
    /// [288, 1]. Errors other than BufferTooSmall report submission in the second value.
    /// After submission the transferred handles belong to the receiver, including on timeout.
    CALL = 18 => READ_WRITE (message_address, output_address, deadline);
    /// Submits a request and returns a Call handle for cancellation and waiting.
    /// Failure publishes no Call and leaves transferred capabilities with the caller.
    START_CALL = 19 => READ_WRITE (message_address);
});
methods!(call {
    /// Copies an available reply and publishes its handles. WouldBlock leaves the Call
    /// pending. Success consumes the reply, and another receive returns InvalidState.
    RECEIVE = 16 => READ (output_address);
    /// Cancels an outstanding Call and invalidates its Reply. Completed calls return
    /// InvalidState. Cancellation does not recover capabilities already submitted.
    CANCEL = 17 => WRITE (); });
methods!(reply {
    /// Sends the reply and transfers its capabilities once. Success consumes the Reply.
    /// Failure before submission leaves the handles owned by the caller.
    SEND = 16 => WRITE (message_address); });
methods!(event {
    /// Sets nonzero signal bits and wakes matching waiters. WAIT_MANY consumes
    /// observed bits. A closed Event returns Closed.
    SIGNAL = 16 => SIGNAL (signal_mask); });
methods!(timer {
    /// Arms one notification at an absolute deadline; a past deadline signals
    /// immediately. u64::MAX is invalid. Rearming replaces the existing deadline.
    ARM = 16 => WRITE (deadline);
    /// Disarms the timer without clearing Event bits already delivered.
    CANCEL = 17 => WRITE (); });
methods!(kernel_log {
    /// Copies a complete log record from sequence and returns [bytes_written, 0].
    /// BufferTooSmall reports the next record size. Reading does not consume log records.
    READ = 16 => READ (sequence, output_address, capacity); });
methods!(entropy {
    /// Fills the output buffer from the seeded generator.
    /// Unseeded state returns InvalidState. Earlier bytes may be written if
    /// generation fails after a chunk has completed.
    GENERATE = 16 => READ (output_address, length);
});
methods!(device_set {
    NEXT = 16 => READ (after_id);
    DESCRIBE = 17 => READ (device_id, output_address, capacity);
    READ = 18 => READ (device_id, field, index, output_address, capacity);
    CLAIM_GROUP = 19 => RESOURCE_SPLIT (group_id, handle_rights);
});
methods!(device {
    /// Maps an authorized MMIO resource into the calling Task and returns [start, length].
    /// The mapping retains the grant and is invalidated when the grant is revoked.
    MAP = 16 => MAP (device_id, resource_index);
    /// Releases the authorized mapping identified by its start address.
    UNMAP = 17 => MAP (start);
    /// Binds an enumerated interrupt to a nonzero Event mask. The Event additionally
    /// requires SIGNAL. Busy reports an already bound source. Failure leaves no enabled route.
    BIND = 18 => DEVICE_CONTROL (device_id, interrupt_index, event_handle, signal_mask);
    /// Masks and removes the authorized interrupt binding and releases its Event.
    UNBIND = 19 => DEVICE_CONTROL (device_id, interrupt_index);
    /// Re-enables a pending authorized interrupt after the driver services it.
    ACK = 20 => IRQ_ACK (device_id, interrupt_index);
    /// Revokes device mappings, interrupt routes, and associated DMA authority.
    REVOKE = 21 => REVOKE ();
    /// Returns a quiesced DMA domain for an authorized device. Failure publishes no handle.
    CREATE_DMA = 22 => DEVICE_CONTROL (device_id, handle_rights);
});
methods!(dma {
    /// Pins a contiguous allocation range for the given direction and returns [bus, length].
    /// The allocation additionally requires DMA_MAP and direction-dependent access rights.
    MAP = 17 => DMA_MAP (allocation, frame_offset, frame_count, direction);
    /// Releases the mapping identified by its bus address. The domain must be Quiesced.
    UNMAP = 18 => DMA_MAP (bus_address);
    /// Moves a Quiesced domain to Active after its mappings are installed.
    RESUME = 19 => DEVICE_CONTROL ();
    /// Records the trusted driver declaration that DMA has stopped and in-flight
    /// requests are complete. The driver must satisfy this condition before calling.
    QUIESCE = 20 => DEVICE_CONTROL ();
    /// Revokes a Quiesced domain and releases its mappings. Active domains cannot
    /// be revoked without stopping DMA. Dropping an active domain stops the system.
    REVOKE = 21 => REVOKE ();
});

/// Queries `[topic, query_address, query_length, output_address, output_capacity]`.
///
/// Each topic declares its rights and fixed query and output forms. Topics with
/// register results require zero output address and capacity. Buffer results
/// return `[bytes_written, 0]`; insufficient capacity returns `BufferTooSmall`
/// with `[required_bytes, 0]` without changing the output or the object.
/// Unknown topics return `Unsupported`; malformed queries return
/// `InvalidArgument`, and missing topic rights return `Denied`.
/// Queries do not block or change the object's lifecycle.
pub const INFO: u64 = 0;
/// Closes the handle without requiring rights and returns zero values.
/// The last owned capability releases its object. CURRENT_THREAD cannot be closed.
pub const CLOSE: u64 = 1;
/// Creates a handle with the requested rights, requiring DUPLICATE.
/// Rights cannot exceed the source handle's rights. Failure publishes no handle.
/// Success returns `[new_handle, 0]` and preserves the source handle.
pub const DUPLICATE: u64 = 2;

pub const fn required_rights(kind: ObjectType, method: u64, topic: u64) -> Option<Rights> {
    match method {
        INFO => crate::info::required_rights(kind, topic),
        CLOSE => Some(Rights::NONE),
        DUPLICATE => Some(Rights::DUPLICATE),
        _ => match kind {
            ObjectType::Task => task::required(method),
            ObjectType::Thread => thread::required(method),
            ObjectType::FrameAllocation => memory::required(method),
            ObjectType::Channel => channel::required(method),
            ObjectType::Reply => reply::required(method),
            ObjectType::Call => call::required(method),
            ObjectType::Event => event::required(method),
            ObjectType::Timer => timer::required(method),
            ObjectType::DeviceSet => device_set::required(method),
            ObjectType::Device => device::required(method),
            ObjectType::KernelLog => kernel_log::required(method),
            ObjectType::DmaDomain => dma::required(method),
            ObjectType::TrapPort => trap_port::required(method),
            ObjectType::TrapContinuation => trap_continuation::required(method),
            ObjectType::Entropy => entropy::required(method),
            ObjectType::Clock | ObjectType::SystemInfo => None,
        },
    }
}

/// Number of used parameter registers for a defined method.
pub const fn argument_count(kind: ObjectType, method: u64) -> Option<usize> {
    match method {
        INFO => Some(5),
        CLOSE => Some(0),
        DUPLICATE => Some(1),
        _ => match kind {
            ObjectType::Task => task::argument_count(method),
            ObjectType::Thread => thread::argument_count(method),
            ObjectType::FrameAllocation => memory::argument_count(method),
            ObjectType::Channel => channel::argument_count(method),
            ObjectType::Call => call::argument_count(method),
            ObjectType::Reply => reply::argument_count(method),
            ObjectType::Event => event::argument_count(method),
            ObjectType::Timer => timer::argument_count(method),
            ObjectType::KernelLog => kernel_log::argument_count(method),
            ObjectType::TrapPort => trap_port::argument_count(method),
            ObjectType::TrapContinuation => trap_continuation::argument_count(method),
            ObjectType::DeviceSet => device_set::argument_count(method),
            ObjectType::Device => device::argument_count(method),
            ObjectType::DmaDomain => dma::argument_count(method),
            ObjectType::Entropy => entropy::argument_count(method),
            ObjectType::Clock | ObjectType::SystemInfo => None,
        },
    }
}
