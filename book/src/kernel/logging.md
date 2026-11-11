# Kernel Logging

The kernel's first log records are produced before the memory allocator and timers are ready, so they are output through the early UART.
They also enter a static ring buffer, so programs can still read these records through KernelLog after the serial port is handed over to user mode.
`Logger` connects record storage with Sink output, and all kernel modules use the Rust `log` facade uniformly.

## Log Production and Storage

Logging uses the standard macros such as `error!`, `warn!`, `info!`, `debug!`, and `trace!`.
The Logger filters records according to the current `LevelFilter`, then formats the target and the message text.
Each record contains an incrementing sequence number, an optional timestamp, a level, a truncation flag, a target, and the body.

The buffer is a statically allocated 64 KiB ring buffer that stores and evicts whole records.
When space runs short, the oldest complete records are dropped to make room for new ones.
Text is truncated at UTF-8 character boundaries, and the truncation state is recorded.

Logging can occur in interrupt context or while other logging is in progress, so the write path tries to take the lock; if the lock is held, this record is not stored and execution continues.
This leaves a gap in the sequence numbers, which lets readers detect that content is missing between two records.

## Sink Registration and Replacement

The Logger registers at most 8 Sinks.
`add_sink` returns a `SinkHandle`, and the handle records a slot and a generation.
Once the slot is reused, the old handle returns `Detached`.

| Operation | Behavior |
|---|---|
| `replace` | Flush the original Sink first, then replace it on success |
| `remove` | Flush first, then remove on success |
| `discard` | Remove directly |

Dropping a `SinkHandle` keeps the registration in place.
Dispatch errors from writes and flushes are counted per Sink in `failures()`.
Sinks run in the control flow that produces the log, and the early serial Sink outputs by polling registers directly.

## From Early Output to the User Service

The architecture entry point initializes the UART and registers the early console Sink.
During this stage, logs go to both the serial port and the buffer.
After timer initialization, the kernel registers a timestamp callback, and later records carry a monotonic nanosecond timestamp.

Once init is established, the boot path passes the KernelLog handle and releases the early console.
devmgr then manages the UART, and the logger service provides interfaces for reading and outputting kernel records and handles user logs.
The kernel buffer persists across the handoff, and the early records that remain use the same read interface as later records.

## Reading by Cursor

KernelLog's `READ` takes a cursor and an output buffer.
A cursor of 0 starts from the earliest record currently retained; any other value finds the first record whose sequence number is greater than or equal to the cursor.
A successful read returns one encoded record, and the reader uses that record's sequence number plus one as the next cursor.

When no record matches, it returns 0.
If the output is too small it returns `BufferTooSmall`, and if the buffer is busy it returns `Busy`.
For example, after reading sequence number 42, the next read continues with cursor 43.
Each reader keeps its own cursor independently, and reads leave the records in the buffer for other readers.

For user logs and persistence, see [Logging Service](../services/logger.md).
