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

`cargo fmt` checks formatting, Clippy checks Rust code, and `cargo test` runs integration tests that satisfy the current target and feature conditions.
Host tests include boot information parsing, hardware binding, and the log interface.
Bare-metal test images are built with the corresponding `baremetal` or `program` feature.

Run the DTB and boot archive parsing tests separately:

```sh
cargo test --locked -p arona-arch --test boot
```

## Tests on Real Architectures

Paging, the kernel heap, Trap, execution contexts, timers, and the interrupt controller are carried by test images in `arch/tests/`.
Generic memory, Task, thread, IPC, and system call scenarios live in `kernel-tests` and are invoked by the architecture entry point.
Test programs for the runtime and system services run in user space and use the same native kernel interface.

The test images cover debug and release builds for RISC-V and LoongArch.
These scenarios actually install exception entry points, manipulate page tables, switch execution contexts, and access emulated devices.
For example, the file system scenario attaches a disk fixture, and the UART scenario feeds characters into QEMU and then observes what the user-space session reads.

To boot a built image, use `cargo xtask run <target> [--release]`.

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

Build the component and a release interrupt harness for the chosen Rust target:

```sh
cargo rustc --locked -p arona-devmgr --features baremetal --test net \
  --target riscv64gc-unknown-none-elf \
  -Zbuild-std=core,alloc,compiler_builtins \
  -Zbuild-std-features=compiler-builtins-mem \
  -- -Cstrip=symbols -Clink-arg=-zmax-page-size=4096
cargo build --locked -p arona-arch --features baremetal,runtime-tests \
  --test interrupt --release --target riscv64gc-unknown-none-elf \
  -Zbuild-std=core,alloc,compiler_builtins \
  -Zbuild-std-features=compiler-builtins-mem
```

Use the `net-*` executable from the target's `debug/deps/` directory as `COMPONENT` and the `interrupt-*` executable from `release/deps/` as `HARNESS`.
Cargo's JSON compiler-artifact messages also identify these paths.
The release harness can run either profile of the component program.

Start the peer in another terminal before QEMU:

```sh
python3 components/devmgr/tests/net/peer.py \
  --port 12345 --qmp /tmp/arona-net.qmp
```

Then start QEMU and enter `net` at the `Test suite:` prompt:

```sh
NET_DEVICE=virtio-net-pci,id=nic,disable-legacy=on
NET_DEVICE="$NET_DEVICE,netdev=net0,mac=52:54:00:12:34:56"
qemu-system-riscv64 -M virt -m 256M -smp 1 \
  -nographic -monitor none -nic none -bios default \
  -kernel "$HARNESS" -initrd "$COMPONENT" \
  -netdev socket,id=net0,connect=127.0.0.1:12345 \
  -device "$NET_DEVICE" \
  -qmp unix:/tmp/arona-net.qmp,server=on,wait=off
```

For LoongArch, build with `loongarch64-unknown-none`, use `qemu-system-loongarch64`, and omit `-bios default`.
The scenario checks repeated queue cycling, TX pressure, short-frame padding, copied session I/O, cancellation, session closure, and device reset before DMA revocation.
It reports `Net driver tests passed` when the component and kernel resource checks finish.

The same scenario supports `host_mtu=576,rx_queue_size=1024,tx_queue_size=256` to exercise a smaller MTU and a different device queue limit.
To exercise fallback configuration, use `status=off,mac=52:54:00:12:34:57` instead of the normal MAC.
That MAC tells the scenario to omit link-change assertions because this device configuration provides no link status feature.
Run a fresh peer for each QEMU instance and give concurrent runs separate ports and QMP paths.

## Boot Logs

In a normal boot log, the architecture entry point first prints CPU and DTB information, and the kernel then prints the state of memory, Trap, the interrupt controller, timers, and the scheduler in initialization order.
`Started init` indicates that the first thread has been created and started; `System services ready` is logged by init after the base services start successfully.

## Choosing the Scope of Verification

Tests should be organized around features, representative errors, and necessary boundaries.
Scenarios involving error returns should also check resources and state after the failure.
For the specific principles and test file structure requirements, see the [Contribution Guidelines](../contributing/guidelines.md).

The verification scope follows the change: parsing changes run the corresponding input scenarios, IPC changes observe communication and resource handoff, and hardware implementation changes run the corresponding architecture images.
Handbook changes check commands, examples, and export results.
