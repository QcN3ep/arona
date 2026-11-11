# IPC Programming

Once two programs connect a pair of Channels, they can send short messages, transfer handles, or send a request and wait for a single reply.
The runtime wraps message encoding and kernel calls, and file, log, and device clients use these interfaces to express their own operations.

## Sending and Receiving

`Message::new(tag, words)` creates a short message, and `Channel::send` consumes the transfer handles in it after a successful submission.
Receiving yields a `ReceivedMessage`, whose `words()` and `handles()` provide the data and objects respectively.
When a message is destroyed, it closes receive handles that have not been taken.

`WaitMode` determines the wait behavior:

| Mode | Meaning |
|---|---|
| `Poll` | Try immediately; return `WouldBlock` if it cannot be completed |
| `Forever` | Wait for completion or the object's end |
| `Until(ns)` | Wait until an absolute monotonic deadline |

The value of `Until` uses the time domain of `thread::now_ns()`; 0 and `u64::MAX` are reserved for encoding polling and infinite waiting respectively.

## Request and Reply Example

The following creates client and server threads within the same Task.
The main thread sends data word 42, and the worker thread takes the Reply from the request and then replies with the same data to the main thread:

```rust
#![no_std]

extern crate runtime;

use runtime::{
    Error, Rights,
    ipc::{CallError, Message, WaitMode},
};

fn main() -> Result<(), Error> {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (client, server) =
        runtime::process::task().channel(rights, rights)?;
    let worker = runtime::thread::spawn(
        move || -> Result<(), Error> {
            let mut request =
                server.receive(WaitMode::Forever)?;
            let reply =
                request.take_reply().ok_or(Error::Protocol)?;
            let mut response =
                Message::new(request.tag, request.words())?;
            reply.send(&mut response)
        },
    )?;
    let mut request = Message::new(1, &[42])?;
    let deadline = runtime::thread::now_ns()?
        .checked_add(1_000_000_000)
        .ok_or(Error::Protocol)?;
    let response = client
        .call(&mut request, Some(deadline))
        .map_err(|error| match error {
            CallError::NotSubmitted(error)
            | CallError::Submitted(error) => error,
        })?;
    let _words = response.words();
    worker.join().map_err(|_| Error::Protocol)??;
    Ok(())
}
```

`client` and `server` are the two endpoints of the channel, and `take_reply` obtains the one-shot reply capability that the kernel delivers along with the call.
The worker thread returns after replying, the main thread obtains the response, and then retrieves the thread result through `join`.
This example sends both kinds of call errors to `main`; for calls that carry resources, the submission state determines handle ownership.

## Handle Transfer

`Transfer` contains `AnyHandle` ownership and receive rights, and is added to a message with `Message::push`.
If push fails, it returns the original Transfer, and the caller continues to own the resource.

After SEND or START_CALL succeeds, the runtime clears the transferred handles from the message and gives up the sender's ownership.
On failure, the message retains resources that have not been submitted.
A synchronous CALL uses the submission state returned by the kernel to decide whether to transfer ownership.

| Call result | Ownership of resources to transfer |
|---|---|
| `CallError::NotSubmitted` | Still in the request Message |
| Success or `CallError::Submitted` | Submitted to the kernel call path |

For example, if a client times out after delivering a write buffer to the file service, the service may already have completed the write.
`Submitted` expresses that the request has been handed over, and the completion status of the file operation is interpreted by the file service protocol.

## Asynchronous Calls and Event Loops

`start_call` returns a Call handle.
Place it with other sources into `wait_many`, and after receiving `READABLE` call `try_receive` to obtain the response.
Use `cancel` to cancel; dropping the handle cleans up the call resources along the object close path.

`PendingCall<T, S, E>` holds a client state `S` and a response decoding function in addition to the Call.
`try_complete` passes the state and the ReceivedMessage to the decoding function and returns the service result `T`.
After a `wait` timeout the call is still retained, and the caller can wait again or cancel.
Clients such as device opening and Linux program startup use this wrapper.

## Using Multi-Object Wait

`WaitItem::new(&handle, conditions)` borrows a handle to observe.
`wait_many` returns an index into the input list and the readiness bits for that entry, and the caller uses them to process the corresponding object.
The kernel identifies wait items by their underlying source; duplicate sources return an error.

```rust
fn wait_for_message(
    channel: &runtime::Handle<runtime::Channel>,
) -> Result<runtime::ipc::WaitResult, runtime::Error> {
    use runtime::ipc::{self, WaitItem, WaitMode};
    ipc::wait_many(
        &[WaitItem::new(
            channel,
            ipc::READABLE | ipc::PEER_CLOSED,
        )],
        WaitMode::Forever,
    )
}
```

When the peer closes, there may still be a final batch of messages in the queue, so the channel reports both readable and closed.
The event loop first receives queued messages and then handles connection termination; with multiple receiving threads, which message is actually obtained is determined by the receive operation.
