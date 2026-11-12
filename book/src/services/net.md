# Network Stack

The `arona-lwip` crate in `components/net/lwip` runs lwIP inside an Arona user-space program.
It currently provides IPv4 TCP and UDP sockets through lwIP's loopback interface.

## Startup and Sockets

Startup begins with 32 unpredictable bytes obtained through a devmgr RNG session.
The caller passes these bytes to `Stack::initialize`, which waits until the protocol thread is ready.
The initialized stack lives until the program exits.
Initialization can be attempted only once, so a failed startup requires restarting the program.

After initialization, `Stack::socket` creates an owned TCP or UDP socket in nonblocking mode.
The current socket table has 32 entries.
Each socket uses an `Endpoint` containing four IPv4 address bytes and a host-order port.
Binding port zero requests an ephemeral local port.

A TCP server binds a local endpoint and starts listening before accepting connections.
On the client side, a nonblocking connect may return `InProgress` while the connection is being established.
The caller can read and clear pending connection errors with `take_error`.

Once a socket can transfer data, send and receive return the transferred byte count.
An operation that cannot proceed immediately returns `WouldBlock`.
For UDP, `receive_from` also returns the sender's address and preserves empty datagrams.
For TCP, a zero-byte receive into a nonempty buffer indicates EOF.
A failed send does not consume the input slice.

Each socket owns its C descriptor and requires exclusive mutable access for operations.
This prevents simultaneous C API calls on the same socket.
A socket can move between threads, and different sockets can progress concurrently.

Nonblocking I/O can still wait for the local TCP/IP thread to process a request.
Shutdown and dropping a socket can wait for protocol cleanup.
These calls therefore need a thread that can sleep without holding up unrelated work.
A failure to close an owned socket terminates the Task.

## OS and FFI Boundary

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

The following commands build the native loopback program and its architecture test image with the pinned toolchain:

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
After the Task terminates, the kernel harness checks resource cleanup.

GitHub CI compiles, lints, and links debug and release programs for both targets.
The functional scenarios run locally in QEMU.
