# IPC and Waiting

File reads, log submissions, and device opens all reach system services through IPC.
A request carries an operation tag, short data, and object handles in the channel; larger data such as file contents lives in page frames, and the message hands over the ability to access it.
The kernel delivers messages and resources, and services interpret the fields and perform the operations.

## Messages and Channels

A Channel object represents one endpoint of a bidirectional channel.
Messages are sent to the peer's receive queue and received in queue order.
Closing a channel changes the state observable by the peer and wakes the relevant waiters.

A native message contains a tag, at most 8 `u64` data words, and at most 8 handle descriptors.
A descriptor contains a handle, an object type, and transfer permissions.
The ABI writes fixed-size records with an explicit little-endian encoding, and services define the meaning of tags and data words themselves.

`SEND` adds a message to the queue.
`RECEIVE` takes a message and installs the transferred objects into the receiver's handle table.
In polling mode, an operation that cannot complete returns `WouldBlock`; waits can use an absolute deadline.

## Requests, Replies, and Commit

A synchronous `CALL` submits a request to a service and waits for a single response.
The kernel adds a Reply object to the request, and the service uses it to respond to the corresponding call once it receives the request.
A Reply can complete one response only, and after it is used or abandoned it is closed according to the call state.

A call has two stages: request submission and response delivery.
If it fails before commit, the transferred handles stay with the caller; after commit, the kernel has taken over the request and the resources to be transferred, even if the thread waiting for the response later times out.
The second result word of CALL reports the commit status, and the runtime converts it into `CallError`.

Receiving and response delivery use reserved handle slots and output callbacks:

1. Check that the user output memory is writable and hold it.
2. Obtain the message to be delivered and reserve the receiving handles.
3. Encode the message and the target handle descriptors, and write them into the output area.
4. Commit message consumption and handle publication.

For example, when a message carries a page frame handle, the output record the receiver obtains contains both the data words and the local handle value.
The kernel finishes writing this record before publishing handles and consuming the message; if the preparation stage fails, the reserved entries are released.

## Asynchronous Calls

`START_CALL` submits a request and returns a Call object.
The service puts the Call into a multi-object wait, continues receiving other requests, and takes the response once the result is ready.
`CANCEL` ends the wait for the result and updates the call state.

A Call manages the wait for the result and resource reclamation; an operation already delivered to a service runs along the service's own processing path.
For example, a write already performed by the file service keeps its effect; what the caller cancels is the wait for this result.
Peer disappearance, replier exit, and caller exit also enter the corresponding completion or close path.

## Event and Timer

An Event stores a set of pending bits.
`SIGNAL(mask)` merges a non-zero mask into it, and sending the same bits multiple times coalesces into the same pending bits.
A successful wait consumes the bits delivered by that wait, and other pending bits remain.

A Timer associates an Event with a mask and signals when the configured monotonic deadline is reached.
Programs can therefore include timeouts in the same wait loop as channels.
For how Event, Timer, and thread waits relate in time, see [Time and Timers](time.md).

## Multi-Object Waits

`WAIT_MANY` takes a set of handles and conditions and returns the input index of a ready entry and the satisfied condition bits.

| Source | Condition |
|---|---|
| Channel | `READABLE`, `WRITABLE`, `PEER_CLOSED` |
| Event | Pending bits specified by the caller |
| Task, Thread | Exit |
| Call | A result or an end state is available |

The same underlying source may appear only once in a single selection.
The implementation first resolves object references, checks for duplicates by sorting on stable identity, and then registers observers.
The duplicate check in this step has `O(n log n)` complexity.
Long lists set scheduling checkpoints in batches, and critical operations on a single object are protected by `InterruptGuard`.

Registration and wakeup are connected through a wait generation and a single completion result.
When the result is ready, the deadline expires, the object is closed, or the current thread exits, the wait registration is removed along the cleanup path.
When several threads wait on the same channel, they can all observe the readable state; actual receives compete for messages according to queue state, and a poll that gets no message returns `WouldBlock`.
