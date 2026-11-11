# Trap and Interrupts

When a program executes a system call, accesses a faulting address, or receives an interrupt, the processor transfers from the current code to the Trap entry.
The entry first saves the interrupted state, architecture code converts the hardware cause into a common `Trap`, and the kernel decides how to continue after handling the event.
This process also connects the kernel with the Linux compatibility service: calls from a supervised program are handed to the service through a TrapPort, and the original thread is resumed when done.

## Entry and State Saving

`hardware::TrapControl` provides the ability to install exception entries and update the privileged stack.
Installation specifies the stack range and the kernel handler function.
When dispatching a user thread, the execution path prepares the corresponding kernel stack and context for that thread.

RISC-V and LoongArch each save general-purpose registers, control state, and scalar floating-point state.
On exit, the floating-point enable state is restored from the saved frame, and the LR/SC reservation state is cleared.
The saved frame remains in the Trap handling path, and the common kernel accesses the information in it through unified events and user state.

## Common Events and Handling Results

`Trap` contains `origin`, `pc`, and `event`.
The origin is either Kernel or User; the main events are as follows:

| Event | Attached information |
|---|---|
| `Syscall` | Object handle, six argument words, PC after the call |
| `PageFault` | Faulting address and read, write, or execute type |
| `TimerInterrupt` | Timer interrupt |
| `ExternalInterrupt` | External device interrupt |
| `SoftwareInterrupt`, `OtherInterrupt` | Other interrupt categories |
| `IllegalInstruction` | Instruction word, when available |
| `OtherException` | Exception category and faulting address, when available |

There are three kinds of handling results, `TrapOutcome`:

| Result | Action of the architecture entry |
|---|---|
| `Resume` | Restore the saved execution state |
| `Return` | Write the status code and two result words, and resume at the address following the syscall |
| `Stop` | Stop this handling path |

`Return` is used for system calls.
Exception classification is done by the architecture layer, while call semantics and device resource handling are done by the common kernel.

## Kernel Dispatch

Kernel-origin timer interrupts enter the scheduler's time handling path.
External interrupts enter device interrupt dispatch.
Other kernel exceptions record the origin, PC, and event, then return `Stop`.

User-origin events enter the current thread's handling path.
Native system calls are handed to the object method dispatcher.
Timer and external interrupts use the same common interrupt handling mechanism.
A user exception terminates the faulting thread, while other threads continue to be managed by the Task and the scheduler.
Before resuming user execution, the handling path checks for preemption requests and thread termination state.

## Claiming and Completing External Interrupts

`InterruptController` initializes routing according to `HardwareInfo` and masks inputs during initialization.
RISC-V platforms use the PLIC, and LoongArch platforms use the corresponding external interrupt controller implementation.

An interrupt source is identified by the controller device identity and line together.
`claim` returns an `InterruptClaim`, and a claim is responsible for its own completion.
Normal handling calls `complete`; when a claim object is dropped directly, the input is masked first and then the claim is completed.
A user driver binds a device interrupt to an Event and bitmask.
When the interrupt arrives, the kernel records the pending state and notifies the event.
After the driver handles the device state, it calls ACK, and the device resource layer then unmasks the corresponding input.

Taking UART as an example, after the controller reports an interrupt, the kernel notifies the Event that the driver is waiting on.
devmgr is woken, reads the UART's interrupt cause and received data, and then acknowledges this round of notification.
The processor entry, interrupt controller, and device driver are connected in sequence by this path.

## Trap Forwarding and continuation

A supervisor creates a TrapPort and binds a target Task to that port.
After binding, system calls, page faults, illegal instructions, and other exceptions form a `TrapInfo`, and the original thread is suspended.
The service receives the event and a `TrapContinuation` from the port.

A continuation corresponds to one suspension of a specific thread and carries a generation.
The handler reads `UserState`, modifies registers, PC, or other user state, and then commits a resume.
The kernel verifies the address and execution conditions of the target state and wakes the corresponding thread.
When a continuation is abandoned, the kernel cancels the corresponding suspension handling.

One Linux system call can span multiple rounds of the service event loop.
After the compatibility service receives a continuation, if it is waiting for a file or device result, it saves the continuation together with the pending operation; when the I/O completes, it writes the return value and then resumes the thread.
While the program is suspended, the service continues to handle events from other threads.

A thread's `INTERRUPT` operation also uses this mechanism.
A ready thread can be suspended and a notification published; the currently running user thread handles registered interrupt requests in the return path.
The Linux compatibility service uses this channel to take over the supervised program's Linux system calls and signal-related execution.
