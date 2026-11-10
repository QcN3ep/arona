# Runtime 概览与编程模型

原生服务从普通的 Rust `main` 开始执行，可以创建 `Vec`、启动闭包线程，再通过通道访问其他服务。
程序入口、分配器和线程环境由 `arona-runtime` 提供，内核能力也通过它封装成 Rust 接口。
init、devmgr、logger、fs 和 linux 都以 `runtime` 的 crate 名使用这个库。

runtime 随程序链接到用户地址空间。
例如 `Vec` 扩容时，它向内核申请页框并建立用户映射；发送消息时，它编码参数并执行原生系统调用。
程序的 Rust 对象与内核资源由这些封装连接起来。

## Runtime 的两类构建

runtime 是使用 `alloc` 的 `no_std` 库。
默认构建提供内核调用接口，`program` feature 为可执行程序加入入口和运行环境：

| 构建方式 | 提供的内容 |
|---|---|
| 默认 feature | 对象与句柄、内存和线程的底层接口、IPC、设备、DMA、Trap 和内核日志接口，以及子 Task 构建辅助类型 |
| `program` 且目标系统为 `none` | 额外提供程序入口、panic 和分配失败处理、全局堆、当前程序环境、TLS，以及安全线程创建与回收 |

系统服务的 `baremetal` feature 会启用 `runtime/program`。
各服务 Cargo 配置中的 `baremetal` feature 选择对应二进制的构建方式。

## 从入口到 main

加载器建立程序的段映射、用户栈和初始线程，把 `ProcessStart` 的地址交给 `_start`。
这份记录含当前 Task 句柄与可选的 bootstrap 通道。
runtime 接收记录，建立初始线程的 TLS，再通过编译器的启动入口调用无参数的 `main`。

运行中的程序通过 `runtime::process::task()` 使用自身 Task，通过 `runtime::process::bootstrap()` 取走启动通道。
bootstrap 是一次交付的资源，取出后由程序持有。

bootstrap 通道中的消息由启动方和被启动程序约定。
init 从中接收启动归档、设备集合和内核日志句柄，devmgr 则接收设备目录及管理连接。
程序入口和 Cargo 配置见[编写原生程序](programs.md)。

## 返回、panic 与退出

`main` 返回时，runtime 先运行当前线程登记的 TLS 析构，再结束 Task。
返回类型决定退出状态：

| 返回类型 | 退出状态 |
|---|---|
| `()` | 0 |
| `usize` | 返回值 |
| `Result<T, E>` | `Ok` 使用 `T` 的退出状态，`Err` 使用 1 |

`Result` 的实现要求 `E: Debug`，错误值转换为退出状态 1。
日志由程序通过 logger 记录，返回路径负责状态交接。
panic 和全局分配失败同样以状态 1 结束 Task。

`process::exit` 和 `thread::exit` 都直接结束控制流，不展开 Rust 栈。
这两个接口直接进入终止路径，当前栈的普通析构和 TLS 析构由正常返回路径处理。
工作线程正常返回时，runtime 执行 TLS 析构并保存返回值，供 `join` 取出。

## 堆与线程

启用程序运行环境后，`alloc` 中的 `Box`、`Vec` 等容器使用 runtime 提供的全局分配器。
该分配器采用 `dlmalloc`，需要扩大堆时向当前 Task 申请页框并建立用户映射，释放完整底层区域时撤销映射。
启动环境随自身 Task 句柄交付这些操作所需的权限。

`runtime::thread::spawn` 接收满足 `Send + 'static` 的闭包，返回 `JoinHandle`。
运行时负责建立线程栈、保护页和 TLS，并在线程停止使用这些资源后回收它们。
`JoinHandle::join` 等待线程结束并取得正常返回值；丢弃 `JoinHandle` 会分离线程，退出后的资源由运行时回收。

这些线程使用同一个 Task 的地址空间。
`JoinHandle` 将内核 Thread 句柄与闭包、返回值、栈和 TLS 一起管理，具体过程见[程序运行环境](environment.md)。

## 对象与句柄接口

`Handle<T>` 拥有当前 Task 中某个对象句柄的所有权。
`T` 表示对象类型，例如 `Task`、`Thread`、`Channel` 或 `FrameAllocation`。
丢弃 `Handle<T>` 会尝试关闭句柄。

`BorrowedHandle<'a, T>` 借用已有句柄，不负责关闭。
`AnyHandle` 用于接收类型在消息到达后才能确定的句柄，可以检查对象类型并通过 `downcast` 转为具体类型。
复制句柄使用 `try_clone(rights)`，需要内核确认源句柄支持复制且新权限不超出源权限。

`T` 在 Rust 中表达对象类型，句柄中的权限则由内核逐次检查。
这套所有权与权限的使用方式见[使用内核能力](capabilities.md)。

## IPC 与服务接口

runtime 提供通道消息、句柄传递、同步调用、异步调用、事件、定时器和多对象等待。
调用方可以选择轮询、无限等待或绝对截止时间。

消息中的 `tag` 和数据字的含义由具体服务约定。
文件、日志与设备服务的协议和客户端封装位于各自的 crate 中，runtime 提供它们共同使用的传输机制。

`Channel::call` 用 `CallError` 区分请求提交前后的错误，同时表达待转移句柄是否已经交出。
请求回复与事件循环的示例见[IPC 编程](ipc.md)。

普通接口使用 `runtime::Error` 返回错误：

| 错误 | 含义 |
|---|---|
| `Error::Kernel` | 内核返回的对象、权限、地址、资源或等待错误 |
| `Error::Protocol` | 内核返回值或交换数据不符合当前接口约定 |

服务客户端还可能定义自己的错误类型，用于表示文件不存在、设备状态或服务协议错误等结果。

## 原生 Rust 与 Linux 程序

原生服务使用 `core`、`alloc` 和 runtime 提供的接口。
Linux 静态程序由兼容服务加载，使用程序自身链接的语言运行时，通过 Linux ABI 请求系统功能。
两类程序都运行在内核 Task 中，入口和调用约定分别由各自的加载路径建立。
