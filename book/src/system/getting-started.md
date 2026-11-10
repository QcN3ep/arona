# 快速开始

`cargo xtask run` 完成系统服务构建、initramfs 打包和内核构建，随后启动 QEMU。
第一次运行会编译整个工作区的相关依赖，之后的构建沿用 Cargo 缓存。

## 开发环境

开发环境使用 Linux、Rust 工具链和 QEMU。
内核由当前 Rust 工具链编译，原生系统服务使用 nightly，以构建 runtime 的程序入口和线程本地存储。

| 工具 | 用途 |
|---|---|
| Rust、Cargo 与 rustup | 编译内核、系统服务和构建工具 |
| Rust nightly | 编译使用自定义 Rust 程序入口和 TLS 的原生程序 |
| `qemu-system-riscv64` | 运行 RISC-V 64 映像 |
| `qemu-system-loongarch64` | 运行 LoongArch 64 映像 |
| `clang`、`ld.lld` | 构建兼容性测试程序 |

安装 Rust 工具链与交叉编译目标：

```sh
rustup toolchain install stable --component rustfmt --component clippy
rustup toolchain install nightly --component rustfmt --component clippy
rustup target add --toolchain stable riscv64gc-unknown-none-elf loongarch64-unknown-none
rustup target add --toolchain nightly riscv64gc-unknown-none-elf loongarch64-unknown-none
```

## 构建并启动

RISC-V 和 LoongArch 的启动命令分别为：

```sh
cargo xtask run riscv64 --release
cargo xtask run loongarch64 --release
```

命令依次构建 init、devmgr、logger、fs 和 linux，将它们打包为 cpio newc 格式的 initramfs，再将归档嵌入内核 ELF。
`--release` 同时选择内核与系统服务的 release 配置，省略时使用 debug。

QEMU 使用 `virt` 机器、256 MiB 内存、一个处理器和终端串口输出，并关闭默认网络设备。
RISC-V 使用 QEMU 的默认 BIOS；LoongArch 使用 QEMU 的 `-kernel` 交接路径。

## 启动后会发生什么

串口首先输出架构启动信息，随后出现 `Entering kernel` 和 `Memory initialized`。
这时内核正在建立地址空间、异常入口、中断控制器和调度器。
`Started init` 表示 init 的线程已建立；它运行后继续启动用户态服务，最后输出 `System services ready`。

串口也在这段过程中完成交接：内核启动阶段使用早期控制台，进入调度器前将其分离，用户态 devmgr 随后接管 UART。
logger 通过 devmgr 的会话输出服务日志。

默认归档包含五个系统服务，init 使用空的应用列表等待服务事件。
加入 Shell 时，将 Linux 静态程序和 `/etc/init.toml` 放入外部根目录，再用 `--rootfs` 一起打包。
[启动配置与使用](configuration.md)给出了交互式 Shell 的配置。

## 构建映像

```sh
cargo xtask build riscv64 --release --initramfs embedded
cargo xtask build loongarch64 --release --initramfs external
```

`embedded` 将 initramfs 嵌入内核 ELF，`external` 生成独立归档，启动时由 QEMU 加载。不指定时使用 `embedded`；省略 `--release` 则使用 debug 构建。

产物默认位于对应目标的构建目录：

| 架构 | 目录 |
|---|---|
| RISC-V 64 | `target/riscv64gc-unknown-none-elf/release/` |
| LoongArch 64 | `target/loongarch64-unknown-none/release/` |

其中 `arch` 是内核 ELF，`initramfs.cpio` 是系统服务归档。
设置 `CARGO_TARGET_DIR` 可以修改构建目录。

映像的组成、外部根目录和手动启动方法见[系统映像](images.md)。
完整选项见[xtask 命令参考](../tools/xtask.md)。

## 构建问题排查

| 现象 | 检查内容 |
|---|---|
| 提示找不到交叉编译目标的 `core` | 为实际使用的 stable 或 nightly 工具链安装目标 |
| 系统服务构建报告不支持所用 feature | 服务使用 nightly，通过 `rustup toolchain install nightly` 安装 |
| 提示找不到 QEMU | 安装所选架构的 `qemu-system-*` |
| 已输出 `System services ready`，没有应用输出 | 检查归档中的 `/etc/init.toml` 和应用文件 |
