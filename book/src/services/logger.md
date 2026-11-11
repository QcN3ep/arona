# Logging Service

After a user program calls `log!`, the message first enters the channel; after logger receives it, it assigns a sequence number and timestamp, stores it in an in-memory record, and then outputs it to the UART.
When init delivers a log file, the same user record is also appended to the file.
logger also provides read and output interfaces for the KernelLog, letting the management side process kernel logs with a unified record format.

## Service Resources and Record Sources

The startup message delivers the UART output session, KernelLog, user submission channel, and optional file session.
The management channel is reserved for init, and user clients write logs and request flush through the submission channel.

Kernel records are kept in the kernel's 64 KiB ring buffer, and user records are kept in logger's own storage.
The two kinds of records use the same record encoding and can be read and output through the logging service.
Their sequence numbers and cursors belong to their sources.

Each record contains a sequence number, an optional monotonic timestamp, a level, a target, a message, and a truncation flag.
The timestamp of a user record is read when the service receives it.
Serial text contains the time, level, source, and target; early kernel records display an early-time marker.

## Three Completion Points

| Interface | Result expressed by success |
|---|---|
| `try_write`, `try_write_fmt` | the request has entered the channel queue |
| `write`, `write_fmt` | the log has entered the service's in-memory record, returning its sequence number |
| `flush` | completes UART output of previous logs and syncs the enabled persistence file |

These three interfaces correspond to different stages through which the same record passes.
The `log` facade attempts to add the message to the queue through a polling send, and the service's reception and processing happen in a later scheduling.
Queue-full, communication, or encoding errors in a `log!` call are counted in `logger::failures()`, and the caller continues execution.
An explicit `write` waits for the service to reply with the record sequence number, and flush continues to wait for the UART and persistence file sync results.
The facade sets a 10-second deadline for flush requests.

## In-Memory Retention and Reading

The service uses a `VecDeque` to store encoded user records, with a retention capacity of 1 MiB.
When a new record makes the total exceed the capacity, whole records are evicted from the earliest position.
Sequence numbers keep increasing, and readers maintain their own cursors.

Control's `read` finds the first user record whose sequence number is greater than or equal to the cursor, and `read_kernel` reads kernel records from the KernelLog.
The result is output through a page frame provided by the caller, and decoding borrows the caller's slice.
`print_kernel` reads and outputs one kernel record and returns its sequence number, by which the caller advances the cursor.

## Persistence File

init hands the File session of `/var/log/system.log` to logger.
The file stores continuously encoded records, and the service maintains the current write offset.
After a user submission is collected into memory, the persistence data and UART output are appended along the processing path.

At startup, the service reads the file sequentially and checks record lengths, encoding, and increasing sequence numbers.
Complete records are restored to the in-memory retention queue, and the latest sequence number is used for subsequent writes.
If a write ends midway, an incomplete record may remain at the end of the file.
The recovery process stops at the last valid record, truncates and syncs the tail, and then continues writing from the recovered latest sequence number.

The file write loop handles short writes, and flush calls the file service's fsync.
After a persistence error, the state is marked Failed, and subsequent flushes return `Unavailable`.
UART output also records its availability state independently, and after an output failure it stops writing to that session.
In-memory records are still maintained along the submission processing path.

## Service Event Loop

The loop polls the management channel and the user submission channel.
When there are user messages, it processes records; when there is no progress, it adds the readable and closed states of both channels to a multi-object wait.
Closure of the control side ends the service, and closure of the user submission side removes that source from the wait list.

Protocol parsing checks the request tag, data words, handles, and output capacity.
Ordinary submissions may have no Reply; explicit writes, reads, and flushes use request replies to report results.
