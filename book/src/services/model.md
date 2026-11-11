# Service Collaboration Model

When logger outputs a log, it writes the text to the UART session delivered by devmgr; when persistence is enabled, it also appends the record through fs's file session.
These calls connect three separate Tasks, each service keeps its own state, and they collaborate through channels.
init is responsible for establishing these connections, allocating boot resources to the corresponding services, and holding their control channels and Task handles.

## Startup and Dependencies

| Service | Main resources obtained at startup | Capabilities provided to other programs |
|---|---|---|
| fs | boot archive, management channel | file and directory sessions, path operations, mount management |
| devmgr | DeviceSet, management channel | device discovery and UART, block device, RTC, random number sessions |
| logger | UART output session, KernelLog, optional log file | user log submission, reading, and flushing |
| linux | file client, optional RTC and random number sessions | Linux program startup, system calls, and exit status |

The dependency relationships determine the startup order.
init first reads fs from the archive, then gives the archive to fs to build the file tree.
After that, loading other services uses the file interface.
Once devmgr starts, it provides block devices and serial ports, so fs can mount disks and logger can obtain an output session and log file.
Finally, linux receives application startup requests with file, time, and random number connections.

## Management Channels and Session Channels

The management interface is responsible for establishing clients, mounting file systems, discovering devices, or starting programs.
The session interface is responsible for operations that continue after a single open, such as reading files or UART data.

Opening a session usually involves the following steps:

1. The client establishes a pair of Channels.
2. The server endpoint is transferred to the service along with the open request.
3. The service validates the parameters and permissions and establishes session state.
4. After a successful open, the client accesses the session through the retained endpoint.

For example, an opened file is associated with a server node by a dedicated channel.
The client holds a File, and the server stores a FileSession; after the client closes the endpoint, fs reclaims that session's node references and pending state.

## Message Conventions

Service protocols use message tags to indicate operations or results and use data words to convey fields such as lengths, offsets, and request identities.
Paths, file contents, and larger buffers are handed over through FrameAllocation.
The receiver checks the message shape, handle count, object types, permissions, and data ranges.

Conventions and encodings are maintained within the service crates, such as `fs/src/protocol.rs` and `logger/src/protocol.rs`.
Client types encapsulate these fields, letting callers express requests through Rust methods.
These messages are handed over through the native ABI's channels, page frames, and wait mechanisms.

## Blocking and Asynchronous Completion

CALL leaves the initiating thread waiting on a single request.
When a service handles multiple sources at the same time, it uses START_CALL to save pending objects and then waits together with control channels, device events, and exit notifications.

For example, when a program waits for file or terminal input, the Linux service saves the continuation and the pending operation and continues handling Traps from other programs.
Device services reply to the original request after I/O completes, while init observes both application run results and service exits.

The completion point of a request is defined by the corresponding service.
For example, a successful log send means the message has been queued, an explicit write returning a sequence number means the record has been collected into memory, and flush continues to complete serial and file synchronization.
These completion points correspond to the different client interfaces in [Logging Service](logger.md).

## Lifetime

init's Service object holds the service Task and terminates the corresponding service on destruction.
A service observes whether the launcher still holds the control connection by the closure of the management channel.
devmgr closes the worker thread control ends in its own exit path and waits for worker threads to be reclaimed.
Device and file sessions each hold the backing resources they require.

Exit of a core service is observed by the init supervisor, which returns an ending state.
Exit and restart of configured applications are managed by an independent application policy, see [init](init.md).
