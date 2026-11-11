# Device Resource Management

Before a UART driver starts sending or receiving, it finds the UART in the device directory, claims its resource group, and then maps the registers and binds the interrupt.
These steps are managed by the kernel device module, and once they complete, the user-mode driver in devmgr can operate the device.
The same resource management also covers PCI devices, DMA mappings, and revocation on exit.

## Device Directory and Resource Groups

The device directory is built from `HardwareInfo` and includes devices found by PCI enumeration.
Each entry stores the device identifier, location, parent device, name, match identity, MMIO, interrupts, and attributes.
Users enumerate entries and read fields through DeviceSet's `NEXT`, `DESCRIBE`, and `READ`.
Variable-length fields can be read by offset, and the actual number of bytes copied is returned.

`DeviceUsePolicy` distinguishes devices reserved for the kernel from those that can be handed to users.
User devices form authorization groups according to resource relationships, and `CLAIM_GROUP` establishes a Device capability for an entire group.
Occupancy and conflicts over the same resource are checked in the directory state.

A device authorization token carries an open reference count and a revocation state.
Duplicated Device handles share the same authorization.
When the last open Device is released, the authorization is revoked, and internal references then complete the cleanup of mappings and notifications.

## MMIO Mappings

Device's `MAP` specifies a device member and a register region number.
The kernel verifies that the member belongs to the authorized group, then establishes a device mapping in the caller's address space and returns the virtual range.
The mapping retains references to the backing resource and the address space, and the device module reclaims it when the device unmaps or the authorization is revoked.

User drivers read and write registers through the runtime's MMIO access types.
Register widths, offsets, and protocols are determined by the specific driver.
The kernel controls the mappable ranges and mapping attributes.

Devices used by the kernel itself, such as the interrupt controller, are mapped by `MmioMapper` according to the ranges in the hardware directory.
It holds its own register mappings through the same memory management capabilities.

## Interrupt Grants and Acknowledgment

A driver binds a device interrupt to an Event and a non-zero notification bit.
The kernel stores the interrupt source, the authorization, the event reference, and the pending state.
The external interrupt dispatcher claims the input, marks the source as pending, and notifies the Event.

The driver handles device registers and queues in its event loop, then calls `ACK`.
ACK acknowledges the current notification round and restores input.
Unbinding, closing the event, and revoking the authorization remove the route and mask the corresponding interrupt.

## PCI Resources

A PCI host's ECAM, memory windows, and interrupt routing come from the hardware description.
The kernel enumerates functions on the bus, reads their identifiers and types, handles bridge and BAR resources, and establishes parent-child relationships and device groups.
Bus numbers of bridges already configured by firmware are used as the existing topology.

PCI configuration space access, address translation, and memory window validation are performed through hardware interfaces.
BARs published to the device directory use memory resources.
PCI bus master state is controlled by the DMA lifetime management path.

## DMA Objects and Backing Memory

VirtIO drivers fill queues with bus addresses that the device can access, and the device uses them to read and write buffers.
The kernel keeps these mappings and their backing page frames in a DmaDomain, and records whether the device can currently perform DMA.
Device creates this object for devices in its group.
The object starts as `Quiesced`; after mappings are registered, `RESUME` moves it to `Active`.
After the driver stops the queues and drains in-flight accesses, it calls `QUIESCE`; the mappings can then be removed or revoked.

| State | Meaning |
|---|---|
| `Quiesced` | Device DMA is quiescent; resources can be adjusted |
| `Active` | The device may use the registered DMA ranges |
| `Revoked` | The authorization has been revoked |
| `Poisoned` | Resources were closed or hardware start/stop reached a state from which it cannot continue |

Mappings are registered per page frame range within an allocation, checking physical contiguity, device address width, and bus address translation.
`ToDevice` retains read access, and the device-write direction retains write access.
Page frame references persist until the DMA mapping is safely removed, and conflicts between CPU mappings and DMA writes or execution are checked through page frame access records.

The current configuration uses the `CompetitionTrusted` DMA profile.
Device accesses use platform bus addresses, and lifetime operations include synchronization and control of the PCI bus master.
If ownership is lost while DMA is active, the device may still be accessing buffers.
This error path retains the page frames and halts the machine, and the resource state becomes `Poisoned`.
