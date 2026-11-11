# xtask Command Reference

`xtask` is the host build tool within the workspace.
`.cargo/config.toml` defines `cargo xtask` as `cargo run --package xtask --`.
It combines Cargo builds, service archive packaging, QEMU startup, and handbook builds.

```text
cargo xtask <build|run|package-init|package-linux-test> <riscv64|loongarch64> [OPTIONS]
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
| `--rootfs PATH` | None | Append the files, directories, and symbolic links from a host directory to the archive |

For example:

```sh
cargo xtask build riscv64 --release
cargo xtask run loongarch64 --release --initramfs external
cargo xtask package-init riscv64 --rootfs ./rootfs
```

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

## Build Artifacts

The default Rust output paths are as follows, where `PROFILE` is `debug` or `release`:

| Target | Artifact directory |
|---|---|
| `riscv64` | `target/riscv64gc-unknown-none-elf/PROFILE/` |
| `loongarch64` | `target/loongarch64-unknown-none/PROFILE/` |

The directory contains the kernel `arch`, the service ELFs, and the archive.
On success, `package-init` and `package-linux-test` print the full path of the archive.
When any external build or run command fails, xtask returns a non-zero status.
