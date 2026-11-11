# Build and Run Workflow

After changing the kernel or a service, you usually use xtask to rebuild images and run QEMU; to check an individual crate, calling Cargo directly is enough.
xtask strings together cross-crate build and packaging steps while continuing to use Cargo's dependency resolution and incremental compilation internally.

## Common Loop

```sh
cargo xtask build riscv64 --release
cargo xtask run riscv64 --release
```

`build` is suitable for checking build artifacts, and `run` starts QEMU after performing the same build.
To switch platforms, change the argument to `loongarch64`.
Omitting `--release` uses the debug profile.

A normal build proceeds in the following order:

1. Compile init, devmgr, logger, fs, and linux with the pinned toolchain.
2. Write the service ELFs and the optional external root directory into a newc archive.
3. Set the archive path and build the kernel in the selected embedded or standalone mode.
4. The run command starts QEMU for the specified platform.

Sub-builds use `--locked` and pass the target and build profile uniformly to the services and the kernel.
Service ELFs are stripped of symbols, while the kernel ELF keeps what the corresponding Cargo profile produces.

## Preparing Programs and Configuration

Place additional programs, symbolic links, and `/etc/init.toml` in a single root directory:

```sh
cargo xtask run riscv64 --release --rootfs ./rootfs
```

Linux static programs are loaded by the compatibility service; for the linking and entry conventions of native services, see [Writing Native Programs](../runtime/programs.md).
Packaging preserves Unix permissions, and the executable permission participates in the Linux program load checks.

To inspect packaging results, `package-init` produces the archive; `build` continues to the kernel after the same packaging flow:

```sh
cargo xtask package-init loongarch64 --release --rootfs ./rootfs
cargo xtask build loongarch64 --release --initramfs external --rootfs ./rootfs
```

For manual QEMU commands in standalone mode, see [System Images](../system/images.md).

## Locating Build Artifacts

By default, artifacts are in `target/<Rust target>/<debug or release>/`.
`arch` is the kernel ELF, the services use their own binary names, and `initramfs.cpio` is the normal boot archive.
To use a separate cache directory for a set of builds:

```sh
CARGO_TARGET_DIR=target/work cargo xtask build riscv64 --release
```

xtask resolves a relative build directory against the workspace root so that internal Cargo invocations use the same directory.
The external root directory, by contrast, is resolved relative to the user's working directory at invocation time.

## Checking Local Code with Cargo

`cargo check` is suitable for checking types and interfaces while editing, Clippy checks lints, and integration tests verify functionality.
The cross target and features determine which implementations are checked, for example:

```sh
cargo check --locked -p arona-arch --features baremetal \
  --target riscv64gc-unknown-none-elf
cargo check --locked -p arona-init --features baremetal \
  --target loongarch64-unknown-none
```

## Handbook Workflow

```sh
cargo xtask book
cargo xtask book serve --open
```

The prose lives in `book/src/`, and the HTML and PDF output in `target/book/`.
For maintaining the table of contents, plugins, and exports, see the [Maintenance Guide](../contributing/book.md).
