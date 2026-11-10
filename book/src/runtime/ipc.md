# IPC 编程

两个程序连接一对 Channel 后，就能发送短消息、交接句柄，或以一次请求等待一次回复。
runtime 将消息编码和内核调用封装起来，文件、日志和设备客户端再用这些接口表达各自的操作。

## 发送与接收

`Message::new(tag, words)` 创建短消息，`Channel::send` 在成功提交后消耗其中的转移句柄。
接收得到 `ReceivedMessage`，其 `words()` 和 `handles()` 分别提供数据与对象。
消息析构时关闭尚未取走的接收句柄。

`WaitMode` 决定等待方式：

| 方式 | 含义 |
|---|---|
| `Poll` | 立即尝试，不能完成则返回 `WouldBlock` |
| `Forever` | 等待完成或对象结束 |
| `Until(ns)` | 等待到绝对单调截止时间 |

`Until` 的数值使用 `thread::now_ns()` 的时间域，0 和 `u64::MAX` 分别留给轮询与无限等待编码。

## 请求与回复示例

下面在同一个 Task 内建立客户端和服务端两个线程。
主线程发出数据字 42，工作线程从请求中取出 Reply，再将同样的数据回复给主线程：

```rust
#![no_std]

extern crate runtime;

use runtime::{
    Error, Rights,
    ipc::{CallError, Message, WaitMode},
};

fn main() -> Result<(), Error> {
    let rights = Rights::READ_WRITE | Rights::WAIT;
    let (client, server) =
        runtime::process::task().channel(rights, rights)?;
    let worker = runtime::thread::spawn(
        move || -> Result<(), Error> {
            let mut request =
                server.receive(WaitMode::Forever)?;
            let reply =
                request.take_reply().ok_or(Error::Protocol)?;
            let mut response =
                Message::new(request.tag, request.words())?;
            reply.send(&mut response)
        },
    )?;
    let mut request = Message::new(1, &[42])?;
    let deadline = runtime::thread::now_ns()?
        .checked_add(1_000_000_000)
        .ok_or(Error::Protocol)?;
    let response = client
        .call(&mut request, Some(deadline))
        .map_err(|error| match error {
            CallError::NotSubmitted(error)
            | CallError::Submitted(error) => error,
        })?;
    let _words = response.words();
    worker.join().map_err(|_| Error::Protocol)??;
    Ok(())
}
```

`client` 和 `server` 是通道的两个端点，`take_reply` 取得内核随调用交付的一次性回复能力。
工作线程回复后返回，主线程取得响应，再通过 `join` 收回线程结果。
这个例子将两种调用错误都交给 `main`；携带资源的调用则用提交状态确定句柄归属。

## 句柄转移

`Transfer` 包含 `AnyHandle` 所有权及接收权限，使用 `Message::push` 加入消息。
push 失败返回原 Transfer，资源继续由调用者持有。

SEND 或 START_CALL 成功后，runtime 清空消息中的已转移句柄，并放弃发送方所有权。
失败时消息保留尚未提交的资源。
同步 CALL 使用内核返回的提交状态决定是否转移所有权。

| 调用结果 | 待转移资源的归属 |
|---|---|
| `CallError::NotSubmitted` | 仍在请求 Message 中 |
| 成功或 `CallError::Submitted` | 已提交给内核调用路径 |

例如，客户端向文件服务交付写入缓冲区后超时，服务可能已经完成写入。
`Submitted` 表达这次请求已交出，文件操作的完成状态由文件服务协议解释。

## 异步调用与事件循环

`start_call` 返回 Call 句柄。
将它与其他来源放入 `wait_many`，收到 `READABLE` 后调用 `try_receive` 取得响应。
取消使用 `cancel`，丢弃句柄则沿对象关闭路径清理调用资源。

`PendingCall<T, S, E>` 在 Call 之外保存客户端状态 `S` 和响应解码函数。
`try_complete` 将状态和 ReceivedMessage 交给解码函数，返回服务结果 `T`。
`wait` 超时后仍保留本次调用，调用者可以再次等待或取消。
设备打开与 Linux 程序启动等客户端使用这一封装。

## 多对象等待的使用

`WaitItem::new(&handle, conditions)` 借用待观察句柄。
`wait_many` 返回输入列表中的下标和就绪位，调用者据此处理对应对象。
内核按底层来源识别等待项，重复来源返回错误。

```rust
fn wait_for_message(
    channel: &runtime::Handle<runtime::Channel>,
) -> Result<runtime::ipc::WaitResult, runtime::Error> {
    use runtime::ipc::{self, WaitItem, WaitMode};
    ipc::wait_many(
        &[WaitItem::new(
            channel,
            ipc::READABLE | ipc::PEER_CLOSED,
        )],
        WaitMode::Forever,
    )
}
```

对端关闭时，队列里还可能有最后一批消息，通道因而同时报告可读和关闭。
事件循环先接收已排队的消息，再处理连接结束；有多个接收线程时，实际取得哪条消息由接收操作确定。
