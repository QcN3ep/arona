# Linux Compatibility Service

When running a Linux Shell, the program itself resides in a separate supervised Task, and the linux service is responsible for interpreting the Linux system calls it issues.
Opening a file connects to fs, terminal input and output connect to devmgr, while process identities, fds, signals, and Linux address space records are kept in the compatibility service.
The kernel hands the program's calls and exceptions to it through a TrapPort.

## Starting a Linux Program

init provides the service with a file client and optional RTC and random number sessions.
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
| `syscall/process/` | clone, exec, exit, wait, identity, thread, and futex |
| `syscall/memory/` | mmap, munmap, mprotect, and related memory operations |
| `syscall/signal.rs` | signal system calls |
| `syscall/system/` | uname, clock, sleep, and random numbers |

The dispatcher returns `ENOSYS` for unmatched calls.
Arguments, flags, and error conversions of recognized calls are checked by their handling modules.

## File Descriptors and Paths

The FileTable maps Linux fds to open file descriptions.
A description holds a file service File, Directory, pipe end, or terminal, and duplicated fds share the corresponding open state.
Each fd also stores descriptor attributes such as close-on-exec.
An eventfd shares a counter across duplicated descriptors and inherited process descriptors.
Reads wait for a nonzero counter, and writes wait for enough room in the counter.
Nonblocking operations report `EAGAIN` when they would wait.
Selected files under `/proc` and `/sys` expose read-only snapshots taken when the descriptor is opened.
These include process status and arguments, mount information, the release version, and the online CPU list.
Virtual character descriptions implement `/dev/null` and `/dev/zero`, while `/dev/random` and `/dev/urandom` read from the configured random number session.

| Operation group | Implemented calls |
|---|---|
| Open and descriptors | `openat`, `close`, `dup`, `dup3`, `fcntl` |
| Byte I/O | `read`, `write`, `pread64`, `pwrite64`, `readv`, `writev`, `preadv`, `pwritev` |
| File status | `fstat`, `newfstatat`, `statx`, `statfs`, `fstatfs`, `lseek`, `ftruncate`, `fsync`, `fdatasync` |
| Directory and paths | `getdents64`, `chdir`, `fchdir`, `getcwd`, `mkdirat`, `unlinkat`, `readlinkat`, `symlinkat`, `linkat`, `renameat`, `renameat2` |
| Waiting and pipes | `pipe2`, `eventfd2`, `ppoll`, `pselect6` |

Path operations convert the current directory or dirfd into a file service directory anchor.
Permission modes, umask, and open flags are interpreted in the compatibility layer, while the actual file content and persistence are handled by the file service.
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

## Address Space

The compatibility layer's AddressSpace records program mappings and their backing page frames and operates the native address space through the supervised Task.
`brk` manages the heap range after the data segment.
`mmap` establishes anonymous or file-private mappings and handles address hints, page alignment, `MAP_FIXED` replacement, and `MAP_FIXED_NOREPLACE`.
Anonymous shared mappings retain a shared backing relationship between the relevant processes.

File-private mappings read content through the file service, allocate and fill the backing page frames, and then map them with the requested permissions.
`munmap` and `mprotect` modify the native mapping and the compatibility layer records.
User buffer reads and writes are completed through the target Task's memory interface and converted into Linux address errors.

When duplicating a process, it walks the original address space, copies or shares data according to the mappings' backing relationships, and establishes the child Task's image.
exec reads the new image, constructs a new execution environment, and handles thread, file, and signal state according to exec semantics.
The `execve` path also recognizes a `#!` interpreter line.
It passes the optional argument as one string, followed by the script path and the original arguments after `argv[0]`.
Interpreter scripts may nest up to four levels.
Service startup requests currently require ELF images.

## Processes, Threads, and Signals

The PID namespace records PIDs, TIDs, parent-child relationships, process groups, sessions, and exit changes.
`clone` distinguishes process duplication from thread creation by flags and establishes the corresponding resource relationships respectively.
A process clone with `CLONE_VM | CLONE_VFORK` shares the parent's page frames and suspends the calling parent thread until the child executes a new image or exits.
`wait4` and `waitid` observe child process changes, and thread exits and process group exits are handled along their respective paths.
When requested, wait calls return the child's recorded thread execution time in the user-time fields of `rusage`.
The remaining resource counters are zero.

A thread stores the clear-tid address and futex wait state.
futex combines user word validation, wait registration, wakeup, and timeout into compatibility layer operations.

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

## Terminals and Shell

The startup request delivers UART input and output sessions, and the FileTable establishes standard streams for the program.
The terminal layer stores termios, window size, input buffer, controlling session, and foreground process group.
The line discipline handles canonical and raw input, echo, erase, line ending, EOF, and control character signals.
The read path also handles `VMIN`, `VTIME`, and non-blocking mode.

In canonical mode, input characters first enter the line buffer, and a line is delivered to the read request only after a carriage return ends it, while backspace modifies the line being edited.
Raw mode handles characters directly according to the termios settings, and read timing is determined by parameters such as `VMIN` and `VTIME`.

Terminal ioctls query and modify attributes, window, and controlling terminal state.
The foreground process group determines the recipients of terminal control character signals, and the session and process group state work with the Shell to complete job control.
Terminal output is finally submitted through devmgr's UART session.

## Time and System Information

The monotonic clock and sleep use native time capabilities, and realtime queries are obtained through the RTC session.
`clock_gettime`, `clock_getres`, `gettimeofday`, `nanosleep`, and `clock_nanosleep` handle time requests within their respective parameter ranges.
`getrandom` obtains numbers through the random number device session.
`uname` returns Linux compatibility identifiers, the Arona version, and the machine type.
