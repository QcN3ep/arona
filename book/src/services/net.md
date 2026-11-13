# Network Stack

The network service runs as an independent Task and provides native IPv4 TCP and UDP sockets.
The `arona-net` client communicates with it through channels and does not link lwIP.
The service links the maintained lwIP adapter in `components/net/lwip`.
This version provides loopback networking without requiring a network interface.

## Native Socket Requests

Init starts `/bin/net` after the device and log services.
It supplies a separate devmgr RNG session, and net obtains its startup seed before reporting readiness.
The default QEMU command includes a modern VirtIO RNG even when `--net` is absent.
A missing or failed entropy source causes startup to fail.
Init supervises net with the other base services and stops it after Linux during ordered shutdown.

`Control::connect` returns a native client capability.
A client creates a socket with `Client::socket` or begins creation with `start_socket`.
Each socket has a dedicated channel and a server-owned lwIP socket.
The native interface exposes no C descriptor, register mapping, or DMA authority.

Use `Socket::start` to submit one `Request`, then call `complete` or `try_complete` to collect its result.
`perform` combines submission and completion.
These requests cover bind, listen, accept, connect, send, receive, endpoint queries, options, shutdown, and close.
For UDP, `Connect` sets or replaces the default peer and `Disconnect` removes it while keeping the socket open.
`Send` without a default UDP peer returns `NotConnected`.
The supported options are address reuse and TCP_NODELAY.
`Socket::kind` reports the socket type, and `TakeError` reads and clears its pending connection error.

The service performs a nonblocking attempt for each socket operation.
It returns `WouldBlock` when that attempt cannot make progress and `InProgress` when a connection is still being established.
A TCP attempt transfers at most 64 KiB and reports its actual byte count.
A UDP send contains one datagram with at most 65,507 bytes.
`ReceiveDatagram` returns the whole datagram and its source endpoint, including empty datagrams.
The client receives payload storage with READ rights through the `Data` result.

Each socket retains one outstanding request with a monotonically increasing identity.
The service snapshots send inputs and reserves receive storage before consuming data.
It keeps the result, received bytes, or accepted socket until the client acknowledges delivery.
Repeated result retrieval returns the same completion and does not repeat I/O.
The client acknowledges a received result before returning it to the caller.

A timeout preserves the request identity and any result already received by the client.
Call `complete` again to resume it.
Starting another request while one remains outstanding returns `Busy`.
`forget_reply` discards a reply wait while keeping the operation recoverable.
Canceling a runtime Call also affects only the reply wait.

`Socket::cancel` reports whether it canceled an unexecuted request, found committed work, or found shutdown or close still running.
The caller then collects the retained result with `complete`.
Dropping the socket closes its session, abandons any unacknowledged result, and queues cleanup.
A service failure closes its session channels, so pending calls and readiness waiters observe failure.

## Native Readiness

`Socket::readiness` returns a non-consuming snapshot.
Use `subscribe` to obtain an independent Event, a registration channel, and the initial snapshot.
Dispatchers can use `start_subscribe` and `Subscription::start_snapshot` to collect these replies asynchronously.
Each registration lasts until its channel closes.
Callbacks update adapter state, and the service forwards changes to client Events outside those callbacks.

Take a snapshot before attempting I/O.
If the attempt returns `WouldBlock`, pass that snapshot to `Subscription::wait` and retry after waking.
Notifications can coalesce, and a stale notification is harmless because the next snapshot reflects current state.
Waits observe both the Event and registration-channel closure, so a stopped service cannot leave an Event-only wait asleep.
The registration does not keep its socket session alive.

## lwIP Adapter

The following interfaces are internal to the service and its adapter scenarios.

### Startup and Sockets

Startup begins with 32 unpredictable bytes obtained through a devmgr RNG session.
The caller passes these bytes to `Stack::initialize`, which starts the protocol and teardown threads before returning.
The initialized stack lives until the program exits.
Initialization can be attempted only once, so a failed startup requires restarting the program.

After initialization, `Stack::socket` creates an owned TCP or UDP socket in nonblocking mode.
Socket entries are allocated as needed and are limited by available memory and descriptor numbers.
Each socket uses an `Endpoint` containing four IPv4 address bytes and a host-order port.
Binding port zero requests an ephemeral local port.

A TCP server binds a local endpoint and starts listening before accepting connections.
On the client side, a nonblocking connect may return `InProgress` while the connection is being established.
The caller can read and clear pending connection errors with `take_error`.

Once a socket can transfer data, send and receive return the transferred byte count.
An operation that cannot proceed immediately returns `WouldBlock`.
For UDP, `receive_from` also returns the sender's address and preserves empty datagrams.
`disconnect` removes a UDP socket's default peer.
For TCP, a zero-byte receive into a nonempty buffer indicates EOF.
A failed send does not consume the input slice.

Each socket owns its C descriptor and requires exclusive mutable access for operations.
This prevents simultaneous C API calls on the same socket.
A socket can move between threads, and different sockets can progress concurrently.

Nonblocking I/O can still wait for the local TCP/IP thread to process a request.
Shutdown and close transfer socket ownership to a shared teardown worker, which keeps protocol cleanup off the caller's thread.

### Readiness and Waiting

`Socket::readiness` returns a snapshot without consuming data or clearing `SO_ERROR`.
The snapshot includes read and write readiness, a connection error indication, shutdown state, and a change sequence.
Read readiness can indicate a pending accept, incoming data, or EOF.
Write readiness after a connection attempt requires checking `take_error` before transferring data.
The error indication remains set after a connection failure, even when `take_error` has cleared the pending error value.

Call `subscribe` before checking readiness or attempting an operation that may need to wait.
Each subscription has its own Event, so independent waiters receive notifications independently.
Take a snapshot before attempting I/O and pass it to `Subscription::wait` if the attempt returns `WouldBlock`.
The wait compares change sequences before sleeping and checks them again after waking.
This covers changes during registration and between the snapshot, I/O attempt, and wait.
An older Event bit cannot erase a newer sequence change.

Notifications coalesce, and readiness is a hint to retry the operation.
The operation can still return `WouldBlock`, so repeat the same sequence when necessary.
A subscription also exposes its Event for use with `runtime::ipc::wait_many`.
Use one waiting consumer per subscription and recheck its snapshot after the Event wakes.

Snapshots remain tied to the original socket after close.
They report a terminal closed state and never start observing a replacement that reuses the C descriptor.
Subscriptions retain notification storage until they are dropped, but they do not keep the socket open.

### Shutdown and Close

`Socket::shutdown` consumes the socket and returns a `PendingShutdown`.
The worker holds exclusive C API access until shutdown finishes.
`try_complete` or a successful `wait` returns both the socket and the shutdown result.
The caller gets ownership back even if shutdown fails.
After a successful write shutdown, it can continue receiving through the returned socket.

Dropping a pending shutdown requests close after the worker finishes shutdown.
Calling `PendingShutdown::close` follows the same order and returns a `PendingClose` that can be waited on.
This keeps close and shutdown serialized without enabling concurrent C calls on the same socket.

`Socket::close` consumes the socket and returns a `PendingClose` immediately after queuing the work.
Dropping a socket queues the same operation and leaves cleanup to the worker.
Wait for explicit close completion before relying on the released port being available for another bind.
A successful close wait means the C close call has returned, while TCP may still retain protocol state for FIN delivery or TIME_WAIT.
A failure to close an owned socket terminates the Task.

Pending operations expose completion Events for `wait_many` as well as polling and waiting methods.
Timeouts leave the operation pending and can be followed by another wait.
One worker processes the queue, so a slow teardown can delay later teardown operations while ordinary I/O on other sockets continues.
Queue links and completion Events are reserved when creating or accepting a socket, so submitting shutdown or close does not allocate storage.

### OS and FFI Boundary

One TCP/IP thread handles protocol state, timers, callbacks, and loopback input.
Other threads submit work through lwIP mailboxes and wait for completion through semaphores.
These facilities use `runtime::sync`, which keeps queued messages and semaphore counts independently of Event notifications.
The protocol thread yields between messages so other Arona threads can run during continuous protocol work.

The adapter also provides a recursive protection lock for lwIP's shared bookkeeping.
A thread-local nesting count allows the holder to enter that lock again.
Each thread has its own errno storage for socket errors.
These callbacks run on ordinary Arona threads and do not support hardware interrupt context.

The Rust interface keeps C descriptors, pointers, and layouts inside the child crate.
A small C bridge converts socket addresses and checks the shared endpoint layout against Rust.
This lets other service crates use the stack through safe Rust interfaces.

Socket entries stay at stable addresses in a list ordered by descriptor.
Lookup retains an entry under the same lock used to remove it.
Closing releases the entry after its final user or callback leaves, so its descriptor cannot be reused while the entry is still in use.
Callbacks also check their connection identity before updating an entry.
The C API uses poll for readiness checks because select's fixed descriptor bitmap does not support this storage model.

The socket event hook publishes the readiness flags and sequence together through an atomic value.
It signals subscribers after releasing the C protection lock and retains the socket entry until notification finishes.
The callback does not allocate or send service IPC.
Receive completion also refreshes readiness for partially consumed buffers and records EOF.
Events received before accept publishes the new descriptor remain available in its initial snapshot.

Creating or accepting a socket first reserves its entry storage.
If this reservation fails, the operation returns `NoMemory` without creating protocol resources or consuming a pending connection.
After memory becomes available, the caller can retry the operation.

The C build uses the checked-in `sources.toml`, Clang builtin headers, and explicit target ABI flags.
RISC-V uses `rv64gc/lp64d`.
LoongArch uses `lp64d` with LSX and LASX disabled.
The resulting library does not link host libc or pthreads.

Instead, C allocation goes through the fallible runtime allocator.
The adapter checks size multiplication for overflow and returns null when allocation fails.
Each allocation provides 16-byte alignment and retains the layout needed to free it.

The startup seed initializes ChaCha20 for lwIP's random choices.
TCP initial sequence numbers use a separate SipHash key derived from that generator.
The adapter hashes the connection tuple and adds a four-microsecond clock value, following [RFC 6528](https://www.rfc-editor.org/rfc/rfc6528#section-3).
Protocol callbacks use this local state without further RNG IPC.

Assertions report through the program's log facade and terminate the Task.
Synchronization errors also terminate the Task when lwIP's wait API cannot represent them.
The build disables optional C debug and statistics output.

## Build and Validation

The C build requires Clang and LLVM's archiver.
The default executable names are `clang` and `llvm-ar`.
The `ARONA_CC` and `ARONA_AR` environment variables select different executables when needed.

Build the service and its separate native client scenarios with:

```sh
cargo xtask package-net-test riscv64 --release
```

Use `loongarch64` for LoongArch, or omit `--release` for a debug build.
Boot the archive at `target/<triple>/<profile>/initramfs-net-test.cpio` with the architecture test image, attach the VirtIO RNG, and select `net-service` at `Test suite:`.
The scenarios cover TCP streams between separate Tasks, complete UDP datagrams, request recovery, cancellation, half-close, session failure, and receive allocation failure.
The architecture harness checks resource reclamation after the test Tasks exit.

The following commands build the adapter loopback program and its architecture test image with the pinned toolchain:

```sh
cargo rustc --locked -p arona-lwip --features baremetal --test loopback \
  --target riscv64gc-unknown-none-elf \
  -Zbuild-std=core,alloc,compiler_builtins \
  -Zbuild-std-features=compiler-builtins-mem \
  -- -Cstrip=symbols -Clink-arg=-zmax-page-size=4096
cargo build --locked -p arona-arch --features baremetal,runtime-tests \
  --test interrupt --target riscv64gc-unknown-none-elf --release \
  -Zbuild-std=core,alloc,compiler_builtins \
  -Zbuild-std-features=compiler-builtins-mem
```

For LoongArch, replace the target triple with `loongarch64-unknown-none`.
The first command also accepts `--release` to build an optimized loopback program.
The architecture test image uses release mode to fit the existing boot mapping.

Boot the architecture test image in QEMU with the loopback program supplied through `-initrd`.
Attach an RNG device with `-device virtio-rng-pci,disable-legacy=on` and enter `lwip` at `Test suite:`.
The scenario obtains entropy from that device through a devmgr RNG session, then exchanges TCP and UDP data between Arona threads.
Two threads also create and reuse groups of 64 UDP sockets while an existing TCP connection remains open.
Memory pressure checks cover failed socket creation and acceptance, followed by successful retries with the pending data intact.
Readiness scenarios wait for accept, connect, data, send capacity, EOF, and connection errors.
They also exercise independent subscribers, old notifications, close and descriptor reuse, half-close, and close queued behind shutdown.
Another connection transfers data before waiting for a submitted close to finish.
After the Task terminates, the kernel harness checks resource cleanup.

GitHub CI compiles, lints, and links debug and release programs for both targets.
The functional scenarios run locally in QEMU.
