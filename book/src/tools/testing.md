# Testing and Debugging

Arona's integration tests observe results and resource state from the feature's entry point.
For example, DTB parsing reads input on the host and checks the hardware description, thread switching runs actual contexts in QEMU, and IPC scenarios have different threads exchange messages and handles.
These scenarios are carried by host Cargo tests, architecture test images, and user-space test programs respectively.

## Host Checks and Tests

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings --force-warn clippy::too_many_lines
cargo test --locked --workspace --tests --features arona-fs/host-tests
```

`cargo xtask test host` runs the same host integration tests.

`cargo fmt` checks formatting, Clippy checks Rust code, and `cargo test` runs integration tests that satisfy the current target and feature conditions.
Host tests include boot information parsing, hardware binding, and the log interface.
Bare-metal test images are built with the corresponding `baremetal` or `program` feature.

Run the DTB and boot archive parsing tests separately:

```sh
cargo test --locked -p arona-arch --test boot
```

## Tests on Real Architectures

Paging, the kernel heap, Trap, execution contexts, timers, and the interrupt controller are carried by test images in `arch/tests/`.
Generic memory, Task, thread, IPC, and system call scenarios live in `testkit` and are invoked by the architecture entry point.
Test programs for the runtime and system services run in user space and use the same native kernel interface.

The test images cover debug and release builds for RISC-V and LoongArch.
These scenarios actually install exception entry points, manipulate page tables, switch execution contexts, and access emulated devices.
For example, the file system scenario attaches a disk fixture, and the UART scenario feeds characters into QEMU and then observes what the user-space session reads.

Run the QEMU tests through xtask:

```sh
cargo xtask test riscv64 --release
cargo xtask test loongarch64 --release --filter execution::kernel::ipc
cargo xtask test riscv64 --list
```

`#[testkit::ktest]` marks a safe `fn()` in an architecture test target.
Rust's custom test harness collects the functions from that target and its modules.
The architecture entry point initializes the boot console and calls the generated harness.
Each selected case runs in a fresh QEMU instance, so initialization and global state do not depend on earlier cases.
User-space scenarios retain their component programs and are launched by named cases in the interrupt image.

The runner builds the required programs, prepares disk fixtures, and supplies console input and the Ethernet peer.
It saves `serial.log`, `qemu.log`, the command, and a result summary under `target/tests/`.
`--timeout SECONDS` sets the time limit for each QEMU run.
An unexpected panic, an early QEMU exit, or a timeout fails the case.
Cases that deliberately trigger fatal exceptions declare the expected diagnostics in `#[testkit::ktest("diagnostic", ...)]`.
Those diagnostics must appear after the selected case starts.

CI runs all functional QEMU tests for both architectures in debug and release builds.
CI builds its container image before running the Rust checks, builds, and QEMU jobs, which all use the resulting image digest.
The image contains the pinned Rust toolchain, LLVM, QEMU 11.1.3, OpenSBI, Python, and ext4 tools.
Each QEMU job uploads the test logs, commands, and result summaries even when a test fails.
QEMU jobs allow up to 900 seconds per case in release builds and 5400 seconds in debug builds.

The `CI image` workflow can also be started manually.
Pull requests from forks use the published `ghcr.io/qcn3ep/arona-ci:latest` image.
Publish the image once and make the GHCR package public before running CI, so pull requests can use it without credentials.

Performance cases use `#[testkit::kperf]` on `fn(&mut testkit::perf::Perf)` and share the same registration and selection mechanism.
The `kperf` architecture target collects performance cases from its subsystem modules.

User performance cases use the same `#[kperf]` macro and `Perf::iter` interval as kernel cases.
Run `cargo xtask perf run riscv64 --release --case runtime --list` to list runtime cases, or use `--case fs` for filesystem cases.
The runner packages the selected program as the initial Task and discovers its compiled case descriptors over the console.
`--filter`, `--iterations`, and `--mode` have the same meaning for all three case collections.
Reusable kernel scenarios live in `testkit` beside their functional counterparts.
Run performance cases through [the perf command](xtask.md#performance-cases).

`cargo xtask run` boots the normal system.

## Ext4 Disk Images

The file system scenarios mount an ext4 image from an attached block device.
The Linux suite harness mounts a 16 MiB extent of the first block device at `/var`.
The filesystem scenarios also exercise a larger image with flex block groups, indexed directories, lazy bitmaps, and metadata checksums.
Create a compatible image with:

```sh
dd if=/dev/zero of=disk.img bs=1M count=16
mkfs.ext4 -q -F -b 4096 -I 256 \
  -O none,has_journal,extent,filetype,sparse_super,large_file \
  -E lazy_itable_init=0 \
  disk.img
```

The command retains the journal because the driver requires it for writes.
The driver accepts block sizes of 1024, 2048, and 4096 bytes and inode sizes of 128 or 256 bytes.
The driver supports `64bit` descriptors and metadata checksums, but block addresses and the filesystem block count must fit in 32 bits.
The mounted block count times the block size must stay within the mount extent, so keep the image within the 16 MiB the Linux suite passes, and mount the image with a clean filesystem state.

Attach the image to QEMU with a VirtIO block device:

```sh
-drive if=none,id=hd,file=disk.img,format=raw \
-device virtio-blk-pci,drive=hd
```

PCI block devices use the modern interface, including transitional devices.
The RISC-V block scenario also exercises the default legacy MMIO transport.

## VirtIO Net Frames

The `components/devmgr/tests/net/` program exchanges Ethernet frames with a host peer through a real QEMU VirtIO net device.
The peer uses QEMU's socket network backend and QMP to echo frames, send invalid Ethernet lengths, and lower and restore the link.
It needs no host TAP interface or IP configuration.

Run the network cases with:

```sh
cargo xtask test riscv64 --release --filter interrupt::net
cargo xtask test loongarch64 --release --filter interrupt::net
```

The runner starts the peer and assigns a separate port and QMP socket to each driver run.
The driver scenario checks queue cycling, TX pressure, short-frame padding, session I/O, cancellation, session closure, and device reset before DMA revocation.
The network service scenario checks loopback sockets without an Ethernet device.

## Linux Block Device Tests

The Linux test archive includes `/bin/linux-block`, which verifies the block device interface from a Linux guest.
It expects a second disk with an MBR table and two primary partitions; the first disk remains mounted at `/var`.

`cargo xtask test riscv64 --release --filter interrupt::linux` runs this scenario with a fresh ext4 root disk and an MBR disk.
The same case also supplies the terminal input used by the Linux tests.
The block checks cover partition capacities, metadata, byte I/O, sector preservation, and partition boundaries.

## Boot Logs

In a normal boot log, the architecture entry point first prints CPU and DTB information, and the kernel then prints the state of memory, Trap, the interrupt controller, timers, and the scheduler in initialization order.
`Started init` indicates that the first thread has been created and started; `System services ready` is logged by init after the base services start successfully.

## Choosing the Scope of Verification

Tests should be organized around features, representative errors, and necessary boundaries.
Scenarios involving error returns should also check resources and state after the failure.
For the specific principles and test file structure requirements, see the [Contribution Guidelines](../contributing/guidelines.md).

The verification scope follows the change: parsing changes run the corresponding input scenarios, IPC changes observe communication and resource handoff, and hardware implementation changes run the corresponding architecture images.
Handbook changes check commands, examples, and export results.
