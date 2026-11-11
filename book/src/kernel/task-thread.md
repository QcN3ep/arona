# Task and Thread

When running a service, the kernel uses a Task to hold its address space, handle table, and user threads, and a Thread to represent each schedulable execution flow.
Threads in the same Task share user mappings and the handle table, and each keeps its own register state and stack.
This lets a service isolate resources using separate Tasks and also handle requests internally with multiple threads.

## Task Resources and States

A Task's control record and runtime resources are stored separately.
The control record retains the identity, state, and exit waiters; the runtime resources include the address space, handle table, thread set, and Trap bindings.
After a Task exits, memory and handles are released with the runtime resources, while the control record continues to provide the exit status to observers.

| State | Meaning |
|---|---|
| `Created` | Resources established, waiting to start a thread |
| `Active` | There has been running activity |
| `Dying(status)` | Termination requested, stopping threads and reclaiming resources |
| `Dead(status)` | Runtime resources have exited, final state observable |

Creation relationships are recorded when a child Task is created.
The creator installs handles into the child Task, allocates and maps memory, and establishes user threads through the corresponding permissions.
When runtime resources are reclaimed, handles are closed, the address space is released, and related Trap ports are handled.

`terminate` requests termination of the threads in a Task.
The interface that waits for a Task to exit returns the status after final exit; a wait with a deadline can also return a timeout.

## Thread States and Resources

Each Thread owns an execution context and a kernel stack with a guard page.
A user thread is additionally associated with its owning Task, and its user stack and program mappings are located in that Task's address space.
The current kernel thread stack size is 32 KiB, and thread object capacity is 4096.

| State | Typical way to enter |
|---|---|
| `Created` | Create thread |
| `Ready` | Start, wake from a wait, or yield the processor |
| `Running` | Dispatched by the scheduler |
| `Blocked` | Wait for a notification, a deadline, or the handling of a forwarded event |
| `Dying(status)` | Termination requested |
| `Dead(status)` | Has left the execution context and committed exit |

`start` adds a created thread to the ready queue.
`yield_now` hands the current thread back to the scheduler.
`exit` ends the current thread, and the exit status is readable by waiters.
User exceptions are handled per thread; the Task lifetime is maintained jointly by its thread exits and the Task management path.

## Execution Context

`hardware::Execution` provides context layout, initialization, switching, restoring, and user register state access.
The common `Context` allocates dedicated heap storage for architecture state.
The architecture implementation saves general-purpose registers and scalar floating-point state, and distinguishes the currently active context from saved contexts.

When dispatching a user thread, the scheduler selects its address space, prepares the privileged stack used by the exception entry, and then restores the context.
The hardware state needed for user return and re-entry into the kernel is maintained by architecture code.
Scheduling policy, thread sets, and resource reclamation are maintained by the common kernel.

Only after the current context has left its own stack does code on the scheduling stack reclaim that thread's context and kernel stack.
The Thread control object remains available for other threads to query and wait for the exit result.

## FIFO Scheduling and Preemption Requests

The ready queue uses FIFO: enqueued threads are appended to the tail, and dispatch takes from the head.
The scheduler handles four kinds of switch-out reasons: yield, block, exit, and preemption.
After each dispatch, it returns to the scheduling stack to process state commits and resource reclamation, and gives pending interrupts a chance to enter the handling path.

The preemption mechanism provides `request_preemption` and `set_preemption_deadline`.
The former requests rescheduling at the user return boundary; the latter registers a one-shot absolute time event and sets a preemption request when it expires.
A thread's one-shot preemption deadline is cleared when it switches out.
Kernel execution leaves the current context at controlled switching points.

When there is no ready thread, the scheduler uses the CPU's wait-for-interrupt capability to enter the idle state.
After a notification or timer event makes a thread ready, it enters the dispatch loop again.

## Waiting and Waking

When waiting for a message, a message may arrive exactly between checking the queue and entering the blocked state.
The kernel first uses `prepare_wait` to establish a wait with a generation, then registers the `Waker` with the event source, and finally commits the block.
If the message arrives before the commit, the result is first stored in the wait state and the thread then obtains the result directly; if it arrives after the commit, the notification puts the blocked thread back on the ready queue.

`Waker::wake` completes only the one wait corresponding to its own generation.
The next round of waiting uses a new generation, and the previous round's notification becomes invalid.
Notification, timeout, and object exit race for this round's result through the same completion path.
`Wait` destruction removes the deadline and the exit wait registration.

When a thread or Task exits, it notifies the exit waiters.
Multi-object waiting further combines these sources with channels, events, and asynchronous calls; see [IPC and Waiting](ipc.md).
