//! 生成物，**请勿手改**（docs/12 §4.2 P1-6）。
//!
//! 唯一权威：assets/table_html_whitelist.json；生成器：tools/generate-table-whitelist.mjs。
//! 本文件与页面 src/bin/web/index.html 的生成区共享同一 fingerprint，由
//! src/output/html.rs 的 fingerprint 测试对拍三份。改规范后重新运行生成器。
#![allow(dead_code)] // 常量面向 Rust / 页面 / 测试多张面孔，任一单独构建里可能有未消费项。

/// 规范化白名单的 sha256 指纹（对权威文件重算，见生成器）。
#[rustfmt::skip] // 生成物：整行按生成器写法保留，不让 cargo fmt 回绕，保证 --check 字节对拍。
pub(crate) const FINGERPRINT: &str = "32cf61e192b52ea82bd3d74cd735c90a8edd840bd126f835f29d399b6f77d789";

/// 净化白名单标签集合，`[x 顺序即权威顺序]`。
#[rustfmt::skip]
pub(crate) const ALLOWED_TABLE_TAGS: &[&str] = &[
    "html",
    "body",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "caption",
    "colgroup",
    "col",
    "tr",
    "th",
    "td",
    "br",
    "sub",
    "sup",
    "b",
    "i",
    "u",
    "em",
    "strong",
    "span",
    "p",
    "div",
    "ul",
    "ol",
    "li",
    "code",
    "small",
];

/// 净化白名单属性（只有整数 span 属性；align/valign 已按安全交集移除）。
#[rustfmt::skip]
pub(crate) const ALLOWED_TABLE_ATTRS: &[&str] = &["colspan", "rowspan"];

/// span 属性下界（含）。
pub(crate) const MIN_SPAN: u32 = 1;

/// span 属性上界（含）。相对旧库实现由 1000 收紧到 64。
pub(crate) const MAX_SPAN: u32 = 64;

/// 单元格文本策略（元数据；净化前置条件由引擎的 Escape 口径保证）。
pub(crate) const TEXT_POLICY: &str = "escape";
