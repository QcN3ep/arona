# 代码组织

仓库按系统中的职责划分 Cargo crate。
从底层向上阅读时，`abi` 定义交接类型，`kernel` 提供资源机制，`arch` 完成硬件操作并生成内核 ELF；用户态的 runtime 和服务则在这些机制上构建运行环境与系统功能。

## 工作区成员

| 目录 | Cargo 包名 | 主要内容 |
|---|---|---|
| `abi/` | `arona-abi` | 对象类型、权限、方法和跨边界交换格式 |
| `kernel/` | `arona-kernel` | 特权态内核机制及硬件接口定义 |
| `arch/` | `arona-arch` | 硬件接口实现与内核可执行文件 |
| `kernel-tests/` | `arona-kernel-tests` | 通过真实硬件能力执行的内核集成测试 |
| `components/runtime/` | `arona-runtime` | 原生 Rust 运行环境及内核调用封装 |
| `components/init/` | `arona-init` | 基础服务启动和配置应用的监督 |
| `components/devmgr/` | `arona-devmgr` | 设备管理、驱动和设备会话 |
| `components/logger/` | `arona-logger` | 用户日志服务及客户端 |
| `components/fs/` | `arona-fs` | 文件接口、客户端、服务和文件系统实现 |
| `components/linux/` | `arona-linux` | Linux 程序执行与 ABI 兼容服务 |
| `xtask/` | `xtask` | 宿主机构建、打包和运行工具 |

多个 crate 使用简短的 Rust 库名。
例如 Cargo 包 `arona-kernel` 在代码中以 `kernel` 引用，`arona-runtime` 以 `runtime` 引用。
Cargo 命令中的 `-p` 使用包名。

## 核心依赖关系

`abi` 定义内核与程序之间共同使用的约定和类型，`kernel` 和 runtime 分别依赖它。

`arch` 依赖 `kernel`，实现后者定义的硬件接口，并把入口代码和内核机制链接成 `arch` ELF。
具体的 RISC-V 和 LoongArch 指令、寄存器和页表格式分别放在对应架构目录下。
架构之间通用的硬件封装放在 `arch/src/` 的公共模块中。

系统服务依赖 runtime 使用内核能力。
服务 crate 同时提供供其他服务链接的客户端库。
例如 `components/fs/` 中既有 File 客户端，也有 fs 二进制：init 链接前者编码读取请求，后者在自己的 Task 中处理请求。

## 内核目录

| 模块 | 内容 |
|---|---|
| `kernel/src/boot/` | 启动信息、内核初始化、首个程序与启动资源交付 |
| `kernel/src/hardware/` | CPU、MMU、执行上下文、Trap、时钟和设备访问接口 |
| `kernel/src/memory/` | 页框、分配器、映射、地址空间、直接映射和内核堆 |
| `kernel/src/task/` | Task 生命周期、对象与句柄表 |
| `kernel/src/thread/` | 线程、调度、等待、截止时间和用户态执行 |
| `kernel/src/ipc/` | 通道、调用、响应、事件、定时器及多对象等待 |
| `kernel/src/device/` | 设备资源注册、授权、PCI 与 DMA 生命周期 |
| `kernel/src/syscall/` | 原生对象方法的鉴权和分派 |
| `kernel/src/logger/` | 内核日志、Sink 和记录缓冲区 |
| `kernel/src/trap.rs` 与 `trap/` | Trap 分派与用户态异常交付 |

查找一项跨层功能时，可以沿接口继续阅读。
例如，用户映射从 `syscall` 进入 `memory`，页表管理调用 `hardware::mmu`，对应架构的页表项编码位于 `arch`。

## 实现约束与测试位置

实现遵循以下规则：

| 范围 | `unsafe` | 条件编译 |
|---|---|---|
| kernel 实现、kernel-tests、abi | 禁止 | 禁止 |
| `kernel/tests/` | 允许 | 禁止 |
| `components/runtime/` | 允许 | 允许 |
| 其他 `components/` | 禁止 | 禁止 |
| `arch/` | 允许 | 允许 |

Rust 实现中的安全前提和注释规则见[贡献规范](guidelines.md)。

可以在宿主机执行的集成测试放在对应 crate 的 `tests/` 中。
需要真实页表、异常入口、线程切换或设备的测试由架构测试映像和组件程序承载，通用内核场景放在 `kernel-tests`。
测试布局见[测试与调试](../tools/testing.md)。

## 构建文件与文档

根目录的 `Cargo.toml` 维护工作区成员、共享版本、lint 和构建配置。
`Cargo.lock` 固定工作区的依赖解析结果，`.cargo/config.toml` 定义 `cargo xtask` 别名。
架构链接脚本放在 `arch/src/<架构>/link.ld`，原生 Rust 程序的公共链接脚本放在 `components/runtime/link.ld`。

本书位于 `book/`，目录由 `book/src/SUMMARY.md` 定义。
根目录 `CONTRIBUTING.md` 是贡献规范的维护位置，手册通过 include 引入它。
