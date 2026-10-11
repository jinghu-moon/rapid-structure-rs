//! 表格定位：从页面已有 det 文本框的**空间分布**推断表格区域
//! （`docs/09-table-recognition-execution.md` §12 = `docs/07` 的 R8，门禁 **G4**）。
//!
//! # 为什么不用模型
//!
//! 表格区域内的文本框天然呈现规整的行列结构：同一行的框中心高度接近、行高一致，
//! 相邻两行的框在水平方向上互相覆盖（同一列）。这是**已被 det 计算出来的**信息，
//! 再为它跑一个整页 CNN 是重复计费。G4 因此把"表格定位不新增整页 CNN"写成
//! **结构不变量**：本模块不持有任何会话字段、不读任何模型文件，
//! [`TextDistributionDetector::detect`] 只用入参的图像与文本框。
//!
//! 代价是这套启发式有明确的退化面：无线框表格（没有行列可对齐）、跨页表格
//! （分页后各自不完整）、嵌套表格（内层破坏外层列对齐）。这些退化是**未测的**
//! （`docs/07` §7.1 已登记），要在 E8 的表格测试集上量化而不是靠直觉；
//! 专用模型（`PP-LCNet_x1_0_table_cls`、`RT-DETR-L_*_table_cell_det`）因此
//! 降级为"实证不达标时的备选"，不预先引入。
//!
//! # 与 `docs/07` §7.1 接口草案的一处偏离（已登记）
//!
//! §7.1 写的是 `detect(&mut self, image: &RecImage, page_lines: &[LineResult])`，
//! 但 [`crate::ocr::types::LineResult`] 只有 `text` / `score` / `word_info`，
//! **没有任何几何字段** —— 按原文签名拿不到文本框坐标，定位无从谈起。
//! 真正带几何的是 `src/ocr/det/detector.rs:70` 的 `pub boxes: Vec<Quad>`，
//! 它与 `src/ocr/pipeline/types.rs:10-13` 的 `txts` / `scores` 同序并行。
//! 本模块因此定义自己的入参 [`PageLine`]（多边形 + 文本 + 分数），
//! 而**不**引入 `pub(crate)` 的 `ExecutionOutput`：表格域不该依赖 `ocr::pipeline`
//! 的内部类型。接线时由调用方负责把三者 zip 成 `Vec<PageLine>`。
//!
//! # 生产消费者
//!
//! E14 之前本模块只被自己的测试消费，用**模块级** `allow(dead_code)` 而不是整个表格域的
//! allow 是为了让另外 8 个模块继续接受 `-D warnings` 的死代码检查
//! （`src/table/mod.rs` 的模块级 allow 已在 E13 移除）。
//! **E14 已把本模块接进整页管线**（`src/ocr/pipeline/rapid_structure.rs` 的
//! `recognize_with_table`），因此那条 allow 已按原计划删除。

//!
//! # 判据（第一性）
//!
//! 一段文字"像表格"的不可再还原的证据只有一个：**相邻两行里存在多个互相
//! 水平覆盖的框**。一列竖排的框（段落、目录、清单）每对相邻行只有 1 个匹配，
//! 所以 [`TableDetectorConfig::min_columns`] 取 2 就把它们排除掉了 —— 这是
//! 本模块唯一真正的判别式，其余都是围绕它的稳健性处理（行聚类容差、
//! 行间距上限、行高一致性对分数的加权）。
//!
//! # 折行单元格不得打断行区间
//!
//! OCR 的文本框是**一行视觉文本**，**不是**一个单元格：单元格里的文字折行会在
//! 行列表里插进只含 1 个框的行，它们既当不了行区间的起点，也无法与相邻行共享
//! `min_columns` 个列 —— 于是**每一处折行都打断一次行区间**，一张表被切成"若干
//! 连续单行"的碎片。实测（用户上传的 982x694 生产记录表，12 个视觉行、其中 4 行
//! 折行）：行列表被撑到 19 行，只检出 2 个各 3 行的区域，其余 6 行退回文本区 ——
//! 页面上的表格"只识别了不到一半"。
//!
//! 修法是把这类**细行**（框数不足 `min_columns`）当作行之间的**填充**而不是行：
//! 延展区间时跨过它们（它们仍计入区域的 `members` 与包围盒），列匹配只在
//! **具备列结构的行**（锚行）之间做。判据本身、参数、分数公式一处未改 ——
//! 只是不再把"视觉行"当成"表格行"。
//!
//! 不是可选的稳健性打磨：**折行在真实表格里是常态**，所以它是本模块在生产里
//! 最容易触发的失效路径。已知的遗留：表格上方的**跨列标题行**（表题）与表头之间
//! 也没有列结构，因此它与表题在几何上不可区分，会被留在区域之外、作为普通文本
//! 区域输出（`DetectedRegion.rows` 也因此是真实行数的下界，权威网格由结构模型给出）。

use std::cmp::Ordering;

use crate::{
    Quad,
    config::RecImage,
    error::{RapidOcrError, Result},
};

/// 页面上一行已识别的文本：det 文本框 + 识别结果。
///
/// 与 [`crate::ocr::types::LineResult`] 的区别是**带几何**。三者同序并行的来源
/// 见模块文档。
#[derive(Debug, Clone, PartialEq)]
pub struct PageLine {
    /// det 文本框，四个角点，原图像素坐标。
    pub polygon: Quad,
    /// 该框的识别文本。定位本身不用它，保留是为了让调用方一次 zip 完。
    pub text: String,
    /// 该框的识别置信度。定位本身不用它。
    pub score: f32,
}

impl PageLine {
    /// 轴对齐包围盒。四角顺序任意，取分量极值。
    pub fn bounds(&self) -> Bounds {
        Bounds::of_polygon(&self.polygon)
    }
}

/// 轴对齐包围盒，原图像素坐标。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    /// 四角点包围盒。非有限分量按 `f32::max` / `f32::min` 的语义被忽略
    /// （`NaN` 不会传播成 `NaN` 边界）。
    pub fn of_polygon(polygon: &Quad) -> Self {
        let mut bounds = Self {
            left: f32::INFINITY,
            top: f32::INFINITY,
            right: f32::NEG_INFINITY,
            bottom: f32::NEG_INFINITY,
        };
        for point in polygon {
            bounds.left = bounds.left.min(point[0]);
            bounds.top = bounds.top.min(point[1]);
            bounds.right = bounds.right.max(point[0]);
            bounds.bottom = bounds.bottom.max(point[1]);
        }
        bounds
    }

    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    pub fn center_y(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }

    /// 两个盒子的最小外接盒。
    pub fn union(&self, other: &Bounds) -> Bounds {
        Bounds {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    /// 四边各向外推 `margin`（负值即收缩）。
    pub fn expand(&self, margin: f32) -> Bounds {
        Bounds {
            left: self.left - margin,
            top: self.top - margin,
            right: self.right + margin,
            bottom: self.bottom + margin,
        }
    }

    /// 水平方向的重叠长度；不相交时为 `0.0`。
    pub fn horizontal_overlap(&self, other: &Bounds) -> f32 {
        (self.right.min(other.right) - self.left.max(other.left)).max(0.0)
    }
}

/// 聚类后的一行：成员下标（按左边界升序）+ 该行的外接盒。
#[derive(Debug, Clone, PartialEq)]
pub struct LineRow {
    /// 指向入参切片的下标，按左边界升序。
    pub members: Vec<usize>,
    pub bounds: Bounds,
}

impl LineRow {
    /// 本行的列数（成员框个数）。合并单元格会让它小于表格的真实列数，
    /// 所以它是列数的**下界**。
    pub fn column_count(&self) -> usize {
        self.members.len()
    }
}

/// 定位参数。全部是**无量纲比例或计数**，没有一个是像素常量。
///
/// 之所以不设像素常量：同一套参数要同时服务 400 px 宽的裁剪表和 4000 px 宽的整页，
/// 任何绝对像素阈值都会在其中一侧失效。
#[derive(Debug, Clone, PartialEq)]
pub struct TableDetectorConfig {
    /// 构成一个表格区域所需的最少行数。
    pub min_rows: usize,
    /// 相邻两行之间构成"同一表格"所需的最少互相覆盖框数。
    ///
    /// **这是唯一的判别式**：取 2 就把竖排段落（每对相邻行只 1 个匹配）排除掉。
    pub min_columns: usize,
    /// 行聚类容差，单位是**全页文本框高度的中位数**。
    pub row_center_tolerance: f32,
    /// 两个框算作"同一列"所需的最小水平覆盖率，单位是二者宽度的较小值。
    pub column_overlap_ratio: f32,
    /// 相邻两行之间允许的最大垂直空隙，单位是**行高中位数**。
    pub max_row_gap: f32,
    /// 区域外扩边距，单位是区域长边。
    pub region_margin_ratio: f32,
}

impl Default for TableDetectorConfig {
    fn default() -> Self {
        Self {
            min_rows: 2,
            min_columns: 2,
            row_center_tolerance: 0.5,
            column_overlap_ratio: 0.5,
            max_row_gap: 1.5,
            region_margin_ratio: 0.02,
        }
    }
}

impl TableDetectorConfig {
    /// 拒绝会在运行期静默退化成"永远不成立"或"永远成立"的配置。
    pub fn validate(&self) -> Result<()> {
        if self.min_rows == 0 {
            return Err(RapidOcrError::Config(
                "table detection: min_rows must be at least 1".to_string(),
            ));
        }
        if self.min_columns < 2 {
            return Err(RapidOcrError::Config(format!(
                "table detection: min_columns must be at least 2, got {}; with fewer columns a \
                 vertical list of lines is indistinguishable from a table",
                self.min_columns
            )));
        }
        for (name, value) in [
            ("row_center_tolerance", self.row_center_tolerance),
            ("max_row_gap", self.max_row_gap),
            ("region_margin_ratio", self.region_margin_ratio),
        ] {
            if !(value.is_finite() && value >= 0.0) {
                return Err(RapidOcrError::Config(format!(
                    "table detection: {name} must be a finite non-negative number, got {value}"
                )));
            }
        }
        if !(self.column_overlap_ratio.is_finite()
            && self.column_overlap_ratio > 0.0
            && self.column_overlap_ratio <= 1.0)
        {
            return Err(RapidOcrError::Config(format!(
                "table detection: column_overlap_ratio must be in (0, 1], got {}",
                self.column_overlap_ratio
            )));
        }
        Ok(())
    }
}

/// 纯几何阶段检出的一个区域，**尚未裁图**。
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedRegion {
    /// 成员框的外接盒，未加边距。
    pub bounds: Bounds,
    /// 置信度，见 [`detect_regions`] 的定义。
    pub score: f32,
    /// **锚行数**：区间内具备列结构（框数 ≥ `min_columns`）的行数。折行造成的
    /// 细行计入 `bounds` 与 `members`，但不计入此数 —— 所以它是真实行数的**下界**，
    /// 权威的行列网格由结构模型给出（见 `TableOutcome`）。
    pub rows: usize,
    /// 各行列数的最大值（合并单元格使它可能高于真实列数）。
    pub columns: usize,
    /// 成员框下标，按行、再按左边界升序。
    pub members: Vec<usize>,
}

/// 检出并裁好的表格区域。字段与 `docs/07` §7.1 的草案一致。
#[derive(Debug, Clone)]
pub struct TableRegion {
    /// 原图坐标的轴对齐四边形（左上 → 右上 → 右下 → 左下），已夹到图像范围内。
    pub polygon: Quad,
    pub score: f32,
    /// 按 [`TableRegion::polygon`] 裁出的像素。
    pub crop: RecImage,
    /// `crop` 左上角在原图中的坐标（整数，因此与裁图逐像素对齐）。
    pub origin: [f32; 2],
}

/// 表格定位器。
pub trait TableDetector {
    /// 用**已经算好的**页面文本框定位表格区域，不引入新的模型推理。
    fn detect(&mut self, image: &RecImage, page_lines: &[PageLine]) -> Result<Vec<TableRegion>>;
}

/// 文本分布启发式定位器：无模型、无外部资产、无内部可变状态。
#[derive(Debug, Clone, Default)]
pub struct TextDistributionDetector {
    config: TableDetectorConfig,
}

impl TextDistributionDetector {
    pub fn new(config: TableDetectorConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }
}

impl TableDetector for TextDistributionDetector {
    fn detect(&mut self, image: &RecImage, page_lines: &[PageLine]) -> Result<Vec<TableRegion>> {
        let margin_for =
            |bounds: &Bounds| self.config.region_margin_ratio * bounds.width().max(bounds.height());
        let mut regions = Vec::new();
        for region in detect_regions(page_lines, &self.config) {
            let expanded = region.bounds.expand(margin_for(&region.bounds));
            let (left, top, right, bottom) =
                clamped_rect(&expanded, image.width(), image.height())?;
            let crop = crop_rect(image, left, top, right, bottom)?;
            regions.push(TableRegion {
                polygon: [
                    [left as f32, top as f32],
                    [right as f32, top as f32],
                    [right as f32, bottom as f32],
                    [left as f32, bottom as f32],
                ],
                score: region.score,
                crop,
                origin: [left as f32, top as f32],
            });
        }
        Ok(regions)
    }
}

/// 按行中心把文本框聚类成行，行内按左边界升序。
///
/// 容差取**全页文本框高度的中位数**乘 [`TableDetectorConfig::row_center_tolerance`]：
/// 中位数而非均值，因为整页里通常混着标题（很高）与脚注（很矮），
/// 均值会被它们拉偏。行中心用**运行均值**递推，避免同一行里成员一多就漂移。
pub fn group_rows(lines: &[PageLine], config: &TableDetectorConfig) -> Vec<LineRow> {
    if lines.is_empty() {
        return Vec::new();
    }
    let bounds: Vec<Bounds> = lines.iter().map(PageLine::bounds).collect();
    let heights: Vec<f32> = bounds.iter().map(Bounds::height).collect();
    let tolerance = config.row_center_tolerance * median_of(&heights);

    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|a, b| {
        bounds[*a]
            .center_y()
            .partial_cmp(&bounds[*b].center_y())
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.cmp(b))
    });

    let mut grouped: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = vec![order[0]];
    let mut center = bounds[order[0]].center_y();
    for &index in &order[1..] {
        let next_center = bounds[index].center_y();
        if (next_center - center).abs() <= tolerance {
            current.push(index);
            center = current
                .iter()
                .map(|member| bounds[*member].center_y())
                .sum::<f32>()
                / current.len() as f32;
        } else {
            grouped.push(std::mem::take(&mut current));
            current = vec![index];
            center = next_center;
        }
    }
    grouped.push(current);

    grouped
        .into_iter()
        .map(|mut members| {
            members.sort_by(|a, b| {
                bounds[*a]
                    .left
                    .partial_cmp(&bounds[*b].left)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.cmp(b))
            });
            let mut row_bounds = bounds[members[0]];
            for &member in &members[1..] {
                row_bounds = row_bounds.union(&bounds[member]);
            }
            LineRow {
                members,
                bounds: row_bounds,
            }
        })
        .collect()
}

/// 两行之间"互为同一列"的框对数（一对一匹配，不重复计数）。
///
/// 匹配是**贪心 + 最大覆盖率优先**：对 `a` 的每个成员挑一个尚未被占用、
/// 覆盖率最高且达到 [`TableDetectorConfig::column_overlap_ratio`] 的 `b` 成员。
/// 一对多不算匹配 —— 一个跨两列的单元格不会同时顶替两列。
pub fn shared_column_count(
    a: &LineRow,
    b: &LineRow,
    lines: &[PageLine],
    config: &TableDetectorConfig,
) -> usize {
    let mut used = vec![false; b.members.len()];
    let mut matched = 0;
    for &left_index in &a.members {
        let left = lines[left_index].bounds();
        let mut best: Option<(usize, f32)> = None;
        for (slot, &right_index) in b.members.iter().enumerate() {
            if used[slot] {
                continue;
            }
            let right = lines[right_index].bounds();
            let smaller = left.width().min(right.width()).max(f32::EPSILON);
            let ratio = left.horizontal_overlap(&right) / smaller;
            if ratio >= config.column_overlap_ratio
                && best.is_none_or(|(_, current)| ratio > current)
            {
                best = Some((slot, ratio));
            }
        }
        if let Some((slot, _)) = best {
            used[slot] = true;
            matched += 1;
        }
    }
    matched
}

/// 检出全部表格区域，按自上而下、互不重叠的行区间切分。
///
/// 分数 = **列对齐率 × 行高一致性**：
/// - 列对齐率 = `Σ 相邻行对的实际匹配数 / Σ min(上行框数, 下行框数)`，
///   即"每一对相邻行里，有多少比例的框找到了正下方的同列框"；
/// - 行高一致性 = `1 / (1 + 变异系数)`，`变异系数 = 行高标准差 / 行高均值`。
///   行高一致是 `docs/09` §12.1 明写的信号，用这个单调有界的形状把它乘进分数，
///   而不是当成硬门槛 —— 当成门槛会让"某一行是合并单元格"直接漏检。
///
/// 分数只表达"像表格的程度"，**不参与是否检出的判断**：检出由
/// `min_rows` / `min_columns` / `max_row_gap` 三个判据决定。
///
/// 折行造成的**细行**（框数不足 `min_columns`）不算新的一行：它们是上一个
/// 单元格的续行，延展行区间时被**跨过**（仍计入区域成员的包围盒与 `members`），
/// 列匹配只在**具备列结构的行**（下称锚行）之间做。否则折行会打断行区间，
/// 把一张表切成"连续单行"的碎片（见模块文档"折行单元格必须先归并"）。
pub fn detect_regions(lines: &[PageLine], config: &TableDetectorConfig) -> Vec<DetectedRegion> {
    let rows = group_rows(lines, config);
    if rows.len() < config.min_rows {
        return Vec::new();
    }
    let row_heights: Vec<f32> = rows.iter().map(|row| row.bounds.height()).collect();
    let max_gap = config.max_row_gap * median_of(&row_heights).max(f32::EPSILON);
    let is_anchor = |row: &LineRow| row.column_count() >= config.min_columns;

    let mut regions = Vec::new();
    let mut start = 0;
    while start < rows.len() {
        if !is_anchor(&rows[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        // 列匹配与 `rows`/分数都只认锚行；`end` 则覆盖整段区间（含中间的细行）。
        let mut anchor = start;
        let mut anchored = 1;
        let mut matched = 0;
        let mut possible = 0;
        while end + 1 < rows.len() {
            let next = &rows[end + 1];
            let gap = next.bounds.top - rows[end].bounds.bottom;
            if gap > max_gap {
                break;
            }
            if !is_anchor(next) {
                end += 1;
                continue;
            }
            let shared = shared_column_count(&rows[anchor], next, lines, config);
            if shared < config.min_columns {
                break;
            }
            matched += shared;
            possible += rows[anchor].column_count().min(next.column_count());
            end += 1;
            anchor = end;
            anchored += 1;
        }
        if anchored >= config.min_rows {
            let members = &rows[start..=end];
            let mut bounds = members[0].bounds;
            let mut columns = 0;
            let mut flat = Vec::new();
            for row in members {
                bounds = bounds.union(&row.bounds);
                columns = columns.max(row.column_count());
                flat.extend_from_slice(&row.members);
            }
            let alignment = if possible == 0 {
                0.0
            } else {
                matched as f32 / possible as f32
            };
            regions.push(DetectedRegion {
                bounds,
                score: alignment
                    * row_height_consistency(members.iter().filter(|row| is_anchor(row))),
                rows: anchored,
                columns,
                members: flat,
            });
        }
        start = end + 1;
    }
    regions
}

/// 行高一致性，落在 `(0, 1]`：全部行高相同为 `1.0`。
///
/// 只喂**锚行**：折行的细行比整行矮，把它们算进来会让"折行更多"的页面分数更低 ——
/// 那是把"更像表格"罚成"更不像表格"，方向错了。
fn row_height_consistency<'a>(rows: impl Iterator<Item = &'a LineRow>) -> f32 {
    let heights: Vec<f32> = rows.map(|row| row.bounds.height()).collect();
    if heights.is_empty() {
        return 0.0;
    }
    let mean = heights.iter().sum::<f32>() / heights.len() as f32;
    if !mean.is_finite() || mean <= 0.0 {
        return 0.0;
    }
    let variance =
        heights.iter().map(|h| (h - mean) * (h - mean)).sum::<f32>() / heights.len() as f32;
    1.0 / (1.0 + variance.sqrt() / mean)
}

/// 把浮点包围盒夹成图像范围内的整数矩形 `(left, top, right, bottom)`。
///
/// 外扩后的区域经常越界（表格贴着页边），夹取是**预期行为**；但夹完之后退化
/// （零宽或零高）说明这个区域根本不在图上，此时报错而不是返回一张空图。
pub fn clamped_rect(
    bounds: &Bounds,
    width: usize,
    height: usize,
) -> Result<(usize, usize, usize, usize)> {
    let max_x = width as f32;
    let max_y = height as f32;
    let left = bounds.left.clamp(0.0, max_x).floor() as usize;
    let top = bounds.top.clamp(0.0, max_y).floor() as usize;
    let right = bounds.right.clamp(0.0, max_x).ceil() as usize;
    let bottom = bounds.bottom.clamp(0.0, max_y).ceil() as usize;
    if right <= left || bottom <= top {
        return Err(RapidOcrError::InvalidImage(format!(
            "table region {:?} does not intersect the {width}x{height} image",
            bounds
        )));
    }
    Ok((left, top, right, bottom))
}

/// 按**已夹取**的整数矩形裁剪，保留源图的通道顺序。
///
/// **不用** `crop_text_regions`：那个函数做的是透视裁剪外加
/// `height / width >= 1.5` 时的 90° 旋转（识别器的行归一化），
/// 表格区域既不该被透视也不该被旋转 —— 旋转会让行变成列。
/// 浮点边界先经 [`clamped_rect`] 夹进图像再调用本函数。
pub fn crop_rect(
    image: &RecImage,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) -> Result<RecImage> {
    let width = image.width();
    let height = image.height();
    if right > width || bottom > height || right <= left || bottom <= top {
        return Err(RapidOcrError::InvalidImage(format!(
            "crop rect ({left}, {top})-({right}, {bottom}) is not inside the {width}x{height} image"
        )));
    }
    let crop_width = right - left;
    let crop_height = bottom - top;
    let row_bytes = crop_width * 3;
    let source = image.as_bytes();
    let mut data = vec![0_u8; row_bytes * crop_height];
    for y in top..bottom {
        let source_start = (y * width + left) * 3;
        let target_start = (y - top) * row_bytes;
        data[target_start..target_start + row_bytes]
            .copy_from_slice(&source[source_start..source_start + row_bytes]);
    }
    RecImage::new(crop_width, crop_height, data, image.color_order())
}

/// 中位数；非有限值被丢弃，全空时返回 `0.0`。
fn median_of(values: &[f32]) -> f32 {
    let mut sorted: Vec<f32> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return 0.0;
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Bounds, PageLine, TableDetector, TableDetectorConfig, TextDistributionDetector,
        clamped_rect, detect_regions, group_rows, shared_column_count,
    };
    use crate::config::RecImage;

    fn line(left: f32, top: f32, right: f32, bottom: f32) -> PageLine {
        PageLine {
            polygon: [[left, top], [right, top], [right, bottom], [left, bottom]],
            text: "cell".to_string(),
            score: 0.9,
        }
    }

    /// 规整网格：`rows` 行 × `cols` 列，单元格 `cell_w × cell_h`，列间距 `gap_x`，行间距 `gap_y`。
    fn grid(
        rows: usize,
        cols: usize,
        cell_w: f32,
        cell_h: f32,
        gap_x: f32,
        gap_y: f32,
    ) -> Vec<PageLine> {
        let mut lines = Vec::new();
        for row in 0..rows {
            for col in 0..cols {
                let left = col as f32 * (cell_w + gap_x);
                let top = row as f32 * (cell_h + gap_y);
                lines.push(line(left, top, left + cell_w, top + cell_h));
            }
        }
        lines
    }

    /// 由文本框外接盒构造一张可分辨像素的图，用于验证裁剪。
    fn canvas(width: usize, height: usize) -> RecImage {
        let mut data = vec![0_u8; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) * 3;
                data[index] = (x % 256) as u8;
                data[index + 1] = (y % 256) as u8;
                data[index + 2] = ((x + y) % 256) as u8;
            }
        }
        RecImage::from_bgr_u8(width, height, data).expect("canvas must be constructible")
    }

    #[test]
    fn a_three_by_three_grid_is_one_region_with_a_perfect_score() {
        let lines = grid(3, 3, 100.0, 20.0, 10.0, 5.0);
        let regions = detect_regions(&lines, &TableDetectorConfig::default());
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].rows, 3);
        assert_eq!(regions[0].columns, 3);
        assert_eq!(regions[0].members.len(), 9);
        assert!(
            (regions[0].score - 1.0).abs() < 1e-6,
            "a perfect grid must score 1.0, got {}",
            regions[0].score
        );
        assert!((regions[0].bounds.left - 0.0).abs() < 1e-6);
        assert!((regions[0].bounds.right - 320.0).abs() < 1e-6);
        assert!((regions[0].bounds.bottom - 70.0).abs() < 1e-6);
    }

    /// **本模块唯一的判别式**：一列竖排的框不是表格。
    #[test]
    fn a_vertical_column_of_lines_is_not_a_table() {
        let lines: Vec<PageLine> = (0..6)
            .map(|i| line(0.0, i as f32 * 30.0, 200.0, i as f32 * 30.0 + 20.0))
            .collect();
        assert_eq!(group_rows(&lines, &TableDetectorConfig::default()).len(), 6);
        assert!(detect_regions(&lines, &TableDetectorConfig::default()).is_empty());
    }

    /// 段落：每行两个框，但相邻两行的框在水平方向上完全错开 —— 有行，没有列。
    #[test]
    fn a_paragraph_whose_lines_do_not_share_columns_is_not_a_table() {
        let mut lines = Vec::new();
        for i in 0..5 {
            let top = i as f32 * 30.0;
            let left = i as f32 * 250.0;
            lines.push(line(left, top, left + 80.0, top + 20.0));
            lines.push(line(left + 120.0, top, left + 200.0, top + 20.0));
        }
        let config = TableDetectorConfig::default();
        // 行聚类仍然成立（每行的两个框中心高度相同）。
        assert_eq!(group_rows(&lines, &config).len(), 5);
        // 但相邻两行共享 0 列，所以一个区域都不该检出。
        assert!(detect_regions(&lines, &config).is_empty());
    }

    #[test]
    fn two_tables_far_apart_are_reported_separately() {
        let mut lines = grid(3, 3, 100.0, 20.0, 10.0, 5.0);
        let offset = 5000.0;
        for extra in grid(3, 3, 100.0, 20.0, 10.0, 5.0) {
            let mut shifted = extra;
            for point in &mut shifted.polygon {
                point[1] += offset;
            }
            lines.push(shifted);
        }
        let regions = detect_regions(&lines, &TableDetectorConfig::default());
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].members.len(), 9);
        assert_eq!(regions[1].members.len(), 9);
        assert!(regions[1].bounds.top >= offset);
    }

    #[test]
    fn a_lone_row_is_not_a_table() {
        let lines = grid(1, 4, 100.0, 20.0, 10.0, 5.0);
        assert!(detect_regions(&lines, &TableDetectorConfig::default()).is_empty());
    }

    #[test]
    fn row_grouping_absorbs_small_vertical_jitter() {
        // 三个框的中心分别偏离 2 / 0 / -2，远小于行高 20 的一半。
        let lines = vec![
            line(0.0, 0.0, 50.0, 20.0),
            line(60.0, 2.0, 110.0, 22.0),
            line(120.0, -2.0, 170.0, 18.0),
        ];
        let rows = group_rows(&lines, &TableDetectorConfig::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].members, vec![0, 1, 2]);
    }

    #[test]
    fn a_wide_gap_splits_two_regions_even_when_columns_align() {
        let mut lines = grid(2, 3, 100.0, 20.0, 10.0, 5.0);
        // 后两行整体下移 5000：列完全对齐，但空隙远超行高中位数。
        for extra in grid(2, 3, 100.0, 20.0, 10.0, 5.0) {
            let mut shifted = extra;
            for point in &mut shifted.polygon {
                point[1] += 5000.0;
            }
            lines.push(shifted);
        }
        let regions = detect_regions(&lines, &TableDetectorConfig::default());
        assert_eq!(regions.len(), 2);
    }

    /// 匹配是**双向一对一**的：跨两列的单元格既不能顶替两个列，也不能被两个列顶替。
    #[test]
    fn shared_column_count_refuses_one_to_many_matches() {
        let config = TableDetectorConfig::default();
        // 上行 1 个跨两列的框，下行 2 个框。
        let lines = vec![
            line(0.0, 0.0, 210.0, 20.0),
            line(0.0, 25.0, 100.0, 45.0),
            line(110.0, 25.0, 210.0, 45.0),
        ];
        let rows = group_rows(&lines, &config);
        assert_eq!(rows.len(), 2);
        // 无论哪个方向，都只能配成 1 对：宽框占掉一侧的唯一名额，
        // 窄框各自占掉另一侧的一个名额。
        assert_eq!(shared_column_count(&rows[0], &rows[1], &lines, &config), 1);
        assert_eq!(shared_column_count(&rows[1], &rows[0], &lines, &config), 1);
    }

    /// 一对一的后果：单行合并单元格会让列数被低估，因此 `min_columns = 2`
    /// 对"两行、每行只有一个跨列框"这种表头 + 单列布局会漏检 —— 这是有意的，
    /// 因为"1 列"与竖排段落无法区分。
    #[test]
    fn a_two_row_single_column_layout_is_refused_by_design() {
        let lines = vec![line(0.0, 0.0, 200.0, 20.0), line(0.0, 25.0, 200.0, 45.0)];
        assert!(detect_regions(&lines, &TableDetectorConfig::default()).is_empty());
    }

    #[test]
    fn the_score_falls_when_one_row_pair_loses_a_column() {
        let mut lines = grid(3, 3, 100.0, 20.0, 10.0, 5.0);
        // 第三行最右的框右移 400：与第二行不再同列，但第三行仍与第二行有 2 个匹配。
        let last = lines.len() - 1;
        for point in &mut lines[last].polygon {
            point[0] += 400.0;
        }
        let regions = detect_regions(&lines, &TableDetectorConfig::default());
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].rows, 3);
        assert!(
            regions[0].score < 1.0 && regions[0].score > 0.5,
            "expected a partial score, got {}",
            regions[0].score
        );
    }

    /// 单元格折行**不得**把表格切成碎片：折行产生的细行是填充，不是新的一行。
    #[test]
    fn a_wrapped_cell_does_not_split_the_region() {
        // 2 列 3 行；第 2 行右列的文字折成两行，且折行与左列**不同 y**（真实情形）。
        let lines = vec![
            line(0.0, 0.0, 100.0, 20.0),
            line(200.0, 0.0, 300.0, 20.0),
            line(0.0, 30.0, 100.0, 50.0),
            line(200.0, 28.0, 300.0, 40.0),
            line(200.0, 42.0, 300.0, 54.0),
            line(0.0, 60.0, 100.0, 80.0),
            line(200.0, 60.0, 300.0, 80.0),
        ];
        let regions = detect_regions(&lines, &TableDetectorConfig::default());
        assert_eq!(regions.len(), 1, "折行不得切碎表格，得到 {regions:?}");
        let region = &regions[0];
        assert_eq!(region.columns, 2);
        assert_eq!(
            region.rows, 3,
            "折行的两个细行只算填充，锚行仍是 3 个（真实行数的下界）"
        );
        assert_eq!(region.members.len(), 7, "7 个文本框一个都不能丢");
        assert!(
            region.score > 0.95,
            "折行只该让行高一致性略降（折了一行的并集高 2 px），列对齐率必须仍是 1.0，\
             得到 {}",
            region.score
        );
    }

    /// 用户实测的生产记录表（982x694、12 个视觉行、2 列、22 格，其中 4 行的
    /// 第 2 列折成两行）：**修复前** 27 个文本框被聚成 19 行，只有"连续单行"的
    /// 片段存活 → 只检出 2 个各 3 行的区域，其余 6 行退回文本区（页面上的表格
    /// "只识别了不到一半"）。坐标是本机 `/run` 输出的原文，四舍五入到 0.1 px。
    ///
    /// 表题（第 1 个文本框，跨两列）留在区域之外：它与表格的列结构无关，几何上
    /// 与"表格上方的表题"不可区分 —— 这是本模块的已知边界，不是回归。
    #[test]
    fn a_production_log_whose_cells_wrap_is_one_region() {
        let lines = vec![
            line(369.0, 13.0, 590.0, 38.0),   // 0: 6月14日生产情况回顾：
            line(50.0, 47.0, 129.0, 80.0),    // 1: 时间段
            line(523.0, 49.0, 617.0, 78.0),   // 2: 生产情况
            line(179.0, 83.0, 948.0, 107.0),  // 3: 小圈调试、启动所有机床，…
            line(5.0, 96.0, 134.0, 124.0),    // 4: 8：00~9：00
            line(228.0, 111.0, 912.0, 136.0), // 5: 重新对刀、对机器手位置。…
            line(4.0, 155.0, 145.0, 185.0),   // 6: 9：00~10：00
            line(251.0, 158.0, 888.0, 180.0), // 7: 小圈正常出料到检验，大圈…
            line(7.0, 218.0, 154.0, 243.0),   // 8: 10：00~11：00
            line(206.0, 217.0, 933.0, 241.0), // 9: 小圈正常出料到检验，大圈…
            line(8.0, 278.0, 153.0, 301.0),   // 10: 11：00~12：00
            line(239.0, 277.0, 900.0, 302.0), // 11: 中午吃饭，大圈预停，小圈…
            line(185.0, 324.0, 955.0, 346.0), // 12: 申博康第二批吃饭，汪东升…
            line(7.0, 338.0, 154.0, 363.0),   // 13: 12：00~13：00
            line(469.0, 351.0, 670.0, 375.0), // 14: 磨冷却液小，打维修
            line(182.0, 383.0, 956.0, 407.0), // 15: 大圈正常出料，由于大超粗…
            line(7.0, 398.0, 154.0, 423.0),   // 16: 13：00~14：00
            line(553.0, 410.0, 584.0, 438.0), // 17: 序
            line(178.0, 443.0, 949.0, 468.0), // 18: 14:30内径精磨维修完…
            line(6.0, 456.0, 155.0, 485.0),   // 19: 14：00~15：00
            line(447.0, 471.0, 692.0, 496.0), // 20: 大小圈均正常出料至检验
            line(7.0, 518.0, 154.0, 543.0),   // 21: 15：00~16：00
            line(326.0, 518.0, 811.0, 542.0), // 22: 大小圈正常出料至检验，检…
            line(7.0, 578.0, 154.0, 603.0),   // 23: 16：00~17：00
            line(272.0, 577.0, 867.0, 602.0), // 24: 16:00小圈开始清料，…
            line(60.0, 631.0, 119.0, 670.0),  // 25: 总结
            line(425.0, 637.0, 714.0, 662.0), // 26: 大圈出料 850，小圈出…
        ];
        let regions = detect_regions(&lines, &TableDetectorConfig::default());
        assert_eq!(regions.len(), 1, "整张表必须是一个区域，得到 {regions:?}");
        let region = &regions[0];
        assert_eq!(region.columns, 2);
        assert_eq!(
            region.rows, 8,
            "8 个锚行；整行都折行的 3 行并入相邻区间、不计入（真实行数的下界）"
        );
        for (label, actual, expected) in [
            ("top", region.bounds.top, 47.0),
            ("bottom", region.bounds.bottom, 670.0),
            ("left", region.bounds.left, 4.0),
            ("right", region.bounds.right, 956.0),
        ] {
            assert!(
                (actual - expected).abs() < 2.0,
                "区域 {label} 应为 {expected} 左右，得到 {actual}"
            );
        }
        assert_eq!(
            region.members.len(),
            26,
            "除表题外的 26 个文本框都必须进入区域，否则回填拿不到它们的文本"
        );
        assert!(
            !region.members.contains(&0),
            "表题在区域之外：它与表格的列结构无关"
        );
    }

    #[test]
    fn inconsistent_row_heights_lower_the_score() {
        let even = grid(3, 3, 100.0, 20.0, 10.0, 5.0);
        let even_score = detect_regions(&even, &TableDetectorConfig::default())[0].score;

        let mut ragged = grid(3, 3, 100.0, 20.0, 10.0, 5.0);
        // 把第一行整体加高 60：列仍对齐（左边界与右边界不变），但行高不一致。
        for entry in ragged.iter_mut().take(3) {
            entry.polygon[3][1] += 60.0;
            entry.polygon[2][1] += 60.0;
        }
        let ragged_score = detect_regions(&ragged, &TableDetectorConfig::default())[0].score;
        assert!(
            ragged_score < even_score,
            "ragged row heights ({ragged_score}) must score below even ones ({even_score})"
        );
    }

    #[test]
    fn the_detector_crops_the_region_and_keeps_the_source_pixels() {
        let lines = grid(2, 2, 20.0, 10.0, 5.0, 5.0);
        let image = canvas(200, 100);
        let mut detector = TextDistributionDetector::default();
        let regions = detector
            .detect(&image, &lines)
            .expect("detection must succeed");
        assert_eq!(regions.len(), 1);
        let region = &regions[0];
        assert!(
            (region.score - 1.0).abs() < 1e-6,
            "a perfect 2x2 grid must score 1.0, got {}",
            region.score
        );
        // 网格外接盒是 (0,0)-(45,25)，外扩 2 % × 长边（45）= 0.9 px，
        // 于是 (-0.9, -0.9)-(45.9, 25.9)，夹到图上再 floor/ceil → (0,0)-(46,26)。
        assert!((region.origin[0] - 0.0).abs() < 1e-6);
        assert!((region.origin[1] - 0.0).abs() < 1e-6);
        assert_eq!(region.crop.width(), 46);
        assert_eq!(region.crop.height(), 26);
        // 裁剪必须逐像素等于源图对应位置。
        let source = image.as_bytes();
        let crop = region.crop.as_bytes();
        for y in 0..region.crop.height() {
            let source_y = y + region.origin[1] as usize;
            let source_start = (source_y * 200 + region.origin[0] as usize) * 3;
            let crop_start = y * region.crop.width() * 3;
            assert_eq!(
                &crop[crop_start..crop_start + region.crop.width() * 3],
                &source[source_start..source_start + region.crop.width() * 3],
                "crop row {y} must match the source"
            );
        }
    }

    #[test]
    fn the_polygon_is_axis_aligned_and_agrees_with_the_origin() {
        let lines = grid(2, 2, 20.0, 10.0, 5.0, 5.0);
        let image = canvas(200, 100);
        let mut detector = TextDistributionDetector::default();
        let region = &detector
            .detect(&image, &lines)
            .expect("detection must succeed")[0];
        assert_eq!(region.polygon[0], region.origin);
        assert_eq!(region.polygon[1][1], region.polygon[0][1]);
        assert_eq!(region.polygon[2][0], region.polygon[1][0]);
        assert_eq!(region.polygon[3][1], region.polygon[2][1]);
        assert_eq!(region.polygon[3][0], region.polygon[0][0]);
        assert_eq!(
            region.polygon[2][0] - region.polygon[0][0],
            region.crop.width() as f32
        );
        assert_eq!(
            region.polygon[2][1] - region.polygon[0][1],
            region.crop.height() as f32
        );
    }

    #[test]
    fn a_region_outside_the_image_is_refused_rather_than_cropped_to_nothing() {
        let outside = Bounds {
            left: 500.0,
            top: 500.0,
            right: 600.0,
            bottom: 600.0,
        };
        let error = clamped_rect(&outside, 100, 100).expect_err("must be refused");
        assert!(
            error.to_string().contains("does not intersect"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_region_that_only_touches_the_image_is_refused() {
        // 零宽度：left == right。
        let degenerate = Bounds {
            left: 50.0,
            top: 10.0,
            right: 50.0,
            bottom: 90.0,
        };
        assert!(clamped_rect(&degenerate, 100, 100).is_err());
    }

    #[test]
    fn the_detector_needs_no_model_and_no_external_asset() {
        // G4 的行为面：默认构造 + 纯内存图像就能出结果，没有任何路径参数。
        let lines = grid(2, 2, 20.0, 10.0, 5.0, 5.0);
        let image = canvas(64, 64);
        let mut detector = TextDistributionDetector::default();
        let regions = detector.detect(&image, &lines).expect("must succeed");
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn an_empty_page_yields_no_region() {
        let image = canvas(64, 64);
        let mut detector = TextDistributionDetector::default();
        let regions = detector.detect(&image, &[]).expect("must succeed");
        assert!(regions.is_empty());
    }

    /// G4 的静态面：本模块的源码里不得出现任何推理会话或模型文件的痕迹。
    ///
    /// 探针字符串**由碎片拼出**，否则 `include_str!` 会把本测试自己的字面量
    /// 也读进来，断言恒假。
    /// ORT 的路径前缀（由碎片拼出，避免本注释自己被扫到），但 `test_support::` 里也含
    /// 同样的序列（`supp` + 前缀），所以必须检查前一个字符不是标识符字符，否则这条断言
    /// 会误报。
    fn mentions_ort_path(source: &str) -> bool {
        let needle = ["ort", "::"].concat();
        let bytes = source.as_bytes();
        let mut from = 0;
        while let Some(offset) = source[from..].find(&needle) {
            let index = from + offset;
            let boundary = index == 0
                || (!bytes[index - 1].is_ascii_alphanumeric() && bytes[index - 1] != b'_');
            if boundary {
                return true;
            }
            from = index + needle.len();
        }
        false
    }

    #[test]
    fn the_module_constructs_no_inference_session() {
        let source = include_str!("detection.rs");
        let needles = [
            ["Ort", "Session"].concat(),
            ["Rapid", "OcrEngine"].concat(),
            ["Table", "StructureSession"].concat(),
            [".on", "nx"].concat(),
            ["with_dimension", "Override"].concat(),
        ];
        for needle in &needles {
            assert!(
                !source.contains(needle.as_str()),
                "src/table/detection.rs must not mention `{needle}`: table detection is \
                 heuristic-only (gate G4)"
            );
        }
        assert!(
            !mentions_ort_path(source),
            "src/table/detection.rs must not call into ORT: table detection is heuristic-only \
             (gate G4)"
        );
    }

    #[test]
    fn the_config_refuses_degenerate_values() {
        let cases = [
            TableDetectorConfig {
                min_columns: 1,
                ..TableDetectorConfig::default()
            },
            TableDetectorConfig {
                min_rows: 0,
                ..TableDetectorConfig::default()
            },
            TableDetectorConfig {
                column_overlap_ratio: 0.0,
                ..TableDetectorConfig::default()
            },
            TableDetectorConfig {
                column_overlap_ratio: 1.5,
                ..TableDetectorConfig::default()
            },
            TableDetectorConfig {
                max_row_gap: f32::NAN,
                ..TableDetectorConfig::default()
            },
        ];
        for config in cases {
            assert!(
                config.validate().is_err(),
                "this configuration must be refused: {config:?}"
            );
        }
        assert!(TextDistributionDetector::new(TableDetectorConfig::default()).is_ok());
    }

    #[test]
    fn the_default_config_is_accepted_by_the_constructor() {
        assert!(TextDistributionDetector::new(TableDetectorConfig::default()).is_ok());
    }

    #[test]
    fn the_constructor_stores_the_configuration_it_was_given() {
        // 用**行为差异**而不是访问器来证明配置真的到达了算法：
        // 要求 3 列时，一个 2 列网格必须被拒；放宽回 2 列又必须被接受。
        let strict = TextDistributionDetector::new(TableDetectorConfig {
            min_columns: 3,
            ..TableDetectorConfig::default()
        })
        .expect("min_columns = 3 is a valid configuration");
        let image = canvas(64, 64);
        let two_columns = grid(3, 2, 20.0, 10.0, 5.0, 5.0);
        assert!(
            strict
                .clone()
                .detect(&image, &two_columns)
                .expect("detection must not fail")
                .is_empty(),
            "a two-column grid must not satisfy min_columns = 3"
        );
        assert_eq!(
            TextDistributionDetector::default()
                .detect(&image, &two_columns)
                .expect("detection must not fail")
                .len(),
            1,
            "the default min_columns = 2 accepts the same grid"
        );
    }
}

/// 真实页面上的定位验收（`docs/09-table-recognition-execution.md` §12.2 第二条）。
///
/// 三张图都不在仓库内（`datasets/` 与 `refer/` 均 **git 未跟踪**），因此整组测试
/// 在资产缺失时优雅跳过 —— 与 `src/table/structure.rs::real_model_tests` 同一条规矩。
///
/// # 判据是 IoU 匹配后的召回率与精确率，不是像素级重合
///
/// 期望值只写"表格大致覆盖哪一块"（一个轴对齐矩形），判定用 `IoU >= 0.5`。
/// 理由：这张图里表格的**真实边界**取决于 det 找到的最外侧文本框，而文本框带
/// 行距留白，逐像素比对会把一个正确的定位判成错的。
#[cfg(test)]
mod real_page_tests {
    use std::path::Path;

    use super::{
        Bounds, PageLine, TableDetector, TableDetectorConfig, TableRegion, TextDistributionDetector,
    };
    use crate::{
        config::{RecImage, RuntimeConfig},
        ocr::det::detector::Detector,
        ocr::pipeline::{
            config::EngineConfig,
            image_ops::{
                PreprocessRecord, apply_vertical_padding, map_boxes_to_original,
                resize_image_within_bounds,
            },
        },
    };

    // ------------------------------------------------------------------
    // 真实页面验收
    //
    // 三张图分别代表三种情形，判据刻意不同，因为"从文本框分布推断"这一原理在
    // 三者上的可用性本来就不同：
    //
    // 1. `table.jpg`            文本密集的单表格 → 强项，用几何 IoU 判。
    // 2. `table_without_txt.jpg` 只有网格线、**一个文字都没有** → 原理性不适用，
    //    期望 0 区域，且这不算失败。
    // 3. `04表格与键值对.png`    多块并排版式 → 已知假合并，用结构判据 + 显式断言
    //    该缺陷仍然存在（修好了必须大声失败）。
    //
    // 几何判据用 IoU ≥ 0.5 而不是逐像素比对：表格区域本身含大量留白，逐像素
    // 比对会把一个正确的定位判成错的。
    // ------------------------------------------------------------------

    /// 人工粗标的表格区域与 IoU 阈值，两者一起用。
    const REGION_IOU: f32 = 0.5;

    /// `table.jpg`：371×293，15 行 × 5 列的密集表格，铺满整张图。
    const DENSE_TABLE_FIXTURE: &str = "table.jpg";

    /// `table_without_txt.jpg`：896×500，只有网格线、**一个文字都没有**。
    const BORDER_ONLY_TABLE_FIXTURE: &str = "table_without_txt.jpg";

    /// `datasets/text/04表格与键值对.png`：3200×2000，含三块结构
    /// （顶部 5×5 统计表、左下键值对、右下带合并单元格的 3 列表）。
    const KEY_VALUE_PAGE_FIXTURE: &str = "04表格与键值对.png";

    /// 顶部统计表的真实外框，**由页面自己的网格线实测**（不依赖 det）：
    /// 在 y ∈ [328, 648] 的横带里，灰度 < 230 且占该带 80 % 以上高度的列恰好有 6 条
    /// —— `160/161, 794/795, 1196/1197, 1714/1715, 2232/2233, 3038/3039`
    /// —— 即 5 列；横向网格线占该带 y ∈ [328, 647]。
    ///
    /// 注意它比 det 检出的区域宽得多（检出右界约 2480）：**det 框紧贴文字而不是单元格**，
    /// 最右列的内容（"置信度 > 0.90"等）左对齐且很短，所以框不延伸到表格右边框。
    /// 这正是下面那条"假合并"的同一个根因。
    const KEY_VALUE_TOP_TABLE: [f32; 4] = [160.0, 328.0, 3039.0, 647.0];

    fn engine_config() -> Option<EngineConfig> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("models")
            .join("configs")
            .join("test-config-small.yaml");
        let config = path.is_file().then(|| EngineConfig::from_yaml_file(&path));
        let config = match config {
            Some(Ok(config)) => config,
            Some(Err(error)) => panic!("{} must parse: {error}", path.display()),
            None => {
                crate::test_support::asset(
                    "table detection engine config (models/configs/test-config-small.yaml)",
                    None,
                );
                return None;
            }
        };
        match config.det.model_path.as_ref() {
            Some(path) if path.is_file() => {}
            _ => {
                crate::test_support::asset("page detector onnx", None);
                return None;
            }
        }
        Some(config)
    }

    /// 跑一次**只有 det** 的页面前向，把文本框映射回原图坐标并包成 [`PageLine`]。
    ///
    /// 这里刻意复刻 `src/ocr/pipeline/rapid_structure.rs:150-219` 与 `:280-286` 的
    /// 前置缩放 + 纵向补白 + 映射回原图三步，否则文本框坐标与传给定位器的原图
    /// 不在同一坐标系，IoU 判定会全错。
    fn page_lines(config: &EngineConfig, image: &RecImage) -> Vec<PageLine> {
        let (proc_img, ratio_h, ratio_w) = resize_image_within_bounds(
            image.clone(),
            config.global.min_side_len,
            config.global.max_side_len,
        )
        .expect("page resize must succeed");
        let (padded, pad_top) = apply_vertical_padding(
            proc_img,
            config.global.width_height_ratio,
            config.global.min_height,
        )
        .expect("vertical padding must succeed");
        let mut detector =
            Detector::new(config.det.clone(), &RuntimeConfig::default()).expect("detector opens");
        let output = detector.detect(&padded).expect("det must run");
        let mut boxes = output.boxes;
        map_boxes_to_original(
            &mut boxes,
            PreprocessRecord {
                ratio_h,
                ratio_w,
                pad_top,
                pad_left: 0,
            },
            image.height(),
            image.width(),
        );
        boxes
            .into_iter()
            .map(|polygon| PageLine {
                polygon,
                text: String::new(),
                score: 1.0,
            })
            .collect()
    }

    fn rect_of(bounds: &Bounds) -> [f32; 4] {
        [bounds.left, bounds.top, bounds.right, bounds.bottom]
    }

    fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
        let width = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
        let height = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
        let intersection = width * height;
        let area_a = (a[2] - a[0]).max(0.0) * (a[3] - a[1]).max(0.0);
        let area_b = (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0);
        let union = area_a + area_b - intersection;
        if union <= 0.0 {
            0.0
        } else {
            intersection / union
        }
    }

    /// 跑一张图的完整链路：det → 文本框 → 定位器。
    ///
    /// 返回 `(检出区域, 文本框数)`；资产缺失时返回 `None`（**skip，不 panic**）。
    fn run(fixture: &str) -> Option<(Vec<TableRegion>, Vec<PageLine>)> {
        let config = engine_config()?;
        let path = crate::test_support::table_detection_fixture(fixture)?;
        let image = RecImage::from_path(&path).expect("fixture must decode");
        let lines = page_lines(&config, &image);
        let mut detector =
            TextDistributionDetector::new(TableDetectorConfig::default()).expect("valid config");
        let regions = detector
            .detect(&image, &lines)
            .expect("detection must succeed");
        if std::env::var_os("RAPID_STRUCTURE_TABLE_DETECTION_DEBUG").is_some() {
            eprintln!(
                "=== {fixture} ({}x{}) lines={} regions={}",
                image.width(),
                image.height(),
                lines.len(),
                regions.len()
            );
            for (index, region) in regions.iter().enumerate() {
                eprintln!(
                    "  #{index} rect={:?} score={:.4}",
                    rect_of(&Bounds::of_polygon(&region.polygon)),
                    region.score
                );
            }
        }
        Some((regions, lines))
    }

    /// 打印每个区域的列带与带间空隙，用于把"假合并"的机制量化
    /// （`RAPID_STRUCTURE_TABLE_DETECTION_DEBUG=1`）。
    fn print_bands(fixture: &str, lines: &[PageLine]) {
        let config = TableDetectorConfig::default();
        for (index, region) in super::detect_regions(lines, &config).iter().enumerate() {
            let mut intervals: Vec<(f32, f32)> = region
                .members
                .iter()
                .map(|member| {
                    let bounds = lines[*member].bounds();
                    (bounds.left, bounds.right)
                })
                .collect();
            intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut bands: Vec<(f32, f32)> = Vec::new();
            for (left, right) in intervals {
                match bands.last_mut() {
                    Some(last) if left <= last.1 => last.1 = last.1.max(right),
                    _ => bands.push((left, right)),
                }
            }
            let mut widths: Vec<f32> = bands.iter().map(|(l, r)| r - l).collect();
            widths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            eprintln!(
                "  {fixture} region #{index} rows={} cols={} bands={} median_band_width={:.1}",
                region.rows,
                region.columns,
                bands.len(),
                widths[widths.len() / 2]
            );
            for pair in bands.windows(2) {
                eprintln!(
                    "    band {:.1}..{:.1}  gap_to_next={:.1}",
                    pair[0].0,
                    pair[0].1,
                    pair[1].0 - pair[0].1
                );
            }
            if let Some(last) = bands.last() {
                eprintln!("    band {:.1}..{:.1}  (last)", last.0, last.1);
            }
        }
    }

    /// 人工核对入口：打印三张图的全部检出与列带诊断。
    ///
    /// ```text
    /// cargo test --lib table::detection::real_page_tests -- --nocapture
    /// RAPID_STRUCTURE_TABLE_DETECTION_DEBUG=1 cargo test --lib table::detection::real_page_tests -- --nocapture
    /// ```
    #[test]
    fn probe_real_pages() {
        let debug = std::env::var_os("RAPID_STRUCTURE_TABLE_DETECTION_DEBUG").is_some();
        for fixture in [
            DENSE_TABLE_FIXTURE,
            BORDER_ONLY_TABLE_FIXTURE,
            KEY_VALUE_PAGE_FIXTURE,
        ] {
            if debug
                && let Some(config) = engine_config()
                && let Some(path) = crate::test_support::table_detection_fixture(fixture)
            {
                let image = RecImage::from_path(&path).expect("fixture must decode");
                print_bands(fixture, &page_lines(&config, &image));
            }
            run(fixture);
        }
    }

    /// 文本密集的单表格：必须被定位成一个区域，且几何上覆盖整张表。
    #[test]
    fn a_dense_text_table_is_located_as_one_region() {
        let Some((regions, lines)) = run(DENSE_TABLE_FIXTURE) else {
            return;
        };
        assert!(
            lines.len() >= 50,
            "the fixture must actually contain text lines, got {}",
            lines.len()
        );
        assert_eq!(
            regions.len(),
            1,
            "a single dense table must yield exactly one region, got {regions:?}"
        );
        // 该图就是一张铺满画布的表格，所以期望盒是整张图。
        let expected = [0.0, 0.0, 371.0, 293.0];
        let actual = rect_of(&Bounds::of_polygon(&regions[0].polygon));
        let overlap = iou(actual, expected);
        assert!(
            overlap >= 0.9,
            "the located region {actual:?} must cover the whole table {expected:?}, IoU = {overlap}"
        );
        assert!(
            regions[0].score >= 0.9,
            "a clean grid must score high, got {}",
            regions[0].score
        );
        // 裁剪图必须与多边形包围盒一致（逐像素来自原图，不是重新渲染）。
        assert_eq!(regions[0].crop.width(), (actual[2] - actual[0]) as usize);
        assert_eq!(regions[0].crop.height(), (actual[3] - actual[1]) as usize);
        assert_eq!(regions[0].origin, [actual[0], actual[1]]);
    }

    /// 只有网格线、一个文字都没有的表格：**原理性不适用**，期望 0 区域。
    ///
    /// 这不是缺陷而是边界：定位器的唯一输入是文本框分布，没有文本就没有输入。
    /// 该情形必须走专用模型（`docs/07` §7.1 的备选路线），而不是靠调参。
    #[test]
    fn a_table_without_any_text_is_not_located_by_design() {
        let Some((regions, lines)) = run(BORDER_ONLY_TABLE_FIXTURE) else {
            return;
        };
        assert!(
            lines.len() <= 2,
            "this fixture is supposed to be text free, but det found {} lines",
            lines.len()
        );
        assert!(
            regions.is_empty(),
            "a text-free table cannot be located from text distribution, got {regions:?}"
        );
    }

    /// 多块并排版式：**已知假合并**。
    ///
    /// 这条测试断言的是**缺陷仍然存在**。它一旦失败，说明定位器学会了分开并排块，
    /// 那就必须同时更新本文档、`docs/09` §12.4 与 `docs/07` §7.1，而不是把断言改松。
    ///
    /// 机制（实测）：det 框紧贴**文字**而不是单元格，所以真表格内部的列间空隙可以
    /// 很大 —— 本页顶部统计表内部最大到 404.8 px；而左下键值对与右下表格之间的
    /// gutter 只有 159.7 px。**没有任何间距阈值能把二者分开**：阈值 > 404.8 就什么
    /// 都切不开，阈值 < 159.7 就会把真表格切碎。
    #[test]
    fn a_multi_block_page_still_merges_side_by_side_blocks() {
        let Some((regions, lines)) = run(KEY_VALUE_PAGE_FIXTURE) else {
            return;
        };
        assert!(
            lines.len() >= 40,
            "the fixture must contain text, got {}",
            lines.len()
        );
        let internal = super::detect_regions(&lines, &TableDetectorConfig::default());
        assert_eq!(
            internal.len(),
            2,
            "expected the top table plus one merged block, got {internal:?}"
        );

        // 区域 #0 = 顶部 5×5 统计表：结构上必须是 5 行 5 列，且落在页面上半部。
        let top = &internal[0];
        assert_eq!(
            (top.rows, top.columns),
            (5, 5),
            "the top statistics table is 5x5 including its header row"
        );
        assert!(
            top.bounds.bottom < 1000.0,
            "the top table must stay in the upper half, got bottom {}",
            top.bounds.bottom
        );
        let top_overlap = iou(rect_of(&top.bounds), KEY_VALUE_TOP_TABLE);
        assert!(
            top_overlap >= REGION_IOU,
            "the top table must match the box measured from the page's own grid lines \
             {KEY_VALUE_TOP_TABLE:?}, got {:?} with IoU = {top_overlap}",
            rect_of(&top.bounds)
        );

        // 区域 #1 = 左下键值对 + 右下表格被**合并**成一个区域（已知假合并）。
        let merged = &internal[1];
        assert!(
            merged.bounds.left <= 200.0 && merged.bounds.right >= 2900.0,
            "the known false merge must span both side-by-side blocks, got {:?}",
            rect_of(&merged.bounds)
        );
        assert_eq!(
            regions.len(),
            2,
            "the public detector must report the same two regions"
        );
    }

    /// 定位器相对整页预算是可忽略的（`docs/09` §12.2 第三条）。
    ///
    /// 注意口径：定位器是纯函数且**尚未接进页面管线**，所以 `ocr_total_ms` 的增量
    /// 按构造为 0；这里量的是它自身在 61 个文本框上的耗时，用来给增量一个上界。
    #[test]
    fn the_locator_costs_far_less_than_the_page_budget() {
        let Some(config) = engine_config() else {
            return;
        };
        let Some(path) = crate::test_support::table_detection_fixture(KEY_VALUE_PAGE_FIXTURE)
        else {
            return;
        };
        let image = RecImage::from_path(&path).expect("fixture must decode");
        let lines = page_lines(&config, &image);
        let mut detector =
            TextDistributionDetector::new(TableDetectorConfig::default()).expect("valid config");
        // 预热一次，避免把首次的分配成本算进去。
        let _ = detector.detect(&image, &lines).expect("warmup");
        let started = std::time::Instant::now();
        for _ in 0..10 {
            let _ = detector.detect(&image, &lines).expect("detection");
        }
        let per_call_ms = started.elapsed().as_secs_f64() * 100.0;
        eprintln!(
            "locator cost on {} lines: {per_call_ms:.3} ms/call",
            lines.len()
        );
        assert!(
            per_call_ms < 50.0,
            "the locator must stay negligible next to a ~1000 ms page budget, got {per_call_ms} ms"
        );
    }
}
