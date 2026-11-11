# Time and Timers

Thread sleep, IPC waits, and device timeouts all use monotonic time.
Arona represents them as nanoseconds and absolute deadlines, aggregated by the scheduler into the next hardware timer event.
A wait is completed by whichever comes first: message arrival or time expiry.

## Monotonic Clock

`hardware::Timer` records a counter base at initialization, and `now_ns` returns the number of nanoseconds elapsed since initialization.
The result remains monotonic and saturates when the value exceeds the representable range.
The interface also provides frequency query, absolute deadline setting, cancellation, and interrupt acknowledgement.

| Architecture | Counting and timing method |
|---|---|
| RISC-V 64 | Reads the time counter, uses the DTB timebase frequency, and programs timer events through SBI TIME |
| LoongArch 64 | Reads a stable counter, derives the frequency from CPUCFG information, and programs events through timer CSRs |

The common `Clock` implementation handles counter-to-nanosecond conversion, rounding deadlines up to hardware ticks, and overflow handling.
The counter and its state are managed by the architecture clock wrapper.

`set_deadline` replaces the current absolute deadline.
An already-expired time request triggers as soon as possible; a failed setting preserves the original setting.
`acknowledge` clears the current interrupt and keeps it disabled, after which the scheduler selects the next target again.

## Wait Deadlines

For example, a program waits for a channel response with a deadline set to the current time plus 100 milliseconds.
If the response arrives first, the thread obtains the result and removes the timed wait; if the time expires first, the scheduler commits `TimedOut`.
The internal `DeadlineQueue` uses a min-heap to store thread waits and records the position of each wait item, supporting early removal.
Notification and timeout race on the same wait generation, and only one result can commit.

After a timer interrupt arrives, the scheduler performs the following in order:

1. Acknowledge the hardware interrupt and read the current time.
2. Trigger expired IPC Timers.
3. Remove expired threads from the wait queue, commit `TimedOut`, and wake them.
4. Check preemption deadlines.
5. Re-arm the hardware timer for the next event.

The scheduler compares the earliest time among thread waits, IPC Timers, and preemption requests.
When there is no pending time event, it cancels the hardware timer interrupt.

## IPC Timer

A Timer is associated with an Event and a non-zero bitmask.
`ARM` sets a one-shot absolute deadline, and when it expires, merges the corresponding bit into the Event.
`CANCEL` revokes an already-set timer event.
Re-arming replaces the original deadline.

A Timer holds a reference to the event.
When the event is closed, the pending trigger items of related Timers are cancelled.
A program can place the Event notified by a Timer, channels, and thread exits into the same multi-object wait, handling time and I/O events together.

## Program-Visible Time

Native programs obtain monotonic nanosecond time through the current Thread's `NOW`, and wait until a deadline through `SLEEP`.
The runtime wraps these operations as thread interfaces.
IPC's `WaitMode::Until` also uses the same absolute time domain.

Wall clock time is provided by the RTC interface of the device management service.
The RTC gives Unix time for calendar time queries; wait deadlines use monotonic time counted from kernel timer initialization.
The kernel log records monotonic timestamps after timer initialization; earlier records remain untimestamped.
