//! 复审 P1-2 回潮守卫：会话构造的**公开面**只剩一条必须出示凭证的入口。
//!
//! `src/runtime/session.rs` 里那条不经凭证的 opener（`open_unchecked`）已收窄成
//! `pub(crate)` + `#[cfg(test)]`。三件事由三种力量分别保证：
//!
//! 1. **编译器**保证 crate 外部拿不到它（`pub(crate)`），也保证生产代码调用不到它
//!    （`#[cfg(test)]` —— 非测试构建里这个函数根本不存在；`cargo check --all-targets`
//!    会连 lib/bin 的**非测试**目标一起编，编过就是证据）；
//! 2. 本文件保证它**不会被改回公开**（那会让"结构上不可绕过"重新变成空话）；
//! 3. 本文件保证没有**第二条**不经凭证的出口从别处漏出来（重导出面、调用点清单）。
//!
//! 形态与既有守卫同族（`tests/exports_dead_code_guard.rs`、`src/bin/serve/mod.rs` 的
//! `dependency_boundary`）：读源码文本断言，不引入新依赖，不需要模型资产。
//! 若哪天把 `open_unchecked` 彻底删掉，本文件与它共存亡 —— 一起删，别留空转的断言。

use std::path::{Path, PathBuf};

/// 不经凭证的那条入口的名字。
const UNCHECKED: &str = "open_unchecked";
/// 扫描范围。
const SRC_ROOT: &str = "src";
/// 定义所在模块（相对 manifest 根）。
const SESSION_MODULE: &str = "src/runtime/session.rs";
/// 只可能是**整文件测试**的两个文件：它们由别处的 `#[cfg(…test…)] mod …;` 挂进来，
/// 文件内部没有测试模块标记，所以扫描规则要单独认它们。
const TEST_ONLY_FILES: [&str; 2] = ["src/test_support.rs", "src/bin/serve/tests.rs"];

fn manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn source(relative: &str) -> String {
    let path = manifest_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{relative} must be readable at {}: {error}", path.display())
    })
}

/// 去掉行注释与文档注释，避免把注释里的名字当成代码引用（行号保持不变）。
fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|line| match line.find("//") {
            Some(index) => &line[..index],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 代码（非注释）里出现 `needle` 的行号（1-based）。
fn code_lines_containing(text: &str, needle: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| line.contains(needle))
        .map(|(index, _)| index + 1)
        .collect()
}

/// 一行是否是"把东西关进测试构建"的 cfg 门（`#[cfg(test)]`、`#[cfg(all(…, test))]` 等）。
fn is_test_gate(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("#[cfg(") && trimmed.contains("test") && !trimmed.contains("not(test")
}

/// 本文件“测试部分”的起点（1-based 行号）：第一个**紧接 `mod …` 声明**的 cfg 测试门。
///
/// 只认“门 + 模块声明”这个形状，是因为定义 `open_unchecked` 的那一行自己就带着
/// `#[cfg(test)]` —— 若把它当分界，它之后的**生产**代码就全被误认成测试代码了。
fn test_part_start(lines: &[&str]) -> Option<usize> {
    lines.iter().enumerate().find_map(|(index, line)| {
        if !is_test_gate(line) {
            return None;
        }
        let next = lines[index + 1..]
            .iter()
            .find(|next| !next.trim().is_empty())?;
        is_module_declaration(next).then_some(index + 1)
    })
}

/// 一行是否是模块声明（允许任意可见性前缀）。
fn is_module_declaration(line: &str) -> bool {
    let mut rest = line.trim();
    loop {
        if let Some(stripped) = rest.strip_prefix("pub(") {
            match stripped.find(')') {
                Some(index) => rest = stripped[index + 1..].trim(),
                None => return false,
            }
            continue;
        }
        if let Some(stripped) = rest.strip_prefix("pub ") {
            rest = stripped.trim();
            continue;
        }
        break;
    }
    rest.starts_with("mod ")
}

fn collect_rs_files(directory: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("the source directory must exist") {
        let path = entry.expect("readable entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

/// `pub(crate)` 且直接挂在 cfg 测试门下；`pub` 写法一律判失败。
#[test]
fn the_unchecked_opener_is_crate_internal_and_test_only() {
    let text = source(SESSION_MODULE);
    let definition = format!("pub(crate) fn {UNCHECKED}(");
    assert!(
        text.contains(&definition),
        "{UNCHECKED} must stay `pub(crate)`: a `pub` one is a complete bypass of the integrity boundary (review P1-2)"
    );
    assert!(
        !text.contains(&format!("pub fn {UNCHECKED}(")),
        "{UNCHECKED} must not be re-exposed as `pub` — external callers would again be able to open any path without a credential"
    );
    // 字节加载出口本身也必须保持私有，否则"绕开凭证"只是换了个名字。
    assert!(
        !text.contains("pub fn open_bytes("),
        "the single byte-loading exit must stay private"
    );

    // 定义前面紧邻的**代码行**只能是 cfg 测试门（中间只允许文档注释与空行）。
    let lines = text.lines().collect::<Vec<_>>();
    let definition_index = lines
        .iter()
        .position(|line| line.contains(&definition))
        .expect("the definition was just found");
    let preceding = lines[..definition_index]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty() && !line.starts_with("///"))
        .unwrap_or_default();
    assert!(
        is_test_gate(preceding),
        "{UNCHECKED} must be compiled only into test builds — a production build must not contain it at all, \
         but the line above the definition is: {preceding:?}"
    );
}

/// 除定义之外，`src/` 里每一条 `open_unchecked(…)` 调用都必须落在测试代码里。
#[test]
fn no_production_code_calls_the_unchecked_opener() {
    let root = manifest_root().join(SRC_ROOT);
    let mut sources = Vec::new();
    collect_rs_files(&root, &mut sources);
    assert!(
        sources.len() >= 100,
        "the scan must actually see the crate, found {} files under {}",
        sources.len(),
        root.display()
    );

    let call = format!("{UNCHECKED}(");
    let definition_line = format!("pub(crate) fn {UNCHECKED}(");
    let mut call_sites = 0;
    for path in &sources {
        let relative = format!(
            "{SRC_ROOT}/{}",
            path.strip_prefix(&root)
                .expect("collected under the source root")
                .to_string_lossy()
                .replace('\\', "/")
        );
        let text = source(&relative);
        let lines = text.lines().collect::<Vec<_>>();
        let references = code_lines_containing(&strip_comments(&text), &call)
            .into_iter()
            .filter(|line| !lines[line - 1].contains(&definition_line))
            .collect::<Vec<_>>();
        call_sites += references.len();
        if references.is_empty() || TEST_ONLY_FILES.contains(&relative.as_str()) {
            continue;
        }
        let boundary = test_part_start(&lines);
        for line in references {
            assert!(
                boundary.is_some_and(|start| line > start),
                "{relative}:{line} calls {UNCHECKED} outside the `#[cfg(…test…)] mod …` part of a non-test \
                 file — production code must go through `OrtSession::open_verified`"
            );
        }
    }
    assert!(
        call_sites > 0,
        "the scan found no call site at all — either the opener was deleted (then delete this guard too) \
         or the needle stopped matching; files scanned: {}",
        sources.len()
    );
}

/// 公开重导出面不得点名任何"不经凭证"的会话入口。
#[test]
fn the_public_reexport_surface_names_no_unchecked_opener() {
    for module in ["src/exports.rs", "src/lib.rs"] {
        let code = strip_comments(&source(module)).to_lowercase();
        assert!(
            !code.contains("unchecked"),
            "{module} must not re-export a session entry that skips the credential (review P1-2)"
        );
    }
    // 反向钉住：守卫针对的是**方法**，不是类型。`OrtSession` 仍对外可见，
    // 因为 `open_verified` 是公开面上唯一那条会话构造入口。
    assert!(
        source("src/exports.rs").contains("OrtSession"),
        "the verified session entry point must stay reachable for library users"
    );
}

/// 上面那条扫描把两个文件整体当测试代码，前提是它们确实靠 cfg 测试门挂进构建。
#[test]
fn the_test_only_files_are_gated_as_test_only() {
    for (module, declaration) in [
        ("src/lib.rs", "mod test_support;"),
        ("src/bin/serve/mod.rs", "mod tests;"),
    ] {
        let text = source(module);
        let index = text.find(declaration).unwrap_or_else(|| {
            panic!(
                "{module} must declare `{declaration}` for the scan's test-only file list to hold"
            )
        });
        let preceding = text[..index]
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("//"))
            .unwrap_or_default();
        assert!(
            is_test_gate(preceding),
            "{module} must gate `{declaration}` behind a cfg(...test...) attribute, found: {preceding:?}"
        );
    }
}
