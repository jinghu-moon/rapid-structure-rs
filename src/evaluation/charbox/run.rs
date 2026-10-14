//! 字符定位基准的执行与报告（docs/13）。
//!
//! # 这一层存在的唯一理由
//!
//! Step 3 要把字符框从"CTC 列索引 + 整行平均字宽"换成 forced alignment。没有一份
//! 换之前测过的数字，那次改动就无法被判断是改进还是退化 —— 而"看起来更准"不是证据。
//! 所以这里产出的不是分数，而是**可复现的基线**：数据集由生成器按像素导出真值，
//! 每一页的 sha256 写进 manifest，跑之前逐张复核。
//!
//! # 两道闸门，不是一个
//!
//! * **文本闸门** `recognition.text == GT 行文本`：这就是"OCR 文本准确率"本身。
//! * **几何闸门**：逐字框数量相等 **且** 拼接后与 GT（去空格）逐字相等。
//!
//! 分开是因为它们会独立失败：识别器完全可能把整行读对却吞掉一个空格（文本闸门失败、
//! 几何闸门通过），也可能字数对但内容错（此时逐字对应关系不成立，任何按位配对的
//! IoU 都是在量两个不同字符）。几何指标只在几何闸门通过的行上累加，闸门通过率本身
//! 是一个被报告的指标 —— 失败不会被静默丢掉。
//!
//! # 三档成本
//!
//! `off / words / chars` 各跑 `repetitions` 遍，报墙钟、分阶段耗时与**响应字节数**。
//! 逐字框的代价最终会体现在响应体上，只报延迟会低估它。
//!
//! # 像素真值不在这里重算
//!
//! "GT 框是否真的框住了墨迹"由生成器负责（它量的是自己刚画出来的像素）。这里再用
//! Rust 量一遍只会留下"两份实现谁对"这个无法回答的问题；Rust 侧查的是与像素无关的
//! 结构不变量（见 `manifest.rs`）与 sha256。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};

use crate::api::{
    ClassifierPlan, ClassifierPolicy, DetectionPolicy, FormulaPolicy, ImageInput, OcrEngine,
    OcrOutput, OcrRegion, OcrRequest, OutputPolicy, PreprocessPolicy, RecognitionPolicy,
    RegionKind, StagePlan, TablePolicy, WordKind, WordOutputMode,
};
use crate::error::{RapidOcrError, Result};
use crate::evaluation::charbox::manifest::{Case, GroundLine, Manifest};
use crate::evaluation::charbox::metrics::{
    LineFrame, Quad, SelectionProbe, char_geometry, match_quads, neighbour_overlap,
    selection_counts,
};
use crate::evaluation::digest::digest_hex;
use crate::evaluation::ocr::character_error_rate;
use crate::evaluation::runtime_report::attach_runtime_fields;
use crate::evaluation::stats::Stats;
use crate::ocr::pipeline::config::EngineConfig;
use crate::ocr::pipeline::rapid_structure::RapidOcrEngine;
use crate::output::json::to_output_json;

pub const DEFAULT_DATASET_RELATIVE: &str = "datasets/text/charbox";
pub const DEFAULT_CONFIG_RELATIVE: &str = "models/configs/test-config.yaml";
/// 行级匹配阈值。字符级不另设阈值：几何闸门要求逐字一一对应，行没配上就没有字符
/// 可算，所以这里只需要一个"这一行被识别到了吗"的判据。
pub const DEFAULT_LINE_IOU: f32 = 0.3;

/// 一次基准运行的参数。
#[derive(Debug, Clone)]
pub struct CharBoxEvalConfig {
    pub dataset: PathBuf,
    pub engine_config: PathBuf,
    pub repetitions: usize,
    pub line_iou: f32,
    /// 只跑前 N 页（0 = 全量）；用于冒烟，不用于出基线。
    pub limit: usize,
    /// 参与成本测量的档位。几何评分始终需要 `Chars`。
    pub modes: Vec<WordOutputMode>,
}

impl Default for CharBoxEvalConfig {
    fn default() -> Self {
        Self {
            dataset: PathBuf::from(DEFAULT_DATASET_RELATIVE),
            engine_config: PathBuf::from(DEFAULT_CONFIG_RELATIVE),
            repetitions: 3,
            line_iou: DEFAULT_LINE_IOU,
            limit: 0,
            modes: vec![
                WordOutputMode::Off,
                WordOutputMode::Words,
                WordOutputMode::Chars,
            ],
        }
    }
}

/// 进度出口：CLI 打到 stderr，测试可以吞掉。
#[derive(Debug, Clone)]
pub enum CharBoxEvent {
    Message(String),
    Progress { done: usize, total: usize },
}

#[derive(Debug, Clone)]
pub struct CharBoxOutcome {
    pub report: Value,
    pub pages: usize,
    pub lines_matched: usize,
    pub chars_scored: usize,
    pub gate_failures: usize,
}

/// 一行（GT 行 + 配上的预测区域）算完之后的全部事实。
///
/// 先攒成值再落账，是因为同一份事实要同时并进"总体"和"该分组"两个累加器；
/// 直接在两个 `&mut Tally` 上写会让同一段逻辑出现两份。
#[derive(Debug, Default)]
struct LineFacts {
    text_exact: bool,
    cer: f64,
    /// 几何闸门结论。用枚举而不是 `Result`：本 crate 的 `Result` 别名只接一个参数
    /// （错误类型固定为 `RapidOcrError`），而这里要存的是"为什么这一行不参与几何
    /// 评分"的**数据**，不是需要向上传播的失败；枚举也才 `Default` 得出来。
    gate: LineGate,
    geometry: Vec<CharSample>,
    overlap_px: Vec<f64>,
    overlap_fraction: Vec<f64>,
    selection: SelectionProbe,
    selection_probes: usize,
    space_words_dropped: usize,
    non_char_words: usize,
}

#[derive(Debug, Default)]
enum LineGate {
    /// 逐字一一对应且文本相等：几何样本可以入账。
    #[default]
    Passed,
    /// 失败原因。它会直接成为报告里 `gate_failures` 的键，所以必须是一句
    /// 可聚合的短标签（带参数的用 `key:detail` 形式），不能把整行文本塞进去。
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
struct CharSample {
    iou_tight: f64,
    iou_band: f64,
    d_start: f64,
    d_end: f64,
    d_center: f64,
    d_cross_start: f64,
    d_cross_end: f64,
    width_ratio: f64,
    coverage: f64,
}

/// 一份（总体或某个分组的）累加器。
#[derive(Debug, Default)]
struct Tally {
    lines_total: usize,
    lines_matched: usize,
    regions_predicted: usize,
    chars_total: usize,
    chars_scored: usize,
    gate_passed_lines: usize,
    text_exact: usize,
    cer: Vec<f64>,
    gate_failures: BTreeMap<String, usize>,
    space_words_dropped: usize,
    non_char_words: usize,
    non_finite_samples: usize,
    iou_tight: Vec<f64>,
    iou_band: Vec<f64>,
    d_start: Vec<f64>,
    d_end: Vec<f64>,
    d_center: Vec<f64>,
    d_cross_start: Vec<f64>,
    d_cross_end: Vec<f64>,
    width_ratio: Vec<f64>,
    coverage: Vec<f64>,
    overlap_px: Vec<f64>,
    overlap_fraction: Vec<f64>,
    selection: SelectionProbe,
    selection_probes: usize,
}

impl Tally {
    fn observe(&mut self, facts: &LineFacts) {
        self.lines_matched += 1;
        self.cer.push(facts.cer);
        if facts.text_exact {
            self.text_exact += 1;
        }
        self.space_words_dropped += facts.space_words_dropped;
        self.non_char_words += facts.non_char_words;
        match &facts.gate {
            LineGate::Passed => {
                self.gate_passed_lines += 1;
                self.chars_scored += facts.geometry.len();
                self.merge_geometry(&facts.geometry);
                for value in &facts.overlap_px {
                    push_f64(&mut self.overlap_px, &mut self.non_finite_samples, *value);
                }
                for value in &facts.overlap_fraction {
                    push_f64(
                        &mut self.overlap_fraction,
                        &mut self.non_finite_samples,
                        *value,
                    );
                }
                self.selection.accumulate(&facts.selection);
                self.selection_probes += facts.selection_probes;
            }
            LineGate::Failed(reason) => {
                *self.gate_failures.entry(reason.clone()).or_insert(0) += 1;
            }
        }
    }

    /// 把一批逐字样本并入各自的样本池。
    ///
    /// 解构 `self` 而不是逐个 `self.push_f64(&mut self.field, ..)`：方法接收者会把整个
    /// `self` 借出去，与第一个参数相撞；解构后每个字段是独立借用，这也符合本层的
    /// 实际需求 —— 九个池子共用一个"非有限值"计数器。
    fn merge_geometry(&mut self, samples: &[CharSample]) {
        let Self {
            iou_tight,
            iou_band,
            d_start,
            d_end,
            d_center,
            d_cross_start,
            d_cross_end,
            width_ratio,
            coverage,
            non_finite_samples,
            ..
        } = self;
        for sample in samples {
            push_f64(iou_tight, non_finite_samples, sample.iou_tight);
            push_f64(iou_band, non_finite_samples, sample.iou_band);
            push_f64(d_start, non_finite_samples, sample.d_start);
            push_f64(d_end, non_finite_samples, sample.d_end);
            push_f64(d_center, non_finite_samples, sample.d_center);
            push_f64(d_cross_start, non_finite_samples, sample.d_cross_start);
            push_f64(d_cross_end, non_finite_samples, sample.d_cross_end);
            push_f64(width_ratio, non_finite_samples, sample.width_ratio);
            push_f64(coverage, non_finite_samples, sample.coverage);
        }
    }

    /// 总体报告：在分组报告的字段之外，额外带上行级区域归属字段。
    fn value(&self) -> Value {
        let mut value = self.common_value();
        let object = value
            .as_object_mut()
            .expect("common_value() always builds an object");
        object.insert(
            "regions_predicted".to_string(),
            json!(self.regions_predicted),
        );
        object.insert(
            "line_precision".to_string(),
            rate(self.lines_matched, self.regions_predicted),
        );
        value
    }

    /// 分组报告。
    ///
    /// 故意**不**带 `regions_predicted` / `line_precision`：一个没配上任何行的预测区域
    /// 无法归属到某个分组，硬凑一个分母只会让分组的 precision 读起来像
    /// "这一组没有多余检测"，而那正是本基准要用来判定的事情之一。
    fn group_value(&self) -> Value {
        self.common_value()
    }

    fn common_value(&self) -> Value {
        json!({
            "lines_total": self.lines_total,
            "lines_matched": self.lines_matched,
            "line_recall": rate(self.lines_matched, self.lines_total),
            "chars_total": self.chars_total,
            "chars_scored": self.chars_scored,
            "char_coverage": rate(self.chars_scored, self.chars_total),
            "geometry_gate_pass_rate_of_matched_lines": rate(self.gate_passed_lines, self.lines_matched),
            "text_exact_lines": self.text_exact,
            "text_exact_rate_of_matched_lines": rate(self.text_exact, self.lines_matched),
            "mean_cer": mean(&self.cer),
            "gate_failures": self.gate_failures,
            "space_only_word_boxes_dropped": self.space_words_dropped,
            "non_char_word_boxes": self.non_char_words,
            "non_finite_samples": self.non_finite_samples,
            "iou_tight": ratio_stats(&self.iou_tight),
            "iou_band": ratio_stats(&self.iou_band),
            "boundary_error": {
                "advance_start": signed_stats(&self.d_start),
                "advance_end": signed_stats(&self.d_end),
                "advance_center": signed_stats(&self.d_center),
                "cross_start": signed_stats(&self.d_cross_start),
                "cross_end": signed_stats(&self.d_cross_end),
            },
            "width_ratio": spread_stats(&self.width_ratio),
            "gt_span_covered_by_prediction": spread_stats(&self.coverage),
            "neighbour_overlap": {
                "pixels": signed_stats(&self.overlap_px),
                "fraction_of_gt_width": spread_stats(&self.overlap_fraction),
            },
            "selection": {
                "probes": self.selection_probes,
                "gt_in_selection": self.selection.gt_in_selection,
                "predicted_in_selection": self.selection.predicted_in_selection,
                "true_positive": self.selection.true_positive,
                "false_positive": self.selection.false_positive,
                "false_negative": self.selection.false_negative,
                "precision": rate(
                    self.selection.true_positive,
                    self.selection.true_positive + self.selection.false_positive
                ),
                "recall": rate(
                    self.selection.true_positive,
                    self.selection.true_positive + self.selection.false_negative
                ),
            },
        })
    }
}

/// 一页的规模摘要（报告里逐页一行，用于定位"哪一页把整组指标拉下去"）。
#[derive(Debug, Clone, Serialize)]
struct PageSummary {
    image: String,
    lines: usize,
    lines_matched: usize,
    chars_total: usize,
    chars_scored: usize,
    gate_failures: usize,
}

fn rate(numerator: usize, denominator: usize) -> Value {
    if denominator == 0 {
        return Value::Null;
    }
    json!(numerator as f64 / denominator as f64)
}

fn mean(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    json!(values.iter().sum::<f64>() / values.len() as f64)
}

/// 有符号误差：`mean` 是系统性偏移（正=沿进给方向偏后），`mean_abs` 才是幅度。
/// 只报其中一个会让"左右各偏一半"与"整体偏 3px"看起来一样。
fn signed_stats(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({ "samples": 0 });
    }
    let abs: Vec<f64> = values.iter().map(|value| value.abs()).collect();
    json!({
        "samples": values.len(),
        "mean": values.iter().sum::<f64>() / values.len() as f64,
        "mean_abs": abs.iter().sum::<f64>() / abs.len() as f64,
        "p50_abs": Stats::quantile(&abs, 0.5),
        "p90_abs": Stats::quantile(&abs, 0.9),
        "min": values.iter().copied().fold(f64::INFINITY, f64::min),
        "max": values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

fn ratio_stats(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({ "samples": 0 });
    }
    let at_least = |threshold: f64| values.iter().filter(|value| **value >= threshold).count();
    json!({
        "samples": values.len(),
        "mean": values.iter().sum::<f64>() / values.len() as f64,
        "p10": Stats::quantile(values, 0.1),
        "p50": Stats::quantile(values, 0.5),
        "at_least_0_5": at_least(0.5),
        "at_least_0_75": at_least(0.75),
        "share_at_least_0_5": rate(at_least(0.5), values.len()),
        "share_at_least_0_75": rate(at_least(0.75), values.len()),
    })
}

fn spread_stats(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({ "samples": 0 });
    }
    json!({
        "samples": values.len(),
        "mean": values.iter().sum::<f64>() / values.len() as f64,
        "p10": Stats::quantile(values, 0.1),
        "p50": Stats::quantile(values, 0.5),
        "p90": Stats::quantile(values, 0.9),
        "min": values.iter().copied().fold(f64::INFINITY, f64::min),
        "max": values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

fn mode_name(mode: WordOutputMode) -> &'static str {
    match mode {
        WordOutputMode::Off => "off",
        WordOutputMode::Words => "words",
        WordOutputMode::Chars => "chars",
    }
}

fn request(bytes: Vec<u8>, words: WordOutputMode) -> OcrRequest {
    OcrRequest {
        input: ImageInput::Encoded(Arc::from(bytes)),
        roi: None,
        scale_hint: None,
        stages: StagePlan {
            detect: true,
            classify: ClassifierPlan {
                policy: ClassifierPolicy::IfAvailable,
                apply_rotation: true,
            },
            recognize: true,
        },
        preprocess: PreprocessPolicy::default(),
        detection: DetectionPolicy::default(),
        recognition: RecognitionPolicy { words },
        output: OutputPolicy::default(),
        formula: FormulaPolicy::default(),
        table: TablePolicy::default(),
    }
}

/// 带文本的区域才是候选：公式/表格区域没有逐字框，参与匹配只会让它们去抢行。
fn text_regions(output: &OcrOutput) -> Vec<&OcrRegion> {
    output
        .regions
        .iter()
        .filter(|region| {
            region.kind == RegionKind::Text
                && region.polygon.is_some()
                && region.recognition.is_some()
        })
        .collect()
}

/// 把一页的识别结果与真值对上，产出一行的事实集合。
fn score_page(
    case: &Case,
    output: &OcrOutput,
    line_iou: f32,
    overall: &mut Tally,
    by_group: &mut BTreeMap<String, Tally>,
) -> PageSummary {
    let regions = text_regions(output);
    let gt_quads: Vec<Quad> = case.lines.iter().map(|line| line.quad).collect();
    let predicted_quads: Vec<Quad> = regions
        .iter()
        .map(|region| {
            region
                .polygon
                .map(|polygon| polygon.points)
                .unwrap_or_default()
        })
        .collect();
    let pairs = match_quads(&gt_quads, &predicted_quads, line_iou);

    let chars_total: usize = case.lines.iter().map(|line| line.chars.len()).sum();
    overall.lines_total += case.lines.len();
    overall.regions_predicted += regions.len();
    overall.chars_total += chars_total;
    let chars_before = overall.chars_scored;
    let mut failures = 0usize;
    for (gt_index, predicted_index, _) in &pairs {
        let line = &case.lines[*gt_index];
        let facts = score_line(line, regions[*predicted_index]);
        failures += usize::from(matches!(facts.gate, LineGate::Failed(_)));
        overall.observe(&facts);
        by_group
            .entry(line.group.clone())
            .or_default()
            .observe(&facts);
    }
    // 分组的 lines_total / chars_total 必须把**没配上**的行也算进去，否则
    // "这一组配上了多少行"会被算成 100%。逐行累加而不是按页累加，是因为一页
    // 里可以混着多个分组。
    for line in &case.lines {
        let group = by_group.entry(line.group.clone()).or_default();
        group.lines_total += 1;
        group.chars_total += line.chars.len();
    }
    PageSummary {
        image: case.image.clone(),
        lines: case.lines.len(),
        lines_matched: pairs.len(),
        chars_total,
        chars_scored: overall.chars_scored - chars_before,
        gate_failures: failures,
    }
}

/// 一行：文本闸门 + 几何闸门 + 通过后的逐字度量与选区探针。
fn score_line(line: &GroundLine, region: &OcrRegion) -> LineFacts {
    // `gate` 默认是 `Passed`：本函数只在确实不满足配对条件时才提前返回并改成
    // `Failed`，读完整个函数还走到末尾的行就是可以几何评分的行。
    let mut facts = LineFacts::default();
    let recognition = region
        .recognition
        .as_ref()
        .expect("text_regions() already filtered for a recognition outcome");
    facts.text_exact = recognition.text == line.text;
    facts.cer = character_error_rate(&line.text, &recognition.text) as f64;

    let Some(frame) = LineFrame::from_quad(&line.quad, &line.layout) else {
        facts.gate = LineGate::Failed("line_frame_degenerate".to_string());
        return facts;
    };
    let line_extent = frame.project(&line.quad);

    let Some(words) = recognition.words.as_ref() else {
        facts.gate = LineGate::Failed("no_words_field".to_string());
        return facts;
    };
    let mut predicted: Vec<(String, Quad)> = Vec::with_capacity(words.len());
    for word in words {
        if word.text.chars().all(|ch| ch.is_whitespace()) {
            facts.space_words_dropped += 1;
            continue;
        }
        if word.kind != WordKind::Char {
            facts.non_char_words += 1;
        }
        predicted.push((word.text.clone(), word.polygon.points));
    }
    let gt_chars: Vec<&str> = line.chars.iter().map(|ch| ch.ch.as_str()).collect();
    let predicted_chars: Vec<&str> = predicted.iter().map(|(text, _)| text.as_str()).collect();
    if predicted.len() != gt_chars.len() {
        facts.gate = LineGate::Failed(format!(
            "char_count_mismatch:{}_vs_{}",
            gt_chars.len(),
            predicted.len()
        ));
        return facts;
    }
    if predicted_chars != gt_chars {
        facts.gate = LineGate::Failed("char_text_mismatch".to_string());
        return facts;
    }

    let quads: Vec<Quad> = predicted.iter().map(|(_, quad)| *quad).collect();
    let gt_quads: Vec<Quad> = line.chars.iter().map(|ch| ch.quad).collect();
    let extents: Vec<_> = quads.iter().map(|quad| frame.project(quad)).collect();
    let gt_extents: Vec<_> = gt_quads.iter().map(|quad| frame.project(quad)).collect();
    for index in 0..quads.len() {
        let geometry = char_geometry(&gt_quads[index], &quads[index], &frame, line_extent.cross);
        facts.geometry.push(CharSample {
            iou_tight: geometry.iou_tight as f64,
            iou_band: geometry.iou_band as f64,
            d_start: geometry.d_start as f64,
            d_end: geometry.d_end as f64,
            d_center: geometry.d_center as f64,
            d_cross_start: geometry.d_cross_start as f64,
            d_cross_end: geometry.d_cross_end as f64,
            width_ratio: geometry.width_ratio as f64,
            coverage: geometry.coverage as f64,
        });
        if index + 1 < quads.len() {
            let overlap = neighbour_overlap(&extents[index], &extents[index + 1]) as f64;
            let gt_width =
                (gt_extents[index + 1].along.1 - gt_extents[index + 1].along.0).max(1.0) as f64;
            facts.overlap_px.push(overlap);
            facts.overlap_fraction.push(overlap / gt_width);
        }
    }

    // 选区探针：整行、行前后两半、以及每个 GT 字符自己的跨度。这就是网页文本层里
    // 一次拖拽能落到的三种位置，不需要随机采样（随机会让基线不可复现）。
    let aligned: Vec<Option<usize>> = (0..quads.len()).map(Some).collect();
    let mut ranges = vec![
        line_extent.along,
        (
            line_extent.along.0,
            (line_extent.along.0 + line_extent.along.1) / 2.0,
        ),
        (
            (line_extent.along.0 + line_extent.along.1) / 2.0,
            line_extent.along.1,
        ),
    ];
    ranges.extend(gt_extents.iter().map(|extent| extent.along));
    for range in ranges {
        facts.selection.accumulate(&selection_counts(
            &frame, &gt_quads, &quads, &aligned, range,
        ));
        facts.selection_probes += 1;
    }
    facts
}

/// 跑完整份基准。
pub fn run_charbox_evaluation(
    config: &CharBoxEvalConfig,
    on_event: &mut dyn FnMut(CharBoxEvent),
) -> Result<CharBoxOutcome> {
    if config.repetitions == 0 {
        return Err(RapidOcrError::InvalidInput(
            "repetitions must be at least 1; with none there is nothing to average".to_string(),
        ));
    }
    if !config.modes.contains(&WordOutputMode::Chars) {
        return Err(RapidOcrError::InvalidInput(
            "the charbox benchmark scores character geometry, so `modes` must include Chars; \
             without it there is nothing to measure, so the run is refused rather than \
             reported as an empty success"
                .to_string(),
        ));
    }
    let root = config.dataset.clone();
    let manifest = Manifest::load_verified(&root)?;
    let mut cases = manifest.cases.iter().collect::<Vec<_>>();
    if config.limit > 0 {
        cases.truncate(config.limit);
    }
    message(on_event, format!("charbox: {} page(s) to run", cases.len()));

    let engine_config = EngineConfig::from_yaml_file(&config.engine_config)?;
    let fingerprint = digest_hex(
        serde_json::to_string(&engine_config)
            .map_err(|error| RapidOcrError::Config(error.to_string()))?
            .as_bytes(),
    );
    let engine = RapidOcrEngine::new(engine_config)?;
    message(
        on_event,
        format!(
            "charbox: engine {} (fingerprint {})",
            config.engine_config.display(),
            fingerprint
        ),
    );

    // 冷启动那一遍会把 ORT 会话构建、字典加载与页缓存缺失都算进第一张图，
    // 所以先跑一次不计数；报告里显式说明做过预热，而不是让读者猜第一页为什么慢。
    let mut engine = engine;
    let warmup = std::fs::read(root.join(&cases[0].image))?;
    engine.recognize(request(warmup, WordOutputMode::Chars))?;

    let mut overall = Tally::default();
    let mut by_group: BTreeMap<String, Tally> = BTreeMap::new();
    let mut costs: BTreeMap<String, CostSamples> = config
        .modes
        .iter()
        .map(|mode| (mode_name(*mode).to_string(), CostSamples::default()))
        .collect();
    let mut pages = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let bytes = std::fs::read(root.join(&case.image))?;
        let mut scored: Option<OcrOutput> = None;
        for repetition in 0..config.repetitions {
            // 每一遍把档位起点轮转一格。不这样做的话，每一页都是同一档第
            // 一个跑（页内文件缓存、页内预热、系统频率都按跑序偏），那个偏移会
            // 原封不动地当成那一档的代价写进报告。
            let mut order = config.modes.clone();
            order.rotate_left(repetition % config.modes.len());
            for mode in &order {
                let started = Instant::now();
                let output = engine.recognize(request(bytes.clone(), *mode))?;
                let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
                let payload =
                    serde_json::to_string(&to_output_json(&output)?).map_err(|error| {
                        RapidOcrError::Decode(format!(
                            "the OCR output is not serializable: {error}"
                        ))
                    })?;
                let payload = payload.len();
                costs
                    .get_mut(mode_name(*mode))
                    .expect("every requested mode has a cost bucket")
                    .push(wall_ms, &output, payload);
                if *mode == WordOutputMode::Chars && scored.is_none() {
                    scored = Some(output);
                }
            }
        }
        for bucket in costs.values_mut() {
            bucket.finish_page();
        }
        let output = scored.expect("modes were validated to contain Chars");
        let summary = score_page(case, &output, config.line_iou, &mut overall, &mut by_group);
        pages.push(summary);
        on_event(CharBoxEvent::Progress {
            done: index + 1,
            total: cases.len(),
        });
    }

    let derived = derived_counts(&manifest);
    // 报告构造只**读**累加器：它还要在调用方继续提供 outcome 的几个计数，
    // 所以这里传引用而不是把所有权交出去。
    let report = build_report(
        config,
        &manifest,
        &root,
        &fingerprint,
        &overall,
        &by_group,
        &costs,
        &pages,
        &derived,
    )?;
    let gate_failures: usize = overall.gate_failures.values().sum::<usize>();
    Ok(CharBoxOutcome {
        report,
        pages: cases.len(),
        lines_matched: overall.lines_matched,
        chars_scored: overall.chars_scored,
        gate_failures,
    })
}

fn derived_counts(manifest: &Manifest) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for case in &manifest.cases {
        for line in &case.lines {
            for ch in &line.chars {
                *counts
                    .entry(ch.derived.clone().unwrap_or_else(|| "unset".to_string()))
                    .or_insert(0) += 1;
            }
        }
    }
    counts
}

#[allow(clippy::too_many_arguments)]
fn build_report(
    config: &CharBoxEvalConfig,
    manifest: &Manifest,
    root: &Path,
    fingerprint: &str,
    overall: &Tally,
    by_group: &BTreeMap<String, Tally>,
    costs: &BTreeMap<String, CostSamples>,
    pages: &[PageSummary],
    derived: &BTreeMap<String, usize>,
) -> Result<Value> {
    // 逐页 `名字\0sha256` 用换行拼接后摘要：NUL 不可能出现在文件名或哈希里，
    // 所以拼接方式无歧义（不需要再包一层 JSON 字符串转义）。
    let listing = manifest
        .cases
        .iter()
        .map(|case| format!("{}\u{0}{}", case.image, case.sha256))
        .collect::<Vec<_>>()
        .join("\n");
    let dataset_digest = digest_hex(listing.as_bytes());
    let chars_total = manifest.total_gt_chars();
    let mut cost_section = serde_json::Map::new();
    for (mode, samples) in costs {
        cost_section.insert(mode.clone(), samples.value());
    }
    let delta =
        |from: &str, to: &str, field: &str| -> Value { paired_delta(from, to, field, costs) };
    // 某个档位没被请求时 `paired_delta` 返回 Null，直接从数组里滤掉：报告里一堆
    // null 会让读者以为对应的那项测量失败了。
    let cost_delta = [
        delta("off", "words", "total_ms"),
        delta("off", "chars", "total_ms"),
        delta("words", "chars", "total_ms"),
        delta("off", "chars", "recognize_ms"),
        delta("words", "chars", "recognize_ms"),
        // 逐字框是在识别阶段的 postprocess 里算出来的（CTC 后处理 + 单应变换），
        // 所以这一项才是它能被分离出来的地方：整页 `total_ms` 由检测主导（本机
        // ≈ 5.5 s），档位差淹在跨页方差里，拿它说"逐字框值多少毫秒"是不成立的。
        delta("off", "chars", "rec_postprocess_ms"),
        delta("words", "chars", "rec_postprocess_ms"),
        delta("off", "chars", "wall_ms"),
        delta("off", "chars", "payload_bytes"),
        delta("words", "chars", "payload_bytes"),
        delta("off", "chars", "word_boxes_mean"),
    ]
    .into_iter()
    .filter(|value| !value.is_null())
    .collect::<Vec<Value>>();
    let mut report = json!({
        "schema": 1,
        "domain": "charbox",
        "dataset": {
            "root": root.display().to_string(),
            "generator": manifest.generator,
            "schema": manifest.schema,
            "cases": manifest.case_count,
            "lines": manifest.line_count,
            "gt_chars": chars_total,
            "gt_derived": derived,
            "dataset_digest": dataset_digest,
            "page_sha256": manifest.cases.iter().map(|case| (case.image.clone(), case.sha256.clone())).collect::<Vec<_>>(),
        },
        "engine": {
            "config": config.engine_config.display().to_string(),
            "fingerprint": fingerprint,
            "repetitions": config.repetitions,
            "warmup_pages": 1,
            "line_iou_threshold": config.line_iou,
            "modes": config.modes.iter().map(|mode| mode_name(*mode)).collect::<Vec<_>>(),
        },
        "overall": overall.value(),
        "by_group": by_group.iter().map(|(group, tally)| (group.clone(), tally.group_value())).collect::<serde_json::Map<_, _>>(),
        "cost": cost_section,
        "cost_delta_method": "每档逐页取 repetitions 的均值，再**逐页配对**相减（页序对齐）；页内的档位运行顺序按 repetition 轮转，避免固定的首跑位置偏差。逐字框的代价主要落在识别阶段的 postprocess 与响应体上，所以除了 total_ms 还单报 rec_postprocess_ms / recognize_ms / wall_ms / payload_bytes / word_boxes_mean。",
        "cost_delta": cost_delta,
        "pages": serde_json::to_value(pages).map_err(|error| {
            RapidOcrError::Decode(format!("the per-page summary is not serializable: {error}"))
        })?,
        "not_reported": [
            "pixel re-derivation of the ground-truth boxes: the generator measures the ink it just drew, and re-implementing that here would leave two implementations to argue about",
            "TEDS-style structural table metrics and formula CDM: different domains, different harnesses (see src/evaluation/mod.rs)",
        ],
    });
    attach_runtime_fields(&mut report)?;
    Ok(report)
}

/// 一档模式的耗时/体积样本。
///
/// 三份池子各有用途：`samples` 是全部逐次样本（分位数），`page` 是当前页尚未
/// 折算的样本，`per_page` 是每页一个均值（配对差）。需要 `per_page` 是因为**页间**
/// 检测耗时的差异比**档位间**差异大两个量级：上一版只报不配对的均值差，
/// `off→chars` 与 `words→chars` 的 `total_ms` 差在负 21.6 ms 与正 30.1 ms 之间
/// 跳，而两者都不可能是逐字框的真实代价（逐字框只动识别后的后处理）。不配对
/// 就是把 15 个不同大小的页面当成同一个总体抽样本。
#[derive(Debug, Default, Clone)]
struct CostSamples {
    samples: FieldPools,
    page: FieldPools,
    per_page: FieldPools,
}

/// 一次运行可读到的全部耗时/体积量。
#[derive(Debug, Clone, Copy)]
struct CostReading {
    wall_ms: f64,
    total_ms: f64,
    detect_ms: f64,
    recognize_ms: f64,
    rec_infer_ms: f64,
    rec_postprocess_ms: f64,
    postprocess_ms: f64,
    payload_bytes: f64,
    word_boxes: f64,
}

/// 同形的一组样本池。字段名就是报告里的键名，`field()` 按名取回，
/// 这样新增一个量只需要改三处（结构体、`record`、`field`）而不是四处。
#[derive(Debug, Default, Clone)]
struct FieldPools {
    wall_ms: Vec<f64>,
    total_ms: Vec<f64>,
    detect_ms: Vec<f64>,
    recognize_ms: Vec<f64>,
    rec_infer_ms: Vec<f64>,
    rec_postprocess_ms: Vec<f64>,
    postprocess_ms: Vec<f64>,
    payload_bytes: Vec<f64>,
    word_boxes_mean: Vec<f64>,
}

impl FieldPools {
    fn record(&mut self, reading: &CostReading) {
        self.wall_ms.push(reading.wall_ms);
        self.total_ms.push(reading.total_ms);
        self.detect_ms.push(reading.detect_ms);
        self.recognize_ms.push(reading.recognize_ms);
        self.rec_infer_ms.push(reading.rec_infer_ms);
        self.rec_postprocess_ms.push(reading.rec_postprocess_ms);
        self.postprocess_ms.push(reading.postprocess_ms);
        self.payload_bytes.push(reading.payload_bytes);
        self.word_boxes_mean.push(reading.word_boxes);
    }

    /// 把当前页的样本折算成一个均值并进 `target`，然后清空自己。
    fn fold_page_into(&mut self, target: &mut Self) {
        let source = self.clone();
        target.wall_ms.push(mean_or_zero(&source.wall_ms));
        target.total_ms.push(mean_or_zero(&source.total_ms));
        target.detect_ms.push(mean_or_zero(&source.detect_ms));
        target.recognize_ms.push(mean_or_zero(&source.recognize_ms));
        target.rec_infer_ms.push(mean_or_zero(&source.rec_infer_ms));
        target
            .rec_postprocess_ms
            .push(mean_or_zero(&source.rec_postprocess_ms));
        target
            .postprocess_ms
            .push(mean_or_zero(&source.postprocess_ms));
        target
            .payload_bytes
            .push(mean_or_zero(&source.payload_bytes));
        target
            .word_boxes_mean
            .push(mean_or_zero(&source.word_boxes_mean));
        *self = Self::default();
    }

    fn field(&self, name: &str) -> &[f64] {
        match name {
            "wall_ms" => &self.wall_ms,
            "total_ms" => &self.total_ms,
            "detect_ms" => &self.detect_ms,
            "recognize_ms" => &self.recognize_ms,
            "rec_infer_ms" => &self.rec_infer_ms,
            "rec_postprocess_ms" => &self.rec_postprocess_ms,
            "postprocess_ms" => &self.postprocess_ms,
            "payload_bytes" => &self.payload_bytes,
            "word_boxes_mean" => &self.word_boxes_mean,
            _ => &[],
        }
    }
}

impl CostSamples {
    fn push(&mut self, wall_ms: f64, output: &OcrOutput, payload: usize) {
        let timings = &output.timings;
        let word_boxes: usize = output
            .regions
            .iter()
            .filter_map(|region| region.recognition.as_ref())
            .map(|recognition| recognition.words.as_ref().map_or(0, Vec::len))
            .sum();
        let reading = CostReading {
            wall_ms,
            total_ms: timings.total_ms as f64,
            detect_ms: timings.detect_ms as f64,
            recognize_ms: timings.recognize_ms as f64,
            rec_infer_ms: timings.recognizer_infer_ms as f64,
            rec_postprocess_ms: timings.recognizer_postprocess_ms as f64,
            postprocess_ms: (timings.total_ms - timings.detect_ms - timings.recognize_ms).max(0.0)
                as f64,
            payload_bytes: payload as f64,
            word_boxes: word_boxes as f64,
        };
        self.samples.record(&reading);
        self.page.record(&reading);
    }

    /// 一页跑完（所有档位、所有遍数）后调用。
    fn finish_page(&mut self) {
        self.page.fold_page_into(&mut self.per_page);
    }

    fn value(&self) -> Value {
        json!({
            "samples": self.samples.wall_ms.len(),
            "pages": self.per_page.wall_ms.len(),
            "wall_ms": { "mean": mean(&self.samples.wall_ms), "p50": Stats::quantile(&self.samples.wall_ms, 0.5) },
            "total_ms": { "mean": mean(&self.samples.total_ms), "p50": Stats::quantile(&self.samples.total_ms, 0.5) },
            "detect_ms": mean(&self.samples.detect_ms),
            "recognize_ms": mean(&self.samples.recognize_ms),
            "rec_infer_ms": mean(&self.samples.rec_infer_ms),
            "rec_postprocess_ms": mean(&self.samples.rec_postprocess_ms),
            "postprocess_ms": mean(&self.samples.postprocess_ms),
            "payload_bytes_mean": mean(&self.samples.payload_bytes),
            "payload_bytes_max": self.samples.payload_bytes.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            "word_boxes_mean": mean(&self.samples.word_boxes_mean),
        })
    }
}

/// 逐页配对的档位差。`from`/`to` 两侧必须是同一次运行里同一页顺序的均值序列，
/// 长度不等意味着某个档位中途失败过 —— 宁可不报，也不拿错位的两列相减。
fn paired_delta(from: &str, to: &str, field: &str, costs: &BTreeMap<String, CostSamples>) -> Value {
    let (Some(left), Some(right)) = (costs.get(from), costs.get(to)) else {
        return Value::Null;
    };
    let (lhs, rhs) = (left.per_page.field(field), right.per_page.field(field));
    if lhs.is_empty() || lhs.len() != rhs.len() {
        return json!({
            "from": from, "to": to, "field": field, "method": "paired_by_page",
            "unavailable": "the two modes do not have one per-page mean each, aligned by page",
        });
    }
    let diffs: Vec<f64> = lhs
        .iter()
        .zip(rhs)
        .map(|(before, after)| after - before)
        .collect();
    json!({
        "from": from,
        "to": to,
        "field": field,
        "method": "paired_by_page",
        "pages": diffs.len(),
        "delta": signed_stats(&diffs),
    })
}

fn mean_or_zero(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// 非有限值不能进样本池：分位数函数会**断言**全有限，让一次九分钟的运行在最后
/// 一步 panic 是不可接受的失败方式；而且一个 NaN 也不该被平均进基线。
fn push_f64(values: &mut Vec<f64>, non_finite: &mut usize, value: f64) {
    if value.is_finite() {
        values.push(value);
    } else {
        *non_finite += 1;
    }
}

fn message(on_event: &mut dyn FnMut(CharBoxEvent), text: String) {
    on_event(CharBoxEvent::Message(text));
}
