# The Arona Book

Welcome to The Arona Book! Arona is a microkernel operating system developed in Rust.

## Structure of This Book

| Part | Contents |
|---|---|
| Arona System | Build, run, system images, configuration, and supported features |
| Arona Kernel | Architecture, safety boundaries, and kernel mechanisms |
| Arona Runtime | Runtime environment and kernel capability interfaces for native Rust programs |
| Arona System Services | init, device management, logging, file, and Linux compatibility services |
| Development Tools | Build, packaging, testing, and debugging |
| Contributing | Code organization, contribution guidelines, checks, and documentation maintenance |

[Getting Started](system/getting-started.md) covers building and running; [Kernel Architecture](kernel/architecture.md) describes how the parts of the system relate.
Writing native programs starts with the [Runtime Overview](runtime/overview.md); see [Code Organization](contributing/code-organization.md) and [Contribution Guidelines](contributing/guidelines.md) for the directory layout and conventions for contributing.

## Offline Reading and PDF

Running `cargo xtask book` generates HTML and a PDF with chapter bookmarks.
See the [Maintenance Guide](contributing/book.md) for build instructions and output paths.

## License

The project, including this book, is licensed under the Mozilla Public License 2.0 or the GNU General Public License version 3.0 or later, at your option.
Licenses for third-party files are recorded in the repository's `REUSE.toml` and `LICENSES/`.
