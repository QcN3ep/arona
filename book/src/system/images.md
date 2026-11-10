# 系统映像

一次系统构建生成内核 ELF `arch` 和启动归档 `initramfs.cpio`。
内核包含特权态代码，归档保存 init、其他系统服务和打包进去的应用文件。
默认构建把归档嵌入 ELF，分发时一个 `arch` 文件就能带上完整的启动内容；独立加载模式则通过 QEMU 的 `-initrd` 传递归档。

## 启动归档的内容

归档中的系统服务位于 `/bin`：

| 归档路径 | 程序 |
|---|---|
| `/bin/init` | 首个原生程序，负责启动其他服务与配置的应用 |
| `/bin/devmgr` | 设备管理服务 |
| `/bin/logger` | 日志服务 |
| `/bin/fs` | 文件服务 |
| `/bin/linux` | Linux 兼容服务 |

归档中的文件名使用 `bin/init` 等相对路径，文件服务将它们呈现为从 `/` 开始的路径。
系统服务采用同一架构、同一 debug 或 release 配置构建，归档中的服务 ELF 会去除符号。

启动归档使用未压缩的 cpio newc 格式。
每项由固定长度的 ASCII 十六进制头、以 NUL 结束的文件名和文件数据构成，文件名及数据的结尾按 4 字节对齐。
末尾使用 `TRAILER!!!` 表示归档结束。
格式说明见 Linux 的 [initramfs buffer format](https://www.kernel.org/doc/html/latest/driver-api/early-userspace/buffer-format.html)。

## 内嵌与独立加载

| 模式 | 构建选项 | 启动时需要的文件 |
|---|---|---|
| 内嵌 | `--initramfs embedded`，也是默认值 | `arch` |
| 独立加载 | `--initramfs external` | `arch` 与 `initramfs.cpio` |

内嵌构建启用 `arona-arch` 的 `embedded-initramfs` feature。
构建脚本通过 `ARONA_INITRAMFS` 找到归档，架构层将其放入内核 ELF 的 `.initramfs` 段，并由只读映像区域承载。
打包阶段生成的 `initramfs.cpio` 也保存在构建目录中。

独立加载模式把归档留在内核 ELF 之外。
`xtask run` 使用 QEMU 的 `-initrd` 传递归档，架构启动代码从交接信息中取得其物理范围。
两种模式最终都定位同一个归档中的 `/bin/init`，并使用相同的内核启动接口。

## 添加外部根目录

`--rootfs PATH` 把一个宿主机目录的内容追加到归档。
例如，将 BusyBox、Shell 链接和启动配置组织为：

```text
rootfs/
  bin/
    busybox
    sh -> busybox
  etc/
    init.toml
```

将这个目录加入 RISC-V 映像：

```sh
cargo xtask run riscv64 --release --rootfs ./rootfs
```

这里的 `bin/busybox` 是 RISC-V 的 Linux 静态 ELF，`bin/sh` 指向它，`etc/init.toml` 告诉 init 怎样启动 Shell。
对应配置见[启动配置与使用](configuration.md)。

打包器支持普通文件、目录和符号链接，保留 Unix 权限位，并按文件名排序遍历目录。
符号链接保存其目标文本，打包时不跟随链接读取目标。
文件路径和符号链接目标采用 UTF-8。
五个系统服务的路径由打包器保留，外部目录中的同名项在打包时报告冲突。

## 从归档到首个程序

架构启动代码先选择归档来源，再扫描 newc 条目并定位 init。
它检查 init 的 ELF 机器类型、段范围、入口、权限等布局信息，作为 `BootProgram` 交给内核。
启动加载器接受与平台匹配的静态 ELF64 `ET_EXEC`，校验可加载段的 R、RW 或 RX 权限。

内核根据程序段建立 init 的映射、栈和线程，并通过 bootstrap 通道交付三项资源：

| 资源 | 用途 |
|---|---|
| 归档的 `FrameAllocation` 句柄及实际字节数 | 让 init 读取服务程序，并把归档交给文件服务 |
| `DeviceSet` 句柄 | 授权并分配设备资源 |
| `KernelLog` 句柄 | 交给日志服务读取内核日志 |

到这里，init 已经拥有加载其他服务的程序数据，以及分配设备和读取内核日志的能力。
它先从归档启动 fs，随后通过文件接口读取、解析和加载其余服务。

## 手动启动 QEMU

内嵌归档的 RISC-V：

```sh
cargo xtask build riscv64 --release --initramfs embedded
qemu-system-riscv64 \
  -M virt -m 256M -smp 1 -nographic -monitor none -nic none \
  -bios default \
  -kernel target/riscv64gc-unknown-none-elf/release/arch
```

独立归档的 LoongArch：

```sh
cargo xtask build loongarch64 --release --initramfs external
qemu-system-loongarch64 \
  -M virt -m 256M -smp 1 -nographic -monitor none -nic none \
  -kernel target/loongarch64-unknown-none/release/arch \
  -initrd target/loongarch64-unknown-none/release/initramfs.cpio
```

手动命令也便于添加磁盘等 QEMU 设备。
`xtask run` 的固定机器参数见[快速开始](getting-started.md)。
