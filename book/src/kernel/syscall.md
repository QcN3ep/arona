# 原生系统调用

原生程序调用 `event.signal(mask)` 时，runtime 将 Event 句柄、SIGNAL 方法和位掩码放入寄存器，再陷入内核。
内核从句柄取得对象，检查方法权限，执行操作并返回结果。
其他原生系统调用也使用这套“句柄、方法、参数”的结构。

`abi` 定义双方共同使用的编码，runtime 提供 Rust 封装。
文件、日志和设备服务在这层机制之上，通过各自的 IPC 协议提供操作。

## 寄存器约定

两种架构使用相同的参数布局：

| 寄存器 | 输入 | 输出 |
|---|---|---|
| `a7` | 目标句柄 | — |
| `a0` | 对象方法 | 状态码，0 表示成功 |
| `a1` | 参数 1 | 结果字 1 |
| `a2` | 参数 2 | 结果字 2 |
| `a3` 至 `a5` | 参数 3 至 5 | — |

RISC-V 使用 `ecall`，LoongArch 使用 `syscall 0`。
较大的结构通过参数中的用户地址传递，内核按 ABI 记录读取或写入。
记录使用明确的字段编码，调用返回值由 `abi::Return` 表达。

## 分派路径

1. 架构 Trap 入口保存状态，将句柄和参数转换为 `TrapEvent::Syscall`。
2. 内核取得当前线程与所属 Task。
3. 根据句柄取得对象类型与权限，或解析 `CURRENT_THREAD`。
4. 查询 `method::required_rights`，检查方法与权限。
5. 执行通用句柄方法，或进入对象类型对应的处理模块。
6. 返回状态与两个结果字，恢复到 syscall 后继地址。

原生方法分派集中在 `kernel/src/syscall/mod.rs`。
Linux 程序的 syscall 经 TrapPort 转交兼容服务，在该服务中按 Linux 约定解释。

## 方法分组

所有对象共用 `INFO`、`CLOSE` 和 `DUPLICATE`。
对象专用方法从 16 开始，按类型复用编号。

| 类型 | 专用操作 |
|---|---|
| Task | 分配页框，映射、保留和查询地址，读取或写入目标内存，创建 Task、Thread、Channel、Event、Timer，安装句柄，设置 Trap，终止与等待 |
| Thread | 状态、启动、终止、等待、退出、让出、休眠、时间、标识、用户状态设置、中断与多对象等待 |
| FrameAllocation | 大小、读取与写入 |
| Channel | 发送、接收、同步调用、启动异步调用 |
| Call、Reply | 接收结果、取消调用、发送一次回复 |
| Event、Timer | 合并位通知、设置或取消截止时间 |
| DeviceSet | 枚举、描述、读取字段、授予设备组 |
| Device | 映射、解除映射、中断绑定、解绑、确认、撤销、创建 DMA 对象 |
| DmaDomain | 状态、映射、解除映射、启用、静止与撤销 |
| TrapPort、TrapContinuation | 接收事件、读取用户状态、恢复 |
| KernelLog | 按游标读取记录 |

方法常量和所需权限集中在 `abi/src/method.rs` 中。
具体输入记录分别位于 `abi/src/memory.rs`、`thread.rs`、`device.rs` 等模块。

## 参数、用户内存与错误

处理函数核对参数范围、记录长度、对象类型以及目标状态。
用户内存经调用者地址空间检查并保持后备页框引用。
跨 Task 内存读取或写入还检查目标 Task 对应的内存权限。

| 错误 | 常见含义 |
|---|---|
| `BadHandle`、`WrongType` | 句柄无效或类型不符 |
| `Denied` | 权限不足 |
| `InvalidArgument`、`InvalidState` | 参数或对象状态不允许操作 |
| `BadAddress` | 用户输入输出地址不可按请求访问 |
| `NoMemory` | 资源分配失败 |
| `WouldBlock`、`TimedOut` | 当前无法完成或等待到期 |
| `Closed`、`PeerClosed`、`ServerGone` | 对象或通信方关闭 |
| `BufferTooSmall` | 输出空间不足 |
| `Unsupported` | 方法或请求不属于该接口接受的操作 |
| `Cancelled`、`Busy` | 请求被取消或资源正在使用 |

内部错误在 syscall 边界转换为 `abi::Error`。
用户内存访问使用独立的错误包装，将无效用户地址转换为 `BadAddress`。
CALL 等带提交语义的操作还返回阶段信息，见[IPC 与等待](ipc.md)。
