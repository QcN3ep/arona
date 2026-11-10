# 使用内核能力

runtime 用 Rust 类型持有内核对象和资源。
一个分配句柄可以交给映射接口，一段 MMIO 映射可以交给设备驱动，而析构将关闭句柄、解除映射等清理接回对象的生命周期。
内核在这些操作中逐次检查权限与状态。

## 拥有、借用和检查句柄

`Handle<T>` 拥有一个句柄，析构时调用 CLOSE。
`BorrowedHandle<'a, T>` 借用已有句柄，其生命周期关联原所有者。
`info` 返回对象类型和权限，`try_clone(rights)` 请求一份缩减或相同权限的新句柄。

IPC 接收的 `AnyHandle` 同样拥有句柄。
程序可以检查 `kind()`，再通过 `downcast::<T>()` 转成具体类型；类型不匹配时返回原 AnyHandle。
转换后的句柄保留原权限。

```rust
use runtime::{AnyHandle, DeviceSet, Error, Handle};

fn read_device_catalog(
    handle: AnyHandle,
) -> Result<Handle<DeviceSet>, Error> {
    handle.downcast().map_err(|_| runtime::Error::Protocol)
}
```

这段代码期望收到设备目录，匹配成功后取得 `Handle<DeviceSet>`。
类型不匹配时，`downcast` 返回原句柄；示例将其随错误路径释放，返回 `Protocol`。

## 页框与映射

Task 的 `allocate` 返回 FrameAllocation 句柄。
`read` 和 `write` 按字节访问分配内容，`map` 则按页框范围将它放入地址空间。
下面将字符串写入一页分配，建立映射，再读取同一份后备数据：

```rust
use runtime::{Rights, memory::Permissions};

fn mapped_bytes() -> Result<(), runtime::Error> {
    let task = runtime::process::task();
    let mut frames =
        task.allocate(1, Rights::READ_WRITE | Rights::MAP)?;
    frames.write(0, b"Arona")?;
    let mut mapping =
        task.map(&frames, 0, 1, None, Permissions::ReadWrite)?;
    let _range =
        mapping.range().ok_or(runtime::Error::Protocol)?;
    let mut bytes = [0; 5];
    frames.read(0, &mut bytes)?;
    mapping.unmap()?;
    Ok(())
}
```

`Mapping` 保存目标 Task 的借用和虚拟范围。
显式 `unmap` 返回解除映射的结果，析构时也会尝试解除映射。
`protect` 修改映射权限，`Reservation` 持有保留范围并在释放时执行 UNRESERVE。
关闭 FrameAllocation 句柄之后，仍然有效的内核映射引用继续保持后备内存。
映射结束与分配所有权释放共同决定最终回收时机。

## 线程与 Task 控制

Task 句柄提供创建子 Task、安装句柄、终止、等待与地址空间操作。
`install` 消耗转移的 Handle；失败时把错误和原 Handle 一并返回。

Thread 句柄提供 start、state、terminate、wait 和 id 等操作。
当前线程还可以查询单调时间、休眠和让出处理器。
闭包线程的栈与结果管理见[程序运行环境](environment.md)。

## 设备、MMIO 与 DMA

DeviceSet 支持枚举、读取描述和授予设备组。
Device 的 `map_mmio` 返回 `Mmio`，提供带偏移与宽度检查的寄存器访问。
`InterruptBinding` 保存设备与 Event 的借用，提供 acknowledge 和 unbind，并负责析构解绑。

`DmaMapping` 与 `DmaBuffer` 持有 DMA 范围、总线地址及后备资源。
程序通过这些封装读写缓冲区，在提交队列前后执行同步，并按对象状态停用、解除映射和撤销。
devmgr 的 VirtIO 驱动使用这一层建立设备队列和数据缓冲区。

## Trap 与内核日志

`trap_port` 创建事件接收端，`bind_trap` 绑定受监督 Task。
接收结果包含事件信息与 continuation。
continuation 读取、修改并恢复用户寄存器状态，恢复失败时把错误和原对象一并返回。

KernelLog 的 `read` 接受游标和输出切片，返回可选的借用记录。
记录文本借用输出切片，读取者处理完本条记录后复用这块存储，再以新游标取得下一条。

## 错误处理

`runtime::Error::Kernel` 保留 ABI 错误码，`Error::Protocol` 表示交接或内核返回数据不符合封装约定。
错误返回同时保留资源的归属。
例如 `install` 失败会交回原 Handle，continuation 恢复失败会交回原 continuation，调用者据此继续处理所持有的对象。
