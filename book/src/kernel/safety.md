# Kernel Safety Boundaries

A buffer may first be allocated by a program, then mapped into an address space, and finally handed to a device for DMA.
This memory has different users at different stages, and the reclamation time depends on when the last use ends.
Arona expresses this relationship through ownership and references, controls operations through handle permissions, and leaves low-level access to architecture code.

## Safe Rust and Hardware Interfaces

`kernel` defines hardware interfaces such as MMU, CPU, execution context, Trap, timer, interrupt controller, MMIO, DMA, and PCI.
`arch` provides implementations and binds them to static slots before entering `boot_kernel`.
The kernel obtains the bound implementations through functions such as `hardware::mmu()`.

When establishing a mapping, the common kernel and the architecture implementation perform these steps in order:

1. The kernel allocates the physical page frames used by the page table and maintains mapping records and page frame references.
2. The kernel decides the virtual range, permissions, and the order in which mappings are established and revoked.
3. The architecture implementation encodes, reads, and writes page table entries, and performs page table switches, TLB flushes, and instruction synchronization.

The kernel implementation forbids the use of `unsafe` and conditional compilation.
Raw pointers, inline assembly, registers, and raw physical memory access are concentrated in the architecture layer.
See [Code Checks](../contributing/checks.md) for the specific file scope.

## Resource Lifetimes Expressed by Types

`Frame` is a copyable physical page frame address identity, and `FrameAllocation` owns one page frame allocation.
Mappings keep an allocation alive through `FrameAllocationReference`, and DMA access keeps backing page frames alive through references carrying an access direction.
After a program closes the original allocation handle, the mapping and DMA references continue to hold the allocation until they are also released.

Object closure follows a similar process.
When a channel is closed, a wait record may still reference it; the kernel first marks the object as closed and wakes the waiters, then destroys the storage as internal references are released.
This way, the closure result can be delivered to threads that are waiting, and the object's storage covers the entire notification process.

## User Memory Access

User addresses in system calls are interpreted in the caller's address space.
The kernel checks the address range, length, and read/write permissions, and keeps the page frame references needed during access through `MemoryBuffer`.
Cross-page input and output are performed per backing page frame.

For example, when reading a user slice that spans two pages, `MemoryBuffer` resolves the mappings of both pages and obtains access references, then reads their backing page frames separately.
During access, these references keep the page frames alive.
`query` and `translate` query according to user address space boundaries; the processor enforces access control based on the user attribute and the read/write/execute bits of page table entries.

## Capabilities and Authorization

Handles belong to each Task's handle table and contain object references and permission bits.
The dispatcher obtains the object from the caller's handle table, then checks the required permissions according to object type and method.

Copying and transferring handles can only preserve or reduce permissions.
When transferring across Tasks, the kernel installs the object in the receiver's handle table and delivers the receiver's handle identity.
Device authorization additionally checks the device group, mappings, interrupt bindings, and revocation state separately.

## Execution Environment Constraints

`# Safety` in hardware interfaces describes the execution environment.
For example, page table operations require the physical access window to be retained, execution context switches require the current stack and target address space to remain valid, and timer and interrupt control operations require local interrupts to be disabled on the owning processor.

`InterruptGuard` saves the interrupt state at entry, disables local interrupts, and restores the original state on drop.
Kernel critical sections use this common type.
Locks are released before context switches.
When a thread requests exit, the exit code still executes on its own stack; the scheduler first switches back to the dedicated scheduling stack, then reclaims the ended thread's context and stack, so reclamation happens after the last use.
