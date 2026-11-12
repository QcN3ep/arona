# Supported Features

Arona's two runtime platforms share the same set of kernel mechanisms and user-space services.
Architecture code handles machine differences such as boot handoff, page tables, and interrupts, while programs access the system through the same native interface.

## Runtime Platforms

| Platform | Rust target | Firmware handoff | Interrupts and time |
|---|---|---|---|
| QEMU RISC-V 64 `virt` | `riscv64gc-unknown-none-elf` | Default BIOS, hart ID, and DTB | PLIC, time counter, and SBI TIME |
| QEMU LoongArch 64 `virt` | `loongarch64-unknown-none` | `-kernel` system table, DTB, and initrd description | LoongArch external interrupt routing, stable counter, and timer CSRs |

xtask starts an emulated machine with a single processor and 256 MiB of RAM by default.
Both platforms use higher-half kernel addresses, final page tables, user address spaces, and scalar floating-point context save and restore.

## Kernel Mechanisms

| Category | Available capabilities |
|---|---|
| Memory | Page frame allocation, reference keep-alive, address reservation, mapping, permission changes, queries, direct map, kernel heap and stack |
| Execution | Task, user and kernel threads, FIFO ready queue, wait and wakeup, exit reclamation, preemption requests, and deadlines |
| Trap | Unified event classification, external interrupt claiming, user state, TrapPort, and continuations |
| IPC | Messages, handle transfer, synchronous and asynchronous calls, one-shot replies, Event, Timer, and waiting on multiple objects |
| Capabilities | Typed objects, per-method permissions, handle copy and reduction, cross-Task installation |
| Devices | Device directory, resource groups, user MMIO, interrupt events, PCI enumeration, DMA mapping and start/stop |
| Logging | `log` facade, early Sink, ring records, timestamps, and cursor reads |

## Native Rust Programs

The runtime provides an ordinary `main` entry, `core` and `alloc` runtime environments, a global user heap, TLS, closure threads, and JoinHandle.
Programs invoke kernel objects through the runtime and access devices, files, and logs through service clients.

The five system services are all native ELF programs, packaged into the newc archive during the build.
The archive supports being embedded in the kernel ELF and standalone `-initrd` loading.
init starts applications according to `/etc/init.toml` and enforces restart policies.

## Devices and Files

| Category | Implementation |
|---|---|
| UART | NS16550-family MMIO UART, configuration, transmit/receive, and interrupt sessions |
| Block devices | Modern VirtIO MMIO and PCI block, reads and writes, completion confirmation, and flush |
| Ethernet frames | Modern VirtIO PCI net, copied RX/TX, MTU, MAC, and link change waits |
| Random numbers | Modern VirtIO PCI RNG |
| RTC | Goldfish and Loongson LS7A time reads |
| Root file tree | Files, directories, and links in the cpio newc archive |
| Persistent files | 4 KiB block ext4, extents, directory modification, JBD2 transactions and recovery |
| Log files | logger record append, history recovery, tail handling, and fsync |

devmgr matches these devices from the boot hardware description and PCI enumeration results, and creates driver instances after obtaining resource authorization.
For ext4's on-disk layout and transaction handling, see the [File Service](../services/fs.md).

## Linux Program Environment

The Linux compatibility service accepts static ELF64 `ET_EXEC` binaries for the current machine.
It provides call paths for files, directories, pipes, terminals, processes and threads, signals, memory mapping, clocks, and random numbers.
For the exact range of calls and input forms, see the [Linux Compatibility Service](../services/linux.md).

Interactive terminals use UART sessions and the compatibility layer's line discipline.
The shell and external commands are packaged from an external root directory, and the init configuration selects the execution path and terminal.
Integration programs in the repository verify these mechanisms, and BusyBox in the development environment can run through the same load path.
