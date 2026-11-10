# The Arona Book

欢迎来到 The Arona Book! Arona 是使用 Rust 开发的微内核操作系统。

## 本书的组成

| 部分 | 内容 |
|---|---|
| Arona 系统 | 构建、运行、映像、配置与支持情况 |
| Arona 内核 | 架构、安全边界和内核机制 |
| Arona Runtime | 原生 Rust 程序的运行环境与内核能力接口 |
| Arona 系统服务 | init、设备管理、日志、文件与 Linux 兼容服务 |
| 开发工具 | 构建、打包、测试与调试 |
| 参与开发 | 代码组织、贡献规范、检查与文档维护 |

[快速开始](system/getting-started.md)介绍构建和运行；[内核架构](kernel/architecture.md)介绍系统各部分的关系。
编写原生程序从[Runtime 概览](runtime/overview.md)开始，参与开发的目录和约定分别见[代码组织](contributing/code-organization.md)与[贡献规范](contributing/guidelines.md)。

## 离线阅读与 PDF

运行 `cargo xtask book` 会生成 HTML 和带章节书签的 PDF。
构建方法与产物路径见[维护手册](contributing/book.md)。

## 许可证

本项目的主要代码与本书采用 Mozilla Public License 2.0。
第三方文件的许可证记录在仓库的 `REUSE.toml` 和 `LICENSES/` 中。
