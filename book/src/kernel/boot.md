# Kernel Boot Process

At boot, the kernel first faces the processor state, physical addresses, and hardware description left behind by the firmware.
The architecture entry point establishes the execution environment and organizes this data into `BootInfo`; `boot_kernel` then initializes the kernel from it and ultimately gets init's first user thread running.

## Architecture Entry Points and the Higher Half

Both architectures go through `_start` and `_start_virtual` to enter `_start_native`.
The first two entry points establish the boot mappings, the stack, and the higher-half execution environment; `_start_native` parses the handoff data in Rust.

| Platform | Firmware input | Handling in `_start_native` |
|---|---|---|
| RISC-V 64 QEMU `virt` | Boot hart ID and DTB address | Reads the DTB and the initramfs range from chosen |
| LoongArch 64 QEMU `virt` | System table address handed over by QEMU | Looks up the DTB and initrd descriptions in the configuration table and reads the processor ID |

RISC-V boot code enters the higher half through a temporary page table.
LoongArch boot code uses DMW to establish a physical access environment, then switches to kernel virtual addresses.
The final address space is established by the common memory initialization code.

After entering the Rust boot logic, the architecture layer clears BSS and initializes the early UART and the log facade.
Early logs go directly to the UART, and from this point records also enter the kernel buffer.

## DTB and Hardware Description

`Dtb` provides a read-only view of the firmware DTB and uses the `fdt` library to read nodes and properties.
`Dtb::dump` writes information such as CPUs, memory, devices, register ranges, interrupt topology, PCI hosts, and consoles into `HardwareInfo`.
Parsing uses fixed-capacity storage and stops the boot on error.

DTB node identifiers and device instance identifiers in the hardware information are used to establish parent-child relationships, interrupt controller references, and resource references.
The memory description handles available and reserved ranges in units of pages, and pages occupied by firmware data are reserved in their entirety.

The architecture layer marks the range containing the DTB as `BootReserved`.
LoongArch also reserves the handoff area at physical addresses `[0, 0x00100000)`.
`prepare_boot_info` verifies the boot processor and reserves the kernel image range.

## Composition of the Boot Information

After hardware parsing completes, the architecture layer hands over the data used for initialization through `BootInfo`:

| Field | Purpose |
|---|---|
| `boot_cpu_id` | Identifies the processor performing initialization |
| `kernel_image` | Physical and virtual layout of the kernel sections and the boot stack range |
| `early_console` | Early console mapping |
| `hardware` | The parsed `HardwareInfo` |
| `init` | The `BootProgram` view of init |
| `initramfs` | The `BootData` of the boot archive |

`BootProgram` provides the common boot code with program segments, the entry point, and data reading.
ELF format parsing remains in the architecture boot code.
init comes from `bin/init` in the initramfs; for choosing between an embedded and a standalone archive, see [System Images](../system/images.md).

## Common Initialization Order

`boot_kernel` initializes in the following order:

1. Set up physical page frame management, the kernel address space, the kernel image, and the RAM direct map.
2. Install the trap entry point on the boot stack.
3. Initialize the interrupt controller and its MMIO mappings from the hardware topology.
4. Initialize the monotonic clock and the timer interrupt, and register the timestamp source with the logging system.
5. Initialize the scheduler.
6. Establish the device resource directory.
7. Create and start init's Task and initial thread.
8. Release the early console and enter the scheduling loop.

The first steps progressively replace the temporary facilities of the boot environment.
The final page tables cover the kernel image and RAM accesses, the trap entry point handles interrupts once they are enabled, and the timer provides time for thread waits and scheduling.
Once memory, execution, and the device resource directory are ready, the boot path can use these common mechanisms to create init.

## How the First Program Runs

The kernel allocates user page frames for init, copies the file contents according to the ELF segments, and zeroes the remainder of each segment.
Mappings use the permissions of the program segments, and the loader checks the executable segment containing the entry point.
The kernel also establishes a user stack with a guard page, writes `ProcessStart`, creates the user execution context, and starts the thread.

`ProcessStart` hands over the current Task handle and the bootstrap channel.
The bootstrap message has tag 0, its data word gives the archive length, and its handles are, in order:

| Position | Object | Purpose |
|---|---|---|
| 0 | `FrameAllocation` | Page frames holding the boot archive |
| 1 | `DeviceSet` | Device enumeration and resource granting |
| 2 | `KernelLog` | Reading kernel logs |

The archive is copied into the frame allocation, and init reads it through the handle it receives.
By this point, the segments, stack, and boot resources used for program execution all have kernel-managed ownership.
init's runtime receives the handoff record and calls `main`; service startup from then on is implemented in user mode.
