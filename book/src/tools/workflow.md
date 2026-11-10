# 构建与运行工作流

修改内核或服务后，通常使用 xtask 重建映像并运行 QEMU；检查某个 crate 时，直接调用 Cargo 即可。
xtask 串起跨 crate 的构建与打包步骤，内部继续使用 Cargo 的依赖解析和增量编译。

## 常用循环

```sh
cargo xtask build riscv64 --release
cargo xtask run riscv64 --release
```

`build` 适合检查构建产物，`run` 在完成相同构建后启动 QEMU。
切换平台时将参数改为 `loongarch64`。
省略 `--release` 使用 debug 配置。

一次正常构建按以下顺序执行：

1. 用 nightly 编译 init、devmgr、logger、fs 和 linux。
2. 将各服务 ELF 与可选外部根目录写入 newc 归档。
3. 设置归档路径，按选择的内嵌或独立模式构建内核。
4. run 命令启动指定平台的 QEMU。

子构建使用 `--locked`，并将目标和构建配置统一传给服务与内核。
服务 ELF 去除符号，内核 ELF 保留相应 Cargo 配置产生的内容。

## 准备程序和配置

将额外的程序、符号链接和 `/etc/init.toml` 放在一个根目录中：

```sh
cargo xtask run riscv64 --release --rootfs ./rootfs
```

Linux 静态程序由兼容服务装载；原生服务的链接与入口约定见[编写原生程序](../runtime/programs.md)。
打包时保留 Unix 权限，可执行权限会参与 Linux 程序装载检查。

查看打包结果时，`package-init` 生成归档；`build` 在同样的打包流程后继续生成内核：

```sh
cargo xtask package-init loongarch64 --release --rootfs ./rootfs
cargo xtask build loongarch64 --release --initramfs external --rootfs ./rootfs
```

独立模式的手动 QEMU 命令见[系统映像](../system/images.md)。

## 定位构建产物

默认产物在 `target/<Rust 目标>/<debug 或 release>/`。
`arch` 为内核 ELF，服务使用各自的二进制名称，`initramfs.cpio` 为正常启动归档。
为一组构建使用独立缓存目录：

```sh
CARGO_TARGET_DIR=target/work cargo xtask build riscv64 --release
```

xtask 将相对构建目录转换为工作区根下的路径，使内部 Cargo 调用使用同一个目录。
外部根目录则按用户调用时的工作目录解析。

## 使用 Cargo 检查局部代码

`cargo check` 适合在编辑期间检查类型与接口，Clippy 检查 lint，集成测试验证功能。
交叉目标与 feature 决定检查哪些实现，例如：

```sh
cargo check --locked -p arona-arch --features baremetal \
  --target riscv64gc-unknown-none-elf
cargo +nightly check --locked -p arona-init --features baremetal \
  --target loongarch64-unknown-none
```

## 手册工作流

```sh
cargo xtask book
cargo xtask book serve --open
```

正文位于 `book/src/`，HTML 和 PDF 位于 `target/book/`。
目录、插件和导出维护见[维护手册](../contributing/book.md)。
