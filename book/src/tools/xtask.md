# xtask Command Reference

`xtask` is the host build tool within the workspace.
`.cargo/config.toml` defines `cargo xtask` as `cargo run --package xtask --`.
It combines Cargo builds, service archive packaging, QEMU startup, and handbook builds.

```text
cargo xtask <build|run|package-init|package-linux-test> <riscv64|loongarch64> [OPTIONS]
cargo xtask perf run <riscv64|loongarch64> --case calibration --mode count [OPTIONS]
cargo xtask book [build|serve] [OPTIONS]
cargo xtask help
```

## Kernel and Archive Commands

| Command | What it does |
|---|---|
| `build` | Build the five system services, package the archive, and build the kernel ELF |
| `run` | Run `build`, then start QEMU |
| `package-init` | Build the system services and package the normal boot archive |
| `package-linux-test` | Build and package the archive used by the Linux integration tests |

| Option | Default | Meaning |
|---|---|---|
| `--release` | Not enabled | Select the release build for the kernel and components |
| `--initramfs embedded\|external` | `embedded` | Choose an embedded or standalone archive; takes effect when building and starting the kernel |
| `--net` | Not enabled | Attach a modern VirtIO PCI NIC with QEMU user networking when running |
| `--rootfs PATH` | None | Append the files, directories, and symbolic links from a host directory to the archive |

For example:

```sh
cargo xtask build riscv64 --release
cargo xtask run loongarch64 --release --initramfs external --net
cargo xtask package-init riscv64 --rootfs ./rootfs
```

Without `--net`, QEMU starts without a NIC.
The option attaches a NIC with MAC `52:54:00:12:34:56`.
The network service starts in either case and provides loopback sockets.
Ethernet attachment and IP configuration are not implemented yet.
QEMU also receives a modern VirtIO RNG for network service startup.

`package-linux-test` uses a test entry point in place of the normal init, adds the compatibility test programs and data, and outputs `initramfs-linux-test.cpio`.
Normal boot and tests each keep their own archive file.

## Handbook Commands

```sh
cargo xtask book
cargo xtask book build
cargo xtask book serve --open
```

When the action is omitted, `build` is performed by default.
Options after `build` and `serve` are passed to mdBook unchanged, for example:

```sh
cargo xtask book build --help
cargo xtask book serve --hostname 127.0.0.1 --port 3000
```

xtask invokes `mdbook`, which reads `book/book.toml`; for tool installation and export instructions, see the [Maintenance Guide](../contributing/book.md).

## Toolchain and Environment Variables

All artifacts are built with the toolchain pinned in `rust-toolchain.toml`; system services use `cargo rustc` and the kernel uses `cargo build`.
Internal Cargo builds use `--locked` and follow the dependency resolution pinned by the workspace `Cargo.lock`.

| Environment variable | Purpose |
|---|---|
| `CARGO_TARGET_DIR` | Determines the Rust build directory and the initramfs output directory; relative paths are interpreted by xtask relative to the workspace root |
| `ARONA_INITRAMFS` | Set by xtask to the archive path for the architecture crate's build script and the embedded archive code |
| `PATH` | Locate external programs such as Cargo, QEMU, mdBook, and its plugins |

The handbook output directory is defined as `target/book/` by `book/book.toml`.
The completed PDF is exported to `docs/arona-book.pdf` and included in the HTML output for download.

## Build Artifacts

The default Rust output paths are as follows, where `PROFILE` is `debug` or `release`:

| Target | Artifact directory |
|---|---|
| `riscv64` | `target/riscv64gc-unknown-none-elf/PROFILE/` |
| `loongarch64` | `target/loongarch64-unknown-none/PROFILE/` |

The directory contains the kernel `arch`, the service ELFs, and the archive.
On success, `package-init` and `package-linux-test` print the full path of the archive.
When any external build or run command fails, xtask returns a non-zero status.

## Native Network Scenarios

`cargo xtask package-net-test riscv64 --release` packages the network service with separate native client Tasks.
The resulting archive is `target/riscv64gc-unknown-none-elf/release/initramfs-net-test.cpio`.
Use `loongarch64` for the other architecture and omit `--release` for debug builds.
Run the archive with the interrupt test image, a modern VirtIO RNG, and the `net-service` suite.
These loopback scenarios do not require `--net` or an Ethernet device.

## Instruction Count Calibration

```sh
cargo xtask perf run riscv64 --release --case calibration --mode count
cargo xtask perf run loongarch64 --release --case calibration --mode count
```

This command builds and runs a single-vCPU calibration image with a host QEMU plugin.
It requires a C compiler, `pkg-config`, GLib development files, `llvm-nm`, and QEMU with plugin support.
The installed `qemu-plugin.h` must use plugin API 7, and the QEMU executable must accept that API.
The plugin is compiled against this header on each run. `CC` selects the host C compiler.

The measured value is the number of guest instruction dispatches between two markers.
Both marker instructions are excluded. Exception handlers, interrupt handlers, and retried instructions within the interval are included.
This value is not a hardware retired-instruction count.
The calibration checks empty intervals, straight-line code, loops, calls, exception retries, interrupts, and code retranslation against known counts.

Results are saved under `target/perf/`, or under `perf/` in `CARGO_TARGET_DIR` when set.
Each run contains `run.json`, `counts.json`, `serial.log`, `qemu.log`, and the compiled plugin.
The compiler dependency file `count.d` records the headers used to build the plugin.
`run.json` records the ELF path and QEMU command. The ELF remains in Cargo's build directory.
A missing or repeated boundary, a count mismatch, a guest failure, or a timeout makes the command fail.
`--timeout SECONDS` sets the guest run limit, which defaults to 30 seconds.
