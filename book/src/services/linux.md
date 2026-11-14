# Linux Compatibility Service

When running a Linux Shell, the program itself resides in a separate supervised Task, and the linux service is responsible for interpreting the Linux system calls it issues.
Opening a file connects to fs, terminal input and output connect to devmgr, while process identities, fds, signals, and Linux address space records are kept in the compatibility service.
The kernel hands the program's calls and exceptions to it through a TrapPort.

## Starting a Linux Program

init provides the service with file and network clients, a read-only kernel log, and optional RTC and random number sessions.
The execution request contains the program path, argv, envp, and terminal session.
`Control::start_run` returns an asynchronous call, and the request completes with an exit status after the program ends.

The loading process proceeds in stages:

1. Open the program through the file service and query its metadata, checking executable permission.
2. Read the file and verify an ELF64 `ET_EXEC` or `ET_DYN` image matching the current machine.
3. Allocate page frames for the loadable segments and copy the data, establishing R, RW, or RX mappings.
4. Construct argc, argv, envp, and the auxiliary vector on the user stack.
5. Create a Task supervised by the TrapPort and an initial thread, set the registers, and start it.

ELF checks include segment boundaries, non-overlapping page ranges, and the entry lying in an executable region.
Position-independent `ET_DYN` images receive an aligned load address.
If the image requests a `PT_INTERP` interpreter, the service loads it at a separate address and starts at its entry point.
The auxiliary vector describes the main program and the interpreter base, so the guest loader can process `PT_DYNAMIC`, relocate the images, and load shared libraries.
When the program file starts with `#!`, the service instead parses the interpreter path and its optional argument, restarts the load through the interpreter, and rewrites `argv` the same way `execve` does, so service startup runs scripts and their interpreters with the original arguments.
The initial stack has a guard page, and the data area contains the argument strings, environment, program header location, and auxiliary vector.
When a random number session is available, 16 bytes of random data are also generated for the startup stack and handed over as `AT_RANDOM`.

## Traps and Asynchronous Dispatch

When a Linux program traps, the kernel publishes a TrapInfo and a continuation and suspends the calling thread.
The compatibility service obtains the Linux system call number and arguments from the UserState according to the machine type, then dispatches to the corresponding module.
On completion, the return value or negative errno is written to the registers, and execution resumes through the continuation.

For example, when the Shell waits for a line of terminal input, the continuation for `read` is saved as Pending together with the input request.
After a character arrives on the UART, the service continues this read, writes the data and return value back to the program, and then resumes execution.
Files, child processes, futexes, and timed waits are also saved as pending operations.
The main loop continues to handle control requests, Traps from other programs, and suspended operations.
When there is no progress, the relevant Calls, events, thread exits, and ports are added to a multi-object wait.

| Implementation group | Corresponding content |
|---|---|
| `syscall/file/` | open, I/O, path, directory, metadata, pipe, poll, and terminal |
| `syscall/mqueue.rs` | POSIX message queues and the `/dev/mqueue` attribute view |
| `syscall/socket/` | TCP and UDP sockets, local Unix socket pairs, messages, addresses, options, and pending I/O |
| `syscall/process/` | clone, exec, exit, wait, identity, thread, futex, scheduling, affinity, and membarrier |
| `syscall/memory/` | mmap, munmap, mprotect, mremap, and related memory operations |
| `syscall/shm.rs` | System V shared memory segments and control operations |
| `syscall/signal.rs` | signal system calls |
| `syscall/system/` | uname, clock, sleep, interval timers, and random numbers |

The dispatcher returns `ENOSYS` for unmatched calls.
Arguments, flags, and error conversions of recognized calls are checked by their handling modules.

## File Descriptors and Paths

The FileTable maps Linux fds to open file descriptions.
A description holds a file service File, Directory, pipe end, terminal, or socket, and duplicated fds share the corresponding open state.
Each fd also stores descriptor attributes such as close-on-exec.
An eventfd shares a counter across duplicated descriptors and inherited process descriptors.
Reads wait for a nonzero counter, and writes wait for enough room in the counter.
Nonblocking operations report `EAGAIN` when they would wait.
Selected files under `/proc` and `/sys` expose read-only snapshots taken when the descriptor is opened.
These include process status and arguments, mount information, the release version, and the online CPU list.
Virtual character descriptions implement `/dev/null` and `/dev/zero`, while `/dev/random` and `/dev/urandom` read from the configured random number session.
`/dev/console` and `/dev/tty` open the terminal that owns the calling process, so console reads, writes, and ioctls use the same line discipline as the standard streams.

## Kernel Log

`init` transfers a read-only `KernelLog` handle to the Linux service, and the `syslog` call reads the same 64 KiB kernel ring that logger consumes.
Each record is formatted as `<level>[seconds.microseconds] target: message` before it is copied to the guest.
`SYSLOG_ACTION_SIZE_BUFFER` reports the ring capacity.
`READ` consumes bytes through a service-wide cursor, and `SIZE_UNREAD` reports the remaining bytes.
The cursor records the kernel sequence and the byte position within a partially read record, so eviction advances it to the oldest retained record.
`READ_ALL` returns the newest visible bytes that fit without moving this cursor.
`READ_CLEAR` returns those bytes and clears their visibility for later snapshots.
`CLEAR` has the same clearing effect without returning text.
These two operations leave the consuming cursor and the kernel ring intact.
This compatibility layer has no capability sets, so effective uid zero stands in for `CAP_SYSLOG`.
Only `OPEN` and `CLOSE` are available to unprivileged processes; control and read actions return `EPERM` otherwise.
Console level actions report `EINVAL` because the service does not own the physical console.
Invalid actions and lengths return `EINVAL`, and an unusable destination buffer returns `EFAULT`.

## Network Tables

`/proc/net/dev`, `/proc/net/route`, `/proc/net/tcp`, `/proc/net/tcp6`, `/proc/net/udp`, `/proc/net/udp6`, and `/proc/net/unix` are read-only snapshots taken when a descriptor is opened.
Socket rows come from the network service's live session table and use the Linux text layout with hexadecimal addresses, ports, states, uid, and inode.
TCP listeners report state `0A`, established connections report `01`, and unconnected or closed entries report `07`.
Interface rows come from lwIP's interface list, and `/proc/net/route` derives a connected route from each IPv4 address and netmask.
The loopback-only stack does not track interface byte counters yet, so the `/proc/net/dev` counters are zero.
No Unix-domain sockets are implemented, so `/proc/net/unix` contains only its header.
Opening `/proc/net` itself returns `ENOTDIR`, and an unknown name below the virtual mounts returns `ENOENT`.

| Operation group | Implemented calls |
|---|---|
| Open and descriptors | `openat`, `close`, `dup`, `dup3`, `fcntl` |
| Byte I/O | `read`, `write`, `pread64`, `pwrite64`, `readv`, `writev`, `preadv`, `pwritev` |
| File status | `fstat`, `newfstatat`, `statx`, `statfs`, `fstatfs`, `lseek`, `ftruncate`, `fsync`, `fdatasync` |
| Access, timestamps, and flushing | `faccessat`, `faccessat2`, `utimensat`, `sync`, `sendfile` |
| Directory and paths | `getdents64`, `chdir`, `fchdir`, `getcwd`, `mkdirat`, `unlinkat`, `readlinkat`, `symlinkat`, `linkat`, `renameat`, `renameat2` |
| Mounting | `mount`, `umount2` |
| Waiting and pipes | `pipe2`, `eventfd2`, `ppoll`, `pselect6` |

`mount` accepts the `tmpfs` filesystem type and resolves the target directory through the file service before adding a mount table entry.
The source name is ignored, read-only and bind-style flags are rejected, and nonzero data options currently report `EINVAL`.
`umount2` removes tmpfs mounts created through `mount` and reports `ENOENT` for a missing target, `EINVAL` for a path that is not one of those mounts, and `EBUSY` for a mount with nested entries.
The startup `/tmp` mount and disk mounts are not removable through `umount2`.
The compatibility layer does not expose disk mounts to programs.
Both calls are dispatched as pending operations, so waiting for the file service does not stall other guests or timer handling.

`mount` and `umount2` check that the caller's uid and euid are 0.
Every Linux process starts with both set to 0, so the check currently rejects only a process that has already dropped its credentials; it does not distinguish ordinary guest programs.
The file service accepts mount requests only on the mount-control channel that init passes to the Linux service, and guest code cannot reach that channel directly.
Restricting mounting to selected programs requires assigning real nonzero identities to the remaining processes, which is not implemented yet.

Path operations convert the current directory or dirfd into a file service directory anchor.
Permission modes, umask, and open flags are interpreted in the compatibility layer, while the actual file content and persistence are handled by the file service.
`O_PATH` requests resolve the node and return a path-only descriptor: it answers status queries, anchors relative opens, and serves as the working directory, while byte I/O, memory mapping, ioctl, and directory reads fail with `EBADF`.
`O_PATH | O_NOFOLLOW` retains the symbolic link itself.
An empty `readlinkat` path reads that link through its descriptor even after unlinking it.
`faccessat` and `faccessat2` resolve the path and compare the requested `R_OK`, `W_OK`, and `X_OK` bits against the inode's mode and the caller's real or effective identities, so an unprivileged caller is denied in the same cases as Linux.
A superuser still needs an execute bit to pass `X_OK` on a regular file.
`utimensat` stores explicit, current, or omitted access and modification timestamps through the file service, and updates the change timestamp to the current time.
`sync` flushes every mounted volume and reports the first flush failure.
`sendfile` stages one page at a time between a regular source file and a regular file, pipe, or connected socket destination, honors an explicit offset pointer, and returns short counts for partial writes.
`statx` also reports the filesystem mount identity and the creation timestamp when the stored inode provides it.
`statfs` resolves the path and reports its filesystem's capacity and identity.
`fstatfs` uses the filesystem held by an open regular file or directory, so renaming or unlinking the file does not change the result.
Statistics distinguish the read-only boot archive from mounted ext4 volumes and account for ext4 blocks reserved for the administrator.
Virtual device descriptors do not yet provide filesystem statistics.
For example, for an ordinary file read, the compatibility service submits the offset and output buffer to fs, writes the data to the target Task after obtaining the result, and updates the open file's offset according to the actual completed length.

The `F_GETPIPE_SZ` and `F_SETPIPE_SZ` commands query and resize a pipe through either endpoint.
Resizing preserves queued bytes and rejects a capacity below the amount already queued.

Monotonic timer descriptors report expiration counts through `read`.
Their shared timer wakes blocked reads and readiness waits, including after another process rearms or disarms the descriptor.

Signal descriptors expose matching pending signals from the reading thread and its process.
Each read returns complete `signalfd_siginfo` records, and poll/select use the calling thread when checking readiness.

## POSIX Message Queues

The compatibility service implements POSIX message queues with the asm-generic call numbers 180 through 185: `mq_open`, `mq_unlink`, `mq_timedsend`, `mq_timedreceive`, `mq_notify`, and `mq_getsetattr`.
These are the numbers used by RISC-V and LoongArch, which share the generic table.
The x86_64 numbers 240 through 245 are not accepted because 242 and 243 collide with `accept4` and `recvmmsg` in this ABI.

The raw `mq_open` and `mq_unlink` calls take a nonempty name without `/`, up to 255 bytes.
Libc removes the leading slash from POSIX queue names before issuing these calls.
The registry belongs to the compatibility service, so all Linux processes it manages see the same queues.
A queue records its creator's effective user and group IDs, its permission bits after umask, `mq_maxmsg`, and `mq_msgsize`.
`mq_open` accepts `O_RDONLY`, `O_WRONLY`, or `O_RDWR`, together with `O_CREAT`, `O_EXCL`, `O_NONBLOCK`, and `O_CLOEXEC`.
An existing queue is reopened when the requested read or write access is allowed by its owner, group, or other bits; root bypasses the check.
`O_CREAT` without `O_EXCL` on an existing name ignores the supplied attributes, and `O_CREAT | O_EXCL` returns `EEXIST`.
When `O_CREAT` is given without an attribute buffer, the queue uses 10 messages and 8,192 bytes per message.
Explicit attributes must have positive `mq_maxmsg` and `mq_msgsize` values, currently bounded at 1,024 messages and 65,536 bytes.

Each message stores its priority and data.
`mq_timedsend` rejects data longer than `mq_msgsize` with `EMSGSIZE` and priorities at or above 32,768 with `EINVAL`.
Queues deliver higher priorities first and preserve FIFO order between equal priorities.
`mq_timedreceive` requires an output buffer at least as large as `mq_msgsize` and reports the message length.
It also writes the priority when the caller supplies a priority pointer.
A full queue or an empty queue returns `EAGAIN` on a nonblocking description, and otherwise saves the calling thread as a pending operation.
Pending sends and receives accept an absolute `CLOCK_REALTIME` timeout, which the service converts through the RTC session into a monotonic deadline.
Expiry returns `ETIMEDOUT`; a delivered signal returns `EINTR`.
Invalid timeout values return `EINVAL`, and a timeout request without an available RTC session returns `EOPNOTSUPP`.

`mq_getsetattr` reports `mq_flags`, `mq_maxmsg`, `mq_msgsize`, and the current message count, and changes only the `O_NONBLOCK` flag.
The flag belongs to the open file description, so duplicated and inherited descriptors observe the change together, while separate `mq_open` calls keep independent flags.
`mq_unlink` removes the name when called by the owner or root; other callers receive `EACCES`.
Open descriptors continue to send and receive after unlink, and the queue object is destroyed when both the registry and the last descriptor release it.
A queue that remains linked after its last descriptor closes keeps its queued messages for the next open.
`fork` copies the descriptors by reference and `dup` shares the same description.
`mq_open` reports the usual `EFAULT`, `ENOENT`, `EEXIST`, `EINVAL`, `EACCES`, `EMFILE`, and `ENAMETOOLONG` failures.

`mq_notify` registers one one-shot notification per queue.
`SIGEV_SIGNAL` validates the signal number and records the signal value, while `SIGEV_NONE` consumes the registration without delivering a signal.
A second registration returns `EBUSY`, and a null event removes the caller's registration.
When a message is added to an empty queue and no receiver is blocked, the service raises the signal through the normal process signal queue; registration on an already nonempty queue delivers immediately.
`SIGEV_THREAD` and `SIGEV_THREAD_ID` return `EOPNOTSUPP` because running a guest callback thread is not implemented, and invalid notification kinds or signal numbers return `EINVAL`.
Process exit clears registrations owned by that process.

`/dev/mqueue` is a virtual path view backed by the queue registry rather than a mounted filesystem.
Opening the directory returns a read-only directory descriptor whose `getdents64` output lists `.`, `..`, and the current queue names.
Opening `/dev/mqueue/<name>` returns a read-only file that formats the queue state as `QSIZE:<count> NOTIFY:<kind> SIGEV:<signo>` for each read.
`newfstatat` and `statx` report the directory mode or the queue's synthetic inode, permission bits, and attribute-file size; an unknown name returns `ENOENT`.
`unlinkat` with a queue path performs the same removal as `mq_unlink`.
Writing an attribute file returns `EROFS`, opening it for writing returns `EROFS`, and the virtual directory cannot be removed.
Because this is a path view, it currently has no mount operation or device number beyond the line it adds to `/proc/mounts`.

## Network Sockets

Linux programs can communicate through IPv4 and IPv6 TCP and UDP on the local loopback interface.
`AF_INET` uses `127.0.0.1`, and `AF_INET6` uses `::1`.
The service translates Linux addresses and flags into requests to net.
External interfaces and pathname Unix domain sockets remain future work.
IPv6 sockets are IPv6-only. IPv4-mapped addresses return `EAFNOSUPPORT`, and nonzero scope identifiers return `EINVAL`.
External destinations return `ENETUNREACH`, and binding an external address returns `EADDRNOTAVAIL`.
Linux IPv6 addresses use the 28-byte `sockaddr_in6` layout. Endpoint queries report this full length even when the output buffer is shorter.
Flow information is ignored on input and zero on output.

The socket calls include `socket`, `bind`, `listen`, `accept`, `accept4`, `connect`, `getsockname`, `getpeername`, and `shutdown`.
Data passes through `read`, `write`, `readv`, `writev`, `sendto`, `recvfrom`, `sendmsg`, or `recvmsg`.
The service reads Linux message headers and iovecs field by field and translates addresses separately from native and C layouts.
Ancillary data and unsupported message flags return `EOPNOTSUPP`.
The supported socket options are listed below.
Reading `SO_ERROR` consumes the pending error, while readiness queries leave it available.

| Option | Behavior |
|---|---|
| `SO_TYPE`, `SO_ERROR` | Read the type or consume the pending error |
| `SO_REUSEADDR`, `TCP_NODELAY` | Read or change the boolean setting |
| `SO_KEEPALIVE` | Read or change keepalive enablement |
| `TCP_KEEPIDLE`, `TCP_KEEPINTVL`, `TCP_KEEPCNT` | Set idle seconds, interval seconds, and probe count |
| `SO_RCVBUF` | Set the UDP byte queue limit or query the fixed TCP receive window |
| `SO_SNDBUF` | Query the fixed TCP send capacity or maximum UDP payload |
| `SO_RCVTIMEO`, `SO_SNDTIMEO` | Read or change the receive or send timeout as a Linux timeval |
| `IPV6_V6ONLY` | Read or retain IPv6-only mode. Disabling it returns `EOPNOTSUPP` |

UDP receive buffer requests are doubled with a minimum effective limit of 256 bytes.
The limit applies to queued payload bytes, and a datagram exceeding the remaining capacity is dropped whole.
TCP buffer resizing and send buffer resizing return `EOPNOTSUPP`.
The TCP capacities come from the stack configuration, so their values do not imply Linux buffer accounting.
Keepalive idle and interval values accept 1 through 32,767 seconds, and the probe count accepts 1 through 127.
Loopback tests verify these settings and live traffic. Probe failure behavior across an external interface remains unvalidated.
Unknown options return `ENOPROTOOPT`, and invalid values return `EINVAL`.
Linger remains unsupported.

Timeouts belong to the shared open file description and are inherited by accepted sockets.
A zero timeval disables the timeout.
The receive timeout applies to receive and accept, while the send timeout applies to send and connect.
An expired data or accept operation returns `EAGAIN` if it made no progress.
A connect timeout returns `EINPROGRESS` and leaves the pending connection available for readiness observation.
Completed partial I/O returns its count before considering timeout expiry.
Readiness waits and nonblocking operations use their own waiting rules.
The service waits for any submitted native attempt to finish before applying a timeout or signal, so a committed transfer cannot be replayed.
Calls with a configured timeout return `EINTR` on a signal even when its handler uses `SA_RESTART`.

UDP sends preserve one datagram, including an empty payload.
The payload limit is 65,507 bytes for IPv4 and 65,487 bytes for IPv6 because lwIP stores the IP header within its 16-bit packet length.
An unconnected socket supplies its destination through `sendto` or `sendmsg`.
Connecting a UDP socket sets its default peer, and another connect can replace that peer.
Connecting with `AF_UNSPEC` removes it.
A send without a destination or default peer returns `EDESTADDRREQ`.
The service copies all send buffers before submitting a datagram, so an invalid input cannot publish a partial packet.

Each UDP receive obtains a whole datagram and its source address.
A short output buffer discards the remaining payload after a successful copy.
`MSG_PEEK` keeps the datagram available, and `MSG_TRUNC` returns its full length even when the output buffer is shorter.
`recvmsg` also reports truncation in its output flags.
A zero-length `read` or `readv` leaves a queued datagram untouched, while a zero-length receive consumes it unless `MSG_PEEK` is set.

A socket belongs to its open file description.
Descriptors created by `dup` or inherited through `fork` share the connection and `O_NONBLOCK` flag.
Each descriptor keeps its own close-on-exec flag.
Closing the last reference closes the native session, and pending operations retain their reference until they finish or are abandoned.

The service makes nonblocking network attempts and saves blocking calls as pending operations.
Each waiter has an independent readiness subscription, which also participates in `ppoll` and `pselect6` alongside other descriptor types.
The dispatcher continues serving other Linux threads while a connection or transfer waits.
A nonblocking connect reports `EINPROGRESS`, and the program can wait for write readiness before checking `SO_ERROR`.

The service collects an in-flight attempt before applying signal interruption, so restarting a call cannot repeat a completed transfer or accept.
Received data remains in the open file description if payload delivery fails or if writing the result header or source address fails.
Receive allocation failures preserve the queued datagram for a later attempt.
TCP reads retain bytes beyond a successful partial copy.
UDP reports `EFAULT` if a payload copy faults, including after an earlier iovec was copied, and keeps the whole datagram for a later receive.
A successful short UDP receive consumes the whole datagram.
Blocking reads, writes, and accepts support signal interruption and `SA_RESTART`.
A broken stream write raises `SIGPIPE` unless `sendto` or `sendmsg` supplies `MSG_NOSIGNAL`.

## Unix Socket Pairs

`socketpair` creates two connected `AF_UNIX` endpoints inside the compatibility service without involving the network service.
The supported types are `SOCK_STREAM` and `SOCK_DGRAM` with protocol zero.
`socketpair` accepts `SOCK_NONBLOCK` and `SOCK_CLOEXEC` alongside the type, rejects other Linux domains with `EAFNOSUPPORT`, and reports unsupported types or protocols with `EPROTONOSUPPORT`.
Pathname Unix sockets, `bind`, `listen`, and `accept` remain unsupported; `connect` on an already connected pair returns `EISCONN`.
Endpoint queries report the two-byte `AF_UNIX` family for the unnamed pair.

Each endpoint owns one 64 KiB queue in each direction, so both sides can read and write independently.
A stream transfer may return a short count when the receive queue fills, and blocking writers wait until the peer makes room.
A datagram is accepted whole or not at all, preserves empty payloads, and is limited to 65,507 bytes.
A short datagram receive discards the remaining payload, `MSG_PEEK` preserves the message, and `MSG_TRUNC` reports its complete length.
`MSG_DONTWAIT` applies per call, while `O_NONBLOCK` belongs to the shared open file description.
`MSG_NOSIGNAL` suppresses `SIGPIPE` on a broken stream write, and the other unsupported message flags return `EOPNOTSUPP`.

`shutdown` closes one or both directions of a single endpoint.
After a write shutdown, the local side can still read, and the peer reads queued bytes followed by end of file.
Further writes on the shut-down side return `EPIPE`, as do writes to a peer that shut down its reading side.
Closing the last descriptor for one endpoint has the same effect as shutting down both of its directions: queued data becomes readable first, and later writes to the closed peer fail with `EPIPE`.
`poll`, `pselect6`, and `ppoll` report data, write capacity, end of file, and hangup; a waiting process is woken when the peer transfers data or closes an endpoint.

The socket options `SO_TYPE`, `SO_ERROR`, `SO_RCVBUF`, `SO_SNDBUF`, `SO_RCVTIMEO`, and `SO_SNDTIMEO` apply to a local pair.
Unknown options return `ENOPROTOOPT`, and setting most other options returns `EOPNOTSUPP`.
Timeouts belong to the endpoint's open file description and behave like network socket timeouts without a native attempt to recover.
Endpoint state is shared by duplicated and inherited descriptors, so `dup`, `dup3`, and `fork` observe the same queues, shutdown state, nonblocking flag, and timeouts.
Unnamed endpoints do not participate in the pathname namespace.

`fstat` reports a socket mode and a stable pair identity for both endpoints.

## Address Space

The compatibility layer's AddressSpace records program mappings and their backing page frames and operates the native address space through the supervised Task.
`brk` manages the heap range after the data segment.
`mmap` establishes anonymous or file-private mappings and handles address hints, page alignment, `MAP_FIXED` replacement, and `MAP_FIXED_NOREPLACE`.
Anonymous shared mappings retain a shared backing relationship between the relevant processes.

File-private mappings read content through the file service, allocate and fill the backing page frames, and then map them with the requested permissions.
`munmap` and `mprotect` modify the native mapping and the compatibility layer records.

`mremap` resizes a mapping in place when the adjacent address range is free, and shrinking unmaps the excess tail.
`MREMAP_MAYMOVE` permits relocation: the compatibility layer moves the recorded pages and their frame references instead of copying the contents, and zero-fills any pages added by a growth.
When moving, file-private mappings keep their data and beyond-end-of-file pages keep their fault behavior.
`MREMAP_FIXED` moves a mapping to the requested address, replaces the mapping already present there, and rejects a destination that overlaps the source range.
Invalid flags, sizes, alignments, and addresses are rejected before any mapping changes, and a failure during a move restores the source and any replaced destination pages.
Growing a private file mapping returns `ENOMEM` because the remap path has no synchronous file read, and `MREMAP_DONTUNMAP` returns `EOPNOTSUPP`.
User buffer reads and writes are completed through the target Task's memory interface and converted into Linux address errors.

When duplicating a process, it walks the original address space, copies or shares data according to the mappings' backing relationships, and establishes the child Task's image.
exec reads the new image, constructs a new execution environment, and handles thread, file, and signal state according to exec semantics.
The `execve` path also recognizes a `#!` interpreter line.
It passes the optional argument as one string, followed by the script path and the original arguments after `argv[0]`.
Interpreter scripts may nest up to four levels.

## System V Shared Memory

The compatibility service keeps a service-wide table of System V shared memory segments keyed by identifier and by non-private key.
`shmget` accepts `IPC_PRIVATE`, `IPC_CREAT`, `IPC_EXCL`, and the low nine mode bits, and reports `EEXIST`, `ENOENT`, `EINVAL`, `EACCES`, or `ENOMEM` for the corresponding failure cases.
A private segment is never reachable by key. An existing key returns its current identifier when the requested size does not exceed the segment size.
Segments are limited to 4096 entries and 1 GiB each.

Segment page frames are allocated when the first `shmat` attaches the segment and are retained by the segment record.
Every attachment of a segment maps the same frames, so writes through one attachment are visible through the others and through forked processes.
`shmat` chooses a free address when `shmaddr` is zero, honors `SHM_RND` by rounding a requested address down to the page boundary, and otherwise requires page alignment.
`SHM_RDONLY` maps the segment read-only; a write through that mapping raises `SIGSEGV`.
A failed attachment leaves no mapping or attachment count behind.
`SHM_REMAP` is not supported and returns `EINVAL`.

`shmdt` requires the exact start address of an attachment. It removes the mapping first and then accounts the detachment, so a failure keeps the attachment intact, and it reports `EINVAL` when `munmap` or `mremap` has already moved the mapping. A fixed `mmap` replacement that would overlap an attachment is rejected for the same reason.
A fork copies the parent's attachments into the child and counts each inherited mapping in `nattch`, while a vfork child shares the address space and only inherits the mapping. An inherited attachment can be detached by the child.
exec and process exit release every attachment owned by the process.

`shmctl` implements `IPC_STAT`, `IPC_SET`, `IPC_RMID`, `IPC_INFO`, and `SHM_INFO`.
`IPC_STAT` writes the 112-byte Linux `shmid64_ds` layout, including the kernel `ipc64_perm` layout with a 16-bit sequence field.
`IPC_SET` accepts a new mode and, for a privileged caller, new user and group owners.
Owners and group members follow the segment mode; the root user bypasses the permission checks.
The owner, creator, or root may change or remove a segment.
Changing its ownership requires the corresponding credentials.

`IPC_RMID` marks the segment deleted and removes its key immediately, so a new `shmget` with that key reports `ENOENT` unless it also passes `IPC_CREAT`, which creates a new segment.
Attachments remain usable, and Linux permits another `shmat` while an attachment keeps the removed segment alive.
The identifier becomes invalid when the last attachment detaches.
`IPC_INFO` reports size and count limits in the 64-bit `shminfo` layout.
`SHM_INFO` reports total and resident page counts.
Invalid control commands return `EINVAL`.
`SHM_LOCK`, `SHM_UNLOCK`, `SHM_STAT`, and `SHM_STAT_ANY` remain future work.

## Processes, Threads, and Signals

The PID namespace records PIDs, TIDs, parent-child relationships, process groups, sessions, and exit changes.
`clone` distinguishes process duplication from thread creation by flags and establishes the corresponding resource relationships respectively.
A process clone with `CLONE_VM | CLONE_VFORK` shares the parent's page frames and suspends the calling parent thread until the child executes a new image or exits.
`wait4` and `waitid` observe child process changes, and thread exits and process group exits are handled along their respective paths.
When requested, wait calls return the child's recorded thread execution time in the user-time fields of `rusage`.
The peak resident set size is reported in KiB.
The remaining resource counters are zero.

A thread stores the clear-tid address, its futex wait state, and the address of an optional robust list head.
futex combines user word validation, wait registration, wakeup, and timeout into compatibility layer operations.
`set_robust_list` records the 24-byte `struct robust_list_head`, and `get_robust_list` reports that head and length for the calling thread or another thread of the same process.
When a thread exits, the whole thread group exits, or exec replaces the image, the service walks the registered list, marks each futex word still owned by the thread with `FUTEX_OWNER_DIED`, and wakes one waiter.
The walk follows the head's `futex_offset`, also handles the pending list operation that covers the lock-acquire race, and stops when user memory is unreadable or the Linux walk limit is reached.
Fork and clone do not transfer a registration to the child thread, and the thread created by exec starts without one.
A woken `FUTEX_WAIT` returns zero, and the guest inspects the futex word for `FUTEX_OWNER_DIED` before recovering the mutex.
The registering call rejects lengths other than 24 with `EINVAL` and unreadable heads with `EFAULT`, while the query reports `ESRCH` for unknown thread IDs and `EFAULT` for unwritable result pointers.

The signal implementation maintains the process-shared disposition and each thread's mask, pending set, and alternate stack.
It supports `kill`, `tkill`, `tgkill`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigpending`, `rt_sigsuspend`, `rt_sigtimedwait`, `sigaltstack`, and `rt_sigreturn`.
On delivery it constructs the signal frame for the current architecture, and on return it restores the user state.
Blocking operations return `EINTR` or execute the restart path according to the delivery action.
A restarted timed futex wait keeps its original deadline.
The signal frame retains that deadline while the handler runs, so futex calls from the handler use their own timeouts.
Realtime waits read the clock again before calculating the remaining duration.

The identity interfaces provide PID, TID, process group, session, and credential queries.
A process starts with root credentials and passes its real, effective, and saved user and group IDs to its children.
The `setuid`, `setgid`, `setreuid`, `setregid`, `setresuid`, and `setresgid` calls update these IDs according to the caller's privilege and saved identities.
Supplementary group lists are currently empty, and `setgroups` accepts an empty list only for root.

`prlimit64` queries and updates the soft and hard limits of the calling process.
The `RLIMIT_NOFILE` soft limit bounds descriptor allocation, `RLIMIT_FSIZE` truncates writes at the limit and reports `EFBIG` afterwards, `RLIMIT_AS` rejects mappings and heap growth beyond the limit, and `RLIMIT_STACK` bounds the initial stack of an executed image.
`RLIMIT_CPU` sends `SIGXCPU` at the soft limit and `SIGKILL` at the hard limit.
Requests for another process return `ESRCH`.
Other resource kinds are recorded without enforcement.

## Scheduling, Affinity, and Memory Barriers

A Linux thread carries its scheduling policy and nice value in the compatibility layer, while the kernel ready queue applies the nice value as the priority.
`sched_setscheduler` accepts `SCHED_OTHER`, `SCHED_BATCH`, and `SCHED_IDLE`, and `sched_getscheduler` reports the stored policy.
Real-time `SCHED_FIFO` and `SCHED_RR` requests return `EINVAL` because the kernel does not provide static real-time priority bands yet, and the reset-on-fork policy flag is not supported.
The policy calls validate the target thread and the `struct sched_param` pointer; non-real-time policies require a zero `sched_priority`.
`sched_getparam` writes that zero priority, and `sched_setparam` accepts only a zero priority.
`SCHED_BATCH` omits the normal wake-up credit.
`SCHED_IDLE` uses a fixed minimal fair weight.

`setpriority` and `getpriority` use `PRIO_PROCESS` targets and the Linux reporting convention `20 - nice`.
Values below -20 or above 19 are clamped rather than rejected. Only `PRIO_PROCESS` is implemented; the `PRIO_PGRP` and `PRIO_USER` target kinds return `EINVAL`.
The nice value belongs to one thread. A process clone and a thread clone inherit the calling thread's policy and nice value, and `execve` preserves both for the replaced thread.

Arona runs on one processor, so CPU affinity requests must include CPU 0.
`sched_setaffinity` requires a mask of at least eight bytes and inspects only the first eight; a mask excluding CPU 0 returns `EINVAL`, while bits for CPUs 64 and above in a longer mask are ignored.
`sched_getaffinity` requires at least eight bytes, writes only those eight bytes, returns that byte count, and reports CPU 0.
The kernel itself keeps no per-thread CPU mask, so these calls report the real single-processor topology instead of pretending to support more CPUs.

`membarrier` is also scoped to this uniprocessor.
The query command returns the supported command mask `543`, covering `GLOBAL`, `GLOBAL_EXPEDITED`, `PRIVATE_EXPEDITED`, the global and private registration commands, and `GET_REGISTRATIONS`.
`GLOBAL`, `GLOBAL_EXPEDITED`, and a registered `PRIVATE_EXPEDITED` call execute a sequentially consistent hardware fence plus compiler barriers before returning zero, which is the ordering a per-CPU barrier would provide when all threads share the only CPU.
Using `PRIVATE_EXPEDITED` without the private registration returns `EPERM`.
Registration state belongs to the process, is inherited across fork, and is cleared by exec.
A nonzero flags argument returns `EINVAL`; the core-serializing and rseq commands are outside the current scope and return `EOPNOTSUPP`, while unknown commands return `EINVAL`.
The call never reports success without an ordering operation or a valid registration requirement.


## Terminals and Shell

The startup request delivers UART input and output sessions, and the FileTable establishes standard streams for the program.
The terminal layer stores termios, window size, input buffer, controlling session, and foreground process group.
The line discipline handles canonical and raw input, echo, erase, line ending, EOF, and control character signals.
The read path also handles `VMIN`, `VTIME`, and non-blocking mode.
When `IUTF8` is enabled, canonical erase removes a complete valid UTF-8 character.
With `IUTF8` disabled, it removes one byte.
The reprint character redraws the pending input line when extended processing and echo are enabled.

In canonical mode, input characters first enter the line buffer, and a line is delivered to the read request only after a carriage return ends it, while backspace modifies the line being edited.
Raw mode handles characters directly according to the termios settings, and read timing is determined by parameters such as `VMIN` and `VTIME`.

Terminal ioctls query and modify attributes, window, and controlling terminal state.
Exclusive mode blocks new opens of the controlling terminal, and injected input follows the line discipline.
Stopping output blocks ordinary writers until it resumes and makes nonblocking writes return `EAGAIN`.
Drain and POSIX break requests wait for queued output.
The UART interface does not generate a break signal.
The foreground process group determines the recipients of terminal control character signals, and the session and process group state work with the Shell to complete job control.
Terminal output is finally submitted through devmgr's UART session.

## Time and System Information

The monotonic clock and sleep use native time capabilities, and realtime queries are obtained through the RTC session.
`/dev/rtc`, `/dev/rtc0`, and `/dev/misc/rtc` are virtual character devices backed by that session, and BusyBox `hwclock` opens them in this fallback order.
They support `open`, `read` of one 36-byte `struct rtc_time`, and the `RTC_RD_TIME` and `RTC_SET_TIME` ioctls.
`read` returns UTC fields in binary form with a 24-hour clock, months counted from zero, and years since 1900.
`RTC_SET_TIME` requires effective user ID 0 and returns `EPERM` otherwise, while malformed calendars return `EINVAL`.
The `RTC_AIE`, `RTC_UIE`, `RTC_PIE`, and `RTC_WIE` controls return `EINVAL` because alarm and interrupt support is not implemented, and other ioctls return `ENOTTY`.
`clock_gettime`, `clock_getres`, `gettimeofday`, `nanosleep`, and `clock_nanosleep` handle time requests within their respective parameter ranges.
`getrandom` obtains numbers through the random number device session.
`uname` returns Linux compatibility identifiers, the Arona version, and the machine type.

## Interval Timers

`setitimer` and `getitimer` provide three process interval timers: `ITIMER_REAL` counts wall time and raises `SIGALRM`, `ITIMER_VIRTUAL` counts CPU time and raises `SIGVTALRM`, and `ITIMER_PROF` counts CPU time and raises `SIGPROF`.
Each process owns at most one timer of each kind.
An `it_value` of zero disarms the timer and clears `it_interval`.
A nonzero `it_interval` reloads the timer after each expiry, and `getitimer` reports the remaining time until the next expiry.
The timer output preserves the interval and truncates the remaining time to microseconds.
`setitimer` also returns the previous value when requested, which includes the remaining time that has already elapsed.
The replacement takes effect before the old value is written, matching Linux when the output pointer faults.
An unknown `which`, a negative time value, or a microsecond field outside `0..1_000_000` returns `EINVAL`; a null `getitimer` output or an unreadable timer structure returns `EFAULT`.
A null `setitimer` input is treated as an all-zero value.

A real timer uses a one-shot native timer armed for the remaining wall time.
When it expires, the service queues the signal through the normal process signal queue and rearms the native timer for the interval.
A CPU timer accounts the process CPU time reported by the kernel and arms a native wake-up for the remaining CPU time while at least one thread is runnable.
Blocked processes keep their remaining CPU time and do not consume it, and their wake-up timer is removed, so a CPU timer does not generate periodic wake-ups or poll.
Wall-time and CPU-time expiry both use the same pending queue and signal delivery path as `kill`, including masking, handlers, and the default actions.
The interval timers are process state rather than thread state: an exiting thread does not change them.
A child created by `fork` starts without interval timers, while the timers of the calling process survive `execve`.

Both CPU timers use the same accounting basis: the kernel records one execution-time counter per thread, and the process total sums its finished threads with the current time of its live threads, including the interval that is currently being dispatched.
`ITIMER_VIRTUAL` therefore includes kernel time consumed by the process instead of counting user mode alone, and `ITIMER_PROF` uses the same value.
The service records a CPU baseline when a timer is installed, reloaded, or replaced, so only time after installation counts.
