# 代码检查

代码检查由 Rust 工具和宿主集成测试组成。
它们检查类型与 lint，以及实际功能行为。
源码限制是模块边界的项目约定。
代码审查继续沿资源的创建、使用和释放过程，核对职责、权限与失败后的状态。

## Rust 检查

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- \
  -D warnings --force-warn clippy::too_many_lines
cargo test --locked --workspace --tests
```

工作区 lint 将 `unsafe_op_in_unsafe_fn` 与 Clippy correctness 设为拒绝，启用通用 Clippy 检查和不安全代码注释检查。
`clippy.toml` 的 `check-private-items = true` 将相关检查也应用到私有项，`too-many-lines-threshold = 80` 提供函数规模提示。


## 源码限制

项目按目录遵循以下规则：

| 目录 | unsafe | cfg、cfg_attr 与 cfg! |
|---|---|---|
| `kernel/` 实现 | 禁止 | 禁止 |
| `kernel/tests/` | 允许 | 禁止 |
| `kernel-tests/`、`abi/` | 禁止 | 禁止 |
| `components/runtime/` | 允许 | 允许 |
| 其他 `components/` | 禁止 | 禁止 |

架构实现放在 `arch/`，允许使用与硬件操作对应的 `unsafe` 和条件编译。


## 文档检查


rustdoc 以拒绝警告的方式构建内核与各目标的接口文档。
手册构建通过 `cargo xtask book` 检查目录、include、插件和输出。

## 审查职责和接口

审查可以从一次完整操作入手。
例如映射用户内存，从 syscall 参数进入地址空间，经过页框引用和页表编辑，再到撤销映射与回收。
沿途核对状态由谁持有、公开入口更新哪些记录，以及失败路径会留下哪些资源。
各层的职责为这条路径提供参照：arch 执行硬件操作，kernel 管理资源，服务解释设备与文件语义，runtime 连接程序运行环境与调用接口。

接口审查关注：

- 名称是否准确表达对象、所有权或地址标识。
- 状态是否有一个明确的持有者，派生信息是否能从已有状态取得。
- 公开入口是否统一维护权限、映射记录和资源生命周期。
- 输入检查是否通过代码完成，环境要求是否在必要的 `# Safety` 中说明。
- 失败发生时，哪些操作已提交，句柄、映射、队列和页框分别归谁持有。
- 锁、关中断范围和等待点是否与调用环境一致。

注释说明代码不能直接表达的必要条件与原因。
通行术语、清楚的类型和较短的调用路径承担主要的可读性工作。

## 按功能验证

测试围绕正常输入、必要边界与代表性错误组织。
错误场景同时观察返回值和失败后的资源状态。
硬件场景通过真实架构实现与 QEMU 设备执行，相关测试与功能改动一起维护。

