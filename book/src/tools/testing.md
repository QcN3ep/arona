# 测试与调试

Arona 的集成测试从功能入口观察结果和资源状态。
例如，DTB 解析在宿主机读取输入并检查硬件描述，线程切换则在 QEMU 中运行实际上下文，IPC 场景让不同线程完成消息与句柄交接。
这些场景分别由宿主 Cargo 测试、架构测试映像和用户态测试程序承载。

## 宿主机检查与测试

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings --force-warn clippy::too_many_lines
cargo test --locked --workspace --tests
```

`cargo fmt` 检查格式，Clippy 检查 Rust 代码，`cargo test` 执行满足当前目标和 feature 条件的集成测试。
宿主测试包括启动信息解析、硬件绑定和日志接口。
裸机测试镜像通过对应的 `baremetal` 或 `program` feature 构建。

单独运行 DTB 和启动归档解析：

```sh
cargo test --locked -p arona-arch --test boot
```

## 真实架构测试

分页、内核堆、Trap、执行上下文、定时器和中断控制器由 `arch/tests/` 中的测试映像承载。
通用的内存、Task、线程、IPC 与系统调用场景放在 `kernel-tests` 中，由架构入口调用。
runtime 和系统服务的测试程序在用户态运行，使用相同的原生内核接口。

测试镜像分别覆盖 RISC-V 与 LoongArch 的 debug 和 release 构建。
这些场景实际安装异常入口、操作页表、切换执行上下文并访问模拟设备。
例如文件系统场景附加磁盘夹具，串口场景向 QEMU 输入字符，再观察用户态会话的读取结果。


可以使用 `cargo xtask run <目标> [--release]` 启动构建好的映像。
## 日志与故障定位


正常启动日志中，架构入口先输出 CPU 和 DTB 信息，内核随后按初始化顺序输出内存、Trap、中断控制器、定时器与调度器的状态。
`Started init` 表示首个线程已建立并启动；`System services ready` 由 init 在基础服务启动成功后记录。

## 如何选择验证范围

测试应围绕功能、代表性错误和必要边界安排。
涉及错误返回的场景还要检查失败后的资源与状态。
具体原则与测试文件结构要求见[贡献规范](../contributing/guidelines.md)。

验证范围随功能选择：解析改动运行相应输入场景，IPC 改动观察通信和资源交接，硬件实现改动运行对应架构映像。
手册改动则检查命令、示例和导出结果。
