# System Images

A system build produces the kernel ELF `arch` and the boot archive `initramfs.cpio`.
The kernel contains privileged-mode code, and the archive stores init, the other system services, and the packaged application files.
The default build embeds the archive into the ELF, so a single `arch` file carries the complete boot contents when distributed; standalone loading mode instead passes the archive through QEMU's `-initrd`.

## Contents of the Boot Archive

The system services in the archive are located in `/bin`:

| Archive path | Program |
|---|---|
| `/bin/init` | The first native program; starts the other services and the configured applications |
| `/bin/devmgr` | Device management service |
| `/bin/logger` | Log service |
| `/bin/fs` | File service |
| `/bin/linux` | Linux compatibility service |

File names in the archive use relative paths such as `bin/init`, and the file service presents them as paths starting from `/`.
The system services are built for the same architecture and the same debug or release profile, and the service ELFs in the archive are stripped of symbols.

The boot archive uses the uncompressed cpio newc format.
Each entry consists of a fixed-length ASCII hexadecimal header, a NUL-terminated file name, and file data, with the file name and data padded to 4-byte boundaries.
A `TRAILER!!!` entry at the end marks the end of the archive.
For a description of the format, see Linux's [initramfs buffer format](https://www.kernel.org/doc/html/latest/driver-api/early-userspace/buffer-format.html).

## Embedded and Standalone Loading

| Mode | Build option | Files needed at boot |
|---|---|---|
| Embedded | `--initramfs embedded`, also the default | `arch` |
| Standalone | `--initramfs external` | `arch` and `initramfs.cpio` |

The embedded build enables the `embedded-initramfs` feature of `arona-arch`.
The build script locates the archive via `ARONA_INITRAMFS`, the architecture layer places it in the `.initramfs` section of the kernel ELF, and it is carried by the read-only image region.
The `initramfs.cpio` produced during packaging is also kept in the build directory.

Standalone loading mode leaves the archive outside the kernel ELF.
`xtask run` passes the archive with QEMU's `-initrd`, and the architecture boot code obtains its physical range from the handoff information.
Both modes ultimately locate `/bin/init` in the same archive and use the same kernel boot interface.

## Adding an External Root Directory

`--rootfs PATH` appends the contents of a host directory to the archive.
For example, arrange BusyBox, the shell links, and the boot configuration as:

```text
rootfs/
  bin/
    busybox
    sh -> busybox
  etc/
    init.toml
```

Add this directory to the RISC-V image:

```sh
cargo xtask run riscv64 --release --rootfs ./rootfs
```

Here `bin/busybox` is a Linux static ELF for RISC-V, `bin/sh` points to it, and `etc/init.toml` tells init how to start the shell.
For the corresponding configuration, see [Boot Configuration and Usage](configuration.md).

The packager supports regular files, directories, and symbolic links, preserves Unix permission bits, and walks directories in file-name order.
Symbolic links store their target text; packaging does not follow links to read the target.
File paths and symbolic link targets use UTF-8.
The paths of the five system services are reserved by the packager, and entries with the same name in external directories are reported as conflicts during packaging.

## From Archive to First Program

The architecture boot code first selects the archive source, then scans the newc entries and locates init.
It checks init's ELF machine type, segment ranges, entry, permissions, and other layout information, and passes it to the kernel as a `BootProgram`.
The boot loader accepts static ELF64 `ET_EXEC` binaries matching the platform and validates that loadable segments have R, RW, or RX permissions.

The kernel sets up init's mappings, stack, and thread from the program segments, and delivers three resources through the bootstrap channel:

| Resource | Purpose |
|---|---|
| The archive's `FrameAllocation` handle and its actual byte count | Lets init read the service programs and hand the archive to the file service |
| `DeviceSet` handle | Authorizes and allocates device resources |
| `KernelLog` handle | Handed to the log service to read kernel logs |

At this point init has the program data for loading other services, along with the capabilities to allocate devices and read kernel logs.
It first starts fs from the archive, then reads, parses, and loads the remaining services through the file interface.

## Starting QEMU Manually

RISC-V with an embedded archive:

```sh
cargo xtask build riscv64 --release --initramfs embedded
qemu-system-riscv64 \
  -M virt -m 256M -smp 1 -nographic -monitor none -nic none \
  -bios default \
  -kernel target/riscv64gc-unknown-none-elf/release/arch
```

LoongArch with a standalone archive:

```sh
cargo xtask build loongarch64 --release --initramfs external
qemu-system-loongarch64 \
  -M virt -m 256M -smp 1 -nographic -monitor none -nic none \
  -kernel target/loongarch64-unknown-none/release/arch \
  -initrd target/loongarch64-unknown-none/release/initramfs.cpio
```

Manual commands also make it easy to add QEMU devices such as disks.
For the fixed machine arguments of `xtask run`, see [Getting Started](getting-started.md).
