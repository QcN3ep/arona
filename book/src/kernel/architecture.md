# Kernel Architecture Overview

In Arona, a single file read passes through several cooperating parts: the compatibility service interprets the Linux call, the file service looks up the file, the device management service operates the disk, and the kernel passes messages between them and schedules threads.
Following this request shows how the system divides responsibilities.

`arch` and `kernel` together form the privileged kernel.
`arch` encapsulates instruction and register operations, while `kernel` maintains resources such as memory, threads, and objects.
The runtime and system services run in user mode; each service has its own address space and cooperates through the IPC provided by the kernel.

## Composition and Dependencies

| Directory | Responsibility |
|---|---|
| `arch/` | Firmware handoff, boot image, hardware interface implementations, privileged instructions, and raw memory access |
| `kernel/` | Memory management, Task, threads, IPC, capabilities, and device resource management |
| `abi/` | Object types, methods, permissions, encodings, and return value conventions shared by the kernel and native programs |
| `components/runtime/` | Native program startup, the Rust runtime environment, and Rust interfaces to kernel capabilities |
| Services in `components/` | System functions such as device drivers, file systems, logging, and Linux compatibility |

Build dependencies point from `arch` to `kernel`.
Hardware interfaces are defined in `kernel::hardware` and implemented by the architecture layer.
Capabilities such as the MMU, execution contexts, traps, and clock and interrupt control are all made available to the kernel through this layer.

The runtime depends on `abi` and translates Rust interfaces into object calls that enter the kernel.
A service's client library and service program can share protocol types; actual requests enter the Task hosting the service through a channel.

## Framekernel Boundaries

Arona's framekernel design is inspired by [Asterinas](https://github.com/asterinas/asterinas).

The `kernel` implementation follows the project constraints of not using `unsafe` or conditional compilation.
Implementations that need raw pointers, inline assembly, or hardware register access live in `arch`.
For example, the kernel maintains page frame allocation policy and address space mapping records, while page table entry access and TLB operations are performed through hardware interfaces.

For example, when establishing a user mapping, the kernel allocates page frames, selects the virtual range, and records the mapping references.
The MMU implementation then writes these decisions into the page tables of the current architecture and completes TLB and instruction synchronization.
The memory module owns the state of page frames and mappings, while the architecture implementation performs the corresponding machine operations.

## How Hardware Capabilities Are Bound

Hardware capabilities are bound to kernel slots through static objects.
For example, `kernel::hardware::mmu()` returns the bound MMU implementation.
Each slot stores its implementation in `spin::Once`; binding twice returns `AlreadyBound`, and using a slot before binding completes panics.

The architecture layer uses `declare_hardware!` to declare static implementations and places the binding functions in the `arona_hardware` linker section.
Boot code calls `arch::hardware::bind()`, which walks the section and runs the binding functions.
Individual hardware capabilities are declared in their own modules, and the boot entry point performs all bindings.

Once binding is complete, the kernel initializes each capability in boot order.
For example, the interrupt controller obtains its topology from `HardwareInfo`, then establishes MMIO mappings through the memory module and configures the actual routing.

## Kernel and System Services

The kernel provides mechanisms such as address spaces, threads, objects, handles, message passing, and device resource authorization.
System services use these mechanisms to implement their own functionality:

| Service | Current responsibility |
|---|---|
| init | Reads the boot configuration, loads and starts the base services, and starts and supervises the configured applications |
| devmgr | Discovers devices, creates driver instances, and manages device sessions and I/O |
| logger | Collects user logs, reads kernel logs, outputs them, and persists them when init provides a file |
| fs | Provides file access and path and mount management, and accesses the boot archive and ext4 storage |
| linux | Loads Linux ELF, maintains Linux process semantics, and handles Linux system calls |

Each system service runs in its own Task with an independent user address space.
Handles refer to operable kernel objects and carry the permissions they allow.
Which devices, memory, or channels a service can access depends on the handles it receives.

Device resource management in the kernel and user-mode drivers take on authorization and operation responsibilities respectively.
The former controls the granting and reclamation of MMIO, DMA, PCI, and interrupt resources; the latter handles device protocols and I/O requests.
After a driver obtains a Device handle, it uses these authorizations to access registers, establish DMA mappings, and receive interrupts.

## Native Calls and Linux Compatibility

Native programs call object methods through handles.
Method numbers are interpreted within an object type, and the kernel checks the permissions required by the method before dispatching.
The runtime wraps register conventions and encodings in Rust interfaces.

Linux programs use the Linux ABI.
The Linux compatibility service receives system calls and exceptions from supervised programs through the kernel's trap interface, maintains state such as processes, file descriptors, and signals, and then completes operations by calling native kernel mechanisms and other services.
Interpretation of Linux system call numbers lives in the compatibility service.

Take `read` on a disk file as an example: the compatibility service uses the fd to find the file session and hands the read request to fs.
fs translates the file offset into disk blocks and submits the I/O through a block device session in devmgr.
When the device completes, the result returns layer by layer; the compatibility service writes the byte count into the Linux return register and then resumes the application thread.
Linux fds and file offsets are maintained by the compatibility service, file system nodes by fs, and physical page frames and threads by the kernel.

## From Boot to Running Services

1. The architecture entry point establishes the mappings and stack needed for boot and reads the firmware handoff information.
2. The architecture layer parses the DTB, prepares the hardware description, the init image, and the initramfs, and binds hardware capabilities.
3. `boot_kernel` initializes memory, traps, the interrupt controller, timers, and the scheduler.
4. The kernel creates init's Task, address space, initial stack, and thread, and passes the boot resources.
5. init starts the file, device management, logging, and Linux compatibility services, then reads the application configuration.

The boot process also hands off gradually along these responsibilities: after the kernel creates the first Task, init takes over loading native services, and the Linux compatibility service takes over the execution environment for Linux applications.
