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
It requires a C compiler, `pkg-config`, GLib development files, `sha256sum`, LLVM's `llvm-nm`, `llvm-readobj`, and `llvm-objcopy`, and QEMU with plugin support.
The installed `qemu-plugin.h` must use plugin API 7, and the QEMU executable must accept that API.
The plugin is compiled against this header on each run. `CC` selects the host C compiler.

The measured value is the number of guest instruction dispatches between two markers.
Both marker instructions are excluded. Exception handlers, interrupt handlers, and retried instructions within the interval are included.
This value is not a hardware retired-instruction count.
The calibration checks empty intervals, straight-line code, loops, calls, exception retries, interrupts, and code retranslation against known counts.

Results are saved under `target/perf/`, or under `perf/` in `CARGO_TARGET_DIR` when set.
Each run contains `run.json`, `counts.json`, `serial.log`, `qemu.log`, and the compiled plugin.
The compiler dependency file `count.d` records the headers used to build the plugin.
The `artifacts/` directory contains the full ELF with symbols and debug information, the stripped runtime ELF, and the RISC-V boot firmware when needed.
The two ELF files are checked for identical load addresses, permissions, sizes, and contents before QEMU starts.
`manifest.json` records file checksums, source changes, build options, and tool versions. `run.json` records the QEMU command and completion status.
A missing or repeated boundary, a count mismatch, a guest failure, or a timeout makes the command fail.
`--timeout SECONDS` sets the guest run limit, which defaults to 30 seconds.

To rerun a saved experiment, pass its result directory:

```sh
cargo xtask perf run riscv64 --artifacts target/perf/RUN_DIRECTORY
```

The saved build profile and guest files are reused without rebuilding the guest, and results go to a new directory.
Checksums and ELF correspondence are checked again. Missing, damaged, or unsupported artifacts cause an error.
The host plugin is rebuilt for each run and its source and checksum are recorded.

Use `--mode profile` to collect instruction hotspots and generate a report:

```sh
cargo xtask perf run riscv64 --release --case calibration --mode profile
cargo xtask perf report target/perf/RUN_DIRECTORY
```

Profiling also requires `llvm-symbolizer` and `llvm-objdump`.
It supports the shared boot mapping and controlled mappings in the calibration image.
The mapping scenario switches between programs at the same virtual address and reuses page tables for a new load instance.
The collector keeps these instances separate and checks instruction bytes against their saved ELF locations.
`counts.json` retains execution contexts, instruction addresses, bytes, and counts for each interval.
`report.md` and `report.json` group these counts by image, function or symbol, and source line, with instruction disassembly and inline frames.
Each group includes unknown entries, and each grouping sums to that interval's total.
Only the innermost inline frame receives the count. Enclosing frames provide context.
Each calibration interval is one work unit.

Mapping attribution uses read-only descriptors supplied by the calibration driver.
RISC-V also checks the active `satp` value. The LoongArch plugin API does not expose page-table roots, so its attribution relies on the controlled descriptor mapping.
Unreadable descriptors and uncertain lifetimes remain unknown, including execution after the scenario removes its descriptor.
This does not provide general process attribution for Arona workloads.

The report command runs offline and reads only the saved artifacts.
The saved ELF files are required. Missing source files do not prevent reporting recorded locations.
Changed code that does not match the saved ELF remains unknown and retains its captured disassembly.
Damaged files and inconsistent counts cause an error. Tool diagnostics are saved in `report.log`.

## System Instruction Collection

Use `--case system` to measure an existing kernel and external initramfs:

```sh
cargo xtask perf run riscv64 --case system --mode count \
  --kernel PATH/TO/arch --initramfs PATH/TO/initramfs.cpio \
  --start START_SYMBOL --end END_SYMBOL \
  --success 'COMPLETION MESSAGE' --workload WORKLOAD_NAME --units 1
```

Build the kernel for an external initramfs and keep its symbols and debug information.
Prepare the workload through the existing init configuration and archive packaging commands.
The collector copies both inputs and derives the runtime kernel from the supplied ELF.
It starts one QEMU vCPU with the existing VirtIO RNG configuration.
Boundaries are exact ELF symbol names or hexadecimal addresses prefixed with `0x`.
Each boundary must execute once. Both boundary instructions are excluded.
Both boundaries must lie in executable kernel segments.
The interval includes every instruction dispatched by the guest, including other tasks and interrupt handlers.
`--units` records the completed work represented by the interval.

The completion message must appear on the serial console after the measured work finishes.
A panic, missing boundary, missing completion message, or timeout makes the run fail.
Use `--case system --artifacts RUN_DIRECTORY` to reuse the saved kernel, archive, and measurement settings.
Use `--mode profile` to generate kernel instruction hotspots from the same inputs.
The report attributes verified bytes in the shared kernel mapping. User addresses and other unresolved code remain unknown.
Repeated unknown addresses are aggregated by PC and bytes without assigning a process or load identity.
Supply component symbols explicitly with `--image bin/fs=PATH/TO/fs`, repeated for other archive entries as needed.
Each supplied ELF must contain debug information and match the load segments of that archive entry.
These files are saved for inspection. They do not establish which user address space executed an instruction.

## Comparing Saved Runs

```sh
cargo xtask perf diff target/perf/BEFORE target/perf/AFTER
```

This command writes `diff.md` and `diff.json` in the second directory without building or running the guest.
It checks saved file checksums and requires matching ISA, case, collection mode, QEMU configuration, firmware, and collector.
System runs must also use the same workload name, work units, boundary expressions, and completion message.
Guest builds may differ. The caller must keep workload inputs and completion semantics comparable.

The comparison shows exact instruction counts and their difference, followed by counts per work unit.
JSON stores the signed integer difference as a decimal string. Percentages and per-unit values use floating point.
Profile comparisons also show image, function, and source-line changes, including unknown counts and coverage.
Groups match exact image names and function names or source locations. Names retain any symbol hashes.
Unmatched groups appear as added or removed. Absolute PCs are not matched across builds.
The Markdown report shows the 20 largest changes in each grouping. JSON retains every group.
