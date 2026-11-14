# Code Organization

The repository divides Cargo crates by their responsibilities in the system.
Reading from the bottom up, `abi` defines handover types, `kernel` provides resource mechanisms, and `arch` performs hardware operations and produces the kernel ELF; user-space runtime and services build the runtime environment and system functionality on these mechanisms.

## Workspace Members

| Directory | Cargo Package Name | Main Contents |
|---|---|---|
| `abi/` | `arona-abi` | Object types, permissions, methods, and cross-boundary exchange formats |
| `kernel/` | `arona-kernel` | Privileged kernel mechanisms and hardware interface definitions |
| `arch/` | `arona-arch` | Hardware interface implementations and the kernel executable |
| `testkit/` | `arona-testkit` | Shared functional and performance case interfaces and kernel scenarios |
| `testkit/macros/` | `arona-testkit-macros` | `ktest` and `kperf` case registration |
| `components/runtime/` | `arona-runtime` | Native Rust runtime environment and kernel call wrappers |
| `components/init/` | `arona-init` | Starting base services and supervising configured applications |
| `components/devmgr/` | `arona-devmgr` | Device management, drivers, and device sessions |
| `components/logger/` | `arona-logger` | User logging service and client |
| `components/fs/` | `arona-fs` | File interfaces, clients, service, and file system implementations |
| `components/linux/` | `arona-linux` | Linux program execution and ABI compatibility service |
| `components/net/` | `arona-net` | Native socket client and network service |
| `components/net/lwip/` | `arona-lwip` | Maintained lwIP C stack and its Rust adapter |
| `xtask/` | `xtask` | Host build, packaging, and run tooling |

Several crates use short Rust library names.
For example, the Cargo package `arona-kernel` is referenced as `kernel` in code, and `arona-runtime` as `runtime`.
The `-p` flag in Cargo commands takes the package name.

## Core Dependencies

`abi` defines conventions and types used jointly by the kernel and programs; `kernel` and runtime depend on it separately.

`arch` depends on `kernel`, implements the hardware interfaces the latter defines, and links entry code and kernel mechanisms into the `arch` ELF.
Architecture-specific RISC-V and LoongArch instructions, registers, and page table formats are placed under the corresponding architecture directories.
Hardware wrappers shared between architectures are placed in common modules under `arch/src/`.

System services depend on the runtime to use kernel capabilities.
Service crates also provide client libraries for other services to link against.
For example, `components/fs/` contains both the File client and the fs binary: init links the former to encode read requests, and the latter handles requests in its own Task.

## Kernel Directories

| Module | Contents |
|---|---|
| `kernel/src/boot/` | Boot information, kernel initialization, the first program, and boot resource delivery |
| `kernel/src/hardware/` | CPU, MMU, execution context, Trap, clock, and device access interfaces |
| `kernel/src/memory/` | Page frames, allocator, mapping, address space, direct mapping, and kernel heap |
| `kernel/src/task/` | Task lifecycle, objects, and handle table |
| `kernel/src/thread/` | Threads, scheduling, waiting, deadlines, and user-space execution |
| `kernel/src/ipc/` | Channels, calls, replies, events, timers, and multi-object waiting |
| `kernel/src/device/` | Device resource registration, authorization, PCI and DMA lifecycle |
| `kernel/src/syscall/` | Permission checking and dispatch of native object methods |
| `kernel/src/logger/` | Kernel logging, Sink, and record buffers |
| `kernel/src/trap.rs` and `trap/` | Trap dispatch and user-space exception delivery |

To trace a cross-layer feature, follow the interfaces.
For example, user mapping enters `memory` from `syscall`, page table management calls `hardware::mmu`, and the page table entry encoding for the corresponding architecture lives in `arch`.

## Implementation Constraints and Test Locations

The implementation follows these rules:

| Scope | `unsafe` | Conditional Compilation |
|---|---|---|
| kernel implementation, testkit, abi | Forbidden | Forbidden |
| `kernel/tests/` | Allowed | Forbidden |
| `components/runtime/` | Allowed | Allowed |
| `components/net/lwip/` | Allowed for the C adapter | Allowed for native platform selection |
| Other `components/` | Forbidden | Forbidden |
| `arch/` | Allowed | Allowed |

See the [Contribution Guidelines](guidelines.md) for safety preconditions and comment rules in Rust implementations.

The network service binary is the only production consumer of the lwIP adapter.
Its client library defines native socket types and channel requests without a C dependency.

The lwIP child crate owns the C build, OS callbacks, allocation, and FFI lifetime handling.
Its safe Rust interface owns socket descriptors and accepts typed endpoints and slices.
This keeps C pointers and layouts inside the child crate.
The adapter uses the runtime's generic facilities without making the runtime depend on lwIP.

Integration tests that can run on the host go in the `tests/` directory of the corresponding crate.
Tests that require real page tables, exception entry points, thread switching, or devices are carried by architecture test images and component programs; general kernel scenarios go in `testkit`.
See [Testing and Debugging](../tools/testing.md) for the test layout.

## Build Files and Documentation

The root `Cargo.toml` maintains workspace members, shared versions, lints, and build configuration.
`Cargo.lock` pins the workspace dependency resolution, and `.cargo/config.toml` defines the `cargo xtask` alias.
Architecture linker scripts live in `arch/src/<arch>/link.ld`, and the common linker script for native Rust programs lives in `components/runtime/link.ld`.

This book is in `book/`, and its table of contents is defined by `book/src/SUMMARY.md`.
The root `CONTRIBUTING.md` is where the contribution guidelines are maintained; the handbook includes it.
