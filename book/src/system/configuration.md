# Boot Configuration and Usage

Once the base services are ready, init reads `/etc/init.toml` and hands each `program` in it to the Linux compatibility service for execution.
An application's arguments, environment, terminal, and restart behavior after exit are all configured here.

## Application Configuration

A configuration that starts an interactive shell looks like this:

```toml
[[program]]
name = "shell"
executor = "linux"
path = "/bin/sh"
args = ["-i"]
terminal = "console"
restart = "always"

[program.env]
PATH = "/bin"
PS1 = "arona$ "
```

The configuration is placed in the external root directory together with the shell and packaged via `--rootfs`.
init runs `/bin/sh -i`, connects the UART as its terminal, and restarts the shell after it exits.

| Field | Meaning |
|---|---|
| `name` | The name init uses to identify and record the application; must be non-empty and unique in the configuration |
| `executor` | The executor; currently only `linux` is accepted |
| `path` | Absolute path of the application |
| `args` | Arguments appended after the application path; defaults to an empty list |
| `env` | Environment variables; defaults to an empty table |
| `terminal` | Optional; currently accepts `console`, which connects the application to console input and output |
| `restart` | Restart policy after exit; defaults to `never` |

init uses `path` as `argv[0]`, then appends `args`.
Each entry in `env` is encoded as `KEY=VALUE` and passed to the execution service; in the example above, `PATH` makes the shell look up external commands in `/bin`, and `PS1` sets the prompt to `arona$ `.

The configuration parser rejects unknown fields, duplicate or empty application names, non-absolute program paths, and NUL bytes in arguments and environment variables.
Environment variable names are non-empty strings that do not contain `=`.
Without `/etc/init.toml`, the application list is empty; if the configuration exists but has invalid syntax or fields, init reports an error and exits.

## Boot Power Action

An image can request one power action after the base services are ready:

```toml
[power]
action = "shutdown"
```

The action runs after init reports `System services ready` and before application supervision starts.
`shutdown` and `reboot` are the accepted values.
init queries the device management service for a platform power controller and reports an error when the platform does not provide the requested action.
Before the action, init stops the Linux compatibility service, then flushes and stops the log service.
It then flushes and unmounts the mounted filesystems before stopping the file service.
The device management service stays available to apply the power action.
This is useful for single-purpose images and for automated boot checks that must stop the machine.

## Application Exit and Restart

| `restart` | Behavior |
|---|---|
| `never` | The application stays stopped after it exits |
| `on-failure` | Restart when the exit code is non-zero or the application is terminated by a signal |
| `always` | Restart regardless of the exit result |

If an application exits right after starting, restarting immediately would make it repeat the startup process continuously.
init applies a backoff for consecutive exits: it starts at 100 milliseconds and grows up to 30 seconds; once a run reaches 30 seconds, the consecutive restart count is reset.
While waiting to restart, the supervision loop keeps handling base service and other application events.

When the supervisor detects that a base service has exited, it stops running, and the service owner cleans up the corresponding Task.

## Shell and Command Files

BusyBox puts multiple commands in a single program and selects which command to run based on the name it is started with.
After linking `/bin/sh`, `/bin/ls`, and `/bin/cat` to `/bin/busybox`, the same program can serve as a shell, a directory viewer, and a file reader respectively.

The shell looks up these external commands in `PATH`, while built-in commands such as `echo` execute directly inside the shell.
The symbolic links in the archive therefore determine which commands can be looked up, and `PATH` determines where to look.
The file tree consists of the archive contents together with the mounts of the file service.

## Default Storage Behavior

The file service uses the boot archive to establish the root file system.
If the device management service provides block devices, init currently selects the first block device enumerated and tries to mount it at `/var` using its full capacity.
The mount reads the ext4 volume from the start of the device.

After a successful mount, init creates or opens `/var/log/system.log` and hands it to the log service.
The default `xtask run` uses the archive root file system and UART log output.
With an ext4 disk attached, `/var` becomes a writable persistent area, and logger's file output is enabled as well.
