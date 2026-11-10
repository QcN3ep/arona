# 程序运行环境

从 `main` 中创建一个 `Vec`，或用 `thread::spawn` 启动工作线程，都依赖程序自身的运行环境。
启用 `runtime/program` 后，runtime 在每个原生程序的地址空间中管理当前 Task、堆、TLS 和线程资源。

## 当前程序

`ProcessStart` 编码为 16 字节，包含当前 Task 和可选 bootstrap 通道的句柄。
自身 Task 句柄带有页框分配、内存映射和终止权限，runtime 初始化时核对交接记录与句柄信息。

| 接口 | 作用 |
|---|---|
| `process::task()` | 借用保存的自身 Task 句柄 |
| `process::bootstrap()` | 取走可选的启动通道 |
| `process::exit(status)` | 结束当前 Task |
| `thread::current()` | 借用当前线程的特殊句柄 |

启动过程建立初始线程的 TLS 后再调用 `main`。
`main` 正常返回时先执行 TLS 析构，再退出 Task。
panic 和分配失败使用退出码 1。
直接调用退出接口结束控制流，不展开当前 Rust 栈。

## 用户堆

全局分配器使用 `dlmalloc`，状态由自旋锁保护。
它通过当前 Task 分配页框、建立读写用户映射，并将得到的虚拟范围交给分配器。
`Box`、`Vec`、`String` 和 `Arc` 等 `alloc` 类型使用这一分配路径。

例如，`Vec` 扩容得到的存储来自 `dlmalloc` 管理的映射范围，释放后先回到这份用户堆中供后续分配复用。
当 `dlmalloc` 归还整个底层范围时，runtime 撤销按页建立的映射，内核再释放相应映射引用。

## 安全线程创建

`thread::spawn` 接受 `FnOnce() -> T`，闭包与返回值都要求 `Send + 'static`。
调用成功返回 `JoinHandle<T>`。
运行时先保存闭包，建立用户栈、保护页与 TLS，再创建内核线程，让新线程从这份闭包开始执行。

默认用户栈为 2 MiB。
`thread::Builder::stack_size` 可以指定栈大小，运行时按页取整并检查至少 16 KiB。
设备驱动工作线程等调用者使用 Builder 选择其栈大小。

工作线程正常返回时执行 TLS 析构，保存返回值并结束内核线程。
`JoinHandle::join` 等待内核线程退出，再取走返回值并回收资源。
`JoinError::Exited` 表示线程退出但没有正常交付返回值，`JoinError::Runtime` 表示等待等运行时操作失败。

丢弃 JoinHandle 会分离线程。
runtime 将其资源交给回收线程持有，工作线程继续执行，退出后由回收线程释放栈、TLS 和结果存储。

## 线程本地存储

链接脚本将 TLS 初始数据放入 `.tdata`，零初始化部分放入 `.tbss`。
runtime 为每个线程复制 TLS 初始映像，清零其余部分，并设置对应架构的线程指针。

`thread_local!` 提供惰性初始化的线程本地值：

```rust
use core::cell::Cell;

runtime::thread_local! {
    static COUNT: Cell<u64> = Cell::new(0);
}

fn next_count() -> u64 {
    COUNT.with(|count| {
        let next = count.get() + 1;
        count.set(next);
        next
    })
}
```

假设两个线程分别调用 `next_count`，各自第一次都得到 1，此后在自己的 `Cell` 中独立累加。
`try_with` 对初始化重入和已析构状态返回 `AccessError`，`with` 在这些状态下 panic。
已初始化且需要析构的值登记到该线程的析构列表，在正常线程返回时处理。

## 子 Task 构建

`ChildTask` 用于构造独立的原生程序环境。
它提供分配、映射、保留范围和安装句柄的接口，`start` 建立用户栈与 ProcessStart，再启动入口。
构建过程中丢弃 ChildTask 会终止尚未交付的子 Task。

`SupervisedTask` 用于受 TrapPort 监督的执行环境。
创建时绑定监督端口，提供地址空间和线程状态设置能力。
Linux 兼容服务通过这一类型建立 Linux 程序。

init 用 ChildTask 加载原生服务，Linux 兼容服务用 SupervisedTask 建立受监督程序。
同一地址空间里的 Rust 闭包线程则由 `thread::spawn` 创建，代码与数据共享该程序的映射。
