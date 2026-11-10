# 支持情况

Arona 的两个运行平台使用同一套内核机制和用户态服务。
架构代码处理启动交接、页表和中断等机器差异，程序通过相同的原生接口访问系统。

## 运行平台

| 平台 | Rust 目标 | 固件交接 | 中断与时间 |
|---|---|---|---|
| QEMU RISC-V 64 `virt` | `riscv64gc-unknown-none-elf` | 默认 BIOS、hart 标识和 DTB | PLIC、time 计数器与 SBI TIME |
| QEMU LoongArch 64 `virt` | `loongarch64-unknown-none` | `-kernel` 系统表、DTB 与 initrd 描述 | LoongArch 外部中断路由、稳定计数器与定时 CSR |

xtask 默认启动单处理器、256 MiB RAM 的模拟机器。
两个平台均使用高半区内核地址、正式页表、用户地址空间和标量浮点上下文保存恢复。

## 内核机制

| 类别 | 已有能力 |
|---|---|
| 内存 | 页框分配、引用保活、地址保留、映射、权限变更、查询、direct map、内核堆与栈 |
| 执行 | Task、用户与内核线程、FIFO 就绪队列、等待唤醒、退出回收、抢占请求和截止时间 |
| Trap | 统一事件分类、外部中断认领、用户状态、TrapPort 与 continuation |
| IPC | 消息、句柄转移、同步与异步调用、一次回复、Event、Timer、多对象等待 |
| 能力 | 类型化对象、逐方法权限、句柄复制与缩减、跨 Task 安装 |
| 设备 | 设备目录、资源组、用户 MMIO、中断事件、PCI 枚举、DMA 映射与启停 |
| 日志 | `log` 门面、早期 Sink、环形记录、时间戳和游标读取 |

## 原生 Rust 程序

runtime 提供普通 `main` 入口、`core` 与 `alloc` 运行环境、全局用户堆、TLS、闭包线程和 JoinHandle。
程序通过 runtime 调用内核对象，通过服务客户端访问设备、文件和日志。

五个系统服务都是原生 ELF 程序，随构建打包到 newc 归档。
归档支持内嵌内核 ELF 和独立 `-initrd` 加载。
init 负责按 `/etc/init.toml` 启动应用和执行重启策略。

## 设备与文件

| 类别 | 实现 |
|---|---|
| 串口 | NS16550 系列 MMIO UART、配置、收发与中断会话 |
| 块设备 | 现代 VirtIO MMIO 和 PCI block、读写、完成确认与 flush |
| 随机数 | 现代 VirtIO PCI RNG |
| RTC | Goldfish 与 Loongson LS7A 时间读取 |
| 根文件树 | cpio newc 归档中的文件、目录与链接 |
| 持久化文件 | 4 KiB 块 ext4、extent、目录修改、JBD2 事务与恢复 |
| 日志文件 | logger 记录追加、历史恢复、尾部处理与 fsync |

devmgr 从启动硬件描述和 PCI 枚举结果中匹配这些设备，取得资源授权后建立驱动实例。
ext4 的磁盘布局和事务处理见[文件服务](../services/fs.md)。

## Linux 程序环境

Linux 兼容服务接受当前机器的静态 ELF64 `ET_EXEC`。
它提供文件、目录、管道、终端、进程与线程、信号、内存映射、时钟、随机数等调用路径。
调用与输入形式的具体范围见[Linux 兼容服务](../services/linux.md)。

交互式终端使用 UART 会话和兼容层行规程。
Shell 及外部命令由外部根目录打包，init 配置选择执行路径与终端。
仓库中的集成程序验证各机制，开发环境中的 BusyBox 可通过相同装载路径运行。
