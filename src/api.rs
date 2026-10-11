//! Stable, application-neutral OCR contracts.

use crate::error::{RapidOcrError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    Bgra8,
    Rgba8,
    Rgb8,
    Gray8,
    GrayAlpha8,
}
impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Bgra8 | Self::Rgba8 => 4,
            Self::Rgb8 => 3,
            Self::Gray8 => 1,
            Self::GrayAlpha8 => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OwnedPixelBuffer {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    pub bottom_up: bool,
    pub data: Arc<[u8]>,
}
impl OwnedPixelBuffer {
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(RapidOcrError::InvalidInput(
                "pixel buffer width/height must be greater than zero".into(),
            ));
        }
        let row = (self.width as usize)
            .checked_mul(self.format.bytes_per_pixel())
            .ok_or_else(|| RapidOcrError::InvalidInput("pixel row size overflow".into()))?;
        if self.stride < row {
            return Err(RapidOcrError::InvalidInput(format!(
                "pixel stride {} is smaller than row size {}",
                self.stride, row
            )));
        }
        let required = self
            .stride
            .checked_mul(self.height as usize)
            .ok_or_else(|| RapidOcrError::InvalidInput("pixel buffer size overflow".into()))?;
        if self.data.len() < required {
            return Err(RapidOcrError::InvalidInput(format!(
                "pixel buffer has {} bytes, requires at least {}",
                self.data.len(),
                required
            )));
        }
        Ok(())
    }
    pub fn to_bgr(&self, roi: Option<RectU32>) -> Result<(Vec<u8>, ImageSize, (u32, u32))> {
        self.validate()?;
        let selected = roi.unwrap_or(RectU32 {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        });
        selected.validate_against(self.width, self.height)?;
        let channels = self.format.bytes_per_pixel();
        let row_bytes = selected.width as usize * 3;
        let mut out = vec![0; row_bytes * selected.height as usize];
        for y in 0..selected.height as usize {
            let sy = if self.bottom_up {
                self.height as usize - 1 - (selected.y as usize + y)
            } else {
                selected.y as usize + y
            };
            let start = sy * self.stride + selected.x as usize * channels;
            let src = &self.data[start..start + selected.width as usize * channels];
            let dst = &mut out[y * row_bytes..(y + 1) * row_bytes];
            for x in 0..selected.width as usize {
                let s = &src[x * channels..(x + 1) * channels];
                let d = &mut dst[x * 3..x * 3 + 3];
                match self.format {
                    PixelFormat::Bgra8 => {
                        let a = s[3] as u16;
                        d[0] = ((s[0] as u16 * a + 255 * (255 - a)) / 255) as u8;
                        d[1] = ((s[1] as u16 * a + 255 * (255 - a)) / 255) as u8;
                        d[2] = ((s[2] as u16 * a + 255 * (255 - a)) / 255) as u8;
                    }
                    PixelFormat::Rgba8 => {
                        let a = s[3] as u16;
                        d[0] = ((s[2] as u16 * a + 255 * (255 - a)) / 255) as u8;
                        d[1] = ((s[1] as u16 * a + 255 * (255 - a)) / 255) as u8;
                        d[2] = ((s[0] as u16 * a + 255 * (255 - a)) / 255) as u8;
                    }
                    PixelFormat::Rgb8 => d.copy_from_slice(&[s[2], s[1], s[0]]),
                    PixelFormat::Gray8 => d.fill(s[0]),
                    PixelFormat::GrayAlpha8 => {
                        let a = s[1] as u16;
                        let value = ((s[0] as u16 * a + 255 * (255 - a)) / 255) as u8;
                        d.fill(value);
                    }
                }
            }
        }
        Ok((
            out,
            ImageSize {
                width: selected.width,
                height: selected.height,
            },
            (selected.x, selected.y),
        ))
    }
}

#[derive(Debug, Clone)]
pub enum ImageInput {
    Encoded(Arc<[u8]>),
    Pixels(OwnedPixelBuffer),
    File(PathBuf),
    Url(String),
    Image(crate::config::RecImage),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RectU32 {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl RectU32 {
    pub fn validate_against(self, w: u32, h: u32) -> Result<()> {
        let r = self
            .x
            .checked_add(self.width)
            .ok_or_else(|| RapidOcrError::InvalidInput("ROI x/width overflow".into()))?;
        let b = self
            .y
            .checked_add(self.height)
            .ok_or_else(|| RapidOcrError::InvalidInput("ROI y/height overflow".into()))?;
        if self.width == 0 || self.height == 0 || r > w || b > h {
            return Err(RapidOcrError::InvalidInput(format!(
                "ROI {self:?} is outside image {w}x{h}"
            )));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TilePolicy {
    pub max_width: u32,
    pub max_height: u32,
    pub overlap: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EnhancementPolicy {
    #[default]
    None,
    ScreenAdaptive,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreprocessPolicy {
    /// Maximum number of decoded pixels accepted for any image input.
    pub max_decode_pixels: u64,
    /// Maximum encoded byte length accepted for any encoded image input.
    ///
    /// This bound applies uniformly to file, URL and caller-supplied encoded
    /// buffers so compressed payloads are rejected before any decode attempt.
    /// URL bodies are additionally streamed with a hard read cap derived from
    /// this value, so an absent or lying `Content-Length` cannot force an
    /// unbounded buffer. Decoded pixel buffers are covered separately by
    /// `max_decode_pixels`.
    pub max_encoded_bytes: u64,
    /// Maximum long side for engine preprocessing.
    ///
    /// `None` defers to `EngineConfig::global.max_side_len`; `Some` overrides
    /// that engine-level bound for the request.
    pub max_side: Option<u32>,
    /// Optional lower bound for text scale used during recognition.
    pub min_text_scale: Option<f32>,
    /// Optional tiling policy for segmented OCR.
    pub tile: Option<TilePolicy>,
    /// Optional image enhancement policy.
    pub enhance: EnhancementPolicy,
}
impl Default for PreprocessPolicy {
    fn default() -> Self {
        Self {
            max_decode_pixels: 24_000_000,
            max_encoded_bytes: 128 * 1024 * 1024,
            // `None` defers to `EngineConfig::global.max_side_len`.
            max_side: None,
            min_text_scale: None,
            tile: None,
            enhance: EnhancementPolicy::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassifierPolicy {
    Off,
    IfAvailable,
    Required,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifierPlan {
    pub policy: ClassifierPolicy,
    pub apply_rotation: bool,
}
impl Default for ClassifierPlan {
    fn default() -> Self {
        Self {
            policy: ClassifierPolicy::IfAvailable,
            apply_rotation: true,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagePlan {
    pub detect: bool,
    pub classify: ClassifierPlan,
    pub recognize: bool,
}
impl Default for StagePlan {
    fn default() -> Self {
        Self {
            detect: true,
            classify: ClassifierPlan::default(),
            recognize: true,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordOutputMode {
    Off,
    Words,
    Chars,
}
impl StagePlan {
    pub fn validate(self, words: WordOutputMode) -> Result<()> {
        if !self.detect
            && !self.recognize
            && matches!(self.classify.policy, ClassifierPolicy::Off)
            && matches!(words, WordOutputMode::Off)
        {
            return Err(RapidOcrError::InvalidInput(
                "at least one OCR stage must be enabled".into(),
            ));
        }
        if !self.recognize && !matches!(words, WordOutputMode::Off) {
            return Err(RapidOcrError::InvalidInput(
                "word output requires recognition stage".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct DetectionPolicy {
    pub box_thresh: Option<f32>,
    pub unclip_ratio: Option<f32>,
    pub text_score: Option<f32>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RecognitionPolicy {
    pub words: WordOutputMode,
}
impl Default for RecognitionPolicy {
    fn default() -> Self {
        Self {
            words: WordOutputMode::Off,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CoordinateSpace {
    #[default]
    Image,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OutputPolicy {
    pub coordinate_space: CoordinateSpace,
}
impl Default for OutputPolicy {
    fn default() -> Self {
        Self {
            coordinate_space: CoordinateSpace::Image,
        }
    }
}
#[derive(Debug, Clone)]
pub struct OcrRequest {
    pub input: ImageInput,
    pub roi: Option<RectU32>,
    pub scale_hint: Option<f32>,
    pub stages: StagePlan,
    pub preprocess: PreprocessPolicy,
    pub detection: DetectionPolicy,
    pub recognition: RecognitionPolicy,
    pub output: OutputPolicy,
    /// 页面级公式路由；默认关闭，关闭时行为与不含公式功能时一致。
    pub formula: FormulaPolicy,
    /// 页面级表格路由；默认关闭，关闭时行为与不含表格功能时一致。
    pub table: TablePolicy,
}
impl OcrRequest {
    pub fn validate(&self) -> Result<()> {
        self.stages.validate(self.recognition.words)?;
        self.formula.validate()?;
        self.table.validate()?;
        if self.formula.enabled {
            self.reject_off_page_premise(
                "formula",
                "formula regions are expressed in original image coordinates",
            )?;
        }
        // 表格路由与公式路由共享同一条坐标前提，因此共享同一条约束（判据只有一份，
        // 见 `reject_off_page_premise`）。表格的定位器吃的是**整页文本框分布**（行高一致性、
        // x 坐标成列），结构模型吃的是整页坐标下的裁剪；`roi` 把输入裁成子矩形、`tile` 把
        // 输入切成多块分别识别，两者都会让"定位器看到的那一页"与"调用方要的那一页"不是同一页。
        // 坐标**恰好**仍能对上（页级文本区域的 polygon 会被 `+ offset` 映射回原图），所以这不是
        // 一次会崩的错，而是一次会静默返回调用方没问过的结果的错 —— 正是结构化拒绝存在的理由。
        if self.table.enabled {
            self.reject_off_page_premise(
                "table",
                "the table locator reads the whole page's text layout",
            )?;
        }
        if self.preprocess.max_decode_pixels == 0 {
            return Err(RapidOcrError::InvalidInput(
                "max_decode_pixels must be greater than zero".into(),
            ));
        }
        if self.preprocess.max_encoded_bytes == 0 {
            return Err(RapidOcrError::InvalidInput(
                "max_encoded_bytes must be greater than zero".into(),
            ));
        }
        if self.preprocess.max_side.is_some_and(|v| v == 0) {
            return Err(RapidOcrError::InvalidInput(
                "max_side must be greater than zero when set".into(),
            ));
        }
        if let Some(v) = self.preprocess.min_text_scale
            && (!v.is_finite() || v <= 0.0)
        {
            return Err(RapidOcrError::InvalidInput(
                "min_text_scale must be finite and greater than zero".into(),
            ));
        }
        if let Some(v) = self.scale_hint
            && (!v.is_finite() || v <= 0.0)
        {
            return Err(RapidOcrError::InvalidInput(
                "scale_hint must be finite and greater than zero".into(),
            ));
        }
        for (name, value) in [
            ("box_thresh", self.detection.box_thresh),
            ("text_score", self.detection.text_score),
        ] {
            if let Some(value) = value
                && (!value.is_finite() || !(0.0..=1.0).contains(&value))
            {
                return Err(RapidOcrError::InvalidInput(format!(
                    "{name} must be finite and in [0,1]"
                )));
            }
        }
        if let Some(value) = self.detection.unclip_ratio
            && (!value.is_finite() || value <= 0.0)
        {
            return Err(RapidOcrError::InvalidInput(
                "unclip_ratio must be finite and greater than zero".into(),
            ));
        }
        if let Some(t) = self.preprocess.tile
            && (t.max_width == 0
                || t.max_height == 0
                || t.overlap >= t.max_width
                || t.overlap >= t.max_height)
        {
            return Err(RapidOcrError::InvalidInput(
                "tile dimensions must be positive and overlap must be smaller than dimensions"
                    .into(),
            ));
        }
        Ok(())
    }

    /// 页面级阶段（公式/表格）共享的坐标前提判据。
    ///
    /// 两条阶段路由都需要**未裁、未分块的整页**（公式区域以原图坐标表达、表格定位器读
    /// 整页文本分布），因此共享同一条 `roi`/`tile` 禁令。模板统一、理由由调用方传入，
    /// 判据在整个 crate 里只出现这一次。
    fn reject_off_page_premise(&self, stage: &str, roi_reason: &str) -> Result<()> {
        if self.roi.is_some() {
            return Err(RapidOcrError::InvalidInput(format!(
                "{stage} routing does not support `roi`: {roi_reason}"
            )));
        }
        if self.preprocess.tile.is_some() {
            return Err(RapidOcrError::InvalidInput(format!(
                "{stage} routing does not support tiled preprocessing"
            )));
        }
        Ok(())
    }
}

/// 本次请求实际要跑的页面级阶段。**顺序不在这里**（顺序由调度器按依赖决定）。
/// 由 [`ValidatedRequest::plan`] 从已校验的 `OcrRequest` 推导，调度器只看本类型、不再看策略字段。
#[derive(Debug, Clone, Default)]
pub struct PageRoute {
    formula: Option<FormulaPolicy>,
    table: Option<TablePolicy>,
}

impl PageRoute {
    pub fn runs_formula(&self) -> bool {
        self.formula.is_some()
    }
    pub fn runs_table(&self) -> bool {
        self.table.is_some()
    }
    pub fn formula(&self) -> Option<&FormulaPolicy> {
        self.formula.as_ref()
    }
    pub fn table(&self) -> Option<&TablePolicy> {
        self.table.as_ref()
    }
}

/// 校验与路由的**唯一产物**：不可变的计划交给调度器。
#[derive(Debug, Clone)]
pub struct ValidatedRequest {
    request: OcrRequest,
    route: PageRoute,
}

impl ValidatedRequest {
    /// 一次完成：校验所有策略与坐标前提（`OcrRequest::validate`）→ 按启用位推导页面路由。
    pub fn plan(request: OcrRequest) -> Result<Self> {
        request.validate()?;
        let route = PageRoute {
            formula: request.formula.enabled.then(|| request.formula.clone()),
            table: request.table.enabled.then(|| request.table.clone()),
        };
        Ok(Self { request, route })
    }
    pub fn request(&self) -> &OcrRequest {
        &self.request
    }
    pub fn route(&self) -> &PageRoute {
        &self.route
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ImageSize {
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    pub points: [[f32; 2]; 4],
}
impl Polygon {
    pub fn validate(self, size: ImageSize) -> Result<()> {
        for [x, y] in self.points {
            if !x.is_finite()
                || !y.is_finite()
                || x < 0.0
                || y < 0.0
                || x > size.width as f32
                || y > size.height as f32
            {
                return Err(RapidOcrError::InvalidInput(format!(
                    "polygon point ({x}, {y}) is outside image {:?}",
                    size
                )));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegionSource {
    Detected { detector_index: usize },
    Input,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextOrientation {
    Deg0,
    Deg180,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionOutcome {
    pub score: f32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationOutcome {
    pub orientation: TextOrientation,
    pub score: f32,
    pub applied_rotation: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WordKind {
    Char,
    Token,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrWord {
    pub text: String,
    pub score: f32,
    pub polygon: Polygon,
    pub kind: WordKind,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecognitionOutcome {
    pub text: String,
    pub score: f32,
    pub words: Option<Vec<OcrWord>>,
}

/// 区域语义：普通文本区域、由公式模型识别的公式区域，或由表格结构模型识别的表格区域。
///
/// 公式区域**不携带** CTC 文本识别结果：公式模型没有可与 CTC 平行解释的
/// per-character 置信度，强行塞进 `RecognitionOutcome` 会伪造置信度语义。
///
/// 表格区域同样不携带 `RecognitionOutcome`：它的单元格文本来自**页级**识别结果，
/// 通过几何匹配回填（见 [`TableOutcome::cells`]）。给表格区域再挂一份 CTC 结果
/// 会让"这份文本到底是被识别出来的还是被匹配进来的"变成不可分辨的问题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    #[default]
    Text,
    Formula,
    Table,
}

/// 表格结构置信档。
///
/// `Low` **不是**错误：它表示逻辑网格有空洞或被截断（见 [`TableDegradation`]），
/// 而表格仍然被完整产出。调用方必须显式区分"良构"与"降级"，不允许把两者都当"成功"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TableStructureConfidence {
    #[default]
    High,
    Low,
}

impl TableStructureConfidence {
    /// 线契约里的拼写，与 `#[serde(rename_all = "snake_case")]` 一致。
    ///
    /// 存在的理由：`src/output/` 的 JSON/HTML 渲染需要把置信档写进**非 serde** 的结构
    /// （`OcrJsonItem` 的 `kind` 就是 `&'static str`）。有了这个方法，拼写只有一处定义，
    /// `the_confidence_name_agrees_with_serde` 断言它与 serde 输出一致。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
        }
    }
}

/// 表格结构降级的原因；非空即 [`TableStructureConfidence::Low`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TableDegradation {
    /// 声明的跨行/跨列没有铺满逻辑网格：某处 span 与网格不一致。
    ///
    /// 典型成因是 span **超出模型词表**（SLANet-plus 的 `rowspan`/`colspan` 只到 20，
    /// 而 TabRecSet 验证集里存在 `colspan="24"`），模型只能声明一个更小的值。
    GridHoles { holes: usize },
    /// 解码在见到 `eos` 之前就用尽了步数上限，token 流被截断。
    Truncated,
}

/// 一个单元格的匹配拓扑（不是"文本是否为空"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableCellMatch {
    /// 结构模型给了全零占位框，没有可用几何。
    BlankBox,
    /// 有几何，但没有文本行落进这个单元格。
    Unmatched,
    /// 恰好一行落进这个单元格。
    Single,
    /// 多行落进这个单元格（跨行单元格，或检测把一个单元格切成了多行）。
    Multiple,
}

/// 表格区域识别结果（页面级输出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableOutcome {
    /// 结构树发射出的完整 HTML（含 `<html><body>` 外壳）。
    pub html: String,
    /// 只有 `<table>` 外壳的变体，供只想嵌一段表格的消费者使用。
    pub table_html: String,
    /// Markdown 呈现。`rowspan`/`colspan` 在 Markdown 里无法表达，跨行跨列的格子
    /// 表现为空位；调用方不应把 Markdown 当成无损表示。
    pub markdown: String,
    pub rows: usize,
    pub cols: usize,
    pub confidence: TableStructureConfidence,
    pub degradations: Vec<TableDegradation>,
    /// 逻辑网格里没有被任何单元格覆盖的格子数（`confidence == Low` 的一个成因）。
    pub holes: usize,
    /// 单元格槽位，与 [`Self::html`] 里的 `<td>` **同序**。
    pub cells: Vec<TableCellOutcome>,
    /// 被本表格消费的页级文本行在 [`OcrOutput::regions`] 中的下标（按行序，升序去重）。
    ///
    /// 这些区域**仍然留在** `regions` 里（不删除、不改写）：删掉它们会破坏阅读顺序与
    /// polygon 语义。给出下标是为了让消费者能自己决定怎么呈现（网页默认把它们折叠进表格）。
    pub source_line_indices: Vec<usize>,
    pub model_id: String,
}

/// 一个单元格的槽位信息（几何 + 回填结果）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableCellOutcome {
    pub row: usize,
    pub col: usize,
    pub rowspan: usize,
    pub colspan: usize,
    pub text: String,
    /// 匹配到的文本行分数的算术平均；未匹配为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    pub match_kind: TableCellMatch,
    /// 单元格在**整页像素坐标系**里的四边形；占位（全零）框为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polygon: Option<Polygon>,
}

/// 公式区域识别结果（页面级输出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormulaOutcome {
    /// RapidDoc 兼容的 LaTeX（含 `fix_latex` 与已实现的 `ftfy` 步骤）。
    pub latex: String,
    pub eos_index: Option<usize>,
    pub truncated: bool,
    pub model_id: String,
    /// 原始 token 序列；仅在 `FormulaPolicy::include_token_ids` 时输出。
    pub token_ids: Option<Vec<i64>>,
}

/// 页面级公式路由策略。
///
/// `enabled = false` 时页面 OCR 行为与不含公式功能时完全一致；启用后的实际流程：
///
/// 1. 用 `detector_path` 指定的公式检测模型（可选）在整页上检测公式区域，
///    并合并调用方通过 `input_regions` 显式声明的区域；
/// 2. 候选区域先按 `min_area_ratio` 过滤、再按 `iou_threshold` 与包含关系消解重叠、
///    按分数排序并由 `max_regions` 截断（见 `formula::route`）；
/// 3. **整块公式区域**在送入普通文本管线之前被抹白，因此该区域内的像素根本不会
///    进入检测/识别，也就不会产生 CTC 文本（执行层面跳过 CTC，而不是事后丢弃结果）；
/// 4. 公式区域使用原图（未抹白）裁剪后交给 `FormulaRecognizer` 识别；
/// 5. 结果是独立的 [`RegionKind::Formula`] 区域，保留 crop 坐标与模型信息。
///
/// 抹白是**按区域**而非“按文本检测框覆盖比例”进行的：实现不会先跑一遍文本检测再
/// 逐个文本框判断覆盖率（那需要额外的检测前向）。因此：
///
/// - 一个误检的公式区域会把区域内的**全部**像素（含其中的普通文字）从文本通道移除，
///   不只是其中被检测到的文本框；召回/精度取舍只能通过 `confidence_threshold` 与
///   `min_area_ratio` 完成（README 的公式限制一节记录了实测误检率）；
/// - 区域内没有被文本检测器识别为文本的内容同样会消失；
/// - 由于文本检测的输入像素被改变，**区域外**的文本也可能被重新分割
///   （实测：“42.7 ms” 被重新切成 “42.7” 与 “ms” 两个区域，内容不丢但区域边界变化）。
///   需要严格保持文本切分的调用方不应启用公式路由。
///
/// 不支持 `roi` 与 `tile` 同时启用（坐标映射语义会分叉），此时返回结构化错误。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormulaPolicy {
    pub enabled: bool,
    /// 公式识别模型（`pp_formulanet_plus_m.onnx`）；`enabled` 时必填。
    pub model_path: Option<PathBuf>,
    /// 可选模型 SHA-256 校验。
    pub expected_model_sha256: Option<String>,
    /// 页面公式检测模型（`pix2text-mfd-1.5.onnx`）；为 `None` 时只处理 `input_regions`。
    pub detector_path: Option<PathBuf>,
    /// 检测模型的可选 SHA-256 校验。
    ///
    /// **与 [`Self::expected_model_sha256`] 是同一条规则**（而不是第二套机制）：给出即校验，
    /// 不匹配就是 [`crate::RapidOcrError::HashMismatch`]（路径 + 期望 + 实际），
    /// 绝不静默加载一个损坏的检测模型。为 `None` 表示"调用方没有可信摘要"（例如
    /// 用户显式给出的本机路径、模型集也没有声明它的哈希），此时如实记录"无法校验"，
    /// 而不是假装校验过。
    pub expected_detector_sha256: Option<String>,
    /// 公式检测置信度阈值。
    pub confidence_threshold: f32,
    /// 公式检测 NMS IoU 阈值。
    pub iou_threshold: f32,
    /// 每页最多保留的公式区域数。
    pub max_regions: usize,
    /// 候选区域面积占原图面积的最小比例，用于过滤明显误检。
    pub min_area_ratio: f32,
    /// 调用方显式声明的公式区域（原图像素坐标）。
    pub input_regions: Vec<Polygon>,
    /// 是否在输出中保留原始 token IDs。
    pub include_token_ids: bool,
}

impl Default for FormulaPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            model_path: None,
            expected_model_sha256: None,
            detector_path: None,
            expected_detector_sha256: None,
            confidence_threshold: crate::formula::detect::DEFAULT_FORMULA_DETECT_CONFIDENCE,
            iou_threshold: crate::formula::detect::DEFAULT_FORMULA_DETECT_IOU,
            max_regions: 64,
            min_area_ratio: 0.000_05,
            input_regions: Vec::new(),
            include_token_ids: false,
        }
    }
}

impl FormulaPolicy {
    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.model_path.is_none() {
            return Err(crate::model_verify::missing_model_path_error("formula"));
        }
        for (name, value) in [
            ("confidence_threshold", self.confidence_threshold),
            ("iou_threshold", self.iou_threshold),
            ("min_area_ratio", self.min_area_ratio),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(RapidOcrError::InvalidInput(format!(
                    "formula {name} must be finite and in [0,1]"
                )));
            }
        }
        if self.max_regions == 0 {
            return Err(RapidOcrError::InvalidInput(
                "formula max_regions must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

/// 页面级表格路由的文本编码策略。
///
/// **没有隐式默认的"随便一个"**：两个取值服务于两个互斥的目的。
///
/// - [`Self::Escape`]（默认）：面向用户可见的输出。单元格文本是**模型产物**，
///   原样拼进 HTML 再交给浏览器渲染就是一个注入面。
/// - [`Self::Raw`]：评测口径。PubTabNet 真值里的单元格文本本身是 HTML 片段
///   （`<` / `>` 在 128,667 个非空单元格里各出现 4,171 次且**成对**），转义会改变
///   与真值的可比性，因此评测必须显式选择 `Raw`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TableTextPolicy {
    #[default]
    Escape,
    Raw,
}

/// 页面级表格路由策略。
///
/// `enabled = false` 时页面 OCR 行为与不含表格功能时**完全一致**（门禁 G6）；
/// 启用后的实际流程：
///
/// 1. 在**原始图**上用 `TextDistributionDetector` 从**已有 det 文本框的分布**推断表格区域
///    （零新增整页 CNN，门禁 G4）；
/// 2. 每个区域裁剪后交给结构模型（`model_path`）推理，得到 bbox 与 token 两路输出；
/// 3. 解码出结构树与逻辑网格，并把单元格几何**平移回整页坐标系**（加上区域原点）；
/// 4. 单元格文本**不重新识别**：用几何匹配从页级识别结果里取（门禁 G2/G7，
///    结构不变量 `recognizer_calls == 0`）；
/// 5. 每个表格是一个独立的 [`RegionKind::Table`] 区域，携带 HTML / Markdown / 单元格槽位。
///
/// **表格像素不抹白**（与公式相反）：表格的单元格文本正是要保留的页级识别结果。
/// 公式与表格同时启用时，公式抹白只作用于文本通道，而表格定位与结构推理都在原始图上进行。
/// 因此**落在公式区域内的表格单元格会匹配不到文本**（那些像素已被抹白）——这是已知的
/// 交互边界，会在 [`TableOutcome::cells`] 的 `match_kind` 上表现为 `Unmatched`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TablePolicy {
    pub enabled: bool,
    /// 结构模型（`slanet-plus.onnx`）；`enabled` 时必填。
    pub model_path: Option<PathBuf>,
    /// 可选模型 SHA-256 校验。
    ///
    /// **与 [`FormulaPolicy::expected_model_sha256`] 是同一条规则**（而不是第二套机制）：
    /// 给出即校验，不匹配就是 [`crate::RapidOcrError::HashMismatch`]。
    /// 为 `None` 时回落到**模型同目录清单**里登记的摘要（`models/table/manifest.json`），
    /// 两者都没有时如实记录"无法校验"。
    pub expected_model_sha256: Option<String>,
    /// 结构模型输入画布的边长（方形）。默认 488。
    pub input_side_len: usize,
    /// 是否用 free-dimension override 把输入 H/W 固定为 `input_side_len`。
    /// **绝不影响解码步数上限**（L 始终动态）。默认 `true`。
    pub fix_input_shape: bool,
    /// 结构会话的 intra 线程数；`None` 表示继承运行时推导值。
    ///
    /// 保留这个旋钮是因为"多会话自旋争抢"的怀疑需要可被实测证伪的入口；
    /// 默认值**不**改（E12 实测：4/8/继承 三者最大偏差 −6.89 %，小于继承值自身的
    /// 9.30 % 重复间离散度，落在噪声带内 → 按判据回退）。
    pub intra_threads: Option<usize>,
    /// 保留一个表格区域所需的最低定位分数（`[0,1]`）。默认 0.0（不过滤）。
    pub min_region_score: f32,
    /// 每页最多保留的表格区域数。默认 8。
    pub max_regions: usize,
    /// 发射到 [`TableOutcome::html`] 时的文本编码策略。
    pub text_policy: TableTextPolicy,
}

impl Default for TablePolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            model_path: None,
            expected_model_sha256: None,
            input_side_len: crate::table::structure::TABLE_STRUCTURE_SIDE_LEN,
            fix_input_shape: true,
            intra_threads: None,
            min_region_score: 0.0,
            max_regions: 8,
            text_policy: TableTextPolicy::Escape,
        }
    }
}

impl TablePolicy {
    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.model_path.is_none() {
            return Err(crate::model_verify::missing_model_path_error("table"));
        }
        if self.input_side_len == 0 {
            return Err(RapidOcrError::InvalidInput(
                "table input_side_len must be greater than zero".into(),
            ));
        }
        if !self.min_region_score.is_finite() || !(0.0..=1.0).contains(&self.min_region_score) {
            return Err(RapidOcrError::InvalidInput(
                "table min_region_score must be finite and in [0,1]".into(),
            ));
        }
        if self.max_regions == 0 {
            return Err(RapidOcrError::InvalidInput(
                "table max_regions must be greater than zero".into(),
            ));
        }
        if self.intra_threads == Some(0) {
            return Err(RapidOcrError::InvalidInput(
                "table intra_threads must be greater than zero when set".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrRegion {
    pub source: RegionSource,
    /// 区域语义；默认为普通文本区域。
    #[serde(default)]
    pub kind: RegionKind,
    pub polygon: Option<Polygon>,
    pub detection: Option<DetectionOutcome>,
    pub classification: Option<ClassificationOutcome>,
    pub recognition: Option<RecognitionOutcome>,
    /// 公式区域结果；仅当 `kind == RegionKind::Formula` 时为 `Some`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<FormulaOutcome>,
    /// 表格区域结果；仅当 `kind == RegionKind::Table` 时为 `Some`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<TableOutcome>,
}

impl OcrRegion {
    /// 构造普通文本区域（公式与表格字段为空）。
    pub fn text(
        source: RegionSource,
        polygon: Option<Polygon>,
        detection: Option<DetectionOutcome>,
        classification: Option<ClassificationOutcome>,
        recognition: Option<RecognitionOutcome>,
    ) -> Self {
        Self {
            source,
            kind: RegionKind::Text,
            polygon,
            detection,
            classification,
            recognition,
            formula: None,
            table: None,
        }
    }

    /// 构造表格区域（不携带 CTC 识别结果，见 [`RegionKind`] 的文档）。
    pub fn table(
        source: RegionSource,
        polygon: Option<Polygon>,
        detection: Option<DetectionOutcome>,
        outcome: TableOutcome,
    ) -> Self {
        Self {
            source,
            kind: RegionKind::Table,
            polygon,
            detection,
            classification: None,
            recognition: None,
            formula: None,
            table: Some(outcome),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StageState {
    #[default]
    Disabled,
    SkippedNoInput,
    SkippedUnavailable,
    Completed {
        items: usize,
    },
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct StageTiming {
    pub preprocess_ms: f32,
    pub infer_ms: f32,
    pub postprocess_ms: f32,
}
impl StageTiming {
    pub fn total_ms(self) -> f32 {
        self.preprocess_ms + self.infer_ms + self.postprocess_ms
    }
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct StageReport {
    pub state: StageState,
    pub timing: Option<StageTiming>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputTimings {
    pub decode_ms: Option<f32>,
    pub resize_ms: Option<f32>,
    pub crop_ms: Option<f32>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StageReports {
    pub input: InputTimings,
    pub detector: StageReport,
    pub classifier: StageReport,
    pub recognizer: StageReport,
    /// 页面级公式路由阶段；未启用时为 `Disabled`。
    #[serde(default)]
    pub formula: StageReport,
    /// 页面级表格阶段；未启用时为 `Disabled`。
    ///
    /// 三段口径与 [`OcrTimings`] 的表格字段一一对应：
    /// `timing.preprocess_ms` = [`OcrTimings::table_detect_ms`]（表格定位）、
    /// `timing.infer_ms` = [`OcrTimings::table_structure_infer_ms`]（结构识别 ONNX 推理）、
    /// `timing.postprocess_ms` = [`OcrTimings::table_backfill_ms`]（单元格内容回填）。
    #[serde(default)]
    pub table: StageReport,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OcrTimings {
    pub decode_ms: f32,
    pub resize_ms: f32,
    pub crop_ms: f32,
    pub preprocess_ms: f32,
    pub detector_preprocess_ms: f32,
    pub detector_infer_ms: f32,
    pub detector_postprocess_ms: f32,
    pub detect_ms: f32,
    pub classifier_preprocess_ms: f32,
    pub classifier_infer_ms: f32,
    pub classifier_postprocess_ms: f32,
    pub classify_ms: f32,
    pub recognizer_preprocess_ms: f32,
    pub recognizer_infer_ms: f32,
    pub recognizer_postprocess_ms: f32,
    pub recognize_ms: f32,
    /// 页面级公式路由耗时（检测 + 裁剪 + 公式识别）；未启用时为 0。
    #[serde(default)]
    pub formula_ms: f32,
    /// 页面级表格定位耗时（表格区域检测）；未启用时为 0。
    #[serde(default)]
    pub table_detect_ms: f32,
    /// 表格结构识别的 ONNX Runtime 推理耗时；未启用时为 0。
    #[serde(default)]
    pub table_structure_infer_ms: f32,
    /// 表格单元格内容回填耗时（整页检测复用 + 几何匹配）；未启用时为 0。
    #[serde(default)]
    pub table_backfill_ms: f32,
    pub postprocess_ms: f32,
    pub total_ms: f32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineInfo {
    pub model_id: String,
    pub provider: ProviderInfo,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub original_size: ImageSize,
    pub processed_size: ImageSize,
    pub coordinate_space: CoordinateSpace,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrOutput {
    pub schema_version: u32,
    pub image: ImageInfo,
    pub stages: StageReports,
    pub regions: Vec<OcrRegion>,
    pub timings: OcrTimings,
    pub engine: EngineInfo,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOrder {
    Reading,
    Detection,
}
impl OcrOutput {
    /// 区域总数（文本 + 公式）。
    ///
    /// 公式区域也是识别结果：只有公式的页面不是空页面。需要文本数量时使用
    /// [`OcrOutput::text_len`]。
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// 带 CTC 文本结果的区域数。
    pub fn text_len(&self) -> usize {
        self.regions
            .iter()
            .filter(|region| region.recognition.is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn reading_order(&self) -> Vec<usize> {
        self.reading_order_groups().into_iter().flatten().collect()
    }

    pub(crate) fn reading_order_groups(&self) -> Vec<Vec<usize>> {
        let mut boxes = Vec::with_capacity(self.regions.len());
        let mut without_polygon = Vec::new();
        for (id, region) in self.regions.iter().enumerate() {
            let Some(polygon) = region.polygon else {
                without_polygon.push(id);
                continue;
            };
            let Some(reading_box) = ReadingBox::from_polygon(id, polygon) else {
                without_polygon.push(id);
                continue;
            };
            boxes.push(reading_box);
        }
        let mut groups = order_reading_groups(&boxes);
        groups.extend(without_polygon.into_iter().map(|id| vec![id]));
        groups
    }
    pub fn plain_text(&self, order: TextOrder) -> String {
        let ids = match order {
            TextOrder::Reading => self.reading_order(),
            TextOrder::Detection => (0..self.regions.len()).collect(),
        };
        ids.into_iter()
            .filter_map(|i| self.regions[i].recognition.as_ref())
            .map(|v| v.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 按阅读顺序返回公式区域 (region id, latex)。
    pub fn formula_latex(&self, order: TextOrder) -> Vec<(usize, &str)> {
        let ids = match order {
            TextOrder::Reading => self.reading_order(),
            TextOrder::Detection => (0..self.regions.len()).collect(),
        };
        ids.into_iter()
            .filter_map(|id| {
                let region = self.regions.get(id)?;
                let formula = region.formula.as_ref()?;
                Some((id, formula.latex.as_str()))
            })
            .collect()
    }

    /// 公式区域数量。
    pub fn formula_count(&self) -> usize {
        self.regions
            .iter()
            .filter(|region| region.kind == RegionKind::Formula)
            .count()
    }

    /// 表格区域数量。
    pub fn table_count(&self) -> usize {
        self.regions
            .iter()
            .filter(|region| region.kind == RegionKind::Table)
            .count()
    }

    /// 按阅读顺序返回表格区域 (region id, 表格结果)。
    ///
    /// 与 [`OcrOutput::formula_latex`] 同形：表格区域的 `html` 就是它在文本通道里的
    /// 唯一表示，所以两种"非文本区域"走同一套遍历规则。
    pub fn tables(&self, order: TextOrder) -> Vec<(usize, &TableOutcome)> {
        let ids = match order {
            TextOrder::Reading => self.reading_order(),
            TextOrder::Detection => (0..self.regions.len()).collect(),
        };
        ids.into_iter()
            .filter_map(|id| {
                let region = self.regions.get(id)?;
                Some((id, region.table.as_ref()?))
            })
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(RapidOcrError::InvalidInput(format!(
                "unsupported OCR schema version {}",
                self.schema_version
            )));
        }
        for region in &self.regions {
            match region.kind {
                RegionKind::Formula => {
                    if region.formula.is_none() {
                        return Err(RapidOcrError::InvalidInput(
                            "formula region must carry a formula outcome".into(),
                        ));
                    }
                    if region.recognition.is_some() {
                        return Err(RapidOcrError::InvalidInput(
                            "formula region must not carry a CTC recognition outcome".into(),
                        ));
                    }
                }
                RegionKind::Text => {
                    if region.formula.is_some() {
                        return Err(RapidOcrError::InvalidInput(
                            "text region must not carry a formula outcome".into(),
                        ));
                    }
                    if region.table.is_some() {
                        return Err(RapidOcrError::InvalidInput(
                            "text region must not carry a table outcome".into(),
                        ));
                    }
                }
                RegionKind::Table => {
                    if region.table.is_none() {
                        return Err(RapidOcrError::InvalidInput(
                            "table region must carry a table outcome".into(),
                        ));
                    }
                    if region.formula.is_some() {
                        return Err(RapidOcrError::InvalidInput(
                            "table region must not carry a formula outcome".into(),
                        ));
                    }
                    // 与公式同一条理由：表格的单元格文本来自页级识别结果的几何匹配，
                    // 不是对这个区域跑 CTC 得到的。挂一份 `RecognitionOutcome` 会让
                    // "识别出来的"与"匹配进来的"不可分辨。
                    if region.recognition.is_some() {
                        return Err(RapidOcrError::InvalidInput(
                            "table region must not carry a CTC recognition outcome".into(),
                        ));
                    }
                }
            }
            if let Some(polygon) = region.polygon {
                polygon.validate(self.image.original_size)?;
            }
            if let Some(detection) = &region.detection
                && (!detection.score.is_finite() || !(0.0..=1.0).contains(&detection.score))
            {
                return Err(RapidOcrError::InvalidInput(
                    "detection score must be finite and in [0,1]".into(),
                ));
            }
            if let Some(classification) = &region.classification
                && (!classification.score.is_finite()
                    || !(0.0..=1.0).contains(&classification.score))
            {
                return Err(RapidOcrError::InvalidInput(
                    "classification score must be finite and in [0,1]".into(),
                ));
            }
            if let Some(recognition) = &region.recognition {
                if !recognition.score.is_finite() || !(0.0..=1.0).contains(&recognition.score) {
                    return Err(RapidOcrError::InvalidInput(
                        "recognition score must be finite and in [0,1]".into(),
                    ));
                }
                if let Some(words) = &recognition.words {
                    for word in words {
                        if !word.score.is_finite() || !(0.0..=1.0).contains(&word.score) {
                            return Err(RapidOcrError::InvalidInput(
                                "word score must be finite and in [0,1]".into(),
                            ));
                        }
                        word.polygon.validate(self.image.original_size)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct ReadingBox {
    id: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl ReadingBox {
    fn from_polygon(id: usize, polygon: Polygon) -> Option<Self> {
        let mut x0 = f32::INFINITY;
        let mut y0 = f32::INFINITY;
        let mut x1 = f32::NEG_INFINITY;
        let mut y1 = f32::NEG_INFINITY;
        for [x, y] in polygon.points {
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Some(Self { id, x0, y0, x1, y1 })
    }

    fn height(self) -> f32 {
        (self.y1 - self.y0).max(0.0)
    }
}

#[derive(Debug, Clone, Copy)]
struct Gap {
    start: f32,
    end: f32,
}

impl Gap {
    fn extent(self) -> f32 {
        (self.end - self.start).max(0.0)
    }

    fn center(self) -> f32 {
        (self.start + self.end) * 0.5
    }
}

#[cfg(test)]
fn order_reading_boxes(boxes: &[ReadingBox]) -> Vec<usize> {
    order_reading_groups(boxes).into_iter().flatten().collect()
}

fn order_reading_groups(boxes: &[ReadingBox]) -> Vec<Vec<usize>> {
    let mut groups = Vec::with_capacity(boxes.len());
    append_reading_order(boxes, &mut groups);
    groups
}

fn append_reading_order(boxes: &[ReadingBox], groups: &mut Vec<Vec<usize>>) {
    if boxes.is_empty() {
        return;
    }
    if boxes.len() == 1 {
        groups.push(vec![boxes[0].id]);
        return;
    }

    let median_height = median_box_height(boxes);
    let vertical_gap = largest_vertical_gap(boxes);
    let horizontal_gap = largest_horizontal_gap(boxes);

    // Large full-width horizontal whitespace is a section break. Split it
    // before column detection so independent top/bottom bands are not
    // interleaved by global x-coordinate columns.
    if let Some(gap) = horizontal_gap
        && gap.extent() >= median_height * SECTION_BREAK_MEDIAN_HEIGHT_FACTOR
        && append_horizontal_split(boxes, gap, groups)
    {
        return;
    }

    // Otherwise vertical whitespace is the strongest signal for columns.
    // Split on it first so two-column pages are read column-by-column rather
    // than by rows.
    if let Some(gap) = vertical_gap {
        let threshold = (median_height * 0.5).max(2.0);
        if gap.extent() >= threshold {
            let split = gap.center();
            let left: Vec<ReadingBox> = boxes.iter().copied().filter(|b| b.x1 <= split).collect();
            let right: Vec<ReadingBox> = boxes.iter().copied().filter(|b| b.x0 >= split).collect();
            if !left.is_empty() && !right.is_empty() && left.len() + right.len() == boxes.len() {
                append_reading_order(&left, groups);
                append_reading_order(&right, groups);
                return;
            }
        }
    }

    // A smaller horizontal gap still separates headings/footers and
    // single-column lines.
    if let Some(gap) = horizontal_gap
        && gap.extent() >= 1.0
        && append_horizontal_split(boxes, gap, groups)
    {
        return;
    }

    append_reading_order_by_lines(boxes, groups);
}

fn append_horizontal_split(boxes: &[ReadingBox], gap: Gap, groups: &mut Vec<Vec<usize>>) -> bool {
    let split = gap.center();
    let top: Vec<ReadingBox> = boxes.iter().copied().filter(|b| b.y1 <= split).collect();
    let bottom: Vec<ReadingBox> = boxes.iter().copied().filter(|b| b.y0 >= split).collect();
    if top.is_empty() || bottom.is_empty() || top.len() + bottom.len() != boxes.len() {
        return false;
    }
    append_reading_order(&top, groups);
    append_reading_order(&bottom, groups);
    true
}

fn append_reading_order_by_lines(boxes: &[ReadingBox], groups: &mut Vec<Vec<usize>>) {
    let mut remaining = boxes.to_vec();
    remaining.sort_by(|a, b| {
        a.y0.total_cmp(&b.y0)
            .then_with(|| a.x0.total_cmp(&b.x0))
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut lines: Vec<Vec<ReadingBox>> = Vec::new();
    for item in remaining {
        let mut best_line: Option<(usize, f32)> = None;
        for (index, line) in lines.iter().enumerate() {
            let line_top = line.iter().map(|b| b.y0).fold(f32::INFINITY, f32::min);
            let line_bottom = line.iter().map(|b| b.y1).fold(f32::NEG_INFINITY, f32::max);
            let line_height = (line_bottom - line_top).max(0.0);
            let overlap = (line_bottom.min(item.y1) - line_top.max(item.y0)).max(0.0);
            let min_height = line_height.min(item.height()).max(0.0001);
            if overlap / min_height >= 0.5
                && best_line.is_none_or(|(_, best_overlap)| overlap > best_overlap)
            {
                best_line = Some((index, overlap));
            }
        }
        if let Some((index, _)) = best_line {
            lines[index].push(item);
        } else {
            lines.push(vec![item]);
        }
    }

    lines.sort_by(|a, b| {
        let a_top = a.iter().map(|b| b.y0).fold(f32::INFINITY, f32::min);
        let b_top = b.iter().map(|b| b.y0).fold(f32::INFINITY, f32::min);
        a_top.total_cmp(&b_top).then_with(|| {
            let a_left = a.iter().map(|b| b.x0).fold(f32::INFINITY, f32::min);
            let b_left = b.iter().map(|b| b.x0).fold(f32::INFINITY, f32::min);
            a_left.total_cmp(&b_left)
        })
    });

    for mut line in lines {
        line.sort_by(|a, b| {
            a.x0.total_cmp(&b.x0)
                .then_with(|| a.y0.total_cmp(&b.y0))
                .then_with(|| a.id.cmp(&b.id))
        });
        groups.push(line.into_iter().map(|b| b.id).collect());
    }
}

const SECTION_BREAK_MEDIAN_HEIGHT_FACTOR: f32 = 1.5;

fn median_box_height(boxes: &[ReadingBox]) -> f32 {
    let mut heights: Vec<f32> = boxes
        .iter()
        .map(|b| b.height())
        .filter(|height| height.is_finite() && *height > 0.0)
        .collect();
    if heights.is_empty() {
        return 1.0;
    }
    heights.sort_by(|a, b| a.total_cmp(b));
    heights[heights.len() / 2]
}

fn largest_vertical_gap(boxes: &[ReadingBox]) -> Option<Gap> {
    largest_gap(boxes.iter().map(|b| (b.x0, b.x1)))
}

fn largest_horizontal_gap(boxes: &[ReadingBox]) -> Option<Gap> {
    largest_gap(boxes.iter().map(|b| (b.y0, b.y1)))
}

fn largest_gap<I>(intervals: I) -> Option<Gap>
where
    I: IntoIterator<Item = (f32, f32)>,
{
    let mut intervals: Vec<(f32, f32)> = intervals
        .into_iter()
        .filter(|(a, b)| a.is_finite() && b.is_finite())
        .map(|(a, b)| if a <= b { (a, b) } else { (b, a) })
        .collect();
    if intervals.len() < 2 {
        return None;
    }
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut merged: Vec<(f32, f32)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
            continue;
        }
        merged.push((start, end));
    }

    merged
        .windows(2)
        .map(|pair| Gap {
            start: pair[0].1,
            end: pair[1].0,
        })
        .filter(|gap| gap.end > gap.start)
        .max_by(|a, b| {
            a.extent()
                .total_cmp(&b.extent())
                .then_with(|| b.start.total_cmp(&a.start))
        })
}

/// 交给 ONNX Runtime 的加速提供者（**EP 链的头部**，不是逐节点执行证据）。
///
/// Windows x64 收窄之后只剩 CPU / DirectML / CUDA；CANN 已整体删除。
///
/// **重要**：`selected_ep` 表示“EP 链已按请求建立且 `is_available()` 自报可用”，
/// **不代表**模型真的在该 EP 上逐节点执行（ORT 不通过该 API 暴露逐节点分配）。
/// 字段名刻意不用 `resolved`：那读起来像“已证明在该 EP 上运行”，而本 API 只能证明
/// “这条链被选中并交给了 ORT”。任何加速结论都必须有实测耗时支撑，见
/// [`crate::runtime::provider`] 与 `docs/04-windows-phase-reports.md` 的阶段 0/2 记录。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedProvider {
    Cpu,
    DirectMl,
    Cuda,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderPreference {
    Cpu,
    DirectMl { device_id: usize },
    Cuda { device_id: usize },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderResolutionInfo {
    pub requested: ProviderPreference,
    /// 交给 ONNX Runtime 的 EP 链头部；`fallback_to_cpu = true` 时为 `Cpu`。
    ///
    /// 逐节点分配不可观测，因此该字段不能作为加速证据（见 [`ResolvedProvider`]）。
    pub selected_ep: ResolvedProvider,
    pub fallback_to_cpu: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub detector: ProviderResolutionInfo,
    pub classifier: Option<ProviderResolutionInfo>,
    pub recognizer: ProviderResolutionInfo,
}
pub trait OcrEngine {
    fn model_id(&self) -> &str;
    fn provider(&self) -> ProviderInfo;
    fn recognize(&mut self, request: OcrRequest) -> Result<OcrOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_empty_plan() {
        let p = StagePlan {
            detect: false,
            classify: ClassifierPlan {
                policy: ClassifierPolicy::Off,
                apply_rotation: false,
            },
            recognize: false,
        };
        assert!(p.validate(WordOutputMode::Off).is_err());
    }
    #[test]
    fn words_require_recognition() {
        let p = StagePlan {
            recognize: false,
            ..StagePlan::default()
        };
        assert!(p.validate(WordOutputMode::Words).is_err());
    }

    #[test]
    fn stage_plan_accepts_all_non_empty_combinations() {
        let classifier = ClassifierPlan {
            policy: ClassifierPolicy::IfAvailable,
            apply_rotation: true,
        };
        for (detect, classify, recognize) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
            (true, true, false),
            (true, false, true),
            (false, true, true),
            (true, true, true),
        ] {
            let plan = StagePlan {
                detect,
                classify: if classify {
                    classifier
                } else {
                    ClassifierPlan {
                        policy: ClassifierPolicy::Off,
                        apply_rotation: false,
                    }
                },
                recognize,
            };
            assert!(
                plan.validate(WordOutputMode::Off).is_ok(),
                "invalid combination: {detect}/{classify}/{recognize}"
            );
        }
    }

    #[test]
    fn polygon_rejects_non_finite_or_out_of_bounds_points() {
        let size = ImageSize {
            width: 100,
            height: 80,
        };
        assert!(
            Polygon {
                points: [[0.0, 0.0], [100.0, 0.0], [100.0, 80.0], [0.0, 80.0]]
            }
            .validate(size)
            .is_ok()
        );
        assert!(
            Polygon {
                points: [[f32::NAN, 0.0]; 4]
            }
            .validate(size)
            .is_err()
        );
        assert!(
            Polygon {
                points: [[101.0, 0.0]; 4]
            }
            .validate(size)
            .is_err()
        );
    }

    #[test]
    fn request_validation_rejects_invalid_policy_values() {
        let request = OcrRequest {
            input: ImageInput::Encoded(Arc::from([] as [u8; 0])),
            roi: None,
            scale_hint: None,
            stages: StagePlan::default(),
            preprocess: PreprocessPolicy {
                min_text_scale: Some(f32::NAN),
                ..PreprocessPolicy::default()
            },
            detection: DetectionPolicy {
                text_score: Some(1.5),
                ..DetectionPolicy::default()
            },
            recognition: RecognitionPolicy::default(),
            output: OutputPolicy::default(),
            formula: FormulaPolicy::default(),
            table: TablePolicy::default(),
        };
        assert!(request.validate().is_err());
    }

    /// 构造一个只改页面级阶段开关的合法请求（启用时补上必需模型路径）。
    fn page_request(formula_enabled: bool, table_enabled: bool) -> OcrRequest {
        let mut request = OcrRequest {
            input: ImageInput::Encoded(Arc::from([] as [u8; 0])),
            roi: None,
            scale_hint: None,
            stages: StagePlan::default(),
            preprocess: PreprocessPolicy::default(),
            detection: DetectionPolicy::default(),
            recognition: RecognitionPolicy::default(),
            output: OutputPolicy::default(),
            formula: FormulaPolicy::default(),
            table: TablePolicy::default(),
        };
        if formula_enabled {
            request.formula = FormulaPolicy {
                enabled: true,
                model_path: Some(PathBuf::from("model.onnx")),
                ..FormulaPolicy::default()
            };
        }
        if table_enabled {
            request.table = TablePolicy {
                enabled: true,
                model_path: Some(PathBuf::from("model.onnx")),
                ..TablePolicy::default()
            };
        }
        request
    }

    /// `PageRoute` 的四种组合：路由按启用位推导，关闭的阶段不进入计划。
    #[test]
    fn plan_derives_route_from_enabled_stages() {
        for (formula_enabled, table_enabled) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let planned = ValidatedRequest::plan(page_request(formula_enabled, table_enabled))
                .expect("a well-formed page request must plan");
            let route = planned.route();
            assert_eq!(route.runs_formula(), formula_enabled);
            assert_eq!(route.runs_table(), table_enabled);
            assert_eq!(route.formula().is_some(), formula_enabled);
            assert_eq!(route.table().is_some(), table_enabled);
        }
    }

    /// 未启用页面级阶段时，`roi`/`tile` 属于普通文本路径的合法能力，plan 必须放行。
    #[test]
    fn plan_allows_roi_and_tile_without_page_stages() {
        let mut request = page_request(false, false);
        request.roi = Some(RectU32 {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        ValidatedRequest::plan(request).expect("plain text may use roi");

        let mut request = page_request(false, false);
        request.preprocess.tile = Some(TilePolicy {
            max_width: 100,
            max_height: 100,
            overlap: 10,
        });
        ValidatedRequest::plan(request).expect("plain text may use tiled preprocessing");
    }

    /// 任一页面级阶段启用时，`roi`/`tile` 都被结构化拒绝；理由与阶段同处一地。
    /// 公式先于表格被校验，因此"两者都启用"命中公式的理由。
    #[test]
    fn plan_rejects_off_page_premise_for_each_enabled_stage() {
        // (formula, table, 期望命中的阶段名)
        for (formula, table, stage) in [
            (true, false, "formula"),
            (false, true, "table"),
            (true, true, "formula"),
        ] {
            let mut with_roi = page_request(formula, table);
            with_roi.roi = Some(RectU32 {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            });
            let message = ValidatedRequest::plan(with_roi)
                .expect_err("page stages forbid roi")
                .to_string();
            assert!(
                message.contains(stage) && message.contains("roi"),
                "stage={stage} premise=roi message={message}"
            );

            let mut with_tile = page_request(formula, table);
            with_tile.preprocess.tile = Some(TilePolicy {
                max_width: 100,
                max_height: 100,
                overlap: 10,
            });
            let message = ValidatedRequest::plan(with_tile)
                .expect_err("page stages forbid tiled")
                .to_string();
            assert!(
                message.contains(stage) && message.contains("tiled"),
                "stage={stage} premise=tiled message={message}"
            );
        }
    }

    #[test]
    fn gray_alpha_pixels_are_composited() {
        let pixels = OwnedPixelBuffer {
            width: 1,
            height: 1,
            stride: 2,
            format: PixelFormat::GrayAlpha8,
            bottom_up: false,
            data: Arc::from([0_u8, 0_u8]),
        };
        let (bgr, _, _) = pixels.to_bgr(None).expect("valid gray alpha");
        assert_eq!(bgr, vec![255, 255, 255]);
    }

    #[test]
    fn default_preprocess_policy_defers_max_side_to_engine_config() {
        let policy = PreprocessPolicy::default();
        assert_eq!(policy.max_side, None);
        assert_eq!(policy.max_encoded_bytes, 128 * 1024 * 1024);
    }

    fn reading_box(id: usize, x0: f32, y0: f32, x1: f32, y1: f32) -> ReadingBox {
        ReadingBox { id, x0, y0, x1, y1 }
    }

    #[test]
    fn reading_order_uses_columns_before_rows() {
        let boxes = vec![
            reading_box(0, 10.0, 10.0, 60.0, 30.0),
            reading_box(1, 10.0, 40.0, 60.0, 60.0),
            reading_box(2, 100.0, 10.0, 150.0, 30.0),
            reading_box(3, 100.0, 40.0, 150.0, 60.0),
        ];
        assert_eq!(order_reading_boxes(&boxes), vec![0, 1, 2, 3]);
    }

    #[test]
    fn reading_order_handles_full_width_heading_then_columns() {
        let boxes = vec![
            reading_box(0, 10.0, 0.0, 190.0, 20.0),
            reading_box(1, 10.0, 30.0, 80.0, 50.0),
            reading_box(2, 10.0, 60.0, 80.0, 80.0),
            reading_box(3, 110.0, 30.0, 190.0, 50.0),
            reading_box(4, 110.0, 60.0, 190.0, 80.0),
        ];
        assert_eq!(order_reading_boxes(&boxes), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn reading_order_handles_three_columns() {
        let boxes = vec![
            reading_box(0, 0.0, 10.0, 30.0, 30.0),
            reading_box(1, 0.0, 40.0, 30.0, 60.0),
            reading_box(2, 50.0, 10.0, 80.0, 30.0),
            reading_box(3, 50.0, 40.0, 80.0, 60.0),
            reading_box(4, 100.0, 10.0, 130.0, 30.0),
            reading_box(5, 100.0, 40.0, 130.0, 60.0),
        ];
        assert_eq!(order_reading_boxes(&boxes), vec![0, 1, 2, 3, 4, 5]);
    }

    #[derive(serde::Deserialize)]
    struct RealLayoutFixture {
        source: String,
        boxes: Vec<RealLayoutBox>,
        expected: Vec<usize>,
    }

    #[derive(serde::Deserialize)]
    struct RealLayoutBox {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    }

    #[test]
    fn reading_order_matches_real_10_columns_layout() {
        let fixture: RealLayoutFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/reading_order_10_columns.json"
        ))
        .expect("real layout fixture should parse");
        let boxes: Vec<ReadingBox> = fixture
            .boxes
            .iter()
            .enumerate()
            .map(|(id, b)| reading_box(id, b.x0, b.y0, b.x1, b.y1))
            .collect();
        assert_eq!(
            order_reading_boxes(&boxes),
            fixture.expected,
            "reading order mismatch for {}",
            fixture.source
        );
    }

    /// **阶段分解恒等式**：每个阶段的 `preprocess + infer + postprocess == *_ms`。
    ///
    /// `runtime::timing::TimingLedger` 把阶段分解当成“模型侧”的完整刻画，因此这条
    /// 恒等式必须成立——实现里若只改了其中一个字段（例如新增了一段不计入 `*_ms` 的
    /// 工作），账本就会静默漂移。
    ///
    /// **页面级不变量不在这里断言**：实测（12 张真实页面，`tests/baseline` 的账本）
    /// `total_ms` 与“页面级分项之和”之间存在系统性差额（默认配置下 debug 残差
    /// −43 到 −73 ms/页，均值 −54.8 ms；release 下约 −6.75 ms/页），因此账本把差额显式
    /// 报告为 `unattributed_ms` + `conservation.conserved = false`，而不是在这里假装它恒为 0。
    /// 这个差额的含义是**口径差异**：`total_ms` 是“外层 `preprocess_ms` 窗口
    /// （`rapid_structure.rs:619-719`）+ 内层 `inner.run()` 窗口（`rapid_structure.rs:106-130`）”的总窗口，
    /// 而命名分量只是这些窗口里的若干子窗口（内层窗口里的 resize / padding / crop / 批次装配
    /// 不属于任何阶段计时）——**两个窗口串行，不是重叠，也没有重复计时**。账本明确写着它是
    /// **诊断**工具、不是验收证据；详见 `src/runtime/timing.rs` 的模块文档、
    /// `LedgerConservation::interpretation` 与
    /// `real_world_outer_window_does_not_conserve_the_reported_total` 测试。
    #[test]
    fn ocr_timings_stage_breakdown_sums_to_each_stage_total() {
        let timings = OcrTimings {
            decode_ms: 4.0,
            resize_ms: 5.0,
            crop_ms: 1.0,
            preprocess_ms: 12.0,
            detector_preprocess_ms: 3.0,
            detector_infer_ms: 400.0,
            detector_postprocess_ms: 17.0,
            detect_ms: 420.0,
            classifier_preprocess_ms: 1.0,
            classifier_infer_ms: 2.0,
            classifier_postprocess_ms: 1.0,
            classify_ms: 4.0,
            recognizer_preprocess_ms: 4.0,
            recognizer_infer_ms: 200.0,
            recognizer_postprocess_ms: 30.0,
            recognize_ms: 234.0,
            formula_ms: 9.0,
            table_detect_ms: 0.0,
            table_structure_infer_ms: 0.0,
            table_backfill_ms: 0.0,
            postprocess_ms: 6.0,
            total_ms: 12.0 + 420.0 + 4.0 + 234.0 + 6.0 + 9.0,
        };
        for (name, parts, total) in [
            (
                "detector",
                [
                    timings.detector_preprocess_ms,
                    timings.detector_infer_ms,
                    timings.detector_postprocess_ms,
                ],
                timings.detect_ms,
            ),
            (
                "classifier",
                [
                    timings.classifier_preprocess_ms,
                    timings.classifier_infer_ms,
                    timings.classifier_postprocess_ms,
                ],
                timings.classify_ms,
            ),
            (
                "recognizer",
                [
                    timings.recognizer_preprocess_ms,
                    timings.recognizer_infer_ms,
                    timings.recognizer_postprocess_ms,
                ],
                timings.recognize_ms,
            ),
        ] {
            let sum: f32 = parts.iter().sum();
            assert!(
                (sum - total).abs() < 1e-3,
                "{name} stage breakdown ({sum}) must equal its stage total ({total})"
            );
        }

        // 输入侧口径（账本按它计算 `input_other_ms` 与 `input_overflow_ms`）：
        // 外层窗口单独立项，`decode_ms` / `resize_ms` 是 `inner.run()` 内的另一个口径，
        // 因此这里只断言外层窗口本身自洽，不断言三项之和一定放得下（见
        // `TimingLedger::input_overflow_ms`）。
        let outer = f64::from(timings.preprocess_ms);
        assert!(
            outer >= 0.0,
            "the outer preprocess window cannot be negative: {outer}"
        );
    }

    // ---------------------------------------------------------------------
    // 表格线契约（docs/11 §2.1 E14.1 / §2.2 E14.2）
    // ---------------------------------------------------------------------

    /// `enabled = false` 时策略必须**完全不校验**：这是"关闭即无影响"（门禁 G6）的
    /// 前提——默认策略里的 `model_path` 是 `None`，若关闭时也校验，每一个既有调用方
    /// 都会在构造默认请求时失败。
    #[test]
    fn a_disabled_table_policy_is_valid_even_without_a_model() {
        let policy = TablePolicy::default();
        assert!(!policy.enabled);
        assert!(policy.model_path.is_none());
        policy.validate().expect("a disabled policy must validate");
    }

    /// 五种拒绝各自要有**可区分**的消息，否则调用方只能看到"配置无效"而不知道该改哪一项。
    #[test]
    fn the_table_policy_rejections_are_distinguishable() {
        let model = PathBuf::from("slanet-plus.onnx");
        let base = TablePolicy {
            enabled: true,
            model_path: Some(model),
            ..TablePolicy::default()
        };
        base.validate().expect("the baseline must be valid");

        let cases: [(&str, TablePolicy, &str); 5] = [
            (
                "no model path",
                TablePolicy {
                    model_path: None,
                    ..base.clone()
                },
                "requires `model_path`",
            ),
            (
                "zero side length",
                TablePolicy {
                    input_side_len: 0,
                    ..base.clone()
                },
                "input_side_len must be greater than zero",
            ),
            (
                "non-finite score",
                TablePolicy {
                    min_region_score: f32::NAN,
                    ..base.clone()
                },
                "min_region_score must be finite and in [0,1]",
            ),
            (
                "score above one",
                TablePolicy {
                    min_region_score: 1.5,
                    ..base.clone()
                },
                "min_region_score must be finite and in [0,1]",
            ),
            (
                "zero max regions",
                TablePolicy {
                    max_regions: 0,
                    ..base.clone()
                },
                "max_regions must be greater than zero",
            ),
        ];
        for (name, policy, expected) in cases {
            let error = policy
                .validate()
                .expect_err(&format!("{name} must be refused"));
            assert!(
                error.to_string().contains(expected),
                "{name}: expected {expected:?}, got {error}"
            );
        }

        // `intra_threads` 是独立的第六条拒绝，单独列在这里以免上面那张表太长。
        let error = TablePolicy {
            intra_threads: Some(0),
            ..base
        }
        .validate()
        .expect_err("intra_threads = 0 must be refused");
        assert!(
            error
                .to_string()
                .contains("intra_threads must be greater than zero"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn the_table_wire_names_are_snake_case() {
        let json = serde_json::to_value(RegionKind::Table).expect("must serialize");
        assert_eq!(json, serde_json::json!("table"));
        let json = serde_json::to_value(TableStructureConfidence::Low).expect("must serialize");
        assert_eq!(json, serde_json::json!("low"));
        let json = serde_json::to_value(TableCellMatch::BlankBox).expect("must serialize");
        assert_eq!(json, serde_json::json!("blank_box"));
        let json =
            serde_json::to_value(TableDegradation::GridHoles { holes: 3 }).expect("must serialize");
        assert_eq!(json, serde_json::json!({"kind": "grid_holes", "holes": 3}));
        let json = serde_json::to_value(TableDegradation::Truncated).expect("must serialize");
        assert_eq!(json, serde_json::json!({"kind": "truncated"}));
        let json = serde_json::to_value(TableTextPolicy::Escape).expect("must serialize");
        assert_eq!(json, serde_json::json!("escape"));
    }

    /// `as_str()` 与 serde 的拼写**必须**一致：它是 `&'static str`，编译器不会替我们
    /// 检查两者是否同步。改一处忘另一处会让 JSON 的 `confidence` 与 `regions[].table.confidence`
    /// 说两套话，而两边都能通过各自的单元测试。
    #[test]
    fn the_confidence_name_agrees_with_serde() {
        for confidence in [
            TableStructureConfidence::High,
            TableStructureConfidence::Low,
        ] {
            let serde_name = serde_json::to_value(confidence).expect("confidence must serialize");
            assert_eq!(
                serde_name,
                serde_json::json!(confidence.as_str()),
                "as_str() and serde disagree for {confidence:?}"
            );
        }
        // 反向对照：两个变体不得给出同一个名字，否则上面的循环对拼写错误不敏感。
        assert_ne!(
            TableStructureConfidence::High.as_str(),
            TableStructureConfidence::Low.as_str()
        );
    }

    /// 策略是**可反序列化**的：服务端要能从请求体里读出来（E16.2 会用到）。
    #[test]
    fn the_table_policy_round_trips_through_json() {
        let policy = TablePolicy {
            enabled: true,
            model_path: Some(PathBuf::from("models/table/slanet-plus.onnx")),
            expected_model_sha256: Some("a".repeat(64)),
            input_side_len: 512,
            fix_input_shape: false,
            intra_threads: Some(4),
            min_region_score: 0.25,
            max_regions: 3,
            text_policy: TableTextPolicy::Raw,
        };
        let json = serde_json::to_string(&policy).expect("must serialize");
        let back: TablePolicy = serde_json::from_str(&json).expect("must deserialize");
        assert_eq!(back.input_side_len, 512);
        assert_eq!(back.intra_threads, Some(4));
        assert_eq!(back.max_regions, 3);
        assert_eq!(back.text_policy, TableTextPolicy::Raw);
        assert!(!back.fix_input_shape);
    }

    /// `OcrRegion.table` 为 `None` 时**不得**出现在 JSON 里：否则每一个文本区域的
    /// 输出都会多一个恒为 `null` 的字段，所有既有快照与体积统计都会变。
    #[test]
    fn an_absent_table_outcome_is_omitted_from_json() {
        let region = OcrRegion::text(
            RegionSource::Input,
            None,
            None,
            None,
            Some(RecognitionOutcome {
                text: "ok".into(),
                score: 0.5,
                words: None,
            }),
        );
        let json = serde_json::to_value(&region).expect("must serialize");
        assert!(
            json.get("table").is_none(),
            "an absent table outcome must not be serialized: {json}"
        );
        assert_eq!(json["kind"], serde_json::json!("text"));
    }
}
