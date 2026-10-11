//! 共享评测框架。
//!
//! 按领域拆分：`evaluation::ocr` 承载普通 OCR 文本/检测指标；
//! `evaluation::formula` 承载公式识别指标；`evaluation::table` 承载表格识别指标。
//! 三类指标不得混合成一个默认汇总。
//!
//! `evaluation::runtime_report` 是唯一**跨域共享**的一层：它只管报告里那四个与域无关的
//! 运行时字段（峰值工作集、内存口径、ORT 指纹与版本）。共享它不违反上面那条规则 ——
//! 共享的是"两份报告跑在同一份运行库上"这个证据，不是指标本身。

pub mod formula;
pub mod ocr;
pub mod runtime_report;
pub mod stats;
pub mod table;
