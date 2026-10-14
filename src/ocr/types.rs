use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Quad;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordType {
    Cn,
    EnNum,
}

/// 逐字符 CTC forced alignment 这一轮的结论（`src/ocr/rec/align.rs`）。
///
/// 它是**记账字段**，不是开关：`Fallback` 意味着逐字框几何退回了"argmax 列 +
/// 整行平均字宽"这套旧估计法，调用方与评测必须能区分这两种框是从哪来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AlignStatus {
    /// 文本为空，没有可对齐的字符。
    #[default]
    Empty,
    /// 对齐可行：[`WordInfo::char_cols`] 为每个非空白字符给出了列区间。
    Aligned,
    /// 对齐不可行（帧数装不下标签序列、类别维越界，或回溯路径自相矛盾）：
    /// `char_cols` 无内容，几何退回旧估计法。**不静默降级。**
    Fallback,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WordInfo {
    pub words: Vec<Vec<String>>,
    pub word_cols: Vec<Vec<usize>>,
    pub word_types: Vec<WordType>,
    pub line_txt_len: f32,
    pub confs: Vec<f32>,
    /// forced alignment 得到的逐字符 CTC 列区间 `[start, end)`，结构与 `words`
    /// 平行（每个 word 内每个非空白字符一项）。`align_status != Aligned` 时为空。
    ///
    /// 注意这里给的是**列（帧）区间**，不是框：区间之间的 blank 归谁、边界落在
    /// 哪个像素，由 `word_boxes.rs` 的铺格决定。
    pub char_cols: Vec<Vec<[usize; 2]>>,
    pub align_status: AlignStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineResult {
    pub text: String,
    pub score: f32,
    pub word_info: Option<WordInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecognizeOutput {
    pub lines: Vec<LineResult>,
    #[serde(skip)]
    pub elapsed: Duration,
    pub preprocess_ms: f32,
    pub infer_ms: f32,
    pub postprocess_ms: f32,
}

impl RecognizeOutput {
    pub fn texts(&self) -> Vec<&str> {
        self.lines.iter().map(|line| line.text.as_str()).collect()
    }

    pub fn scores(&self) -> Vec<f32> {
        self.lines.iter().map(|line| line.score).collect()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordBox {
    pub text: String,
    pub score: f32,
    pub bbox: Quad,
}
