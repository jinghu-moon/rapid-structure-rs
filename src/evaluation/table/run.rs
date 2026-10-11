//! 表格结构评测链路（真值记录 → 预测 HTML → 报告）。
//!
//! # 为什么这段代码在库里而不在 `src/bin/table_eval.rs`
//!
//! `docs/09-table-recognition-execution.md` §9.2 要求
//! `rapidstruct evaluate --manifest <table manifest>` 能产出表格域报告。如果链路留在
//! 二进制里，`rapidstruct` 就只能**再写一份**（或者让 `rapidstruct` 去调用另一个二进制）——
//! 前者的两份实现会各自漂移，后者会把评测结果变成子进程的 stdout 文本。
//! 因此 E8 把链路搬进库里：`src/bin/table_eval.rs` 与 `src/bin/rapidstruct.rs`
//! 都只是这份实现的薄壳。
//!
//! # 打分不在 Rust 里做
//!
//! 官方 TEDS 实现（`refer/PubTabNet-metric/metric.py`，6,181 B，
//! sha256 `a9aedd313678b42e55b06c4b0a365d658ef495aaf3abb7fd6012a6e9f334f9f9`）
//! 的树编辑距离语义（`CustomConfig.rename` 对 `tag`/`colspan`/`rowspan`/`content`
//! 的处理）**就是指标的定义**。在 Rust 里再实现一遍意味着"两个实现谁对"永远无法回答；
//! 本模块只实现**解码**（模型 → HTML），评分留给唯一的那份官方实现
//! （`tools/table_teds.py`）。
//!
//! # 两种口径：结构（TEDS-Struct）与完整（TEDS）
//!
//! 单元格文本有两个来源（[`TableContentSource`]）：
//!
//! - [`TableContentSource::StructureOnly`]：每个单元格传**空串**，对应 `metric.py` 的
//!   `structure_only=True`，只衡量结构。报告标注 `reported_metric = "TEDS-Struct"`。
//! - [`TableContentSource::PageReuse`]：**复用整页 OCR 的结果**做几何匹配
//!   （`docs/09-table-recognition-execution.md` §10 = E9，即 `docs/07` 的 R1）。
//!   报告标注 `reported_metric = "TEDS"`。
//!
//! 把 TEDS-Struct 写成 TEDS 是**错误陈述**，不是简写 —— 两个数字不可互换，
//! 也不可与 PubTabNet 论文里的 SLANet TEDS 直接对比（评测集与词表都不同）。
//!
//! # 为什么页级 OCR 要缓存
//!
//! 页级 OCR 与表格模型**无关**：换结构模型、改回填规则、调结构分辨率都不该让
//! 7,104 张图的页级 OCR 重跑一遍（那是小时级成本）。因此 `PageReuse` 允许给一个
//! 缓存文件（[`OcrCache`]）；命中就不重跑，只在文件末尾追加新样本。
//!
//! 但"与表格模型无关"不等于"与引擎配置无关"：**页级文本是引擎配置的函数**。
//! 因此缓存里存了产出它的引擎指纹（[`engine_fingerprint`]），指纹不符时
//! [`OcrCache::load`] **报错**而不是复用 —— 见那个函数的文档。
//!
//! # 失败样本写空串，不删除
//!
//! 官方 `metric.py` 的 `evaluate` 对空预测返回 0.0，因此失败会**如实拉低**总分。
//! 把失败样本从预测文件里删掉会让 `batch_evaluate` 拿不到键，从而把"模型答错"
//! 变成"样本不存在" —— 那是只报告对模型有利样本的做法，
//! `crate::evaluation::formula::sampling` 的模块文档明确禁止。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    api::{
        ClassifierPlan, ClassifierPolicy, DetectionPolicy, ImageInput, OcrEngine, OcrOutput,
        OcrRequest, OutputPolicy, PreprocessPolicy, RecognitionPolicy, StagePlan, WordOutputMode,
    },
    config::{RecImage, RuntimeConfig},
    error::{RapidOcrError, Result},
    evaluation::{
        runtime_report::attach_runtime_fields,
        table::{
            fixture::{
                SpanLimits, TableFixture, TableSample, load_pubtabnet_val, span_limits, span_usage,
                summarize_span_exceedance,
            },
            protocol::{LOOP_TRIP_CAP, UpperBound, classify_upper_bound, protocol_section},
        },
    },
    ocr::{
        config::RecognizeOptions,
        pipeline::{config::EngineConfig, rapid_structure::RapidOcrEngine},
        rec::recognizer::Recognizer,
    },
    table::{
        cell_content_backfill::{
            BackfillOutcome, BackfillSource, CellRecognizer, TextLine, axis_aligned_bbox, backfill,
        },
        decode::{StructureVocabulary, argmax_structure, decode_token_steps},
        emitter::{
            RenderedTable, StructureConfidence, StructureDegradation, TextPolicy, emit_html,
        },
        ocr_resolution::TableOcrResolution,
        preprocess::table_structure_input,
        structure::{TableStructureConfig, TableStructureModelInfo, TableStructureSession},
    },
};

/// 默认结构模型路径。
pub const DEFAULT_MODEL_RELATIVE: &str = "models/table/slanet-plus.onnx";

/// 单元格文本的来源。
///
/// 两种口径的差别不是"快慢"而是"报的是哪个指标"：`StructureOnly` 报 TEDS-Struct，
/// `PageReuse` 报完整 TEDS。混用会把两个不可比的数字放进同一个字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableContentSource {
    /// 结构口径：每个单元格传空串（`metric.py` 的 `structure_only=True`）。
    StructureOnly,
    /// 复用整页 OCR 的结果做几何匹配（`docs/07` 的 R1 / `docs/09` §10 = E9）。
    PageReuse {
        /// 页级 OCR 的引擎配置 YAML；`None` 用 `EngineConfig::default()`。
        config: Option<PathBuf>,
        /// 页级 OCR 结果的缓存文件；给出时命中就不重跑 OCR。
        cache: Option<PathBuf>,
    },
    /// **对照实现**：逐单元格裁剪喂识别器。
    ///
    /// 这不是生产路径，存在的唯一理由是 `docs/09` §10.3 要求证明"复用整页结果的
    /// 非空率**不低于**逐 cell 裁剪喂 rec"。没有这条对照，"复用没有丢文本"就只是
    /// 一个未经检验的说法。
    CropControl {
        /// 识别器的引擎配置 YAML；`None` 用 `EngineConfig::default()`。
        config: Option<PathBuf>,
    },
}

impl TableContentSource {
    /// 报告里 `metric_scope.reported_metric` 的取值。
    pub fn reported_metric(&self) -> &'static str {
        match self {
            Self::StructureOnly => "TEDS-Struct",
            Self::PageReuse { .. } | Self::CropControl { .. } => "TEDS",
        }
    }

    /// 报告里 `content_source` 的取值（snake_case，供脚本判别）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StructureOnly => "structure_only",
            Self::PageReuse { .. } => "page_reuse",
            Self::CropControl { .. } => "crop_control",
        }
    }

    /// 是否需要从引擎配置里建模型会话。
    pub fn uses_engine(&self) -> bool {
        !matches!(self, Self::StructureOnly)
    }

    /// 报告里 `metric_scope.structure_only` 的取值。
    pub fn is_structure_only(&self) -> bool {
        matches!(self, Self::StructureOnly)
    }
}

/// 一次表格评测的输入。
#[derive(Debug, Clone)]
pub struct TableEvalConfig {
    /// 数据集根目录（含 `derived/pubtabnet_val.jsonl`）。
    pub dataset: PathBuf,
    /// 结构模型路径。
    pub model: PathBuf,
    /// 预测输出（官方 `metric.py` 的 `pred_json`：`{file_name: html}`）。
    pub predictions: PathBuf,
    /// 只跑前 N 个稳定哈希样本；0 = 全量。
    pub limit: usize,
    /// 结构模型输入边长（模型导出固定为 488）。
    pub side_len: usize,
    /// 是否固定输入 H/W（`docs/09` §6）。关掉只用于对照。
    pub fix_input_shape: bool,
    /// ORT intra 线程数；`None` 时沿用 `RuntimeConfig` 的自动推导。
    pub intra_threads: Option<usize>,
    /// 遇到第一个失败样本就退出（默认记录失败并继续）。
    pub fail_fast: bool,
    /// 单元格文本来源。
    pub content: TableContentSource,
    /// 表格路径自己的 det 分辨率（`docs/09` §11 = `docs/07` 的 R2）。
    ///
    /// 只在 `content = PageReuse` 时有意义：只有那条路径会跑 det。
    /// 它**无条件覆盖** `ocr_config` 里的 `global.max_side_len` 与
    /// `det.limit_side_len` / `det.limit_type`（[`TableOcrResolution::apply`]）——
    /// "不继承页面级配置"的实现方式就是覆盖，不是"缺了才填"。
    pub ocr_resolution: TableOcrResolution,
}

impl Default for TableEvalConfig {
    fn default() -> Self {
        Self {
            dataset: PathBuf::from(crate::evaluation::table::fixture::DEFAULT_DATASET_RELATIVE),
            model: PathBuf::from(DEFAULT_MODEL_RELATIVE),
            predictions: PathBuf::from("table-predictions.json"),
            limit: 0,
            side_len: 488,
            fix_input_shape: true,
            intra_threads: None,
            fail_fast: false,
            // 默认保持 E7/E8 的行为：不跑页级 OCR。改成 `PageReuse` 会让"跑一遍表格评测"
            // 悄悄变成"顺带跑一遍全量页级 OCR"。
            content: TableContentSource::StructureOnly,
            ocr_resolution: TableOcrResolution::default(),
        }
    }
}

/// 评测过程中的进度事件。
///
/// 库里**不打印**：同样的链路既要给 CLI 用（要 stderr 进度），也要给未来可能的
/// 服务化调用用（不该往进程 stderr 写）。调用方决定怎么呈现。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableEvalEvent {
    /// 一条人读信息。
    Message(String),
    /// 已完成 `done` / 共 `total` 个样本。
    Progress { done: usize, total: usize },
}

/// 表格域评测 manifest —— `rapidstruct evaluate --manifest` 的域判别入口。
///
/// # 为什么要靠 manifest 而不是新增一个子命令
///
/// `rapidstruct evaluate` 的 OCR manifest 是一个**裸 JSON 数组**（
/// `Vec<EvaluationCase>`）。表格域的报告形状与 OCR 域完全不同（没有 `cases`，
/// 没有 `mean_cer`），硬塞进 `EvaluationSummary` 会违反
/// `crate::evaluation` 的"三类指标不得混入默认汇总"。
///
/// 判别规则：**裸数组 = OCR 域（现状一个字节都不改）；对象且 `domain == "table"`
/// = 表格域**。给 OCR 报告加 `"domain": "ocr"` 会改动一个已经发布的 HTTP 契约
/// （serve 的 `POST /api/evaluate`）而换不到实质好处，因此不加。
///
/// # 路径相对谁解析
///
/// **相对当前工作目录**，与同名 CLI 参数一致。OCR manifest 是把 `case.image`
/// 相对 manifest 所在目录解析的，这里**刻意不照抄**：本 manifest 指向的是仓库里的
/// 数据集与模型（不是与 manifest 同级的样本文件），如果相对 manifest 目录解析，
/// 同一份 manifest 换一个存放位置就指向不同的数据集 —— 那是纯粹的意外。
/// 换句话说，这份 manifest 是"被存下来的 CLI 参数"，参数是 CWD 相对的，它就也是。
#[derive(Debug, Clone, Deserialize)]
pub struct TableEvalManifest {
    /// 域判别字段，必须是 `"table"`。
    pub domain: String,
    #[serde(default)]
    pub dataset: Option<PathBuf>,
    #[serde(default)]
    pub model: Option<PathBuf>,
    #[serde(default)]
    pub predictions: Option<PathBuf>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub side_len: Option<usize>,
    #[serde(default)]
    pub fix_input_shape: Option<bool>,
    #[serde(default)]
    pub intra_threads: Option<usize>,
    #[serde(default)]
    pub fail_fast: Option<bool>,
    /// `"structure-only"`（默认）或 `"page-reuse"`。
    #[serde(default)]
    pub content: Option<String>,
    /// `content = "page-reuse"` 时的页级 OCR 引擎配置 YAML。
    #[serde(default)]
    pub ocr_config: Option<PathBuf>,
    /// `content = "page-reuse"` 时的页级 OCR 结果缓存文件。
    #[serde(default)]
    pub ocr_cache: Option<PathBuf>,
    /// 表格路径自己的 det 分辨率；缺省时用 [`TableOcrResolution::default`]。
    ///
    /// 整个结构写进 manifest 而不是拆成三个标量字段：这三个值是一组（改了
    /// `limit_type` 就改变了 `limit_side_len` 的含义），分开写会允许"半套配置"。
    #[serde(default)]
    pub ocr_resolution: Option<TableOcrResolution>,
}
impl TableEvalManifest {
    /// `domain` 字段的取值。
    pub const DOMAIN: &'static str = "table";

    /// 从一份已解析的 JSON 判断它是不是表格域 manifest。
    ///
    /// 只认对象 + `domain == "table"`。裸数组与其它对象都归 OCR 域，因此**现有
    /// 行为不变**。
    pub fn matches(value: &Value) -> bool {
        value
            .get("domain")
            .and_then(Value::as_str)
            .is_some_and(|domain| domain == Self::DOMAIN)
    }

    /// 合并到默认配置；未给出的字段沿用 [`TableEvalConfig::default`]。
    pub fn into_config(self) -> Result<TableEvalConfig> {
        if self.domain != Self::DOMAIN {
            return Err(RapidOcrError::Config(format!(
                "a table manifest must declare \"domain\": \"{}\", got {:?}",
                Self::DOMAIN,
                self.domain
            )));
        }
        let defaults = TableEvalConfig::default();
        let content = match self.content.as_deref() {
            None | Some("structure-only") => TableContentSource::StructureOnly,
            Some("page-reuse") => TableContentSource::PageReuse {
                config: self.ocr_config,
                cache: self.ocr_cache,
            },
            Some("crop-control") => TableContentSource::CropControl {
                config: self.ocr_config,
            },
            Some(other) => {
                return Err(RapidOcrError::Config(format!(
                    "content must be \"structure-only\", \"page-reuse\" or \"crop-control\", got \
                     {other:?}"
                )));
            }
        };
        Ok(TableEvalConfig {
            dataset: self.dataset.unwrap_or(defaults.dataset),
            model: self.model.unwrap_or(defaults.model),
            predictions: self.predictions.unwrap_or(defaults.predictions),
            limit: self.limit.unwrap_or(defaults.limit),
            side_len: self.side_len.unwrap_or(defaults.side_len),
            fix_input_shape: self.fix_input_shape.unwrap_or(defaults.fix_input_shape),
            intra_threads: self.intra_threads,
            fail_fast: self.fail_fast.unwrap_or(defaults.fail_fast),
            content,
            ocr_resolution: self.ocr_resolution.unwrap_or(defaults.ocr_resolution),
        })
    }
}

/// 一次表格评测的结果。
#[derive(Debug, Clone)]
pub struct TableEvalOutcome {
    /// 完整报告（已并入运行时字段）。
    pub report: Value,
    /// 实际评测的样本数。
    pub selected: usize,
    /// 失败样本数。
    pub failed: usize,
}

impl TableEvalOutcome {
    /// 报告里的某个顶层字段，缺失时返回 `None`。
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.report.get(name)
    }
}

/// 报告里自述"表格路径跑在什么尺度上"的那一块。
///
/// 抽成函数是为了能被单测直接调用：`live_knob` 那条**必须**跟着 `limit_type` 变，
/// 否则读基线的人会以为改 `limit_side_len` 会有影响（`limit_type = "max"` 下
/// `src/ocr/det/detector.rs:215-223` 会丢弃它）。
fn ocr_resolution_report(config: &TableEvalConfig) -> Value {
    json!({
        "limit_side_len": config.ocr_resolution.limit_side_len,
        "limit_type": config.ocr_resolution.limit_type,
        "max_side_len": config.ocr_resolution.max_side_len,
        "live_knob": match config.ocr_resolution.limit_type.as_str() {
            "min" => "limit_side_len (det enlarges until the short side reaches it)",
            _ => "max_side_len (det ignores limit_side_len under limit_type=max)",
        },
        "applies_to": if matches!(config.content, TableContentSource::PageReuse { .. }) {
            "page_reuse"
        } else {
            "nothing (only the page-reuse path runs det)"
        },
    })
}

/// 跑一遍表格结构评测，写预测文件并返回报告。
pub fn run_table_evaluation(
    config: &TableEvalConfig,
    on_event: &mut dyn FnMut(TableEvalEvent),
) -> Result<TableEvalOutcome> {
    let fixture = load_pubtabnet_val(&config.dataset)?;
    let verify = fixture.verify()?;
    let selected = fixture.select(config.limit);
    let sample_set_sha256 = TableFixture::sample_set_digest(&selected);
    let content_sha256 = fixture.content_digest(&selected)?;
    message(
        on_event,
        format!(
            "dataset: {} sample(s) loaded, {} html hashes verified, {} selected (limit {})",
            verify.samples,
            verify.hashed,
            selected.len(),
            config.limit
        ),
    );
    message(
        on_event,
        format!(
            "sample_set_sha256 {sample_set_sha256}  content_sha256 {}",
            content_sha256
                .as_deref()
                .unwrap_or("<unreadable image, not claimed>")
        ),
    );

    // 表格会话自己的配置（含线程旋钮）必须**在探测之前**建好：探针会话与正式会话共用同一套
    // 运行时配置，否则"这个会话用了几个线程"会有两个答案。
    //
    // 期望哈希**必须**来自登记：评测报告会写"这个结果出自哪个模型"，而一个没有可追溯哈希的
    // 模型不该被静默接受（`require_registered_model_hash` 在没有登记时报错）。
    let table_config = TableStructureConfig {
        input_side_len: config.side_len,
        fix_input_shape: config.fix_input_shape,
        intra_threads: config.intra_threads,
        expected_sha256: Some(crate::table::registration::require_registered_model_hash(
            &config.model,
        )?),
    };
    // 运行时配置在这里是**默认值**：线程数不是由"评测"决定的，而是由表格会话自己的
    // `intra_threads` 决定的（经 `effective_runtime_config` 叠加）。把线程策略写在这一层
    // 会变成第二份实现 —— "钉住线程数"的语义只有 `RuntimeConfig::with_pinned_intra_threads`
    // 一处定义。
    let runtime_cfg = table_config.effective_runtime_config(&RuntimeConfig::default());
    let model_info = TableStructureModelInfo::probe(&config.model, &runtime_cfg)?;
    let vocabulary = StructureVocabulary::new(model_info.character_tokens.clone())?;
    let limits = span_limits(&vocabulary);
    let exceedance = summarize_span_exceedance(&selected, &limits, 8);
    message(
        on_event,
        format!(
            "vocabulary: {} fragment(s), {} class(es); span limits rowspan<={:?} colspan<={:?}",
            vocabulary.tokens().len(),
            vocabulary.class_count(),
            limits.max_rowspan,
            limits.max_colspan
        ),
    );
    message(
        on_event,
        format!(
            "constructive upper bound: {} of {} table(s) ({:.4}%) contain a span the vocabulary \
         cannot express",
            exceedance.samples_exceeding,
            exceedance.samples,
            exceedance.fraction() * 100.0
        ),
    );

    let upper_bound = tally_upper_bound(&selected, &limits);
    message(
        on_event,
        format!(
            "loop trip cap {LOOP_TRIP_CAP}: largest truth structure stream is {} token(s); {} of {} \
         table(s) are beyond the cap",
            upper_bound.max_structure_tokens, upper_bound.by_loop_cap, upper_bound.tables
        ),
    );
    message(
        on_event,
        format!(
            "deducted from the regression criterion (still counted in the headline metric): {} of {} \
         table(s) ({:.4}%)",
            upper_bound.deducted,
            upper_bound.tables,
            upper_bound.fraction() * 100.0
        ),
    );

    let mut session = TableStructureSession::open(&config.model, &runtime_cfg, &table_config)?;
    // 报告里写**会话自己**下发给 ORT 的那对数，而不是我们以为下发的那对：两者应当一致，
    // 不一致就说明线程要求在某个环节被丢掉了，而那正是这份报告会被用来回答的问题。
    let effective_threads = session.session_threads();
    debug_assert_eq!(
        effective_threads,
        runtime_cfg.effective_session_threads(),
        "the session must report the thread pair the table config asked for"
    );
    // `None` 表示"没钉住，交给 ORT 决定"，不是 0 —— 打印成 `ort-default` 以免被读成"零线程"。
    let threads_label = |value: Option<usize>| {
        value.map_or_else(|| "ort-default".to_string(), |count| count.to_string())
    };
    message(
        on_event,
        format!(
            "session: input dims {:?}, threads (intra, inter) = ({}, {})",
            session.input_dims(),
            threads_label(effective_threads.0),
            threads_label(effective_threads.1)
        ),
    );

    let mut predictions: BTreeMap<String, String> = BTreeMap::new();
    let mut failures: Vec<Value> = Vec::new();
    let mut decodable = 0_usize;
    let mut preprocess_ms = Vec::with_capacity(selected.len());
    let mut infer_ms = Vec::with_capacity(selected.len());
    let mut decode_ms = Vec::with_capacity(selected.len());
    let mut total_ms = Vec::with_capacity(selected.len());
    let mut cell_counts = Vec::with_capacity(selected.len());
    let mut backfill_stats: Vec<BackfillStats> = Vec::new();
    // 结构可信度分布。**必须与预测一起统计**：`html` 里的表格看起来永远正常，
    // 只有这两个计数器能说明其中有多少张是"模型表达不了真值"的降级产物。
    let mut confidence_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut degradation_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut holes_total = 0_usize;

    // 页级 OCR：只在 `PageReuse` 下构造。引擎建在这里而不是 `predict_one` 里，
    // 因为会话创建（模型加载）是秒级成本，而样本是千级。
    let mut page_ocr = PageOcr::open(&config.content, &config.ocr_resolution)?;
    if let Some(ocr) = page_ocr.as_mut() {
        message(on_event, ocr.describe());
        message(on_event, config.ocr_resolution.describe());
        ocr.prepare(&selected, on_event)?;
    }
    // 对照路径的识别器同理：建一次，逐格复用。
    let mut crop_control = CropRecognizer::open(&config.content)?;
    if let Some(control) = crop_control.as_ref() {
        message(
            on_event,
            "crop control: the recognizer is called once per cell with a bbox (this is the \
             comparison baseline, not the production path)",
        );
        let _ = control;
    }

    let started = Instant::now();
    for (index, sample) in selected.iter().enumerate() {
        // 先取文本来源；取不到（页级 OCR 失败）时按样本失败处理，而不是把整轮中止。
        let mut lines = None;
        if let Some(ocr) = page_ocr.as_mut() {
            match ocr.lines_for(sample) {
                Ok(extracted) => lines = Some(extracted),
                Err(error) => {
                    if config.fail_fast {
                        return Err(error);
                    }
                    predictions.insert(sample.file_name().to_string(), String::new());
                    failures.push(json!({
                        "file_name": sample.file_name(),
                        "image_path": sample.relative_path(),
                        "error": error.to_string(),
                    }));
                    if (index + 1) % 500 == 0 {
                        on_event(TableEvalEvent::Progress {
                            done: index + 1,
                            total: selected.len(),
                        });
                    }
                    continue;
                }
            }
        }
        let source = match (&lines, crop_control.as_mut()) {
            (Some(lines), _) => CellSource::Reuse(lines),
            (None, Some(control)) => CellSource::Crop(control),
            (None, None) => CellSource::Empty,
        };
        match predict_one(&mut session, &vocabulary, &table_config, sample, source) {
            Ok(outcome) => {
                preprocess_ms.push(outcome.preprocess_ms);
                infer_ms.push(outcome.infer_ms);
                decode_ms.push(outcome.decode_ms);
                total_ms.push(outcome.total_ms);
                cell_counts.push(outcome.cells);
                if let Some(stats) = outcome.backfill {
                    backfill_stats.push(stats);
                }
                decodable += 1;
                *confidence_counts
                    .entry(confidence_kind(outcome.confidence))
                    .or_default() += 1;
                for degradation in &outcome.degradations {
                    *degradation_counts
                        .entry(degradation_kind(*degradation))
                        .or_default() += 1;
                    if let StructureDegradation::GridHoles { holes } = degradation {
                        holes_total += holes;
                    }
                }
                predictions.insert(sample.file_name().to_string(), outcome.html);
            }
            Err(error) => {
                if config.fail_fast {
                    return Err(error);
                }
                // 失败样本写空串：官方指标把它算 0 分，从而如实拉低总分（见模块文档）。
                predictions.insert(sample.file_name().to_string(), String::new());
                failures.push(json!({
                    "file_name": sample.file_name(),
                    "image_path": sample.relative_path(),
                    "error": error.to_string(),
                }));
            }
        }
        if (index + 1) % 500 == 0 {
            on_event(TableEvalEvent::Progress {
                done: index + 1,
                total: selected.len(),
            });
        }
    }
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    // 缓存落盘放在主循环之后：即便中途被打断，已写盘的样本也不会重跑。
    if let Some(ocr) = page_ocr.as_mut() {
        ocr.flush()?;
    }
    let ocr_report = page_ocr.as_ref().map(PageOcr::report);
    let crop_report = crop_control.as_ref().map(CropRecognizer::report);

    // 失败按消息分组：20 条各不相同的消息读不出模式，12 × "ends with 1 unclosed tag(s)"
    // 才说明这是**模型的系统性结构错误**而不是随机崩溃。
    let mut failure_kinds: BTreeMap<String, usize> = BTreeMap::new();
    for failure in &failures {
        let text = failure["error"].as_str().unwrap_or_default().to_string();
        *failure_kinds.entry(text).or_default() += 1;
    }

    write_predictions(&config.predictions, &predictions)?;
    let prediction_digest = digest_file(&config.predictions)?;
    let cell_timing = summarize(
        &cell_counts
            .iter()
            .map(|value| *value as f64)
            .collect::<Vec<_>>(),
    );

    let mut report = json!({
        // 报告自报家门：表格报告与 OCR 报告的形状完全不同，消费者不该靠"有没有 cases
        // 字段"去猜自己拿到的是哪一种。
        "domain": "table",
        "tool": "table_eval",
        "content_source": config.content.as_str(),
        // 表格路径自己的 det 分辨率（doc 09 §11 = doc 07 的 R2）。写进报告是必须的：
        // 完整 TEDS 的绝对值绑定在产出文本的那份引擎配置上，而同一份 ocr_config 会被
        // `TableOcrResolution::apply` 覆盖成不同分辨率 —— 只记 YAML 路径不足以自证口径。
        "ocr_resolution": ocr_resolution_report(config),
        "metric_scope": {
            "structure_only": config.content.is_structure_only(),
            "reason": match &config.content {
                TableContentSource::StructureOnly => "cell texts need the page-level OCR + \
                    geometric matching path (doc 09 §10 = E9); every cell text is the empty \
                    string here",
                TableContentSource::PageReuse { .. } => "cell texts are taken from the page-level \
                    OCR result by geometric matching; the recognizer is never called again for \
                    a cell",
                TableContentSource::CropControl { .. } => "cell texts come from cropping every \
                    cell and running the recognizer on it (the comparison baseline of doc 09 \
                    §10.3, not the production path)",
            },
            "reported_metric": config.content.reported_metric(),
            "not_reported": if config.content.uses_engine() {
                json!([])
            } else {
                json!(["TEDS"])
            },
            "warning": "these numbers are not comparable with the SLANet TEDS in the PubTabNet \
                        paper (different evaluation set and vocabulary)",
        },
        "protocol": protocol_section(),
        "model": {
            "path": model_info.model_path,
            "bytes": std::fs::metadata(&config.model)?.len(),
            "sha256": digest_file(&config.model)?.1,
            "input_name": model_info.input_name,
            "input_dims": model_info.input_dims,
            "box_output_name": model_info.box_output_name,
            "structure_output_name": model_info.structure_output_name,
            "structure_last_dim": model_info.structure_last_dim,
            "token_count": vocabulary.tokens().len(),
            "class_count": vocabulary.class_count(),
            "sos_class_index": vocabulary.sos_class_index(),
            "eos_class_index": vocabulary.eos_class_index(),
        },
        "config": {
            "input_side_len": table_config.input_side_len,
            "fix_input_shape": table_config.fix_input_shape,
            "session_input_dims": session.input_dims(),
            "intra_threads": effective_threads.0,
            "inter_threads": effective_threads.1,
        },
        "dataset": {
            "root": config.dataset.display().to_string(),
            "manifest": fixture.manifest_path().display().to_string(),
            "manifest_sha256": digest_file(fixture.manifest_path())?.1,
            "sample_set_sha256": sample_set_sha256,
            "content_sha256": content_sha256,
            "samples_loaded": verify.samples,
            "html_hashes_verified": verify.hashed,
            "limit": config.limit,
            "selected": selected.len(),
            "decodable": decodable,
            "failed": failures.len(),
            "predictions_emitted": predictions.len(),
            "failure_scoring": "a failed sample is written as an empty prediction, which the \
                                official metric scores as 0.0; failures are never dropped",
        },
        "span_limits": {
            "max_rowspan": limits.max_rowspan,
            "max_colspan": limits.max_colspan,
            "source": "derived from the model's own `character` vocabulary, not hardcoded",
        },
        "span_exceedance": {
            "tables": exceedance.samples,
            "tables_exceeding": exceedance.samples_exceeding,
            "fraction": exceedance.fraction(),
            "cells_exceeding": exceedance.cells_exceeding,
            "cells_total": exceedance.cells_total,
            "observed_max_rowspan": exceedance.max_rowspan,
            "observed_max_colspan": exceedance.max_colspan,
            "examples": exceedance.examples.iter().map(|(name, kind, value)| json!({
                "file_name": name,
                "kind": kind.as_str(),
                "value": value,
            })).collect::<Vec<_>>(),
        },
        // 结构可信度：**与预测一起产出**的降级信号（doc 09 §14.1②④）。
        // 一张"看起来正常"的表格与一张"模型表达不了真值"的表格在 `predictions` 里长得
        // 一模一样，只有这一段能把它们分开。降级不是异常：`low` 的样本**照样计分**。
        "structure_confidence": {
            "high": confidence_counts.get("high").copied().unwrap_or(0),
            "low": confidence_counts.get("low").copied().unwrap_or(0),
            "degradations": {
                "grid_holes": degradation_counts.get("grid_holes").copied().unwrap_or(0),
                "truncated": degradation_counts.get("truncated").copied().unwrap_or(0),
            },
            "holes_total": holes_total,
            "scoring": "a low-confidence sample is still scored; the flag says the model could not \
                        express the truth, not that the prediction is unusable",
            "why_grid_holes": "the model cannot emit a span outside its vocabulary (SLANet-plus \
                               stops at 20, the data reaches colspan=24); it then declares a \
                               smaller span and the logical grid is left with a hole, which the \
                               emitter is the only place able to notice",
            "why_truncated": "the structure `Loop` hit its trip cap before emitting `eos`, so the \
                              tail of the structure is missing even though the token stream looks \
                              well-formed",
        },
        "upper_bound": {
            "loop_trip_cap": LOOP_TRIP_CAP,
            "tables": upper_bound.tables,
            "tables_deducted": upper_bound.deducted,
            "fraction": upper_bound.fraction(),
            "by_span_beyond_vocabulary": upper_bound.by_span,
            "by_loop_trip_cap": upper_bound.by_loop_cap,
            "by_both": upper_bound.by_both,
            "largest_structure_tokens": upper_bound.max_structure_tokens,
            "headline_metric": "these samples are still scored (their failures are real 0.0s)",
            "regression_criterion": "these samples are excluded, because the model cannot \
                                     control whether the truth is expressible",
            "examples": upper_bound.examples.iter().map(|(name, reason)| json!({
                "file_name": name,
                "reason": reason,
            })).collect::<Vec<_>>(),
        },
        "timing": {
            "scope": "per-sample, wall clock around each stage; NOT the timing ledger",
            "preprocess_ms": summarize(&preprocess_ms),
            "infer_ms": summarize(&infer_ms),
            "decode_ms": summarize(&decode_ms),
            "total_ms": summarize(&total_ms),
            "loop_wall_ms": wall_ms,
            "cells": cell_timing,
        },
        "page_ocr": ocr_report,
        // 对照路径的识别器调用统计。`null` 表示这次没跑对照口径 —— 与"跑了但一次都没调"
        // 是两件不同的事，所以不给零值。
        "crop_control": crop_report,
        "backfill": if backfill_stats.is_empty() {
            Value::Null
        } else {
            summarize_backfill(&backfill_stats, &config.content)
        },
        "failures": failures,
        "failure_kinds": failure_kinds,
        "outputs": {
            "predictions": {
                "path": config.predictions.display().to_string(),
                "bytes": prediction_digest.0,
                "sha256": prediction_digest.1,
            },
        },
    });
    attach_runtime_fields(&mut report)?;

    Ok(TableEvalOutcome {
        report,
        selected: selected.len(),
        failed: failures.len(),
    })
}

/// 构造性上界在**表格**粒度上的计数。
#[derive(Debug, Clone, Default)]
struct UpperBoundTally {
    tables: usize,
    deducted: usize,
    by_span: usize,
    by_loop_cap: usize,
    by_both: usize,
    max_structure_tokens: usize,
    examples: Vec<(String, &'static str)>,
}

impl UpperBoundTally {
    fn fraction(&self) -> f64 {
        if self.tables == 0 {
            0.0
        } else {
            self.deducted as f64 / self.tables as f64
        }
    }
}

/// 逐样本判定构造性上界。
///
/// 两个判据都是**真值侧**的事实：`span` 来自对真值 HTML 的扫描（上限由模型自己的
/// 词表解析出来），`token` 来自真值记录里的结构串长度。因此"被扣除"与模型质量无关，
/// 这也正是它不该进回归判据的原因。
fn tally_upper_bound(samples: &[&TableSample], limits: &SpanLimits) -> UpperBoundTally {
    const MAX_EXAMPLES: usize = 8;
    let mut tally = UpperBoundTally {
        tables: samples.len(),
        ..UpperBoundTally::default()
    };
    for sample in samples {
        let usage = span_usage(sample.ground_truth(), Some(limits));
        let bound: UpperBound =
            classify_upper_bound(usage.exceeds(), sample.record().structure_tokens);
        tally.max_structure_tokens = tally
            .max_structure_tokens
            .max(sample.record().structure_tokens);
        if !bound.is_deducted() {
            continue;
        }
        tally.deducted += 1;
        match (bound.span_beyond_vocabulary, bound.beyond_loop_trip_cap) {
            (true, true) => tally.by_both += 1,
            (true, false) => tally.by_span += 1,
            (false, true) => tally.by_loop_cap += 1,
            (false, false) => unreachable!("is_deducted() is true only for the three cases above"),
        }
        if tally.examples.len() < MAX_EXAMPLES {
            tally.examples.push((
                sample.file_name().to_string(),
                bound.reason().expect("a deducted sample has a reason"),
            ));
        }
    }
    tally
}

struct Prediction {
    html: String,
    cells: usize,
    preprocess_ms: f64,
    infer_ms: f64,
    decode_ms: f64,
    total_ms: f64,
    /// 只在 `PageReuse` 下有值（`StructureOnly` 不做回填）。
    backfill: Option<BackfillStats>,
    /// 发射器给出的结构可信度档位。**与 `html` 一起产出**，所以调用方拿不到
    /// "有文本但不知道可不可信"的状态（`docs/09` §14.1②）。
    confidence: StructureConfidence,
    /// 命中的降级原因（空 = `confidence == High`）。
    degradations: Vec<StructureDegradation>,
}

/// 可信度档位的**稳定**字符串名。用字符串而不是直接序列化枚举：报告是对外产物，
/// 它的键不该随 Rust 变体改名而改。
fn confidence_kind(value: StructureConfidence) -> &'static str {
    match value {
        StructureConfidence::High => "high",
        StructureConfidence::Low => "low",
    }
}

/// 降级原因的**稳定**字符串名，理由同 [`confidence_kind`]。
fn degradation_kind(value: StructureDegradation) -> &'static str {
    match value {
        StructureDegradation::GridHoles { .. } => "grid_holes",
        StructureDegradation::Truncated => "truncated",
    }
}

/// 发一条人读信息。
///
/// 用自由函数而不是闭包：闭包会**长期独占** `on_event`，于是 `PageOcr::prepare`
/// 这种也要发进度的调用就没法再借一次。
fn message(on_event: &mut dyn FnMut(TableEvalEvent), text: impl Into<String>) {
    on_event(TableEvalEvent::Message(text.into()));
}

/// 页级 OCR 缓存里的一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedLine {
    bbox: [f32; 4],
    text: String,
    score: f32,
}

/// 页级 OCR 结果缓存。
///
/// 见模块文档"为什么页级 OCR 要缓存"。键是**样本的相对路径**（与预测文件同一个
/// 命名空间），不是绝对路径 —— 换机器/换盘符不该让缓存全部失效。
#[derive(Debug, Default, Serialize, Deserialize)]
struct OcrCache {
    version: u32,
    /// 产出这些文本行的引擎指纹（[`engine_fingerprint`]）。
    ///
    /// 只按样本路径做键是不够的：换模型、换 det 分辨率之后旧文本会被静默当成
    /// 新文本用，于是"改了分辨率"的对照实验读到同一批缓存、得出"分辨率无影响"
    /// 的假结论。指纹不符时**报错**，不做部分复用。
    engine_fingerprint: String,
    entries: BTreeMap<String, Vec<CachedLine>>,
}

/// 引擎指纹：这份缓存里的文本行是由哪一份引擎配置产出的。
///
/// 页级文本是"图像字节 + 引擎配置"的函数，所以指纹取整个 [`EngineConfig`] 的
/// 规范化 JSON 的 sha256 —— 不是取配置文件的路径（同一个路径的内容可以被改），
/// 也不是取"模型路径"（漏掉分辨率与 provider）。
///
/// `runtime` 段（线程数 / provider / arena）**也在指纹里**：provider 会改变数值
/// 累加顺序，`docs/10-table-evaluation-protocol.md` §1.2 的逐字节确定性证据是在
/// **固定**线程数与 provider 下取得的，不能外推到换线程。代价是换线程会让缓存
/// 失效一次，这是正确性的价格，不是缺陷。
fn engine_fingerprint(config: &EngineConfig) -> Result<String> {
    let canonical = serde_json::to_string(config).map_err(|error| {
        RapidOcrError::Config(format!(
            "the engine config cannot be fingerprinted: {error}"
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

impl OcrCache {
    /// 当前缓存格式版本。改变字段含义时必须递增，否则旧缓存会被当成新格式读进来。
    const VERSION: u32 = 2;

    fn load(path: &Path, expected_fingerprint: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            RapidOcrError::Decode(format!(
                "the OCR cache {} is unreadable: {error}",
                path.display()
            ))
        })?;
        let cache: Self = serde_json::from_str(&text).map_err(|error| {
            RapidOcrError::Decode(format!(
                "the OCR cache {} is not valid JSON: {error}",
                path.display()
            ))
        })?;
        if cache.version != Self::VERSION {
            return Err(RapidOcrError::Config(format!(
                "the OCR cache {} has version {} but this build writes version {}; delete it or \
                 point --ocr-cache somewhere else",
                path.display(),
                cache.version,
                Self::VERSION
            )));
        }
        if cache.engine_fingerprint != expected_fingerprint {
            return Err(RapidOcrError::Config(format!(
                "the OCR cache {} was produced by a different engine than the one requested \
                 (cache fingerprint {}, requested fingerprint {}); reusing it would silently \
                 attribute another engine's text to this run, so it is refused instead. Delete \
                 the cache, or point --ocr-cache at a file per engine configuration",
                path.display(),
                cache.engine_fingerprint,
                expected_fingerprint
            )));
        }
        Ok(cache)
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string(self).map_err(|error| {
            RapidOcrError::Decode(format!("the OCR cache is not serializable: {error}"))
        })?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// 页级 OCR 的持有者：引擎 + 缓存 + 本轮统计。
///
/// 它**只做页级识别**，回填的几何匹配在 `src/table/cell_content_backfill.rs` 里。
/// 分开的理由就是门禁 G2：回填路径里不能有识别器。
struct PageOcr {
    engine: RapidOcrEngine,
    cache: Option<(PathBuf, OcrCache)>,
    cache_hits: usize,
    cache_misses: usize,
    ocr_ms: Vec<f64>,
    regions: usize,
    engine_label: String,
    /// 真正跑起来的那份引擎配置的指纹；缓存用它自证来源，报告用它自证口径。
    engine_fingerprint: String,
}

/// 页级 OCR 的**来源标签**：记下是哪份配置产出了单元格文本。
///
/// 这不是装饰。完整 TEDS 的绝对值绑定在产出文本的那份引擎配置上（09 §10.5.8 第 3 条），
/// 换配置必须重跑；报告里只写一句常量标签会让基线无法自证其口径。
fn engine_label_for(config: &Option<PathBuf>) -> String {
    match config {
        Some(path) => format!("page OCR: {}", path.display()),
        None => "page OCR: built-in default engine config".to_string(),
    }
}

impl PageOcr {
    /// 按内容来源构造；`StructureOnly` 时返回 `None`（不建引擎）。
    ///
    /// `resolution` 会**覆盖**引擎配置里的分辨率字段（[`TableOcrResolution::apply`]），
    /// 并在覆盖**之后**算指纹 —— 指纹必须描述真正跑起来的那份配置，不是 YAML 里的原文。
    fn open(content: &TableContentSource, resolution: &TableOcrResolution) -> Result<Option<Self>> {
        let TableContentSource::PageReuse { config, cache } = content else {
            return Ok(None);
        };
        // Which OCR config produced the cell text is a provenance fact, not decoration:
        // the full-TEDS number is only interpretable next to it, and swapping the config
        // silently changes the metric's meaning.  Record the path (or say it was the
        // built-in default) instead of a constant label that carries no information.
        let engine_label = engine_label_for(config);
        let mut engine_config = match config {
            Some(path) => EngineConfig::from_yaml_file(path)?,
            None => EngineConfig::default(),
        };
        resolution.apply(&mut engine_config)?;
        let fingerprint = engine_fingerprint(&engine_config)?;
        let engine = RapidOcrEngine::new(engine_config)?;
        let cache = match cache {
            Some(path) => {
                let loaded = if path.exists() {
                    OcrCache::load(path, &fingerprint)?
                } else {
                    OcrCache {
                        version: OcrCache::VERSION,
                        engine_fingerprint: fingerprint.clone(),
                        entries: BTreeMap::new(),
                    }
                };
                Some((path.clone(), loaded))
            }
            None => None,
        };
        Ok(Some(Self {
            engine,
            cache,
            cache_hits: 0,
            cache_misses: 0,
            ocr_ms: Vec::new(),
            regions: 0,
            engine_label,
            engine_fingerprint: fingerprint,
        }))
    }

    fn describe(&self) -> String {
        match &self.cache {
            Some((path, cache)) => format!(
                "{}; cache {} ({} sample(s) already cached)",
                self.engine_label,
                path.display(),
                cache.entries.len()
            ),
            None => format!(
                "{}; no cache (every run re-OCRs every page)",
                self.engine_label
            ),
        }
    }

    /// 预先算出还缺哪些样本，并把总量报出来。
    ///
    /// 这一遍**只查缓存不识别**：让"还要跑 N 张、约 M 分钟"在开始前就可见，
    /// 而不是跑完才知道。
    fn prepare(
        &mut self,
        samples: &[&TableSample],
        on_event: &mut dyn FnMut(TableEvalEvent),
    ) -> Result<()> {
        let missing = self.missing_count(samples);
        if missing == 0 {
            message(
                on_event,
                format!("page OCR: all {} sample(s) are cached", samples.len()),
            );
        } else {
            message(
                on_event,
                format!(
                    "page OCR: {missing} of {} sample(s) need recognition; the rest are cached",
                    samples.len()
                ),
            );
        }
        Ok(())
    }

    fn missing_count(&self, samples: &[&TableSample]) -> usize {
        let Some((_, cache)) = &self.cache else {
            return samples.len();
        };
        samples
            .iter()
            .filter(|sample| !cache.entries.contains_key(sample.relative_path()))
            .count()
    }

    /// 取一张图的页级文本行；命中缓存就不跑引擎。
    fn lines_for(&mut self, sample: &TableSample) -> Result<Vec<TextLine>> {
        let key = sample.relative_path().to_string();
        if let Some((_, cache)) = &self.cache
            && let Some(lines) = cache.entries.get(&key)
        {
            self.cache_hits += 1;
            return Ok(lines.iter().map(cached_to_line).collect());
        }
        self.cache_misses += 1;

        let started = Instant::now();
        let bytes = std::fs::read(sample.image_path())?;
        let output = self.engine.recognize(request_for(bytes))?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        self.ocr_ms.push(elapsed);
        self.regions += output.regions.len();

        let lines = lines_from_output(&output);
        if let Some((path, cache)) = &mut self.cache {
            cache.entries.insert(
                key,
                lines
                    .iter()
                    .map(|line| CachedLine {
                        bbox: line.bbox,
                        text: line.text.clone(),
                        score: line.score,
                    })
                    .collect(),
            );
            // 每 250 张落一次盘：全量一轮是小时级，崩在中途不该把已跑的丢掉。
            if self.cache_misses.is_multiple_of(250) {
                cache.save(path)?;
            }
        }
        Ok(lines)
    }

    fn flush(&mut self) -> Result<()> {
        if let Some((path, cache)) = &self.cache {
            cache.save(path)?;
        }
        Ok(())
    }

    fn report(&self) -> Value {
        json!({
            "engine": self.engine_label,
            "engine_fingerprint": self.engine_fingerprint,
            "cache_hits": self.cache_hits,
            "cache_misses": self.cache_misses,
            "cache_path": self.cache.as_ref().map(|(path, _)| path.display().to_string()),
            "ocr_ms": summarize(&self.ocr_ms),
            "regions_seen": self.regions,
        })
    }
}

fn cached_to_line(line: &CachedLine) -> TextLine {
    TextLine {
        bbox: line.bbox,
        text: line.text.clone(),
        score: line.score,
    }
}

/// 页级 OCR 的请求：只开检测 + 识别。
///
/// 分类器保持 `IfAvailable`（与 `src/bin/rapidstruct.rs` 的默认一致）—— 关掉它会改变
/// 页级管线的行为，而这里的目的是**复用既有页级结果**，不是另开一条管线。
fn request_for(bytes: Vec<u8>) -> OcrRequest {
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
        recognition: RecognitionPolicy {
            words: WordOutputMode::Off,
        },
        output: OutputPolicy::default(),
        formula: Default::default(),
        table: Default::default(),
    }
}

/// 把页级输出摊平成回填要用的文本行。
///
/// 只取**有识别结果且有四边形**的区域：公式区域没有 CTC 文本，缺四边形的区域
/// 无法参与几何匹配。两者都不是错误，只是对表格回填无用。
fn lines_from_output(output: &OcrOutput) -> Vec<TextLine> {
    output
        .regions
        .iter()
        .filter_map(|region| {
            let polygon = region.polygon.as_ref()?;
            let recognition = region.recognition.as_ref()?;
            let mut flat = [0.0_f32; 8];
            for (index, point) in polygon.points.iter().enumerate() {
                flat[index * 2] = point[0];
                flat[index * 2 + 1] = point[1];
            }
            Some(TextLine {
                bbox: axis_aligned_bbox(&flat),
                text: recognition.text.clone(),
                score: recognition.score,
            })
        })
        .collect()
}

/// 单元格文本的取用方式（[`predict_one`] 的输入）。
enum CellSource<'a> {
    /// 结构口径：所有单元格空串。
    Empty,
    /// 复用整页 OCR 的文本行。
    Reuse(&'a [TextLine]),
    /// 对照：逐单元格裁剪喂识别器。
    Crop(&'a mut CropRecognizer),
}

/// 把 `Recognizer` 适配成 [`CellRecognizer`]，供对照路径使用。
///
/// 每格一次 `recognize`（batch 1）而不是攒批：批处理会让 `max_wh_ratio` 取批内最大值
/// 从而**重现** R1 的根因，而对照的意义正是"逐 cell 裁剪喂 rec"这条朴素做法本身。
struct CropRecognizer {
    recognizer: Recognizer,
    calls: usize,
    ms: Vec<f64>,
}

impl CropRecognizer {
    fn open(config: &TableContentSource) -> Result<Option<Self>> {
        let TableContentSource::CropControl { config } = config else {
            return Ok(None);
        };
        let engine_config = match config {
            Some(path) => EngineConfig::from_yaml_file(path)?,
            None => EngineConfig::default(),
        };
        let recognizer = Recognizer::new(engine_config.rec, &engine_config.runtime)?;
        Ok(Some(Self {
            recognizer,
            calls: 0,
            ms: Vec::new(),
        }))
    }

    fn report(&self) -> Value {
        json!({
            "recognizer_calls": self.calls,
            "recognize_ms": summarize(&self.ms),
        })
    }
}

impl CellRecognizer for CropRecognizer {
    fn recognize(&mut self, region: &RecImage) -> Result<(String, f32)> {
        let started = Instant::now();
        let output = self.recognizer.recognize(
            std::slice::from_ref(region),
            RecognizeOptions {
                return_word_box: false,
                return_single_char_box: false,
            },
        )?;
        self.ms.push(started.elapsed().as_secs_f64() * 1000.0);
        self.calls += 1;
        Ok(match output.lines.into_iter().next() {
            Some(line) => (line.text, line.score),
            None => (String::new(), 0.0),
        })
    }
}

/// 一次回填的统计（报告用）。
#[derive(Debug, Clone)]
struct BackfillStats {
    cells: usize,
    non_empty: usize,
    blank_cells: usize,
    unmatched_cells: usize,
    single_line_cells: usize,
    multi_line_cells: usize,
    lines_considered: usize,
    lines_above_table: usize,
    matched_lines_total: usize,
    recognizer_calls: usize,
    text_confidence: Option<f32>,
    backfill_ms: f64,
}

impl BackfillStats {
    fn from_outcome(outcome: &BackfillOutcome, backfill_ms: f64) -> Self {
        Self {
            cells: outcome.cells,
            non_empty: outcome.non_empty(),
            blank_cells: outcome.blank_cells,
            unmatched_cells: outcome.unmatched_cells,
            single_line_cells: outcome.single_line_cells,
            multi_line_cells: outcome.multi_line_cells,
            lines_considered: outcome.lines_considered,
            lines_above_table: outcome.lines_above_table,
            matched_lines_total: outcome.matched_lines_total,
            recognizer_calls: outcome.recognizer_calls,
            text_confidence: outcome.text_confidence().mean,
            backfill_ms,
        }
    }
}

/// 汇总所有样本的回填统计。
///
/// `content` 决定 `recognizer_calls` 的**判据方向**：门禁 G2 要求**生产路径**（复用整页
/// OCR）一次都不调识别器；对照路径（`crop-control`）的存在意义恰恰是每格调一次，
/// 所以对它报同一句"这是 G2 失败"是把判据用错了对象。
fn summarize_backfill(stats: &[BackfillStats], content: &TableContentSource) -> Value {
    let sum = |pick: fn(&BackfillStats) -> usize| stats.iter().map(pick).sum::<usize>();
    let cells = sum(|value| value.cells);
    let non_empty = sum(|value| value.non_empty);
    let recognizer_calls = sum(|value| value.recognizer_calls);
    let mut confidences: Vec<f32> = stats
        .iter()
        .filter_map(|value| value.text_confidence)
        .collect();
    confidences.sort_by(f32::total_cmp);
    let note = match content {
        TableContentSource::StructureOnly | TableContentSource::PageReuse { .. } => {
            "gate G2: the backfill path must never call the recognizer, so a non-zero count here \
             is a failure"
        }
        TableContentSource::CropControl { .. } => {
            "comparison baseline (doc 09 §10.3): one recognizer call per cell is the point of this \
             path, so the zero-call invariant does not apply here"
        }
    };
    json!({
        "cells": cells,
        "non_empty_cells": non_empty,
        "non_empty_rate": if cells == 0 { 0.0 } else { non_empty as f64 / cells as f64 },
        "blank_cells": sum(|value| value.blank_cells),
        "unmatched_cells": sum(|value| value.unmatched_cells),
        "single_line_cells": sum(|value| value.single_line_cells),
        "multi_line_cells": sum(|value| value.multi_line_cells),
        "lines_considered": sum(|value| value.lines_considered),
        "lines_above_table": sum(|value| value.lines_above_table),
        "matched_lines_total": sum(|value| value.matched_lines_total),
        "recognizer_calls": recognizer_calls,
        "recognizer_calls_note": note,
        "text_confidence": if confidences.is_empty() { Value::Null } else {
            Value::from(confidences.iter().sum::<f32>() / confidences.len() as f32)
        },
        "backfill_ms": summarize(&stats.iter().map(|value| value.backfill_ms).collect::<Vec<_>>()),
    })
}

fn predict_one(
    session: &mut TableStructureSession,
    vocabulary: &StructureVocabulary,
    config: &TableStructureConfig,
    sample: &TableSample,
    source: CellSource<'_>,
) -> Result<Prediction> {
    let started = Instant::now();
    let image = RecImage::from_path(sample.image_path())?;
    let (height, width) = (image.height() as f32, image.width() as f32);

    let preprocess_started = Instant::now();
    let (tensor, _canvas) = table_structure_input(&image, config.input_side_len)?;
    let preprocess_ms = preprocess_started.elapsed().as_secs_f64() * 1000.0;

    let infer_started = Instant::now();
    let (boxes, structure) = session.run_with(tensor.view(), |outputs| {
        Ok((outputs.boxes.to_owned(), outputs.tokens.to_owned()))
    })?;
    let infer_ms = infer_started.elapsed().as_secs_f64() * 1000.0;

    let decode_started = Instant::now();
    let steps_count = structure.shape()[1];
    let classes = structure.shape()[2];
    if structure.shape()[0] != 1 || boxes.shape()[0] != 1 {
        return Err(RapidOcrError::Decode(format!(
            "{}: the model returned a batch of {} boxes / {} structures, but this harness runs one \
             image at a time",
            sample.file_name(),
            boxes.shape()[0],
            structure.shape()[0]
        )));
    }
    // 先查类别数再解码：类别数不符时 `decode_token_steps` 会报"某个 class index 越界"，
    // 那是症状而不是原因。这里报的是"模型与词表不配套"。
    if classes != vocabulary.class_count() {
        return Err(RapidOcrError::Decode(format!(
            "{}: the model's structure logits have {classes} classes but the vocabulary has {}",
            sample.file_name(),
            vocabulary.class_count()
        )));
    }
    let argmaxed = argmax_structure(structure.as_slice().expect("contiguous"), steps_count)?;
    let (steps, termination) = decode_token_steps(vocabulary, &argmaxed)?;

    let shape = crate::table::decode::TableShape::new(height, width, config.input_side_len as f32)?;
    let ratios = crate::table::decode::rescale_ratios(width, height, config.input_side_len as f32);
    let box_flat = boxes.as_slice().expect("contiguous");
    let cell_polygons: Vec<[f32; 8]> = steps
        .iter()
        .filter(|step| step.is_cell_start)
        .map(|step| {
            let offset = step.step_index * 8;
            let mut raw = [0.0_f32; 8];
            raw.copy_from_slice(&box_flat[offset..offset + 8]);
            crate::table::decode::rescale_polygon(&shape.denormalize_polygon(&raw), ratios)
        })
        .collect();
    let result =
        crate::table::decode::tokens_to_structure(vocabulary, &steps, termination, &cell_polygons)?;

    // 单元格文本：结构口径传空串；完整口径走几何匹配复用整页 OCR 的结果；
    // 对照口径逐单元格裁剪喂识别器。
    // `structure_to_html` 要求**等长**切片，正是为了不让"这个单元格没有文本"和
    // "调用方忘了给文本"变成同一件事。
    let (texts, backfill_stats) = match source {
        CellSource::Empty => (vec![String::new(); result.cells.len()], None),
        CellSource::Reuse(lines) => {
            let backfill_started = Instant::now();
            // offset 是 `[0.0, 0.0]`：`lines` 来自**整页** OCR，而 `result.cells` 的
            // 坐标是**整张图**（`table_structure_input` 不改坐标系，只做缩放后反算
            // 回原图）。参考实现喂的也是同一张整图，所以这里没有 crop 偏移。
            // 等 E11 把表格定位接进页级管线后，若那时喂的是**裁剪后**的小图，
            // 这个 offset 就必须换成 crop 原点 —— 这正是把它做成参数的原因。
            let outcome = backfill(
                &result.cells,
                BackfillSource::Reuse {
                    lines,
                    offset: [0.0, 0.0],
                },
            )?;
            let backfill_ms = backfill_started.elapsed().as_secs_f64() * 1000.0;
            let stats = BackfillStats::from_outcome(&outcome, backfill_ms);
            (outcome.texts, Some(stats))
        }
        CellSource::Crop(recognizer) => {
            let backfill_started = Instant::now();
            // 对照路径喂的是**同一张整图**（和参考实现一样），裁剪由
            // `cell_content_backfill` 在 `CellSlot::bbox` 上做，因此同样没有 crop 偏移。
            let outcome = backfill(
                &result.cells,
                BackfillSource::Crop {
                    image: &image,
                    recognizer,
                },
            )?;
            let backfill_ms = backfill_started.elapsed().as_secs_f64() * 1000.0;
            let stats = BackfillStats::from_outcome(&outcome, backfill_ms);
            (outcome.texts, Some(stats))
        }
    };
    // 评测口径固定用 `TextPolicy::Raw`：PubTabNet 真值的单元格文本本身就是 HTML 片段
    // （`<sub>` 这类标记成对出现），转义会让预测与真值不可比。用户可见的输出档才用
    // `Escape`（见 `src/table/emitter.rs` 的 `TextPolicy`）。
    let RenderedTable {
        rendered: html,
        confidence,
        degradations,
    } = emit_html(&result, &texts, TextPolicy::Raw)?;
    let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;

    Ok(Prediction {
        html,
        cells: result.cells.len(),
        preprocess_ms,
        infer_ms,
        decode_ms,
        total_ms: started.elapsed().as_secs_f64() * 1000.0,
        backfill: backfill_stats,
        confidence,
        degradations,
    })
}

fn write_predictions(path: &Path, predictions: &BTreeMap<String, String>) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut object = Map::with_capacity(predictions.len());
    for (name, html) in predictions {
        object.insert(name.clone(), Value::String(html.clone()));
    }
    let encoded = serde_json::to_vec(&Value::Object(object)).map_err(|error| {
        RapidOcrError::Decode(format!("predictions are not serializable: {error}"))
    })?;
    std::fs::write(path, encoded)?;
    Ok(())
}

/// 文件的 `(字节数, sha256)`。
pub fn digest_file(path: &Path) -> Result<(u64, String)> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok((bytes.len() as u64, format!("{:x}", hasher.finalize())))
}

/// 报告里的耗时/计数分布。
fn summarize(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({ "count": 0 });
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |q: f64| -> f64 {
        let index = ((sorted.len() - 1) as f64 * q).round() as usize;
        sorted[index]
    };
    let sum: f64 = sorted.iter().sum();
    json!({
        "count": sorted.len(),
        "min": sorted[0],
        "p50": percentile(0.50),
        "p90": percentile(0.90),
        "max": sorted[sorted.len() - 1],
        "mean": sum / sorted.len() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        OcrCache, TableContentSource, TableEvalConfig, TableEvalManifest, engine_fingerprint,
        engine_label_for, ocr_resolution_report, summarize, summarize_backfill,
    };
    use crate::ocr::pipeline::config::EngineConfig;
    use crate::table::ocr_resolution::TableOcrResolution;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_default_configuration_matches_the_documented_paths() {
        let config = TableEvalConfig::default();
        assert_eq!(
            config.dataset.to_string_lossy().replace('\\', "/"),
            "datasets/table/tabrecset"
        );
        assert_eq!(
            config.model.to_string_lossy().replace('\\', "/"),
            "models/table/slanet-plus.onnx"
        );
        // 全量 + 固定形状 + 488 是 E7 基线用的配置，默认值必须与它一致，
        // 否则"用默认值跑一遍"得到的数字与基线不可比。
        assert_eq!(config.limit, 0);
        assert_eq!(config.side_len, 488);
        assert!(config.fix_input_shape);
    }

    #[test]
    fn an_empty_distribution_reports_a_count_instead_of_panicking() {
        assert_eq!(summarize(&[]), json!({ "count": 0 }));
    }

    #[test]
    fn the_summary_reports_the_same_quartiles_as_the_report() {
        let summary = summarize(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(summary["count"], 4);
        assert_eq!(summary["min"], 1.0);
        assert_eq!(summary["max"], 4.0);
        assert_eq!(summary["mean"], 2.5);
    }

    #[test]
    fn a_bare_array_is_not_a_table_manifest() {
        // 裸数组是 OCR 域的现状，必须继续归 OCR 域，否则 `rapidstruct evaluate` 的
        // 既有 manifest 会被误判。
        let value = json!([{ "image": "a.png", "text": "x" }]);
        assert!(!TableEvalManifest::matches(&value));
    }

    #[test]
    fn an_object_without_the_domain_field_is_not_a_table_manifest() {
        assert!(!TableEvalManifest::matches(&json!({ "cases": [] })));
        assert!(!TableEvalManifest::matches(&json!({ "domain": "ocr" })));
    }

    #[test]
    fn the_domain_field_selects_the_table_path() {
        assert!(TableEvalManifest::matches(&json!({ "domain": "table" })));
    }

    #[test]
    fn a_table_manifest_only_overrides_the_fields_it_names() {
        let manifest: TableEvalManifest =
            serde_json::from_value(json!({ "domain": "table", "limit": 8 }))
                .expect("a minimal table manifest parses");
        let config = manifest.into_config().expect("the domain is table");
        assert_eq!(config.limit, 8);
        // 未给出的字段必须沿用默认值，而不是变成 0/false/空路径。
        let defaults = TableEvalConfig::default();
        assert_eq!(config.dataset, defaults.dataset);
        assert_eq!(config.model, defaults.model);
        assert_eq!(config.side_len, defaults.side_len);
        assert_eq!(config.fix_input_shape, defaults.fix_input_shape);
    }

    #[test]
    fn a_wrong_domain_is_refused_rather_than_silently_run() {
        let manifest: TableEvalManifest = serde_json::from_value(json!({ "domain": "ocr" }))
            .expect("the shape parses even though the domain is wrong");
        let error = manifest
            .into_config()
            .expect_err("the table runner must not accept a non-table manifest");
        assert!(
            error.to_string().contains("domain"),
            "the error must name the offending field, got: {error}"
        );
    }

    #[test]
    fn structure_only_is_the_default_content_source() {
        // 默认必须是结构口径：改成 PageReuse 会让"跑一遍表格评测"悄悄变成
        // "顺带跑一遍全量页级 OCR"（小时级），那不是默认值该有的行为。
        let config = TableEvalConfig::default();
        assert_eq!(config.content, TableContentSource::StructureOnly);
        assert!(!config.content.uses_engine());
        assert!(config.content.is_structure_only());
        assert_eq!(config.content.reported_metric(), "TEDS-Struct");
    }

    #[test]
    fn the_manifest_can_select_the_page_reuse_content_source() {
        let manifest: TableEvalManifest = serde_json::from_value(json!({
            "domain": "table",
            "content": "page-reuse",
            "ocr_cache": "Temp/ocr-cache.json",
        }))
        .expect("the manifest parses");
        let config = manifest.into_config().expect("the domain is table");
        assert!(config.content.uses_engine());
        assert!(!config.content.is_structure_only());
        // 报的是完整 TEDS，不是 TEDS-Struct —— 两个数字不可互换。
        assert_eq!(config.content.reported_metric(), "TEDS");
        match &config.content {
            TableContentSource::PageReuse { config, cache } => {
                assert!(
                    config.is_none(),
                    "an omitted --ocr-config means the default engine"
                );
                assert_eq!(
                    cache
                        .as_deref()
                        .map(|path| path.to_string_lossy().replace('\\', "/")),
                    Some("Temp/ocr-cache.json".to_string())
                );
            }
            other => panic!("expected the page-reuse source, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_content_value_is_refused_rather_than_guessed() {
        // "把不认识的取值当成默认值"会让一份写错字（例如 `page-reuse ` 多一个空格）的
        // manifest 静默跑成结构口径，然后报出一个与预期不同的指标名。
        let manifest: TableEvalManifest =
            serde_json::from_value(json!({ "domain": "table", "content": "page_reuse" }))
                .expect("the shape parses");
        let error = manifest
            .into_config()
            .expect_err("an unknown content source must not be silently accepted");
        assert!(
            error.to_string().contains("page-reuse"),
            "the error must list the accepted values, got: {error}"
        );
    }

    #[test]
    fn the_manifest_can_select_the_crop_control_baseline() {
        // 对照口径与复用口径报同一个指标名（都是完整 TEDS），但 `content_source`
        // 必须能区分它们 —— 否则两份报告放在一起看不出哪份是对照。
        let manifest: TableEvalManifest = serde_json::from_value(json!({
            "domain": "table",
            "content": "crop-control",
            "ocr_config": "models/configs/test-config-small.yaml",
        }))
        .expect("the manifest parses");
        let config = manifest.into_config().expect("the domain is table");
        assert_eq!(config.content.as_str(), "crop_control");
        assert_eq!(config.content.reported_metric(), "TEDS");
        assert!(config.content.uses_engine());
        assert!(!config.content.is_structure_only());
        match &config.content {
            TableContentSource::CropControl { config } => {
                assert_eq!(
                    config.as_deref(),
                    Some(Path::new("models/configs/test-config-small.yaml"))
                );
            }
            other => panic!("expected CropControl, got {other:?}"),
        }
    }

    #[test]
    fn the_page_ocr_label_names_the_config_that_produced_the_text() {
        // 完整 TEDS 绑定在产出单元格文本的那份引擎配置上，所以报告必须自证口径。
        // 这里钉死两种形态；常量标签（如"engine ready"）会让基线无法解释自己的数字。
        assert_eq!(
            engine_label_for(&Some(PathBuf::from(
                "models/configs/test-config-small.yaml"
            ))),
            "page OCR: models/configs/test-config-small.yaml"
        );
        assert_eq!(
            engine_label_for(&None),
            "page OCR: built-in default engine config"
        );
    }

    #[test]
    fn the_engine_fingerprint_follows_the_resolution_not_the_file_path() {
        // 指纹必须描述**真正跑起来的那份配置**。如果它只看配置文件路径，那么
        // `TableOcrResolution::apply` 覆盖出来的分辨率变化就不可见 —— 于是 E10 的
        // 分辨率对照实验会读到同一批缓存文本、得出"分辨率无影响"的假结论。
        let mut base = EngineConfig::default();
        TableOcrResolution::default()
            .apply(&mut base)
            .expect("the default resolution is valid");
        let at_default = engine_fingerprint(&base).expect("the config is serializable");

        let mut coarser = EngineConfig::default();
        TableOcrResolution {
            limit_side_len: 960,
            ..TableOcrResolution::default()
        }
        .apply(&mut coarser)
        .expect("960 is a valid limit_side_len");
        let at_960 = engine_fingerprint(&coarser).expect("the config is serializable");

        assert_ne!(at_default, at_960);

        // 反过来：同一个分辨率重复算必须稳定（否则缓存永远命中不了）。
        let mut again = EngineConfig::default();
        TableOcrResolution::default()
            .apply(&mut again)
            .expect("the default resolution is valid");
        assert_eq!(
            engine_fingerprint(&again).expect("the config is serializable"),
            at_default
        );
    }

    #[test]
    fn a_cache_from_another_engine_is_refused_rather_than_reused() {
        // 这是 E10 的前置缺陷：缓存原本只按样本路径做键，换配置/换分辨率后旧文本会被
        // 静默当成新文本用。这里钉死"拒绝"而不是"部分复用"。
        let path = std::env::temp_dir().join(format!(
            "rapid-ocr-table-cache-fingerprint-{}.json",
            std::process::id()
        ));
        let cache = OcrCache {
            version: OcrCache::VERSION,
            engine_fingerprint: "aaaa".to_string(),
            entries: Default::default(),
        };
        cache.save(&path).expect("the temp directory is writable");

        let error = OcrCache::load(&path, "bbbb")
            .expect_err("a fingerprint mismatch must not be silently accepted");
        let text = error.to_string();
        assert!(
            text.contains("aaaa"),
            "the error must name the cached fingerprint: {text}"
        );
        assert!(
            text.contains("bbbb"),
            "the error must name the requested one: {text}"
        );
        assert!(
            text.contains("--ocr-cache"),
            "the error must say how to fix it: {text}"
        );

        // 指纹相符时正常读回。
        let loaded = OcrCache::load(&path, "aaaa").expect("a matching fingerprint loads");
        assert!(loaded.entries.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_older_cache_version_is_refused() {
        // 版本号不是装饰：v1 的缓存没有 `engine_fingerprint` 字段，把它当成 v2 读进来
        // 会得到一个空指纹，于是"指纹相符"永远不成立 —— 但错误信息会指向错误的原因。
        let path = std::env::temp_dir().join(format!(
            "rapid-ocr-table-cache-version-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, r#"{"version":1,"entries":{}}"#)
            .expect("the temp directory is writable");
        let error = OcrCache::load(&path, "aaaa").expect_err("version 1 must not be accepted");
        assert!(error.to_string().contains("version"), "{error}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_manifest_can_override_the_table_ocr_resolution() {
        let manifest: TableEvalManifest = serde_json::from_value(json!({
            "domain": "table",
            "content": "page-reuse",
            "ocr_resolution": {
                "limit_side_len": 960,
                "limit_type": "max",
                "max_side_len": 1280
            },
        }))
        .expect("the manifest parses");
        let config = manifest.into_config().expect("the domain is table");
        assert_eq!(config.ocr_resolution.limit_side_len, 960);
        assert_eq!(config.ocr_resolution.limit_type, "max");
        assert_eq!(config.ocr_resolution.max_side_len, 1280);
    }

    #[test]
    fn the_default_resolution_never_upscales_the_table_image() {
        // 默认值必须能被单测钉住：它是完整 TEDS 基线的一部分（换掉它就要重跑基线）。
        let config = TableEvalConfig::default();
        assert_eq!(config.ocr_resolution, TableOcrResolution::default());
        assert_eq!(config.ocr_resolution.limit_side_len, 736);
        // `"max"` 而不是官方的 `"min"`：官方值的前提是"输入是整页"，而表格路径的输入是
        // 裁剪后的小表（TabRecSet val 有 99.3 % 的短边 < 736），`min` 会把它们放大 24 倍面积。
        assert_eq!(config.ocr_resolution.limit_type, "max");
        assert_eq!(config.ocr_resolution.max_side_len, 736);
    }

    #[test]
    fn the_report_names_the_live_knob_of_the_resolution_policy() {
        // 默认是 `max`：报告必须写明 `limit_side_len` 被 det 忽略，活的是 `max_side_len`。
        let report = ocr_resolution_report(&TableEvalConfig::default());
        assert_eq!(report["limit_type"], "max");
        assert_eq!(report["limit_side_len"], 736);
        assert_eq!(report["max_side_len"], 736);
        assert!(
            report["live_knob"]
                .as_str()
                .expect("live_knob is a string")
                .starts_with("max_side_len"),
            "{report}"
        );

        // `min` 下活的是 `limit_side_len`。这条分支对照不能省：只有默认分支被断言时，
        // `live_knob` 被写死成常量也能通过。
        let config = TableEvalConfig {
            ocr_resolution: TableOcrResolution {
                limit_side_len: 320,
                limit_type: "min".to_string(),
                max_side_len: 2000,
            },
            ..TableEvalConfig::default()
        };
        let report = ocr_resolution_report(&config);
        assert!(
            report["live_knob"]
                .as_str()
                .expect("live_knob is a string")
                .starts_with("limit_side_len"),
            "{report}"
        );
    }

    #[test]
    fn the_report_says_the_resolution_only_applies_to_the_page_reuse_path() {
        // 结构口径下根本不建页级 OCR 引擎，分辨率旋钮不作用于任何东西。
        // 报告必须自述这一点，否则读基线的人会把"结构口径"与"页级复用口径"混为一谈。
        let report = ocr_resolution_report(&TableEvalConfig::default());
        assert_eq!(
            report["applies_to"],
            "nothing (only the page-reuse path runs det)"
        );

        let config = TableEvalConfig {
            content: TableContentSource::PageReuse {
                config: None,
                cache: None,
            },
            ..TableEvalConfig::default()
        };
        assert_eq!(ocr_resolution_report(&config)["applies_to"], "page_reuse");
    }

    #[test]
    fn a_backfill_summary_over_no_samples_reports_counts_not_ratios() {
        // 与 `summarize(&[])` 同一个先例：空输入报"数量为零"，不报 NaN、不 panic。
        // 报告层面还有一道更严的守卫 —— `StructureOnly` 下 `backfill` 字段是 `null`
        // 而不是一个"回填了但一个字都没填上"的零值对象。
        let summary = summarize_backfill(&[], &TableContentSource::StructureOnly);
        assert_eq!(summary["cells"], 0);
        assert_eq!(summary["non_empty_rate"], 0.0);
        assert_eq!(summary["recognizer_calls"], 0);
        assert!(summary["text_confidence"].is_null());
        // G2 的判据方向必须随口径切换：结构/复用口径下非零调用是失败，对照口径下不是。
        assert!(
            summary["recognizer_calls_note"]
                .as_str()
                .expect("the note is a string")
                .contains("gate G2")
        );
        let control = summarize_backfill(&[], &TableContentSource::CropControl { config: None });
        assert!(
            !control["recognizer_calls_note"]
                .as_str()
                .expect("the note is a string")
                .contains("gate G2"),
            "the comparison baseline must not be reported as a gate G2 failure"
        );
    }
}
