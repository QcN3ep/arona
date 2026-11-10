# xtask 命令参考

`xtask` 是工作区内的宿主机构建工具。
`.cargo/config.toml` 把 `cargo xtask` 定义为 `cargo run --package xtask --`。
它负责组合 Cargo 构建、服务归档打包、QEMU 启动和手册构建。

```text
cargo xtask <build|run|package-init|package-linux-test> <riscv64|loongarch64> [OPTIONS]
cargo xtask book [build|serve] [OPTIONS]
cargo xtask help
```

## 内核与归档命令

| 命令 | 执行内容 |
|---|---|
| `build` | 构建五个系统服务，打包归档，构建内核 ELF |
| `run` | 执行 `build`，随后启动 QEMU |
| `package-init` | 构建系统服务并打包正常启动归档 |
| `package-linux-test` | 构建并打包 Linux 集成测试使用的归档 |

| 选项 | 默认值 | 含义 |
|---|---|---|
| `--release` | 未启用 | 为内核与组件选择 release 构建 |
| `--initramfs embedded\|external` | `embedded` | 选择内嵌归档或独立归档；在构建并启动内核时生效 |
| `--rootfs PATH` | 无 | 将宿主目录中的文件、目录和符号链接追加到归档 |

例如：

```sh
cargo xtask build riscv64 --release
cargo xtask run loongarch64 --release --initramfs external
cargo xtask package-init riscv64 --rootfs ./rootfs
```

`package-linux-test` 使用测试入口替代正常 init，加入兼容性测试的程序和数据，输出 `initramfs-linux-test.cpio`。
正常启动与测试各自保留对应的归档文件。

## 手册命令

```sh
cargo xtask book
cargo xtask book build
cargo xtask book serve --open
```

省略动作时默认执行 `build`。
`build` 和 `serve` 后面的选项原样交给 mdBook，例如：

```sh
cargo xtask book build --help
cargo xtask book serve --hostname 127.0.0.1 --port 3000
```

xtask 调用 `mdbook` 读取 `book/book.toml`，工具安装和导出说明见[维护手册](../contributing/book.md)。

## 工具链与环境变量

系统服务通过 `cargo +nightly rustc` 构建，内核使用当前 Cargo 工具链。
内部 Cargo 构建使用 `--locked`，沿用工作区 `Cargo.lock` 固定的依赖解析结果。

| 环境变量 | 用途 |
|---|---|
| `CARGO_TARGET_DIR` | 决定 Rust 构建目录和 initramfs 输出目录；相对路径由 xtask 按工作区根目录解释 |
| `ARONA_INITRAMFS` | 由 xtask 设置为归档路径，供架构 crate 的构建脚本和内嵌归档代码使用 |
| `PATH` | 查找 Cargo、QEMU、mdBook 及其插件等外部程序 |

手册输出目录由 `book/book.toml` 定义为 `target/book/`。

## 构建产物

默认 Rust 输出路径如下，其中 `PROFILE` 为 `debug` 或 `release`：

| 目标 | 产物目录 |
|---|---|
| `riscv64` | `target/riscv64gc-unknown-none-elf/PROFILE/` |
| `loongarch64` | `target/loongarch64-unknown-none/PROFILE/` |

目录中包括内核 `arch`、各服务 ELF 和归档。
`package-init` 和 `package-linux-test` 成功后会输出归档的完整路径。
各个外部构建或运行命令失败时，xtask 返回非零状态。
