# Getting Started

`cargo xtask run` builds the system services, packages the initramfs, and builds the kernel, then starts QEMU.
The first run compiles the relevant dependencies of the whole workspace; later builds reuse the Cargo cache.

## Development Environment

The development environment uses Linux, the Rust toolchain, and QEMU.
The kernel is compiled by the current Rust toolchain, while native system services use nightly to build the runtime's program entry and thread-local storage.

| Tool | Purpose |
|---|---|
| Rust, Cargo, and rustup | Compile the kernel, system services, and build tools |
| Rust nightly | Compile native programs that use a custom Rust program entry and TLS |
| `qemu-system-riscv64` | Run RISC-V 64 images |
| `qemu-system-loongarch64` | Run LoongArch 64 images |
| `clang`, `ld.lld` | Build compatibility test programs |

Install the Rust toolchain and cross-compilation targets:

```sh
rustup toolchain install stable --component rustfmt --component clippy
rustup toolchain install nightly --component rustfmt --component clippy
rustup target add --toolchain stable riscv64gc-unknown-none-elf loongarch64-unknown-none
rustup target add --toolchain nightly riscv64gc-unknown-none-elf loongarch64-unknown-none
```

## Build and Run

The run commands for RISC-V and LoongArch are:

```sh
cargo xtask run riscv64 --release
cargo xtask run loongarch64 --release
```

The command builds init, devmgr, logger, fs, and linux in order, packages them into an initramfs in cpio newc format, and then embeds the archive into the kernel ELF.
`--release` selects the release profile for both the kernel and the system services; when omitted, debug is used.

QEMU uses the `virt` machine, 256 MiB of memory, one processor, and terminal UART output, with the default network device disabled.
RISC-V uses QEMU's default BIOS; LoongArch uses QEMU's `-kernel` handoff path.

## What Happens After Boot

The UART first prints architecture boot messages, followed by `Entering kernel` and `Memory initialized`.
At this point the kernel is setting up the address space, exception entry, interrupt controller, and scheduler.
`Started init` indicates that the init thread has been created; once it runs, it continues to start the user-space services and finally prints `System services ready`.

The UART is also handed over during this process: the kernel boot stage uses an early console, detaches it before entering the scheduler, and the user-space devmgr then takes over the UART.
logger outputs service logs through a devmgr session.

The default archive contains five system services, and init waits for service events with an empty application list.
To add a shell, place the Linux static program and `/etc/init.toml` into an external root directory, then package them together with `--rootfs`.
[Boot Configuration and Usage](configuration.md) shows the configuration for an interactive shell.

## Building Images

```sh
cargo xtask build riscv64 --release --initramfs embedded
cargo xtask build loongarch64 --release --initramfs external
```

`embedded` embeds the initramfs into the kernel ELF, while `external` produces a standalone archive that QEMU loads at boot. When unspecified, `embedded` is used; omitting `--release` uses a debug build.

Artifacts are placed by default in the build directory of the corresponding target:

| Architecture | Directory |
|---|---|
| RISC-V 64 | `target/riscv64gc-unknown-none-elf/release/` |
| LoongArch 64 | `target/loongarch64-unknown-none/release/` |

Here `arch` is the kernel ELF, and `initramfs.cpio` is the system service archive.
Setting `CARGO_TARGET_DIR` changes the build directory.

For the image layout, external root directories, and manual boot methods, see [System Images](images.md).
For the full set of options, see the [xtask Command Reference](../tools/xtask.md).

## Build Troubleshooting

| Symptom | What to check |
|---|---|
| The `core` for the cross-compilation target is reported missing | Install the target for the stable or nightly toolchain actually in use |
| The system service build reports that a used feature is unsupported | The services use nightly; install it with `rustup toolchain install nightly` |
| QEMU is reported missing | Install `qemu-system-*` for the chosen architecture |
| `System services ready` is printed but there is no application output | Check `/etc/init.toml` and the application files in the archive |
