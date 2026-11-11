# Kernel Memory Management

When establishing a block of memory for a program, the kernel first allocates physical page frames, then maps them into virtual addresses; allocation and mapping each hold resources, and the page frames are reclaimed only after the last reference is released.
`kernel::memory` organizes physical allocation, address spaces, the RAM direct map, and the kernel heap around this lifetime.
The public entry point is the global `MemoryManager`, accessed via `memory::manager()`; page table hardware operations are performed through the MMU interface.

## Addresses and Physical Regions

`PhysRange` and `VirtRange` describe physical and virtual byte ranges respectively, using the half-open interval `[start, end)`.
The page size `PAGE_SIZE` is 4096 bytes.
The boot memory map classifies these ranges by purpose:

| State | Meaning |
|---|---|
| `Available` | Can be included in the page frame allocator |
| `BootReserved` | Range reserved during boot |
| `Reserved` | Permanently reserved |
| `NoMap` | Permanently reserved and mappings are prohibited |

The memory map holds at most 256 split regions.
A reservation covers the full envelope of the corresponding pages; available ranges are managed in whole pages.

## Page Frame Identity, Allocation, and References

| Type | What it expresses |
|---|---|
| `Frame` | An aligned physical page frame address, copyable |
| `FrameAllocation` | Ownership of one allocation, may contain multiple page frames |
| `FrameAllocationReference` | A live reference to the entire allocation |
| `FrameAccessReference` | Read, write, or execute access to a particular page frame |
| `ReservedRangeReference` | A reference to a reserved physical range |

`FrameAllocation::frames()` yields `Frame` identities in order.
The addresses obtained from iteration locate page frames; the allocation itself is held by `FrameAllocation` and its references.
An allocation record also contains a `FrameOwner`, a `FramePurpose`, and mapping, DMA, and temporary reference counts.

The allocator maintains separate free page frame lists for Kernel and User.
At initialization, up to 32 MiB is set aside from allocatable memory for the Kernel pool, and the rest goes to the User pool.
A multi-page-frame allocation takes page frames from the corresponding free list and links them into one allocation object using metadata.
These page frames may be at different physical locations; the virtual mapping arranges them in a contiguous virtual range in iteration order.
`MemoryManager::allocate` zeroes all allocated page frames before returning successfully.

Allocator metadata itself resides in dedicated physical page frames.
`ManagedRegion` describes a managed region, `FrameMetadata` manages record storage, and `FrameRecord` stores information about a single page frame and the allocation it belongs to.
`MemoryStats` separately reports the total, free, reserved page frame, and metadata page frame counts for the two pools.

For example, after a program allocates two page frames and establishes a mapping, it can close the allocation handle and continue to access the data through the mapping.
The mapping reference holds the entire allocation; after the mapping is revoked and the last reference is released, the page frames return to their original pool.

## Address Spaces and Mapping Records

`AddressSpace` holds a page table and a mapping index.
A user address space has an explicit user boundary; the kernel address space manages the high-half range.
User page tables share the kernel branch and retain a reference relationship to the kernel page table.

A mapping entry point accepts a `&FrameAllocation` or a `FrameAllocationReference`, along with the range of page frames within the allocation and `MappingOptions`.
`map` chooses a virtual address automatically; `map_at` uses a specified range.
Options include access permissions, maximum permissions, and address alignment requirements.

The mapping index uses an AVL tree, sorted by range start address.
The index is used to check for overlaps, find free ranges, query mappings, and handle range splitting.
Mapping changes also carry a generation, by which internal owners identify the mappings they established.

| Operation | Behavior |
|---|---|
| `map`, `map_at` | Establish the association between backing page frames and a virtual range |
| `reserve`, `reserve_at` | Occupy a virtual address range |
| `unreserve` | Release a reserved range |
| `unmap` | Revoke a mapping and release the mapping reference |
| `protect` | Modify mapping permissions within the maximum permissions |
| `query`, `query_next` | Query the mapping containing a given address or following it |
| `translate` | Query the virtual-to-physical translation in the current address space |

The global limit on user mapping records is 65536; reserved, prepared, and mapped records are all counted.
For example, when unmapping only the pages in the middle of a range, the remaining mapped ranges on either side become independent records.
The kernel reserves the records needed for splitting before modification and releases the reservation on failure.
Page table edits are performed through the `PageTable` internal to the memory module; the public mapping entry points are maintained uniformly by `AddressSpace`.

## Permissions and Aliases

The same physical page frame may appear in both a user mapping and the kernel direct map.
Permission checks therefore consider these aliases together: mappings have read-only, read-write, read-execute, execute-only, and no-access permissions, and page frame access records check for conflicts between writing and execution.
When a physical page frame is used for an executable mapping, the memory manager also adjusts the direct map's write permission for that page frame and synchronizes instruction access.

`protect` establishes a modification record for the entire mapping, then commits the page table and access reference changes.
Mapping establishment and permission modification save intermediate state and roll back on failure, returning a `MemoryError`.
User memory reads and writes use access objects with pinned backing page frames, so a system call retains the needed references during a cross-page access.

## RAM Direct Map and Device Mappings

The RAM direct map establishes a kernel virtual access path for managed RAM.
It uses the final page tables and is shared with the kernel portion of user page tables along with the kernel mappings.
The memory manager reads and writes page frames through this path, for example to zero newly allocated memory or copy program segments.
The boot-time physical access window is also retained for low-level operations.

Device MMIO is managed separately through `DeviceMapping` and user device mappings.
Device ranges, memory attributes, and resource authorization are checked by the device module.
Both kinds of mapping occupy virtual ranges; their backing resources are held by page frame allocations and device authorization respectively.

## Kernel Heap

`HeapAllocator` obtains memory from the Kernel page frame pool and establishes mappings in the kernel address space.
Small objects use 8 size classes divided by powers of 2, ranging from 16 to 2048 bytes; larger objects or those requiring higher alignment use dedicated allocations.
The occupancy state of small objects is recorded in bitmaps.

Allocation results enter kernel data structures through ownership types:

| Type | Purpose |
|---|---|
| `HeapAllocation` | Holds a block of heap memory satisfying a `Layout` |
| `HeapBox<T>` | Owns a heap object |
| `HeapArc<T>` | Holds a heap object with shared ownership |

`allocate_pinned` can obtain a heap object at a fixed location.
When small objects are allocated repeatedly, free slabs remain cached for reuse by objects of the same class.
`reclaim_heap` reclaims free caches and returns the number of page frames returned.
`HeapStats` provides heap usage and cache statistics.

Kernel thread stacks use `KernelStack` to hold page frames and a mapping, with a guard page established beside the stack.
A stack is reclaimed by the scheduling path only after the corresponding execution context has left it.
