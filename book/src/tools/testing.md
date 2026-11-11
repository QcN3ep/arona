# Testing and Debugging

Arona's integration tests observe results and resource state from the feature's entry point.
For example, DTB parsing reads input on the host and checks the hardware description, thread switching runs actual contexts in QEMU, and IPC scenarios have different threads exchange messages and handles.
These scenarios are carried by host Cargo tests, architecture test images, and user-space test programs respectively.

## Host Checks and Tests

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings --force-warn clippy::too_many_lines
cargo test --locked --workspace --tests
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

## Boot Logs

In a normal boot log, the architecture entry point first prints CPU and DTB information, and the kernel then prints the state of memory, Trap, the interrupt controller, timers, and the scheduler in initialization order.
`Started init` indicates that the first thread has been created and started; `System services ready` is logged by init after the base services start successfully.

## Choosing the Scope of Verification

Tests should be organized around features, representative errors, and necessary boundaries.
Scenarios involving error returns should also check resources and state after the failure.
For the specific principles and test file structure requirements, see the [Contribution Guidelines](../contributing/guidelines.md).

The verification scope follows the change: parsing changes run the corresponding input scenarios, IPC changes observe communication and resource handoff, and hardware implementation changes run the corresponding architecture images.
Handbook changes check commands, examples, and export results.
