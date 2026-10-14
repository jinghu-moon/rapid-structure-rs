//! 字符定位基准（docs/13）。
//!
//! 与 `evaluation::ocr`（文本/检测）、`evaluation::formula`、`evaluation::table` 并列，
//! 但**不共享汇总**：这里量的是"每个字符的框画在对不对的地方"，不是"文本读对没读对"。
//! 把它们混成一个默认汇总会让一次字符框改动在 CER 上看起来"没有影响"，而它确实可能
//! 没有影响 —— 这正是需要分开报的理由。
//!
//! * [`metrics`]：纯几何度量（行局部坐标系、tight/band 两种 IoU、选区 P/R）。
//! * [`manifest`]：数据集读取 + sha256 复核 + 与像素无关的结构自检。
//! * [`run`]：跑三档 `words` 输出、累加指标、产出报告。
//!
//! 数据集**不入库**（渲染结果绑定本机字体版本），由 `tools/build_charbox_dataset.py`
//! 生成；它同时负责"GT 框确实框住了墨迹"这件事，本模块不重做一遍。

pub mod manifest;
pub mod metrics;
pub mod run;
