# init

After the kernel hands the boot archive, device directory, and kernel log handle to init, the construction of the user-space system begins here.
init loads the base services one by one, delivers the resources they use, and then starts applications according to `/etc/init.toml`.
Once in stable operation, it waits for state changes of applications and services and decides whether to restart applications according to the configuration.

## Receiving Boot Resources

runtime stores the current Task and the bootstrap channel in the program environment.
init receives a message tagged 0 from the channel, which contains one data word and three handles.
The data word gives the archive length, and the handles are the archive page frame, DeviceSet, and KernelLog in order.

After verifying the message, init uses `Archive::new` to build a read-only index for the newc archive.
It reads the ELF header of `/bin/init` to determine the current machine type, and subsequent native services are validated against this type.
The archive page frame is then handed to the file service, and init reads the remaining programs through the file client.

## Service Startup Order

| Order | Action |
|---|---|
| 1 | Load fs from the archive, hand over the archive, and establish the file client |
| 2 | Mount a fresh tmpfs at `/tmp` |
| 3 | Read devmgr from fs, hand over the DeviceSet, and wait for driver initialization to complete |
| 4 | Query the first block device and, if present, mount its full range as ext4 at `/var` |
| 5 | Open the UART output session, start logger, and hand over the KernelLog and the optional persistence file |
| 6 | Establish the local log facade, start net with a separate RNG session, and wait for loopback readiness |
| 7 | Start linux and hand over the file, RTC, and random number sessions |
| 8 | Record `System services ready`, read the configuration, and enter the supervision loop |

fs starts first because it establishes file access capability directly from the archive; init then mounts a tmpfs at `/tmp` so temporary files do not depend on persistent storage. devmgr next provides device sessions, letting persistent storage and serial output join the system.
After the `/var` mount succeeds, init creates the log directory, opens or creates `/var/log/system.log`, and hands the file session to logger.
Only after each service replies with its initialization result does init continue to use the interfaces it provides.

## Loading Native Services

`program::Loader` reads ELF through `ReadAt`.
It checks the machine type, loadable segments, page-aligned ranges, and entry, and uses a ChildTask to establish an isolated address space.
Segment contents are written into newly allocated page frames, zero-initialized parts within a segment remain zero, and the segments are then mapped according to permissions.

The loader uses a ChildTask to establish a user stack with a guard page and a ProcessStart.
Services use a 64 KiB initial user stack.
init selects the permissions on each service's handle to its own Task according to that service's responsibilities and delivers the remaining objects through bootstrap messages.

`Service` holds the returned Task handle.
If loading fails midway, the ChildTask reclaims the child Task under construction; after successful startup, the Service takes over lifetime management.

## Configured Applications

Each `[[program]]` in `/etc/init.toml` declares a name, executor, path, arguments, environment, terminal, and restart policy.
Linux programs in the configuration are loaded by the linux service, and init retains the asynchronous run request to observe their exit.

When `terminal = "console"`, init opens the corresponding UART session and gives it to the execution service.
The application's console session and logger's output session are established separately, and devmgr allocates input and output permissions according to the access mode.
For the specific configuration syntax and how to prepare Shell files, see [Boot Configuration and Usage](../system/configuration.md).

When the configuration file does not exist, an empty program list is used.
Syntax or field errors are recorded, and then init ends.

## Supervision Loop

The supervisor maintains the start, run, stop, and restart-wait states of each program.
Device session opening and program run requests use asynchronous calls, and the loop observes these requests, the exit of base service Tasks, and the next restart time at the same time.

Taking a Shell with `restart = "always"` as an example, the first start enters the running state, and after the user exits it switches to restart-wait; when the time arrives it reopens the required sessions and executes the program again.
When exits repeat within a short time, the restart delay increases exponentially from 100 milliseconds, up to 30 seconds; after a run reaches 30 seconds, the consecutive restart count is reset.
`on-failure` decides whether to enter this path based on the exit code or signal, while `never` keeps an exited application stopped.

Exit of a base service makes the supervisor end and return an error state.
Init closes the remaining control channels and sessions, then waits for the services to exit.
This lets device drivers reset their hardware before releasing DMA memory.
If a service does not exit before the deadline, init terminates its Task.

When the configuration requests a boot power action, init stops the base services before applying it.
The Linux compatibility service stops first.
Closing its control channel ends its supervised programs.
The network service stops after Linux releases its resources.
The log service then flushes and stops.
Once its files are released, the file service flushes and unmounts its filesystems before it stops.
The device management service stays available until it performs the shutdown or reboot through the platform power controller.
If a service does not exit after its control channel closes, init terminates its Task and reports that fallback.
