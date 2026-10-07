//! 清单必须是**真正的 TOML**，而不只是「我们自己那个子集解析器能读」。
//!
//! 这个文件的存在是因为一个真实 bug：`mcs51.toml` 和 `mcs51 new` 的模板里
//! 用了 `;` 当注释，而 TOML 的注释符是 `#`。我们自己的解析器当时两种都认，
//! 所以工具链一路正常 —— 但编辑器、linter 和任何别的 TOML 工具都会报错。
//!
//! 修法有两层：
//!
//! 1. 解析器不再接受 `;`，并给出明确提示（见 `manifest.rs`）；
//! 2. 这里用**真正的 TOML 解析器**校验仓库里每一个 `.toml`，
//!    以及 `mcs51 new` 会写出的那份模板。

use std::path::{Path, PathBuf};

/// 收集目录下所有 `.toml`，跳过构建产物目录。
fn collect_toml(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // 不爬进构建目录和依赖缓存。
            if matches!(
                name.as_ref(),
                "target" | ".cargo-home" | ".git" | "node_modules"
            ) {
                continue;
            }
            collect_toml(&path, out);
        } else if name.ends_with(".toml") {
            out.push(path);
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("找到仓库根目录")
}

#[test]
fn every_toml_in_the_repository_is_valid_toml() {
    let root = repo_root();
    let mut files = Vec::new();
    collect_toml(&root, &mut files);
    assert!(
        files.len() >= 8,
        "应该找到若干个 .toml，实际 {}：{files:#?}",
        files.len()
    );

    let mut failures = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).expect("读 .toml");
        if let Err(error) = text.parse::<toml::Table>() {
            failures.push(format!(
                "{}:\n  {error}",
                path.strip_prefix(&root).unwrap_or(path).display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "这些 .toml 不是合法的 TOML：\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_scaffold_template_is_valid_toml() {
    // `mcs51 new` 会把它原样写成 mcs51.toml，所以它必须是合法 TOML。
    // 这一段曾经用的是 `;` 注释 —— 也就是说每个新工程拿到的都是非法清单。
    let text = mcs51_build::TEMPLATE_MANIFEST;
    let table = text
        .parse::<toml::Table>()
        .unwrap_or_else(|e| panic!("mcs51 new 的清单模板不是合法 TOML：{e}\n---\n{text}"));
    // 顺带确认模板里该有的段都在。
    for section in ["project", "build", "sim", "flash"] {
        assert!(table.contains_key(section), "模板缺少 [{section}] 段");
    }
}

#[test]
fn an_application_template_is_still_assembly() {
    // 别把 TOML 的规则套到 .asm 上：汇编里 `;` 就是注释，必须保留。
    let text = mcs51_build::TEMPLATE_APP;
    assert!(text.contains("; blink.asm"), "汇编模板的注释被误改了");
    assert!(text.contains(".area CODE"));
}

#[test]
fn a_semicolon_comment_is_rejected_with_a_helpful_message() {
    let error =
        mcs51_build::Manifest::parse("[project]\nname = \"x\"\n; 这是分号注释\n", ".").unwrap_err();
    let text = error.to_string();
    assert!(text.contains("`#`"), "应该告诉用户正确的注释符：{text}");
    assert!(
        text.contains("第 3 行") || text.contains("line 3"),
        "{text}"
    );
    assert!(
        text.contains("; 这是分号注释"),
        "应该回显出错的那一行：{text}"
    );
}

#[test]
fn a_semicolon_inside_a_string_is_fine() {
    // 引号里的分号是数据，不是注释。
    let manifest = mcs51_build::Manifest::parse(
        "[flash]\nport = \"/dev/tty;0\"\nprotocol = \"stc89\"\n",
        ".",
    )
    .expect("字符串里的分号不该报错");
    assert_eq!(manifest.flash.port, "/dev/tty;0");
}

#[test]
fn a_hash_comment_still_works_in_both_positions() {
    let manifest = mcs51_build::Manifest::parse(
        "# 整行注释\n[flash]\nport = \"/dev/ttyUSB0\"   # 行尾注释\n",
        ".",
    )
    .expect("`#` 注释必须可用");
    assert_eq!(manifest.flash.port, "/dev/ttyUSB0");
}

#[test]
fn a_scaffolded_project_manifest_parses_with_a_real_toml_parser() {
    // 端到端：让 `mcs51 new` 真的写一份出来，再用真解析器读。
    let dir = std::env::temp_dir().join("mcs51-manifest-scaffold");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mcs51.toml"), mcs51_build::TEMPLATE_MANIFEST).unwrap();

    let text = std::fs::read_to_string(dir.join("mcs51.toml")).unwrap();
    text.parse::<toml::Table>()
        .unwrap_or_else(|e| panic!("脚手架写出的清单不是合法 TOML：{e}"));

    let manifest = mcs51_build::Manifest::load(&dir).expect("我们自己的解析器也要能读");
    assert_eq!(manifest.project.name, "blink");
    assert_eq!(manifest.build.language, "asm");
    assert_eq!(manifest.sim.cpu_hz, 11_059_200);
}
