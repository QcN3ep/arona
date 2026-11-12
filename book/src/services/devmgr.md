# Device Management Service

Both logger and Shell use the serial port, but they submit text and read data through UART sessions, while register operations are performed by devmgr's drivers.
devmgr discovers devices from the DeviceSet, selects drivers, and creates worker threads, then delivers various device sessions to clients.
The kernel grants these drivers register, interrupt, and DMA resources.

## Matching and Instances

`DeviceDescription` combines the device description, identity, and attribute queries into a view that drivers can use.
A driver's `probe` identifies the device from these fields and generates an instance configuration after a match.
The device manager stores the device instance identity, resource group, driver configuration, and runtime state.

| Driver | Matching and transport | Operations provided |
|---|---|---|
| UART | Device Tree `ns16550`, `ns16550a` | configuration, input, output, flush |
| Block device | modern VirtIO MMIO or PCI block | capacity, read/write, flush, request cancellation and acknowledgment |
| Ethernet | modern VirtIO PCI net | exclusive copied frame RX/TX, MAC, MTU, link state |
| Random numbers | modern VirtIO PCI RNG | read device random bytes |
| RTC | Goldfish RTC, Loongson LS7A RTC | read Unix time |
| Power | Device Tree `syscon` with poweroff/reboot operations | capability query, shutdown, reboot |

The manager returns ready instances by device type and cursor.
Devices in the same group share the resource authorization holder, and each instance has its own control channel, event, and worker thread.
The worker thread user stack is 64 KiB.

## Opening Sessions

The client specifies a device identity and hands the newly created session endpoint to the manager.
The manager finds the instance and forwards the request to the corresponding worker thread.
The driver checks the session permission and device state, replies with the open result, and then handles requests on the session channel.

The client first obtains a device identity by enumerating by type, then requests to open the corresponding instance.
An asynchronous open stores the result in a Call; for example, init keeps observing service exits while waiting for a console session.
Each session cleans up pending requests and device access state when it closes.

## UART Registers and Configuration

The UART driver computes register locations from the `reg-shift`, `reg-io-width`, `reg-offset`, and byte order attributes.
Register access widths are 1, 2, and 4 bytes, and the mapping and access range are checked by the MMIO wrapper.
The clock source can be `clock-frequency` or a referenced fixed-clock.

`Settings` describes the baud rate, data bits, parity, and stop bits.
The driver configures the UART FIFO, receive and transmit state, and binds the device interrupt to an Event.
Reception checks the line status and returns the read result and error state to the caller.

The UART output session is used by clients such as logger for writing, and the input/output session is used by interactive terminals.
When a client writes, short text is encoded into the message words; the driver extracts the data and feeds it into the transmit path.
When a receive interrupt arrives, the driver reads the registers, collects characters into a buffer, and then replies to the session waiting for input.
The service loop handles session messages, interrupt notifications, and pending operations together.

The data amount per message is given by `uart::MAX_BYTES`, and `write_all` submits long text continuously.

`flush` waits for pending transmit data to complete.
After handling the interrupt cause in the registers, the driver acknowledges the kernel interrupt notification through the InterruptBinding.
On exit, it unbinds, disables UART interrupts, and reclaims the MMIO.

## VirtIO Queues and DMA

The modern VirtIO transport handles reset, feature negotiation, queue configuration, notification, and device state checks.
The PCI transport reads configuration capabilities and BARs, while the MMIO transport uses the register regions published by the firmware.
The driver negotiates `VIRTIO_F_VERSION_1` and the corresponding device features.
PCI probing validates the common, notification, and ISR regions independently of the device type.
The device configuration region can be absent when the driver does not need configuration fields.
Each configuration read checks the field's range and alignment.
The block driver always reads capacity and reads the block size only when that feature was negotiated.

Queues use a split virtqueue.
Queue creation takes an explicit queue index and selects that queue when checking its size and programming its descriptor, available, and used ring addresses.
Each Queue retains its own size, ring positions, pending descriptors, and notification information.
Configuring another queue does not replace the notification state of an existing queue.
PCI notifications use the selected queue's notification offset and carry its queue index.
MMIO notifications write the queue index to the notification register.

On submission, the driver describes buffers with descriptors, places the chain's head in the available ring, and notifies the device.
After the device finishes processing the chain, it writes a completion to the used ring, where the driver collects the result.
The used length counts bytes written by the device, so a chain containing only device-readable buffers must complete with zero bytes.
A chain containing device-writable buffers must report a nonzero length within their total capacity.
The device driver then checks the result against its operation's requirements.

Queue page frames and data buffers obtain bus addresses through the runtime's DMA interface, with synchronization performed before and after submission.
The driver holds the running queues and buffers and releases the DMA resources after the device has stopped and been reset.
Block and RNG currently use queue 0.

The block device transfers at most 4096 bytes at a time and maintains at most 16 requests.
The client submits reads and writes with a request identity, queries or waits for completion, and then ACKs to release the corresponding result.
The cancel interface returns a result depending on whether the request has already been handed to the device.
When the device supports the FLUSH feature, the service provides the corresponding persistence boundary.

The RNG driver submits a writable DMA buffer; after the device completes, it reads the random bytes and returns them to the client.
The RTC driver reads seconds and nanoseconds through its registers, and the LS7A path converts the calendar registers into Unix time.

## Power Operations

The architecture layer parses `syscon-poweroff` and `syscon-reboot` nodes and publishes the selected
register operations on the referenced system controller under the `power` property namespace.
A controller that provides at least one operation becomes assignable and is marked as having no DMA,
so the driver can claim its resource group and map the registers.

The driver reads the operation as an offset, mask, and value, then updates the register with
`(old & !mask) | (value & mask)` using the configured access width and byte order.
The capability query reports whether shutdown and reboot are available.
An action that the controller does not describe returns `Unsupported`.
The QEMU `virt` machine exposes both operations through its `sifive,test0` system controller.

## Ethernet Frame Sessions

The net driver uses RX queue 0 and TX queue 1 on a modern VirtIO PCI device.
It requires the device MAC feature and negotiates MTU and link status when available.
The effective MTU is at most 1500 bytes, with 1500 as the default when the device does not advertise an MTU.
Without the status feature, the link is treated as up.
Configuration reads use the transport generation to obtain a consistent MAC, MTU, and link snapshot.

Each queue uses at most 32 DMA slots, limited by its negotiated size.
Every buffer begins with the modern 12-byte VirtIO net header, which the session does not expose.
Checksum and segmentation offloads, mergeable receive buffers, control queues, and additional queue pairs are disabled.
The frame length includes the Ethernet header and an optional VLAN tag, and excludes FCS.
The driver pads transmitted frames shorter than 60 bytes with zeroes.
It discards received frames outside the supported length range and replenishes their slots.
Unexpected checksum or segmentation metadata is a device fault.

An authorized client opens one exclusive `net::Session` for the device.
Another open returns `Busy` until that session closes.
The session transfers copied frame bytes through ordinary frame allocations and grants no register or DMA access.
A receive waits for one complete frame, while a link watch waits for a sequence different from the client's last snapshot.
Only one receive and one watch can wait in a session at a time.
Their Call handles can be included in `wait_many` alongside other service work.

Transmit submission copies the frame into a free DMA slot.
A full TX queue returns `Busy` without publishing the frame, and a down link returns `Unavailable`.
The successful reply arrives after device completion, whose used length is zero because TX buffers are device-readable.
Completion proves that the device has finished reading the buffer, but provides no peer delivery guarantee.
Canceling a reply cannot recall an already submitted frame, so a lost reply leaves transmission uncertain.

The service retains at most 32 received frames and drops new arrivals when this cache is full.
It replenishes DMA buffers independently of client reads and discards frames when no session is open.
A canceled receive reply leaves its frame in the cache.
Canceled receives and watches retain their bounded request slots until a frame or link change resolves them, or the session closes.
Closing the session clears its requests and cached frames, while submitted TX buffers remain owned until device completion.
Closing the worker control channel resets the device before quiescing its DMA domain and releasing its buffers.
Pending callers observe service failure if their reply disappears.

## Failure Handling and Cleanup

After a worker thread returns normally or with an error, the manager joins the thread and records the instance's failure state.
The next time the device is opened, the manager attempts to re-establish the instance.
An instance that is initializing returns `Busy`, and a failed instance returns the corresponding error.

On exit, the manager closes the worker threads' control channels and waits for the threads to be reclaimed.
Each driver's exit path stops I/O, releases interrupts and DMA, and releases resource authorization references.
Fault results from the device itself and channel termination are reported separately to the requester.
