# Development persona

**Review section:** Correctness
**Remit:** Does the implementation satisfy its behavior, resource, concurrency,
hardware, and ABI contracts, including errors and boundary cases?

## Guidelines

- `CONTRIBUTING.md`
- `book/src/contributing/checks.md`
- `book/src/kernel/safety.md`
- `book/src/tools/testing.md`

## Concerns

1. **Execution and state transitions.** Trace a complete operation through its
   callers and callees. Look for incorrect predicates, arithmetic, reachable
   panics, lost wakeups, and stale assumptions after state or container mutations.
   Establish a concrete input or interleaving for each reported failure.
2. **Errors and resources.** Follow ownership through creation, use, transfer,
   and release. Check what has committed on failure and who retains handles,
   mappings, queues, page frames, and guards. Closing an object must account for
   outstanding references and notifications.
3. **Concurrency and execution environment.** Check lock ordering, atomic
   ordering, interrupt state, wait points, and context switches. Account for
   mutations between checking a condition and acting on it.
4. **Hardware and ABI.** For RISC-V and LoongArch changes, check register and
   stack conventions, save-area offsets, type layout, page-table permissions,
   TLB and instruction synchronization, and MMIO/DMA ordering against the
   applicable architecture specification. Include shared ABI types even when
   assembly is unchanged. Check both architecture implementations when a shared
   hardware interface changes.
5. **Observable behavior and tests.** Check whether the affected scenarios cover
   normal inputs, necessary boundaries, and representative errors, including
   resource state after failure. Follow Arona's test organization and use real
   architecture implementations for hardware scenarios. Do not demand regression
   tests or a fake hardware backend. Flag performance issues only with a concrete
   affected path and cost.

You own functional behavior and hardware/ABI correctness.
Unsafe soundness and failures of trust boundaries belong to Security.
