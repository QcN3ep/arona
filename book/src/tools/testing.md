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

Performance cases use `#[testkit::kperf]` on `fn(&mut testkit::perf::Perf)` and share the same registration and selection mechanism.
The `kperf` architecture target collects performance cases from its subsystem modules.
Reusable kernel scenarios live in `testkit` beside their functional counterparts.
Run performance cases through [the perf command](xtask.md#performance-cases).

`cargo xtask run` boots the normal system.

## Ext4 Disk Images

The file system scenarios mount an ext4 image from an attached block device.
The Linux suite harness mounts the first block device at `/var` with a 16 MiB extent, and the ext4 driver accepts only a narrow feature set, so an image created with default `mkfs.ext4` options is rejected during mount.
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
The `64bit` and metadata checksum features are unsupported and must remain disabled.
The mounted block count times the block size must stay within the mount extent, so keep the image within the 16 MiB the Linux suite passes, and mount the image with a clean filesystem state.

Attach the image to QEMU with a VirtIO block device:

```sh
-drive if=none,id=hd,file=disk.img,format=raw \
-device virtio-blk-pci,drive=hd
```

The default transitional device and the `disable-legacy=on` non-transitional device are both detected and driven through the modern VirtIO interface.

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

## Boot Logs

In a normal boot log, the architecture entry point first prints CPU and DTB information, and the kernel then prints the state of memory, Trap, the interrupt controller, timers, and the scheduler in initialization order.
`Started init` indicates that the first thread has been created and started; `System services ready` is logged by init after the base services start successfully.

## Choosing the Scope of Verification

Tests should be organized around features, representative errors, and necessary boundaries.
Scenarios involving error returns should also check resources and state after the failure.
For the specific principles and test file structure requirements, see the [Contribution Guidelines](../contributing/guidelines.md).

The verification scope follows the change: parsing changes run the corresponding input scenarios, IPC changes observe communication and resource handoff, and hardware implementation changes run the corresponding architecture images.
Handbook changes check commands, examples, and export results.
