//! P0-1 回潮守卫：断言已删除的死代码不再出现在公开重导出出口 `src/exports.rs`。
//!
//! 这三个公式输出函数（`to_formula_html` / `to_formula_json` / `to_formula_markdown`）生产零调用，
//! 已在 P0-1 从 crate 根公开面删除，其语义测试迁移到了真正在用的输出入口。放在独立文件里，
//! 是为了让这些被禁的名字只出现在**本测试**中，而不会被 `src/exports.rs` 的自扫描误判。
use std::path::PathBuf;

#[test]
fn removed_formula_reexports_do_not_reappear_in_exports_rs() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/exports.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("exports.rs must exist at {}: {error}", path.display()));
    for forbidden in ["to_formula_html", "to_formula_json", "to_formula_markdown"] {
        assert!(
            !source.contains(forbidden),
            "`{forbidden}` must stay off the public surface (P0-1 dead-code cleanup)"
        );
    }
}
