# 维护手册

本书使用 [mdBook](https://rust-lang.github.io/mdBook/) 的默认主题。
[mdbook-admonish](https://tommilligan.github.io/mdbook-admonish/) 渲染提示块，[mdbook-pdf](https://github.com/HollowMan6/mdbook-pdf) 通过 Chromium 导出 PDF，`mdbook-pdf-outline` 为 PDF 添加分层章节书签。

## 安装手册工具

手册构建使用 Rust 工具、uv 管理的 Python 插件和 Chromium：

```sh
cargo install mdbook --version 0.4.51 --locked
cargo install mdbook-admonish --version 1.20.0 --locked
cargo install mdbook-pdf --version 0.1.11 --locked
UV_TOOL_BIN_DIR="$HOME/.cargo/bin" uv tool install mdbook-pdf-outline==0.1.6
export PATH="$HOME/.cargo/bin:$PATH"
```

这组版本使用 mdBook 0.4 接口。
书签插件与 mdBook 安装在同一可执行目录，PDF 后端调用 Chromium 完成页面打印。

## 预览与导出

```sh
cargo xtask book
cargo xtask book serve --open
```

第一个命令构建全部输出，第二个命令启动预览并在源文件变化后重新构建。
HTML 与 PDF 后端都参与构建和预览。

| 产物 | 路径 |
|---|---|
| HTML 入口 | `target/book/html/index.html` |
| 原始 PDF | `target/book/pdf/output.pdf` |
| 带章节书签的 PDF | `target/book/pdf-outline/output.pdf` |

`pdf-outline` 使用已生成的 HTML 和 PDF，按正文标题添加层次书签。
最后一项保留章节导航，适合离线阅读和分发。

## 章节与目录

目录在 `book/src/SUMMARY.md` 中维护，正文按系统、内核、runtime、系统服务、工具和参与开发分类。
目录项链接正文文件，章节之间通过 `../tools/xtask.md` 这样的相对链接导航。
`book.toml` 中的 `create-missing = false` 让构建检查目录与正文是否对应。

本书描述当前代码的行为。
涉及接口时应核对函数签名、权限要求、错误和资源生命周期；涉及使用步骤时应核对构建选项、产物和实际输出。
正文只记录已实现的功能，使用通行术语，并给出必要的规范出处。

章节从使用过程或运行流程展开，说明各部分怎样配合。
代码片段用于展示调用，表格用于查阅参数与类型。

## 共享内容与插件资源

贡献规范从根目录 `CONTRIBUTING.md` 引入，修改原文件即可。
构建配置会观察该文件的变化。

提示块的语法为：

````markdown
```admonish info title="标题"
提示内容。
```
````

`book/mdbook-admonish.css` 是插件生成的标准样式。
升级插件后，在 `book/` 中执行 `mdbook-admonish install .` 更新资源，并检查 `book.toml` 中的样式路径和预处理器配置。
该样式的 MIT 许可证信息记录在 `REUSE.toml` 与 `LICENSES/MIT.txt` 中。

修改目录或正文后，应重新构建并检查 HTML 链接、PDF 的中文显示、代码排版和书签跳转。
