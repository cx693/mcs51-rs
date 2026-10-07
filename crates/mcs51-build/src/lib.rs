//! # mcs51-build —— 工程清单解析
//!
//! `mcs51` 命令行本来是个纯二进制，但[清单解析器](manifest)值得单独暴露出来：
//! 测试要拿它跟**真正的 TOML 解析器**对照，确认我们写出的 `mcs51.toml`
//! 是合法 TOML，而不只是「我们自己那个子集解析器能读」。
//!
//! 这个区分不是学术问题 —— 曾经有个 bug 是 `mcs51.toml` 和 `mcs51 new` 的
//! 模板都用 `;` 当注释（TOML 里应该是 `#`）。当时的解析器两种都认，
//! 于是工具链一路正常，而编辑器、linter 和任何别的 TOML 工具全都报错。
//!
//! 现在解析器只认 `#`，遇到 `;` 会给出带行号和列号的提示。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod manifest;

pub use manifest::{FlashSection, Manifest, ManifestError, TEMPLATE_APP, TEMPLATE_MANIFEST};
