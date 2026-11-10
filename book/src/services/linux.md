# Linux 兼容服务

运行 Linux Shell 时，程序本身位于独立的受监督 Task 中，linux 服务负责解释它发出的 Linux 系统调用。
打开文件会连接到 fs，终端输入输出会连接到 devmgr，而进程标识、fd、信号和 Linux 地址空间记录保存在兼容服务中。
内核通过 TrapPort 把程序的调用和异常交给它处理。

## 启动一个 Linux 程序

init 向服务提供文件客户端和可选的 RTC、随机数会话。
执行请求包含程序路径、argv、envp 和终端会话。
`Control::start_run` 返回异步调用，程序结束后以退出状态完成该请求。

装载流程按阶段进行：

1. 通过文件服务打开程序并查询元数据，检查可执行权限。
2. 读取文件，校验与当前机器匹配的 ELF64 `ET_EXEC`。
3. 为可加载段分配页框并复制数据，建立 R、RW 或 RX 映射。
4. 构造用户栈中的 argc、argv、envp 和 auxiliary vector。
5. 创建受 TrapPort 监督的 Task 和初始线程，设置寄存器后启动。

ELF 检查包含段边界、页范围不重叠以及入口处于可执行区域。
初始栈带保护页，数据区包含参数字符串、环境、程序头位置及 auxiliary vector。
取得随机数会话时，还为启动栈生成 16 字节随机数据并交接 `AT_RANDOM`。

## Trap 与异步分派

Linux 程序陷入时，内核发布 TrapInfo 和 continuation，暂停发起调用的线程。
兼容服务根据机器类型从 UserState 中取得 Linux 系统调用号与参数，再分派到对应模块。
完成时将返回值或负 errno 写入寄存器，并通过 continuation 恢复执行。

例如，Shell 等待一行终端输入时，`read` 对应的 continuation 与输入请求一起保存为 Pending。
UART 有字符到达后，服务继续处理这次读取，把数据和返回值写回程序，再恢复执行。
文件、子进程、futex 和时间等待也以待完成操作保存。
主循环继续处理控制请求、其他程序的 Trap 和已挂起操作。
没有进展时，把相关 Call、事件、线程退出与端口加入多对象等待。

| 实现分组 | 对应内容 |
|---|---|
| `syscall/file/` | 打开、I/O、路径、目录、元数据、管道、poll 与终端 |
| `syscall/process/` | clone、exec、退出、等待、身份、线程与 futex |
| `syscall/memory/` | mmap、munmap、mprotect 和相关内存操作 |
| `syscall/signal.rs` | 信号系统调用 |
| `syscall/system/` | uname、时钟、休眠与随机数 |

分派器遇到未匹配的调用返回 `ENOSYS`。
已识别调用的参数、标志和错误转换由其处理模块检查。

## 文件描述符与路径

FileTable 将 Linux fd 映射到打开文件描述。
描述持有文件服务 File、Directory、管道端或终端，复制 fd 共享对应打开状态。
每个 fd 还保存 close-on-exec 等描述符属性。

| 操作组 | 已实现的调用 |
|---|---|
| 打开与描述符 | `openat`、`close`、`dup`、`dup3`、`fcntl` |
| 字节 I/O | `read`、`write`、`pread64`、`pwrite64`、`readv`、`writev`、`preadv`、`pwritev` |
| 文件状态 | `fstat`、`newfstatat`、`statx`、`lseek`、`ftruncate`、`fsync`、`fdatasync` |
| 目录与路径 | `getdents64`、`chdir`、`fchdir`、`getcwd`、`mkdirat`、`unlinkat`、`readlinkat`、`symlinkat`、`linkat`、`renameat`、`renameat2` |
| 等待与管道 | `pipe2`、`ppoll`、`pselect6` |

路径操作把当前目录或 dirfd 转为文件服务的目录锚点。
权限模式、umask 和打开标志在兼容层解释，具体文件内容及持久化由文件服务完成。
例如一次普通文件读取，兼容服务向 fs 提交偏移和输出缓冲，取得结果后将数据写入目标 Task，并按实际完成长度更新打开文件的偏移。

## 地址空间

兼容层 AddressSpace 记录程序映射及其后备页框，通过受监督 Task 操作原生地址空间。
`brk` 管理数据段之后的堆范围。
`mmap` 建立匿名或文件私有映射，处理地址提示、页对齐和 `MAP_FIXED_NOREPLACE`。
匿名共享映射在相关进程间保留共享后备关系。

文件私有映射通过文件服务读取内容，分配并填充后备页框，再按请求权限映射。
`munmap` 和 `mprotect` 修改原生映射及兼容层记录。
用户缓冲读写通过目标 Task 的内存接口完成，并转换为 Linux 地址错误。

进程复制时遍历原地址空间，依据映射的后备关系复制或共享数据，建立子 Task 的映像。
exec 则读取新映像、构造新执行环境并按 exec 语义处理线程、文件与信号状态。

## 进程、线程与信号

PID 命名空间记录 PID、TID、父子关系、进程组、会话和退出变化。
`clone` 按标志区分进程复制与线程创建，分别建立对应的资源关系。
`wait4` 和 `waitid` 观察子进程变化，线程退出与进程组退出沿各自路径处理。

线程保存 clear-tid 地址和 futex 等待状态。
futex 将用户字校验、等待登记、唤醒与超时组合为兼容层操作。

信号实现维护进程共享的 disposition 和各线程的掩码、pending 与备用栈。
支持 `kill`、`tkill`、`tgkill`、`rt_sigaction`、`rt_sigprocmask`、`rt_sigpending`、`rt_sigsuspend`、`rt_sigtimedwait`、`sigaltstack` 和 `rt_sigreturn`。
交付时构造当前架构的信号 frame，返回时恢复用户状态。
阻塞操作根据交付动作返回 `EINTR` 或执行重启路径。

身份接口提供 PID、TID、进程组和会话查询。
UID、EUID、GID 和 EGID 查询返回 0。

## 终端与 Shell

启动请求交付 UART 输入和输出会话，FileTable 为程序建立标准流。
终端层保存 termios、窗口大小、输入缓冲、控制会话和前台进程组。
行规程处理 canonical 与 raw 输入、回显、擦除、行结束、EOF 和控制字符信号。
读取路径还处理 `VMIN`、`VTIME` 与非阻塞模式。

在 canonical 模式下，输入字符先进入行缓冲，回车结束一行后才交付读取请求，退格则修改正在编辑的行。
raw 模式按 termios 设置直接处理字符，读取时机由 `VMIN`、`VTIME` 等参数决定。

终端 ioctl 查询和修改属性、窗口以及控制终端状态。
前台进程组决定终端控制字符信号的接收者，会话与进程组状态配合 Shell 完成作业控制。
终端输出最终通过 devmgr 的 UART 会话提交。

## 时间与系统信息

单调时钟和休眠使用原生时间能力，realtime 查询通过 RTC 会话取得。
`clock_gettime`、`clock_getres`、`gettimeofday`、`nanosleep` 和 `clock_nanosleep` 在各自参数范围内处理时间请求。
`getrandom` 通过随机数设备会话取数。
`uname` 返回 Linux 兼容标识、Arona 版本和机器类型。
