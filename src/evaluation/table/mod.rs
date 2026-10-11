//! 表格识别评测。
//!
//! 与 `evaluation::formula` 的关系是**并列**而不是复用：公式域的主指标是
//! 文本编辑距离，表格域的主指标是结构编辑距离（TEDS），两者的样本键、报告字段与
//! 失败分类都不同。共享的只有 `evaluation::ocr` 的文本/几何指标与 `evaluation::stats`。
//!
//! # 本模块在 E7 只提供"能跑出第一个数字"的最小面
//!
//! `docs/09-table-recognition-execution.md` §9（E8）负责把评测协议定稿（ε、网格规则、
//! 构造性上界的扣除口径、报告 schema）。E7 需要的只是：加载 TabRecSet val 的
//! **真值**记录（`tools/table_detokenize.py` 的产物）、把图像路径解析成绝对路径、
//! 以及"因 span 超出 SLANet 词表而**必然**出错"的表格占比 —— 最后这项必须在看第一个
//! TEDS 数字**之前**算出来，否则会误判模型不可用（§8.4）。
//!
//! 预测执行与报告落盘在 `src/bin/table_eval.rs`，评分交给 `tools/table_teds.py`
//! （官方 `metric.py` 的 Python 依赖无法在 Rust 侧复现，也不该复现两份）。
//!
//! # E8 之后本模块多出两块
//!
//! - [`protocol`]：评测协议里可执行的那一半（ε、网格规则、构造性上界扣除）。
//!   协议文档是 `docs/10-table-evaluation-protocol.md`。
//! - [`run`]：把"真值记录 → 预测 HTML → 报告"这条链路从 `src/bin/table_eval.rs`
//!   搬进库里，使 `rapidstruct evaluate`（`src/bin/rapidstruct.rs`）与 `table_eval`
//!   两个入口跑的是**同一份**实现，而不是两份会各自漂移的复制。

pub mod fixture;
pub mod protocol;
pub mod run;

pub use fixture::{
    DEFAULT_DATASET_RELATIVE, DEFAULT_GT_HTML_RELATIVE, DEFAULT_MANIFEST_RELATIVE, SpanExceedance,
    SpanKind, SpanLimits, TableFixture, TableRecord, TableSample, load_pubtabnet_val, span_limits,
    span_usage, summarize_span_exceedance,
};
pub use protocol::{
    AUTHORITATIVE_GRID_SOURCE, GridSource, LOOP_TRIP_CAP, ProtocolSection, REPEAT_EPSILON,
    UNCOVERED_POSITION_RULE, UncoveredPosition, UpperBound, classify_upper_bound, protocol_section,
    within_repeat_epsilon,
};
pub use run::{
    TableContentSource, TableEvalConfig, TableEvalManifest, TableEvalOutcome, run_table_evaluation,
};
