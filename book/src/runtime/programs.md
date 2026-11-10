# 编写原生程序

一个原生程序从 `#![no_std]` 和普通的 `main` 开始。
链接 runtime 后，程序获得启动入口、堆、线程本地存储和退出处理，再通过类型化句柄访问内核能力。
本章从一个返回成功状态的程序开始，逐步加入容器、线程和服务连接。

## 程序源文件

最简单的程序直接返回退出状态：

```rust
#![no_std]

extern crate runtime;

fn main() -> usize {
    0
}
```

`extern crate runtime` 把程序启动所需的运行时链接进来。
应用使用 Rust 的 `main` 约定，runtime 接收启动交接并调用编译器生成的入口。
返回 0 表示成功。

加入 `alloc` 后，就能使用容器并将数据交给闭包线程：

```rust
#![no_std]

extern crate alloc;
extern crate runtime;

use alloc::vec::Vec;

fn main() -> Result<(), runtime::Error> {
    let values: Vec<u64> = (1..=4).collect();
    let worker = runtime::thread::spawn(move || {
        values.into_iter().sum::<u64>()
    })?;
    let _total =
        worker.join().map_err(|_| runtime::Error::Protocol)?;
    Ok(())
}
```

`move` 将 `values` 交给工作线程，线程遍历其中的四个整数并返回总和 10。
主线程在 `join` 处等待结果，这段时间其他就绪线程可以运行。
线程返回后，runtime 取得结果并回收它的栈与 TLS；`?` 则把运行时错误交给 `main` 的返回路径，以退出码 1 结束程序。

## Cargo 配置与链接

原生程序依赖 `arona-runtime`，使用 `runtime` 作为库名，并启用 `program` feature。
仓库中的服务将它纳入各自的 `baremetal` feature，例如：

```toml
[features]
baremetal = ["runtime/program"]

[dependencies]
runtime = { package = "arona-runtime", path = "../runtime" }
```

这是 `components/` 下服务 crate 的依赖写法。
二进制目标以 `required-features = ["baremetal"]` 选择这套运行环境。

链接脚本为 `components/runtime/link.ld`。
它定义 `_start`，将文本、只读数据和可写数据分别组织为页对齐的 `PT_LOAD`，并为 TLS 定义 `PT_TLS` 与所需符号。
程序虚拟布局从 `0x100000` 开始。

现有服务的 `build.rs` 在目标系统为 `none` 时把这一脚本传给链接器。
xtask 对服务使用 nightly Rust，指定目标架构、`baremetal` feature、符号裁剪和 4096 字节最大段对齐。
构建流程见[构建与运行工作流](../tools/workflow.md)。

## 启动交接与服务请求

程序开始执行时，runtime 已接收 `ProcessStart` 中的自身 Task 句柄和 bootstrap 通道。
服务在 `main` 中取出通道，接收启动方交付的资源：

```rust
use runtime::{Error, ipc::ReceivedMessage};

fn receive_startup() -> Result<ReceivedMessage, Error> {
    let channel = runtime::process::bootstrap()
        .ok_or(runtime::Error::Protocol)?;
    channel.receive(runtime::ipc::WaitMode::Forever)
}
```

取出的通道由程序持有，启动消息的 tag、数据字和句柄顺序由程序与启动方共同约定。
例如 init 取得启动归档，devmgr 取得设备集合；它们在各自入口中解码消息并建立服务状态。

## 依赖服务的客户端

服务客户端由库代码和通道两部分组成。
`fs`、`logger`、`devmgr` 提供 Rust 方法与编码，启动消息或其他 IPC 则交付对应的服务连接。

拿到文件服务通道后，程序建立 `Client`，文件操作就经 IPC 进入 fs Task。
日志门面通过 logger 客户端发送消息，设备会话通过 devmgr 返回的专用通道访问设备。
这些客户端的初始化与权限见[服务协作模型](../services/model.md)。
