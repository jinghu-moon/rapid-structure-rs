//! 单元格内容回填：把**已经识别好的**页面级文本按几何关系灌进表格结构。
//!
//! 本模块的语义逐行对应参考实现 `rapid_table/table_matcher/main.py`（242 行，类
//! `TableMatch`）与 `rapid_table/table_matcher/utils.py` 的 `distance` / `compute_iou`。
//!
//! # 为什么不能逐 cell 裁剪再喂识别器
//!
//! 三条互相独立的第一性事实（`docs/09-table-recognition-execution.md` §10 根因）：
//!
//! 1. **批内最大宽高比会污染整批。** `src/ocr/rec/preprocess.rs` 的
//!    `batch_shape_for(max_wh_ratio, rec_image_shape)` 里
//!    `dst_width = ((img_height as f64) * max_wh_ratio) as usize`，而 `max_wh_ratio` 取自
//!    **批内最大值**（`src/ocr/rec/recognizer.rs` 每批先算 `max_wh_ratio` 与 `wh_ratio_list`，
//!    再整批 pad 到同一宽度）。表格里只要有一个跨列合并单元格，同批**所有**单元格都被拉宽；
//!    实测整批 pad 到 640 时每格成本是 `W=64` 的 **9.4 倍**。
//! 2. **识别器是单行的。** 它把任意高度的裁剪等宽拉伸到固定高 48 px，多行单元格会被压成一行。
//! 3. **裁剪本身是重复付费。** 每个单元格一次 crop + resize + 归一化，而整页路径已经付过一次。
//!
//! 因此生产路径是 [`BackfillSource::Reuse`]：整页检测与识别只跑一次，本模块只做几何匹配。
//!
//! # 门禁 G2
//!
//! [`BackfillOutcome::recognizer_calls`] 在复用路径上**恒为 0**。这不是"我们记得不要调"，
//! 而是结构性的：[`match_cells`] 的签名里**没有任何识别器**，`BackfillSource::Reuse` 的
//! 变体里也没有，因此"回填时又调了一次 rec"在类型层面就写不出来。
//! [`BackfillSource::Crop`] 只是验收用的对照物（§10.3 第 2 条要求"非空率不低于逐 cell 裁剪
//! 喂 rec 的对照实现"）—— 没有对照物，那句话无法被检验。
//!
//! # 与参考实现的四处**有意**偏离
//!
//! 1. **全零占位框保留槽位。** 参考实现 `post_process.py` 的 `normalize_bboxes` 会调用
//!    `filter_blank_bbox` 把全零框从 `cell_bboxes` 数组里**删掉**，于是其后所有下标与
//!    `td_index` 脱钩 —— 这是一个潜伏 bug（中间位置出现一个占位框，它后面每个单元格的
//!    文本都会串位）。本仓库在 [`CellSlot::bbox`] 里把占位框记为 `None` 而**保留槽位**，
//!    本模块把 `None` 槽位排除出候选但**保留其索引**。见
//!    `a_blank_placeholder_keeps_its_slot_index`。
//! 2. **不做 `<b>` 增删。** 参考实现在多行匹配时会 `content = content[3:]`（只要串里**任意
//!    位置**出现 `<b>` 就砍掉前 3 个字符）与 `content = content[:-4]`（同理砍掉后 4 个字符），
//!    并用 `<b>` 包裹整个单元格。本仓库的识别器从不输出 `<b>`，而 PubTabNet 真值里确实存在
//!    含 `<`/`>` 的真实文本（E6 实测 4,171 个单元格含 `<`），盲目砍 3/4 个字符会**静默损坏
//!    真实文本**。
//! 3. **文字置信度是**本模块**的选择。** 参考实现只用 `rec_res[i][0]`（文本），丢弃
//!    `rec_res[i][1]`（分数）。`docs/09-table-recognition-execution.md` §10.1 要求"结构置信度
//!    与文字置信度分开保存"，因此这里把匹配到的行分数取**算术平均**记为单元格的文字置信度，
//!    并且**绝不**把它与 [`StructureResult::confidence`](crate::table::decode::StructureResult)
//!    混成一个数。
//! 4. **行边界由文本行推，不由模型框推（E25）。** 参考实现（也是本模块 E25 之前的实现）对
//!    每个行独立地在**所有**单元格里取最优 IoU，于是"模型框整体下移 8–15 px"会把折行单元格的
//!    续行判给**上一行**（实测 IoU `0.195` 对 `0.189`）——症状是"上一行的单元格里混进下一行的
//!    第一句"。修法是先按文本行把 y 轴切成 `rows` 个**行带**（行数用结构模型的、边界用文本行的
//!    间隙），再在行带内取最优列：见 [`assign_row_bands`]。行带不可用或带内够不着任何框时
//!    退回参考实现的行为，因此这条偏离只会**改变归属**，不会丢文字。
//!
//! [`CellSlot::bbox`]: crate::table::decode::CellSlot::bbox
//! [`StructureResult::confidence`]: crate::table::decode::StructureResult::confidence

use crate::{
    Quad, config::RecImage, error::Result, table::decode::CellSlot,
    vision::rotate_crop::rotate_crop_image,
};

/// 几何匹配的最小 IoU，取自参考实现 `TableMatch.match_result` 的默认参数 `min_iou=0.1**8`。
///
/// **不要"顺手改成 0.1"。** 这个值小到几乎等于 0，它的作用不是"要求重叠"，而是
/// "拒绝完全不相交的框"（`compute_iou` 对不相交返回 0.0，于是 `1 - 0.0 = 1.0`，
/// 而 `1.0 >= 1.0 - min_iou` 成立 → 被拒）。
///
/// 精确值（Python 3.12.8 实测）：`0.1**8 = 1.0000000000000005e-08`，与字面量 `1e-8`
/// 相差 3 个 ulp（`0x3e45798ee2308c3d` vs `0x3e45798ee2308c3a`）。但真正参与比较的是
/// `1 - min_iou`，而两者的 `1 - min_iou` **都等于 `0.99999999`**（该值的 ulp ≈ 1.1e-16，
/// 远大于 3e-24 的差），因此**判定结果逐位相同**。`f64::powi` 不是 `const fn`，故这里
/// 只能用字面量。见 `the_min_iou_threshold_matches_the_reference_magic_number`。
pub const TABLE_MATCH_MIN_IOU: f64 = 1e-8;

/// 页面级一行文本（轴对齐框，页面像素坐标系）。
///
/// 字段与参考实现 `format_ocr_results`（`rapid_table/utils/utils.py:15`）产出的
/// `(dt_boxes, rec_res)` 一一对应：`bbox` 是 `[x_min, y_min, x_max, y_max]`，
/// 且该函数已经把坐标裁剪到图像边界内。
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub bbox: [f32; 4],
    pub text: String,
    pub score: f32,
}

/// 一个单元格的匹配拓扑（不是"文本是否为空"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellMatchKind {
    /// 结构模型给了全零占位框，没有可用几何。
    BlankBox,
    /// 有几何，但没有文本行落进这个单元格。
    Unmatched,
    /// 恰好一行落进这个单元格。
    Single,
    /// 多行落进这个单元格（跨行单元格，或检测把一个单元格切成了多行）。
    Multiple,
}

/// 回填结果。
///
/// `texts` / `text_scores` / `kinds` / `matched_lines` 四者与传入的 `cells`
/// **等长同序** —— 第 `i` 项就是 `cells[i]` 的内容。这个同序性是回填的唯一契约：
/// 发射器按槽位取文本，错位不会报错，只会静默产出错误的 HTML。
#[derive(Debug, Clone, PartialEq)]
pub struct BackfillOutcome {
    /// 每个槽位的文本。未匹配的槽位是**空字符串**，不是"缺失"。
    pub texts: Vec<String>,
    /// 每个槽位的文字置信度（匹配到的行分数的算术平均）；未匹配为 `None`。
    pub text_scores: Vec<Option<f32>>,
    /// 每个槽位的匹配拓扑。
    pub kinds: Vec<CellMatchKind>,
    /// 每个槽位匹配到的行在输入 `lines` 中的下标（按行序）。`Crop` 路径恒为空。
    pub matched_lines: Vec<Vec<usize>>,
    pub cells: usize,
    pub blank_cells: usize,
    pub unmatched_cells: usize,
    pub single_line_cells: usize,
    pub multi_line_cells: usize,
    /// 被 `filter_ocr_result` 丢弃的行数（整行都在所有单元格上边界之上）。
    pub lines_above_table: usize,
    /// 进入匹配的行数（= `lines.len() - lines_above_table`）。
    pub lines_considered: usize,
    /// 匹配成功的 `(单元格, 行)` 对数。
    pub matched_lines_total: usize,
    /// 本次匹配是否用了**文本行带**来定行边界（`assign_row_bands`）。
    ///
    /// `false` 表示退回了逐行几何匹配（间隙互相交错、或网格行数比文本行还多）。
    /// 这个量存在的唯一理由是让"修法有没有在这张图上生效"变成可断言的：否则
    /// "文本还是串位的"与"修法根本没生效"从外部看一模一样。
    pub text_bands_applied: bool,
    /// 实际调用 [`CellRecognizer::recognize`] 的次数。**复用路径恒为 0**（门禁 G2）。
    pub recognizer_calls: usize,
}

impl BackfillOutcome {
    /// 文本非空的槽位数。
    pub fn non_empty(&self) -> usize {
        self.texts.iter().filter(|text| !text.is_empty()).count()
    }

    /// 文本非空率，分母是**全部槽位**（含占位框）。
    ///
    /// 分母不取"有几何的槽位"，因为结构模型给出多少个槽位是模型自己的输出；
    /// 把占位框从分母里拿掉会让"模型多吐一个空单元格"变成"识别率上升"。
    pub fn non_empty_rate(&self) -> f64 {
        if self.cells == 0 {
            0.0
        } else {
            self.non_empty() as f64 / self.cells as f64
        }
    }

    /// 文字置信度的汇总。
    ///
    /// **与结构置信度是两回事**：结构置信度描述"这张表的骨架有多可信"，文字置信度描述
    /// "灌进去的字有多可信"。把它们相乘或取平均都得不到一个有定义的量。
    pub fn text_confidence(&self) -> TextConfidence {
        let mut sum = 0.0f64;
        let mut scored = 0usize;
        for score in self.text_scores.iter().flatten() {
            sum += *score as f64;
            scored += 1;
        }
        TextConfidence {
            cells: self.cells,
            scored_cells: scored,
            mean: if scored == 0 {
                None
            } else {
                Some((sum / scored as f64) as f32)
            },
        }
    }
}

/// 回填得到的文字置信度汇总。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextConfidence {
    pub cells: usize,
    /// 有分数的槽位数（未匹配与占位框都没有分数）。
    pub scored_cells: usize,
    /// 有分数的槽位的分数均值；一个都没有时为 `None`（**不是 `Some(0.0)`**）。
    pub mean: Option<f32>,
}

/// 逐 cell 裁剪喂识别器的对照实现所依赖的识别入口。
///
/// 复用路径**不实现也不调用**它 —— 这是门禁 G2 的结构性保证。
pub trait CellRecognizer {
    /// 识别一块裁剪区域，返回 `(文本, 分数)`。
    fn recognize(&mut self, region: &RecImage) -> Result<(String, f32)>;
}

/// 回填的文本来源。
pub enum BackfillSource<'a> {
    /// **生产路径**：复用整页检测与识别的结果。
    ///
    /// `offset` 是这张表格的裁剪区域在**页面**中的左上角。参考实现把同一张整图同时喂给
    /// 结构模型与识别器（`rapid_table/main.py:98-110`，无 crop），因此两套坐标天然同一系；
    /// 本仓库走"页面里的小表"路线，必须显式平移，否则坐标系错误会被误判成"检测差"。
    Reuse {
        lines: &'a [TextLine],
        offset: [f32; 2],
    },
    /// **对照物**（`docs/09-table-recognition-execution.md` §10.3 第 2 条）：逐 cell 裁剪。
    ///
    /// 这里 `cells` 的 bbox 必须是**表格图像自身**的坐标系，不需要 `offset` ——
    /// `image` 就是表格裁剪本身。
    Crop {
        image: &'a RecImage,
        recognizer: &'a mut dyn CellRecognizer,
    },
}

/// 8 点多边形（或参考实现里长度为 8 的 bbox）→ 轴对齐 `[x_min, y_min, x_max, y_max]`。
///
/// 与参考实现 `TableMatch.match_result` 里的
/// `[np.min(pred_box[0::2]), np.min(pred_box[1::2]), np.max(pred_box[0::2]), np.max(pred_box[1::2])]`
/// 一致。本仓库的 [`CellSlot::bbox`] 已经是降维后的形式，这个函数留给"从多边形进来的调用方"。
///
/// [`CellSlot::bbox`]: crate::table::decode::CellSlot::bbox
pub fn axis_aligned_bbox(polygon: &[f32; 8]) -> [f32; 4] {
    crate::vision::geometry::aabb_from_flat8(polygon)
}

/// 轴对齐 bbox → 裁剪用的四点四边形（左上、右上、右下、左下）。
pub fn bbox_to_quad(bbox: [f32; 4]) -> Quad {
    [
        [bbox[0], bbox[1]],
        [bbox[2], bbox[1]],
        [bbox[2], bbox[3]],
        [bbox[0], bbox[3]],
    ]
}

/// 参考实现 `utils.distance`：两个框的 L1 角点距离，再叠加**较小**的那个对角项。
///
/// ```text
/// d1 = |x3-x1| + |y3-y1|      （左上角距离）
/// d2 = |x4-x2| + |y4-y2|      （右下角距离）
/// distance = d1 + d2 + min(d1, d2)
/// ```
///
/// 它只用于**同 IoU 时的平局裁决**，因此量纲不重要，重要的是与参考实现逐位一致。
pub fn rectangle_distance(a: [f32; 4], b: [f32; 4]) -> f64 {
    let (x1, y1, x2, y2) = (a[0] as f64, a[1] as f64, a[2] as f64, a[3] as f64);
    let (x3, y3, x4, y4) = (b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64);
    let corner_start = (x3 - x1).abs() + (y3 - y1).abs();
    let corner_end = (x4 - x2).abs() + (y4 - y2).abs();
    corner_start + corner_end + corner_start.min(corner_end)
}

/// 参考实现 `utils.compute_iou`：轴对齐框的 IoU。
///
/// **参考实现的形参名是错的，算术是对的。** 它的签名是 `compute_iou(rec1, rec2)`，文档说
/// `(y0, x0, y1, x1)` 即 `(top, left, bottom, right)`，但唯一调用方传进去的是
/// `[x_min, y_min, x_max, y_max]`。于是：
///
/// - `S = (rec[2]-rec[0]) * (rec[3]-rec[1]) = (x_max-x_min) * (y_max-y_min)` —— **仍是面积**；
/// - `left_line = max(rec1[1], rec2[1])` 取的是**两个 y_min**，`right_line = min(rec1[3], rec2[3])`
///   取的是**两个 y_max** → `right-left` 是 **y 方向重叠**；
/// - `top_line = max(rec1[0], rec2[0])` 取的是**两个 x_min**，`bottom_line = min(rec1[2], rec2[2])`
///   取的是**两个 x_max** → `bottom-top` 是 **x 方向重叠**；
/// - `intersect = (right-left) * (bottom-top)` = (y 重叠) × (x 重叠) —— 乘法可交换，**仍是交面积**；
/// - 早退条件 `left >= right || top >= bottom` 等价于"任一方向无重叠"，**仍然正确**。
///
/// 所以这不是 bug，是一处"名字骗人"的代码。这里按正确的名字重写，数值行为完全一致。
/// 见 `the_swapped_parameter_names_do_not_change_the_iou`。
///
/// 参考实现在 `sum_area - intersect == 0` 时会 `0/0`；它自己的早退条件已经排除了那条路径
/// （`intersect <= min(S1, S2)`，要 `S1 + S2 - intersect == 0` 必须两个面积都为 0，而零面积框
/// 必然触发早退）。这里仍然显式挡住，因为 `NaN` 会污染后面的排序比较且**不会报错**。
pub fn rectangle_iou(a: [f32; 4], b: [f32; 4]) -> f64 {
    // 实现收敛到 `vision::geometry::aabb_iou`（P1-1）：逐位口径不变，本函数的既有
    // 用例（`the_swapped_parameter_names_do_not_change_the_iou` 等）即其数值对拍。
    crate::vision::geometry::aabb_iou(a, b)
}

/// 把多个匹配到同一单元格的行拼成一个字符串。
///
/// 逐行对应参考实现 `get_pred_html` 的多匹配分支。**只在匹配到多于一行时才做首尾空格处理**
/// —— 单匹配时参考实现原样使用识别结果，这里也原样使用。
///
/// 有意省略参考实现的 `<b>` 增删（见模块文档第 2 点）。剩下的顺序必须逐条保留：
///
/// ```text
/// 空串 → 跳过；首字符是空格 → 去掉；去掉后为空 → 跳过；
/// 不是最后一行且末尾不是空格 → 补一个空格
/// ```
///
/// 注意最后一条只在"不是最后一行"时补空格，所以"最后一行是空串被跳过"会留下一个**尾随空格**
/// —— 这是参考实现的行为，这里照抄，不"顺手修好"。
pub fn join_line_texts(texts: &[&str]) -> String {
    if texts.is_empty() {
        return String::new();
    }
    if texts.len() == 1 {
        return texts[0].to_string();
    }

    let mut out = String::new();
    for (index, raw) in texts.iter().enumerate() {
        let mut content = *raw;
        if content.is_empty() {
            continue;
        }
        if content.starts_with(' ') {
            content = &content[1..];
        }
        if content.is_empty() {
            continue;
        }
        out.push_str(content);
        if index != texts.len() - 1 && !content.ends_with(' ') {
            out.push(' ');
        }
    }
    out
}

/// Join matched OCR lines while preserving visual line breaks inside a cell.
///
/// OCR can split one visual line into several horizontally adjacent boxes, so a newline
/// must be based on vertical geometry rather than on the number of matched boxes. The
/// threshold is derived from the selected line heights and therefore scales with the page.
pub fn join_text_lines(lines: &[TextLine], indices: &[usize]) -> String {
    if indices.is_empty() {
        return String::new();
    }
    if indices.len() == 1 {
        return lines[indices[0]].text.clone();
    }

    let mut selected: Vec<&TextLine> = indices.iter().map(|&index| &lines[index]).collect();
    selected.sort_by(|a, b| {
        let a_center = (a.bbox[1] + a.bbox[3]) / 2.0;
        let b_center = (b.bbox[1] + b.bbox[3]) / 2.0;
        a_center
            .total_cmp(&b_center)
            .then_with(|| a.bbox[0].total_cmp(&b.bbox[0]))
    });

    let mut heights: Vec<f32> = selected
        .iter()
        .map(|line| (line.bbox[3] - line.bbox[1]).max(0.0))
        .filter(|height| height.is_finite() && *height > 0.0)
        .collect();
    heights.sort_by(f32::total_cmp);
    let median_height = heights.get(heights.len() / 2).copied().unwrap_or(1.0);
    let same_line_center_delta = (median_height * 0.75).max(1.0);

    let mut visual_lines: Vec<Vec<&str>> = Vec::new();
    let mut previous_center: Option<f32> = None;
    for line in selected {
        let center = (line.bbox[1] + line.bbox[3]) / 2.0;
        let same_visual_line = previous_center
            .is_some_and(|previous| (center - previous).abs() <= same_line_center_delta);
        if same_visual_line {
            visual_lines
                .last_mut()
                .expect("a previous center implies a visual line")
                .push(line.text.as_str());
        } else {
            visual_lines.push(vec![line.text.as_str()]);
        }
        previous_center = Some(center);
    }

    visual_lines
        .iter()
        .map(|texts| join_line_texts(texts))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 几何匹配，`min_iou` 取参考实现的默认值 [`TABLE_MATCH_MIN_IOU`]，不施加偏移。
pub fn match_cells(cells: &[CellSlot], lines: &[TextLine]) -> BackfillOutcome {
    match_cells_with_min_iou(cells, lines, [0.0, 0.0], TABLE_MATCH_MIN_IOU)
}

/// 几何匹配，`offset` 加到**单元格** bbox 上（把表格裁剪坐标平移到页面坐标）。
pub fn match_cells_with_offset(
    cells: &[CellSlot],
    lines: &[TextLine],
    offset: [f32; 2],
) -> BackfillOutcome {
    match_cells_with_min_iou(cells, lines, offset, TABLE_MATCH_MIN_IOU)
}

/// 用**文本行**决定每个文本行归到**哪一模型行**（`None` = 不参与，调用方退回逐格几何匹配）。
///
/// # 为什么不能逐行独立地在所有单元格里取最优 IoU
///
/// 结构模型给得准的是"这张表有几行"，给不准的是"行边界在 y 的哪里"：实测（`Temp\e23-real-table.png`，
/// 12 行 2 列）模型框整体比真实行**低 8–15 px**，于是一个折行单元格的续行恰好压在模型框边界上 ——
/// 它与上一行、下一行的 IoU 是 `0.195` 对 `0.189` —— 被判给**上一行**。症状是"上一行的单元格里
/// 混进下一行的第一句"，且从出错处起每行都错一格（`docs/11` §4.10 / §4.12）。
///
/// # 修法：行数用模型的，行边界用文本的
///
/// 1. **合格间隙**：纵向与表格范围相交的行按中心 y 排序后，"上一行的下沿 ≤ 下一行的上沿"
///    的相邻两行之间才是行边界候选（取其中点）。同一视觉行上的左右两个单元格互相重叠，
///    永远不产生候选 —— 实测 6 列表头 `A B C D E F` 的 y 抖动只有 1–2 px，绝不会被当成边界。
/// 2. 在合格间隙里取**最大的 `rows-1` 个**作边界（`rows` 从单元格网格数出来，含 `rowspan`）；
///    并列时取靠上的那个。
/// 3. 只有"选中的最小间隙 > 其余所有间隙的最大值"时才认这条划分；合格间隙不足 `rows-1` 个
///    或判据不成立时返回 `None`，调用方退回逐格匹配。
/// 4. 按边界给每个文本行的**中心**定行号。
///
/// 判据里没有魔数（比较的是两个间隙集合的上下确界与"是否纵向不相交"）。
/// **已经量测过的替代设计都更差**（全量 TEDS，7,104 样本）：只做视觉分组 + 保序 DP
/// `0.54395`、把模型边界吸附到最近合格间隙 `0.57229`、不做"合格"过滤 `0.57766`、
/// 按**相邻行中心距**而不是**纵向净空**挑边界 `0.58356`；本条 `0.58364`（入库基线 `0.58340`）。
/// 同一二进制连跑两次的预测逐字节相同（`ee528ea737e909fd`），所以这些差量都是代码差量。
/// 见 `docs/11` §4.12 的证据表。
fn assign_rows_from_text(
    lines: &[TextLine],
    kept: &[usize],
    table_top: f32,
    table_bottom: f32,
    rows: usize,
) -> Option<Vec<Option<usize>>> {
    if rows == 0 {
        return None;
    }
    let center = |index: usize| (lines[index].bbox[1] + lines[index].bbox[3]) / 2.0;

    // 纵向与表格范围相交的行才参与：页面上下方的正文不能给表格定行。
    let mut inside: Vec<usize> = kept
        .iter()
        .copied()
        .filter(|&index| lines[index].bbox[3] >= table_top && lines[index].bbox[1] <= table_bottom)
        .collect();
    if inside.is_empty() {
        return None;
    }

    let mut assigned = vec![None; lines.len()];
    if rows == 1 {
        for &index in &inside {
            assigned[index] = Some(0);
        }
        return Some(assigned);
    }

    // 合格间隙：相邻两行**纵向不相交**（上一行下沿 ≤ 下一行上沿）才有资格当行边界。
    inside.sort_by(|&a, &b| center(a).total_cmp(&center(b)).then(a.cmp(&b)));
    let mut gaps: Vec<f64> = Vec::new();
    for pair in inside.windows(2) {
        let (upper, lower) = (pair[0], pair[1]);
        if lines[upper].bbox[3] <= lines[lower].bbox[1] {
            gaps.push(((lines[upper].bbox[3] + lines[lower].bbox[1]) / 2.0) as f64);
        }
    }
    if gaps.len() < rows - 1 {
        return None;
    }

    // 在合格间隙里取最大的 `rows-1` 个作边界；并列时取靠上的那个（先出现的）。
    // 判据还要求"选中的最小间隙 > **其余所有**间隙的最大值"：间隙互相交错说明这条划分不可信。
    let mut ranked: Vec<usize> = (0..gaps.len()).collect();
    ranked.sort_by(|&a, &b| gaps[b].total_cmp(&gaps[a]).then(a.cmp(&b)));
    let boundaries = &ranked[..rows - 1];
    let chosen_min = boundaries
        .iter()
        .map(|&index| gaps[index])
        .fold(f64::INFINITY, f64::min);
    let unchosen_max = (0..gaps.len())
        .filter(|index| !boundaries.contains(index))
        .map(|index| gaps[index])
        .fold(f64::NEG_INFINITY, f64::max);
    if chosen_min <= unchosen_max {
        return None;
    }
    let mut snapped: Vec<f64> = boundaries.iter().map(|&index| gaps[index]).collect();
    snapped.sort_by(f64::total_cmp);

    // 严格大于：行中心正好落在边界上时留给上面的那一行 —— 与参考实现"平局取第一个候选"
    // 同一口径（见 `the_tie_break_prefers_the_first_candidate`）。
    for &index in &inside {
        let y = center(index) as f64;
        let mut row = 0usize;
        while row < snapped.len() && y > snapped[row] {
            row += 1;
        }
        assigned[index] = Some(row);
    }
    Some(assigned)
}

/// 几何匹配的完整形式。
///
/// 行级流程在参考实现 `TableMatch.process_one` 的基础上多了**第一步**：
///
/// ```text
/// filter_ocr_result（丢掉整行在表格上边界之上的行）
///   → assign_rows_from_text（模型给行数与大致边界，文本的合格间隙把它吸附到精确位置）
///   → match_result（该模型行内按 (1-IoU, distance) 取最优列，再按 min_iou 拒绝）
///   → get_pred_html（多行拼接）
/// ```
///
/// **一个行只进一个单元格；一个单元格可以收多行。** 这不是对称的，参考实现也不对称。
///
/// 第二步与参考实现的分工不同（参考实现只有一步、直接对所有单元格取最优）：见
/// `assign_rows_from_text` 的实测依据。行归属不可用时（返回 `None`）退回"对所有单元格
/// 取最优"，也就是参考实现的行为 —— 这条退路是刻意的：修法不能在它不适用的图上制造新错误。
pub fn match_cells_with_min_iou(
    cells: &[CellSlot],
    lines: &[TextLine],
    offset: [f32; 2],
    min_iou: f64,
) -> BackfillOutcome {
    let mut outcome = BackfillOutcome {
        texts: vec![String::new(); cells.len()],
        text_scores: vec![None; cells.len()],
        kinds: vec![CellMatchKind::BlankBox; cells.len()],
        matched_lines: vec![Vec::new(); cells.len()],
        cells: cells.len(),
        blank_cells: 0,
        unmatched_cells: 0,
        single_line_cells: 0,
        multi_line_cells: 0,
        lines_above_table: 0,
        lines_considered: 0,
        matched_lines_total: 0,
        recognizer_calls: 0,
        text_bands_applied: false,
    };

    // 候选 = 有 bbox 的槽位。**下标必须保留**：占位框不能让后面的槽位前移。
    let candidates: Vec<(usize, [f32; 4])> = cells
        .iter()
        .enumerate()
        .filter_map(|(index, cell)| {
            cell.bbox.map(|bbox| {
                (
                    index,
                    [
                        bbox[0] + offset[0],
                        bbox[1] + offset[1],
                        bbox[2] + offset[0],
                        bbox[3] + offset[1],
                    ],
                )
            })
        })
        .collect();

    outcome.blank_cells = cells.len() - candidates.len();
    for (index, _) in &candidates {
        outcome.kinds[*index] = CellMatchKind::Unmatched;
    }

    if candidates.is_empty() {
        outcome.lines_above_table = lines.len();
        outcome.unmatched_cells = 0;
        return outcome;
    }

    // `filter_ocr_result`：y1 是**所有**单元格 y 坐标的全局最小值。
    let table_top = candidates
        .iter()
        .map(|(_, bbox)| bbox[1] as f64)
        .fold(f64::INFINITY, f64::min);
    let kept: Vec<usize> = (0..lines.len())
        .filter(|&index| lines[index].bbox[3] as f64 >= table_top)
        .collect();
    outcome.lines_above_table = lines.len() - kept.len();
    outcome.lines_considered = kept.len();

    // 行归属：行数从网格里数（含 `rowspan`），"哪条文本行归哪一模型行"从文本布局推。见
    // [`assign_rows_from_text`]。
    let rows = cells
        .iter()
        .map(|cell| cell.logic.row_start + cell.rowspan.max(1))
        .max()
        .unwrap_or(0);
    let table_bottom = candidates
        .iter()
        .map(|(_, bbox)| bbox[3])
        .fold(f32::NEG_INFINITY, f32::max);
    let assigned_rows = assign_rows_from_text(lines, &kept, table_top as f32, table_bottom, rows);
    outcome.text_bands_applied = assigned_rows.is_some();
    // 每一模型行有哪些候选列。指派不可用时是空表，于是下面的候选恒为全集。
    let per_row: Vec<Vec<usize>> = match &assigned_rows {
        None => Vec::new(),
        Some(_) => (0..rows)
            .map(|row| {
                (0..candidates.len())
                    .filter(|&position| {
                        let logic = &cells[candidates[position].0].logic;
                        row >= logic.row_start && row <= logic.row_end
                    })
                    .collect()
            })
            .collect(),
    };

    // 在给定的候选子集里取 `(1-IoU, distance)` 最小者。
    //
    // **严格**小于 → 平局保留先出现的候选。参考实现是 `distances.index(sorted_distances[0])`：
    // `sorted` 稳定 + `.index` 用 `==` 比元组，同样取第一个达到最优键的元素。
    let pick = |positions: &[usize], line_bbox: [f32; 4]| -> Option<(usize, f64, f64)> {
        let mut best: Option<(usize, f64, f64)> = None;
        for &position in positions {
            let (_, cell_bbox) = candidates[position];
            let iou_term = 1.0 - rectangle_iou(line_bbox, cell_bbox);
            let distance = rectangle_distance(line_bbox, cell_bbox);
            let better = match best {
                None => true,
                Some((_, best_iou, best_distance)) => {
                    (iou_term, distance) < (best_iou, best_distance)
                }
            };
            if better {
                best = Some((position, iou_term, distance));
            }
        }
        best
    };
    // 参考实现：`if sorted_distances[0][1] >= 1 - min_iou: continue`。
    let acceptable = |best: Option<(usize, f64, f64)>| matches!(best, Some((_, iou_term, _)) if iou_term < 1.0 - min_iou);
    let all_positions: Vec<usize> = (0..candidates.len()).collect();

    for &line_index in &kept {
        let line_bbox = lines[line_index].bbox;
        // 先在**指派到的那一模型行**的单元格里找。不够格时退回全集（旧行为）——
        // 这条退路保证行归属只改变文字的**归属**，绝不引入"原本能匹配的行现在被丢掉"：
        // 丢文字是静默的信息损失，比放错行严重得多。
        let banded = assigned_rows
            .as_ref()
            .and_then(|assigned| assigned[line_index])
            .and_then(|row| pick(per_row[row].as_slice(), line_bbox));
        let mut best = banded;
        if !acceptable(best) {
            let fallback = pick(&all_positions, line_bbox);
            if acceptable(fallback) {
                best = fallback;
            }
        }

        let Some((position, iou_term, _)) = best else {
            continue;
        };
        if iou_term >= 1.0 - min_iou {
            continue;
        }
        let (slot_index, _) = candidates[position];
        outcome.matched_lines[slot_index].push(line_index);
    }

    for index in 0..cells.len() {
        let rows = &outcome.matched_lines[index];
        if rows.is_empty() {
            continue;
        }
        outcome.texts[index] = join_text_lines(lines, rows);

        let sum: f64 = rows.iter().map(|&row| lines[row].score as f64).sum();
        outcome.text_scores[index] = Some((sum / rows.len() as f64) as f32);

        if rows.len() > 1 {
            outcome.kinds[index] = CellMatchKind::Multiple;
            outcome.multi_line_cells += 1;
        } else {
            outcome.kinds[index] = CellMatchKind::Single;
            outcome.single_line_cells += 1;
        }
        outcome.matched_lines_total += rows.len();
    }
    outcome.unmatched_cells = outcome
        .kinds
        .iter()
        .filter(|kind| **kind == CellMatchKind::Unmatched)
        .count();

    outcome
}

/// 回填：按 [`BackfillSource`] 选择复用或对照路径。
///
/// 这是**唯一**同时存在两条路径的入口，也正是门禁 G2 的断言点：同一个函数在复用分支上
/// 返回的 [`BackfillOutcome::recognizer_calls`] 必然是 `0`，在对照分支上等于"真的裁了并识别了
/// 多少次"。见 `the_reuse_path_calls_the_recognizer_zero_times`。
pub fn backfill(cells: &[CellSlot], source: BackfillSource<'_>) -> Result<BackfillOutcome> {
    match source {
        BackfillSource::Reuse { lines, offset } => Ok(match_cells_with_min_iou(
            cells,
            lines,
            offset,
            TABLE_MATCH_MIN_IOU,
        )),
        BackfillSource::Crop { image, recognizer } => {
            let mut outcome = BackfillOutcome {
                texts: vec![String::new(); cells.len()],
                text_scores: vec![None; cells.len()],
                kinds: vec![CellMatchKind::BlankBox; cells.len()],
                matched_lines: vec![Vec::new(); cells.len()],
                cells: cells.len(),
                blank_cells: 0,
                unmatched_cells: 0,
                single_line_cells: 0,
                multi_line_cells: 0,
                lines_above_table: 0,
                lines_considered: 0,
                matched_lines_total: 0,
                recognizer_calls: 0,
                // 对照路径逐格裁剪识别，压根没有"行"的概念，谈不上行带。
                text_bands_applied: false,
            };

            for (index, cell) in cells.iter().enumerate() {
                let Some(bbox) = cell.bbox else {
                    continue;
                };
                // 有几何但还没拿到文本 —— 裁剪塌缩时它就停在这个状态。
                outcome.kinds[index] = CellMatchKind::Unmatched;
                let Some(quad) = clamped_quad(bbox, image.width(), image.height()) else {
                    // 裁剪区域在图像内塌缩成零面积：没有像素可以识别，也不该记一次调用。
                    continue;
                };
                let region = rotate_crop_image(image, quad)?;
                let (text, score) = recognizer.recognize(&region)?;
                outcome.recognizer_calls += 1;
                outcome.kinds[index] = CellMatchKind::Single;
                outcome.texts[index] = text;
                outcome.text_scores[index] = Some(score);
            }

            outcome.blank_cells = outcome
                .kinds
                .iter()
                .filter(|kind| **kind == CellMatchKind::BlankBox)
                .count();
            outcome.unmatched_cells = outcome
                .kinds
                .iter()
                .filter(|kind| **kind == CellMatchKind::Unmatched)
                .count();
            outcome.single_line_cells = outcome
                .kinds
                .iter()
                .filter(|kind| **kind == CellMatchKind::Single)
                .count();
            Ok(outcome)
        }
    }
}

/// bbox → 裁剪四边形，并把坐标夹到图像内（照 `src/ocr/pipeline/image_ops.rs:78-80` 的做法）。
///
/// 夹完之后若任一方向没有正长度，返回 `None` —— 参考实现在这种情况下会把空数组交给识别器，
/// 那是在制造一个没有定义的问题；这里明确拒绝。
fn clamped_quad(bbox: [f32; 4], width: usize, height: usize) -> Option<Quad> {
    if width == 0 || height == 0 {
        return None;
    }
    let max_x = width.saturating_sub(1) as f32;
    let max_y = height.saturating_sub(1) as f32;
    let x_min = bbox[0].clamp(0.0, max_x);
    let y_min = bbox[1].clamp(0.0, max_y);
    let x_max = bbox[2].clamp(0.0, max_x);
    let y_max = bbox[3].clamp(0.0, max_y);
    if x_max <= x_min || y_max <= y_min {
        return None;
    }
    Some(bbox_to_quad([x_min, y_min, x_max, y_max]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::decode::LogicCell;

    fn slot(index: usize, bbox: Option<[f32; 4]>) -> CellSlot {
        CellSlot {
            cell_index: index,
            tag: "<td></td>".to_string(),
            rowspan: 1,
            colspan: 1,
            score: 0.9,
            bbox,
            logic: LogicCell {
                row_start: index,
                row_end: index,
                col_start: 0,
                col_end: 0,
            },
        }
    }

    fn line(bbox: [f32; 4], text: &str) -> TextLine {
        TextLine {
            bbox,
            text: text.to_string(),
            score: 0.8,
        }
    }

    fn solid(width: usize, height: usize) -> RecImage {
        RecImage::from_bgr_u8(width, height, vec![0u8; width * height * 3])
            .expect("synthetic image must be valid")
    }

    #[derive(Default)]
    struct CountingRecognizer {
        calls: usize,
        regions: Vec<(usize, usize)>,
    }

    impl CellRecognizer for CountingRecognizer {
        fn recognize(&mut self, region: &RecImage) -> Result<(String, f32)> {
            self.calls += 1;
            self.regions.push((region.width(), region.height()));
            Ok((format!("cell{}", self.calls), 0.5))
        }
    }

    // ---- 几何 ----

    #[test]
    fn the_reference_distance_formula_is_reproduced() {
        // d1 = |20-0| + |20-0| = 40；d2 = |30-10| + |30-10| = 40
        // distance = 40 + 40 + min(40, 40) = 120
        assert_eq!(
            rectangle_distance([0.0, 0.0, 10.0, 10.0], [20.0, 20.0, 30.0, 30.0]),
            120.0
        );
        // 同尺寸同位置 → 0
        assert_eq!(
            rectangle_distance([0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 10.0, 10.0]),
            0.0
        );
    }

    #[test]
    fn the_swapped_parameter_names_do_not_change_the_iou() {
        // 参考实现把 (x_min,y_min,x_max,y_max) 喂给了形参名为 (y0,x0,y1,x1) 的函数。
        // 这里按正确的名字算，结果必须一致：
        // 交 = 5*5 = 25；面积 100 与 25 → 25 / (100 + 25 - 25) = 0.25
        assert!((rectangle_iou([0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 5.0, 5.0]) - 0.25).abs() < 1e-12);
        // 非对称：A 完全包住 B
        assert!((rectangle_iou([0.0, 0.0, 4.0, 4.0], [0.0, 0.0, 2.0, 2.0]) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn disjoint_and_edge_touching_rectangles_have_zero_iou() {
        assert_eq!(
            rectangle_iou([0.0, 0.0, 10.0, 10.0], [20.0, 20.0, 30.0, 30.0]),
            0.0
        );
        // 共享一条边 → 重叠为 0 → 参考实现的早退条件成立
        assert_eq!(
            rectangle_iou([0.0, 0.0, 10.0, 10.0], [10.0, 0.0, 20.0, 10.0]),
            0.0
        );
    }

    #[test]
    fn degenerate_zero_area_rectangles_do_not_produce_nan() {
        let iou = rectangle_iou([5.0, 5.0, 5.0, 5.0], [0.0, 0.0, 10.0, 10.0]);
        assert_eq!(iou, 0.0);
        assert!(!iou.is_nan());
        let iou = rectangle_iou([5.0, 5.0, 5.0, 5.0], [5.0, 5.0, 5.0, 5.0]);
        assert_eq!(iou, 0.0);
        assert!(!iou.is_nan());
    }

    #[test]
    fn the_min_iou_threshold_matches_the_reference_magic_number() {
        // 字面量与 `0.1**8` 相差 3 个 ulp，但参与比较的是 `1 - min_iou`，
        // 而两者的 `1 - min_iou` 逐位相同 —— 判定结果完全一致。
        let reference = 0.1f64.powi(8);
        assert_ne!(TABLE_MATCH_MIN_IOU.to_bits(), reference.to_bits());
        assert_eq!(
            (1.0 - TABLE_MATCH_MIN_IOU).to_bits(),
            (1.0 - reference).to_bits()
        );
        assert_eq!(1.0 - TABLE_MATCH_MIN_IOU, 0.99999999);
    }

    // ---- 匹配 ----

    #[test]
    fn a_line_lands_in_the_cell_that_contains_it() {
        let cells = vec![
            slot(0, Some([0.0, 0.0, 50.0, 20.0])),
            slot(1, Some([50.0, 0.0, 100.0, 20.0])),
        ];
        let lines = vec![
            line([5.0, 2.0, 45.0, 18.0], "left"),
            line([55.0, 2.0, 95.0, 18.0], "right"),
        ];
        let outcome = match_cells(&cells, &lines);

        assert_eq!(outcome.texts, vec!["left".to_string(), "right".to_string()]);
        assert_eq!(outcome.matched_lines, vec![vec![0], vec![1]]);
        assert_eq!(
            outcome.kinds,
            vec![CellMatchKind::Single, CellMatchKind::Single]
        );
        assert_eq!(outcome.matched_lines_total, 2);
        assert_eq!(outcome.non_empty(), 2);
        assert_eq!(outcome.recognizer_calls, 0);
    }

    #[test]
    fn a_single_matched_line_is_used_verbatim() {
        // 参考实现只在多匹配时做首尾空格处理；单匹配时原样使用。
        let cells = vec![slot(0, Some([0.0, 0.0, 100.0, 20.0]))];
        let lines = vec![line([5.0, 2.0, 95.0, 18.0], "  padded  ")];
        let outcome = match_cells(&cells, &lines);
        assert_eq!(outcome.texts, vec!["  padded  ".to_string()]);
    }

    #[test]
    fn a_cell_can_receive_several_lines_and_they_are_joined_with_line_breaks() {
        let cells = vec![slot(0, Some([0.0, 0.0, 100.0, 60.0]))];
        let lines = vec![
            line([5.0, 2.0, 95.0, 18.0], "first"),
            line([5.0, 22.0, 95.0, 38.0], "second"),
            line([5.0, 42.0, 95.0, 58.0], "third"),
        ];
        let outcome = match_cells(&cells, &lines);

        assert_eq!(outcome.texts, vec!["first\nsecond\nthird".to_string()]);
        assert_eq!(outcome.kinds, vec![CellMatchKind::Multiple]);
        assert_eq!(outcome.multi_line_cells, 1);
        assert_eq!(outcome.matched_lines_total, 3);
        // 分数是匹配到的三行的均值
        assert_eq!(outcome.text_scores, vec![Some(0.8)]);
    }

    #[test]
    fn join_line_texts_skips_empty_lines_and_spaces_only_the_non_final_ones() {
        assert_eq!(join_line_texts(&["a"]), "a");
        assert_eq!(join_line_texts(&["a", "b"]), "a b");
        assert_eq!(join_line_texts(&["a", "", "b"]), "a b");
        // 首尾空格被处理，但串**中间**的空格保留：`" a "` → `"a "` → 末尾已是空格故不再补
        assert_eq!(join_line_texts(&[" a ", " b "]), "a b ");
        // 末项为空串被跳过 → 前一项已经补过空格 → 留下尾随空格（参考实现的行为）
        assert_eq!(join_line_texts(&["a", ""]), "a ");
        assert_eq!(join_line_texts(&[]), "");
    }

    #[test]
    fn horizontally_split_boxes_stay_on_one_visual_line() {
        let lines = vec![
            line([0.0, 2.0, 30.0, 18.0], "left"),
            line([31.0, 3.0, 60.0, 19.0], "right"),
        ];
        assert_eq!(join_text_lines(&lines, &[0, 1]), "left right");
    }

    #[test]
    fn vertically_separated_boxes_keep_a_cell_line_break() {
        let lines = vec![
            line([0.0, 2.0, 60.0, 18.0], "first"),
            line([0.0, 22.0, 60.0, 38.0], "second"),
        ];
        assert_eq!(join_text_lines(&lines, &[0, 1]), "first\nsecond");
    }

    #[test]
    fn a_line_above_every_cell_is_dropped_before_matching() {
        let cells = vec![slot(0, Some([0.0, 100.0, 100.0, 120.0]))];
        let lines = vec![
            line([5.0, 10.0, 95.0, 30.0], "page header"),
            line([5.0, 102.0, 95.0, 118.0], "cell"),
        ];
        let outcome = match_cells(&cells, &lines);

        assert_eq!(outcome.lines_above_table, 1);
        assert_eq!(outcome.lines_considered, 1);
        assert_eq!(outcome.texts, vec!["cell".to_string()]);
    }

    #[test]
    fn a_line_that_overlaps_nothing_is_rejected_rather_than_forced() {
        // 两个互不相交的框 → IoU 0 → `1 - 0 = 1 >= 1 - min_iou` → 拒绝。
        // 这就是 `0.1**8` 的真实作用：不是"要求重叠"，而是"拒绝零重叠"。
        let cells = vec![slot(0, Some([0.0, 0.0, 10.0, 10.0]))];
        let lines = vec![line([500.0, 500.0, 600.0, 600.0], "far away")];
        let outcome = match_cells(&cells, &lines);

        assert_eq!(outcome.texts, vec![String::new()]);
        assert_eq!(outcome.kinds, vec![CellMatchKind::Unmatched]);
        assert_eq!(outcome.unmatched_cells, 1);
        assert_eq!(outcome.non_empty(), 0);
        assert_eq!(outcome.non_empty_rate(), 0.0);
        assert_eq!(outcome.text_confidence().mean, None);
    }

    #[test]
    fn a_barely_overlapping_box_is_accepted() {
        // 只要重叠严格大于 0，`1 - iou < 1`，而 `1 - min_iou = 0.99999999`
        // → 除非 IoU 小于 1e-8 才会被拒。重叠 1 像素足够。
        let cells = vec![slot(0, Some([0.0, 0.0, 10.0, 10.0]))];
        let lines = vec![line([9.0, 9.0, 20.0, 20.0], "corner")];
        let outcome = match_cells(&cells, &lines);
        assert_eq!(outcome.texts, vec!["corner".to_string()]);
    }

    #[test]
    fn the_tie_break_prefers_the_first_candidate() {
        // 两个完全相同的单元格，一个行居中落在两者之间 → 距离与 IoU 都相同。
        // 参考实现 `distances.index(sorted_distances[0])` 取第一个，这里必须一致。
        let cells = vec![
            slot(0, Some([0.0, 0.0, 10.0, 10.0])),
            slot(1, Some([0.0, 0.0, 10.0, 10.0])),
        ];
        let lines = vec![line([0.0, 0.0, 10.0, 10.0], "ambiguous")];
        let outcome = match_cells(&cells, &lines);

        assert_eq!(outcome.matched_lines, vec![vec![0], Vec::new()]);
        assert_eq!(outcome.texts, vec!["ambiguous".to_string(), String::new()]);
        assert_eq!(
            outcome.kinds,
            vec![CellMatchKind::Single, CellMatchKind::Unmatched]
        );
    }

    #[test]
    fn a_blank_placeholder_keeps_its_slot_index() {
        // 参考实现的 `filter_blank_bbox` 会把占位框删掉，于是它后面每个槽位都前移一位。
        // 这里必须保留槽位：行落在**第二个**槽位，结果就必须出现在下标 1。
        let cells = vec![slot(0, None), slot(1, Some([0.0, 0.0, 50.0, 20.0]))];
        let lines = vec![line([5.0, 2.0, 45.0, 18.0], "second slot")];
        let outcome = match_cells(&cells, &lines);

        assert_eq!(
            outcome.texts,
            vec![String::new(), "second slot".to_string()]
        );
        assert_eq!(outcome.matched_lines, vec![Vec::new(), vec![0]]);
        assert_eq!(outcome.blank_cells, 1);
        assert_eq!(outcome.kinds[0], CellMatchKind::BlankBox);
        assert_eq!(outcome.kinds[1], CellMatchKind::Single);
        // 分母是全部槽位（含占位框）→ 1/2
        assert_eq!(outcome.non_empty_rate(), 0.5);
    }

    #[test]
    fn a_cell_without_a_bbox_is_a_blank_box_not_an_unmatched_cell() {
        let cells = vec![slot(0, None), slot(1, Some([0.0, 0.0, 10.0, 10.0]))];
        let outcome = match_cells(&cells, &[]);
        assert_eq!(outcome.blank_cells, 1);
        assert_eq!(outcome.unmatched_cells, 1);
        assert_eq!(
            outcome.kinds,
            vec![CellMatchKind::BlankBox, CellMatchKind::Unmatched]
        );
    }

    #[test]
    fn the_offset_shifts_the_cells_into_page_coordinates() {
        // 单元格 bbox 在表格裁剪坐标系里；行在页面坐标系里。
        // 表格裁剪的左上角在页面 (100, 200)，不施加偏移就一个都匹配不上。
        let cells = vec![slot(0, Some([0.0, 0.0, 50.0, 20.0]))];
        let lines = vec![line([105.0, 202.0, 145.0, 218.0], "inside")];

        assert_eq!(match_cells(&cells, &lines).texts, vec![String::new()]);
        let shifted = match_cells_with_offset(&cells, &lines, [100.0, 200.0]);
        assert_eq!(shifted.texts, vec!["inside".to_string()]);
    }

    #[test]
    fn an_empty_input_does_not_panic_and_reports_counts_not_ratios() {
        let outcome = match_cells(&[], &[line([0.0, 0.0, 1.0, 1.0], "x")]);
        assert_eq!(outcome.cells, 0);
        assert_eq!(outcome.non_empty_rate(), 0.0);
        assert_eq!(outcome.text_confidence().mean, None);
        assert_eq!(outcome.lines_above_table, 1);
        assert_eq!(outcome.lines_considered, 0);
    }

    // ---- 门禁 G2：两条路径 ----

    #[test]
    fn the_reuse_path_calls_the_recognizer_zero_times() {
        let cells = vec![
            slot(0, Some([0.0, 0.0, 50.0, 20.0])),
            slot(1, Some([0.0, 20.0, 50.0, 40.0])),
            slot(2, None),
        ];
        let lines = vec![
            line([2.0, 2.0, 48.0, 18.0], "a"),
            line([2.0, 22.0, 48.0, 38.0], "b"),
        ];

        let reuse = backfill(
            &cells,
            BackfillSource::Reuse {
                lines: &lines,
                offset: [0.0, 0.0],
            },
        )
        .expect("the reuse path cannot fail");
        assert_eq!(reuse.recognizer_calls, 0);
        assert_eq!(
            reuse.texts,
            vec!["a".to_string(), "b".to_string(), String::new()]
        );
        assert_eq!(reuse.non_empty(), 2);
    }

    #[test]
    fn the_crop_control_calls_the_recognizer_once_per_cell_with_a_bbox() {
        let cells = vec![
            slot(0, Some([0.0, 0.0, 50.0, 20.0])),
            slot(1, Some([0.0, 20.0, 50.0, 40.0])),
            slot(2, None),
        ];
        let image = solid(200, 100);
        let mut recognizer = CountingRecognizer::default();

        let cropped = backfill(
            &cells,
            BackfillSource::Crop {
                image: &image,
                recognizer: &mut recognizer,
            },
        )
        .expect("an axis-aligned crop inside the image must succeed");

        assert_eq!(cropped.recognizer_calls, 2);
        assert_eq!(recognizer.calls, 2);
        assert_eq!(
            cropped.texts,
            vec!["cell1".to_string(), "cell2".to_string(), String::new()]
        );
        assert_eq!(cropped.blank_cells, 1);
        assert_eq!(cropped.unmatched_cells, 0);
        assert_eq!(cropped.kinds[0], CellMatchKind::Single);
        assert_eq!(cropped.kinds[2], CellMatchKind::BlankBox);
        assert_eq!(cropped.non_empty_rate(), 2.0 / 3.0);
    }

    #[test]
    fn the_two_paths_agree_on_which_cells_are_empty() {
        // §10.3 第 2 条要求复用路径的非空率**不低于**对照实现。
        // 这里的合成数据里两者非空集合相同；真正的对照要在数据集上跑（见 E9 完成记录）。
        let cells = vec![slot(0, Some([0.0, 0.0, 50.0, 20.0])), slot(1, None)];
        let lines = vec![line([2.0, 2.0, 48.0, 18.0], "a")];
        let reuse = backfill(
            &cells,
            BackfillSource::Reuse {
                lines: &lines,
                offset: [0.0, 0.0],
            },
        )
        .unwrap();

        let image = solid(200, 100);
        let mut recognizer = CountingRecognizer::default();
        let cropped = backfill(
            &cells,
            BackfillSource::Crop {
                image: &image,
                recognizer: &mut recognizer,
            },
        )
        .unwrap();

        assert_eq!(reuse.non_empty(), 1);
        assert_eq!(cropped.non_empty(), 1);
        assert_eq!(reuse.non_empty_rate(), cropped.non_empty_rate());
    }

    // ---- 文字置信度与结构置信度分开 ----

    #[test]
    fn text_confidence_is_reported_separately_from_the_structure_confidence() {
        let cells = vec![
            slot(0, Some([0.0, 0.0, 50.0, 20.0])),
            slot(1, Some([0.0, 20.0, 50.0, 40.0])),
        ];
        let mut lines = vec![
            line([2.0, 2.0, 48.0, 18.0], "a"),
            line([2.0, 22.0, 48.0, 38.0], "b"),
        ];
        lines[0].score = 0.6;
        lines[1].score = 0.9;
        let outcome = match_cells(&cells, &lines);

        let confidence = outcome.text_confidence();
        assert_eq!(confidence.cells, 2);
        assert_eq!(confidence.scored_cells, 2);
        assert_eq!(confidence.mean, Some(0.75));
        // 结构置信度是 `CellSlot::score`（0.9），两者是独立的量
        assert_ne!(confidence.mean, Some(cells[0].score));
    }

    #[test]
    fn a_partially_scored_outcome_averages_only_the_scored_cells() {
        let cells = vec![
            slot(0, Some([0.0, 0.0, 50.0, 20.0])),
            slot(1, Some([0.0, 20.0, 50.0, 40.0])),
        ];
        let mut lines = vec![line([2.0, 2.0, 48.0, 18.0], "a")];
        lines[0].score = 0.4;
        let outcome = match_cells(&cells, &lines);

        let confidence = outcome.text_confidence();
        assert_eq!(confidence.cells, 2);
        assert_eq!(confidence.scored_cells, 1);
        assert_eq!(confidence.mean, Some(0.4));
    }

    // ---- 行带（E25） ----

    /// 一个逻辑网格里的单元格（`row` / `col` 显式给出，便于构造 2 列多行的表）。
    fn cell(row: usize, col: usize, bbox: [f32; 4]) -> CellSlot {
        CellSlot {
            cell_index: row * 16 + col,
            tag: "<td></td>".to_string(),
            rowspan: 1,
            colspan: 1,
            score: 0.9,
            bbox: Some(bbox),
            logic: LogicCell {
                row_start: row,
                row_end: row,
                col_start: col,
                col_end: col,
            },
        }
    }

    /// 生产现场那张表的**真实坐标**（`Temp\e23-real-table.png` 的 12:00 / 13:00 两行，
    /// 行号在这里压成 0 / 1）：模型框整体比真实行低 8–15 px，折行续行"大圈正常出料…"
    /// 与上一行、下一行的 IoU 是 `0.195` 对 `0.189` —— 谁大纯属模型框抖动。
    ///
    /// 这是 E25 的失效输入：修复前它被判给**上一行**，于是 12:00 的格子里混进 13:00 的内容，
    /// 13:00 的格子只剩一个"序"字。
    fn real_wrapped_rows() -> (Vec<CellSlot>, Vec<TextLine>) {
        let cells = vec![
            cell(0, 0, [1.8, 334.0, 171.4, 397.0]),
            cell(0, 1, [181.1, 332.7, 972.5, 397.5]),
            cell(1, 0, [1.9, 394.2, 171.9, 458.0]),
            cell(1, 1, [168.3, 392.5, 971.9, 459.7]),
        ];
        let lines = vec![
            line([185.0, 324.0, 955.0, 346.0], "申博康第二批吃饭，内径"), // 12:00 第 1 行
            line([7.0, 338.0, 154.0, 363.0], "12：00~13：00"),            // 12:00 时间格
            line([469.0, 351.0, 670.0, 375.0], "磨冷却液小，打维修"),     // 12:00 第 2 行（折行）
            line([182.0, 383.0, 956.0, 407.0], "大圈正常出料，内径精磨"), // 13:00 第 1 行
            line([7.0, 398.0, 154.0, 423.0], "13：00~14：00"),            // 13:00 时间格
            line([553.0, 410.0, 584.0, 438.0], "序"),                     // 13:00 第 2 行（折行）
        ];
        (cells, lines)
    }

    #[test]
    fn a_wrapped_continuation_stays_in_its_own_row() {
        let (cells, lines) = real_wrapped_rows();
        let outcome = match_cells(&cells, &lines);

        assert!(
            outcome.text_bands_applied,
            "the row bands must engage on this input: the text gaps are 15.5/12.5 px inside the \
             rows and 32 px between them"
        );
        // 每行两个单元格：左列是时间格，右列是折行正文。
        assert_eq!(outcome.texts[0], "12：00~13：00");
        assert_eq!(outcome.texts[2], "13：00~14：00");
        assert_eq!(
            outcome.texts[1], "申博康第二批吃饭，内径\n磨冷却液小，打维修",
            "the 12:00 cell must hold the 12:00 row only"
        );
        assert_eq!(
            outcome.texts[3], "大圈正常出料，内径精磨\n序",
            "the 13:00 cell must hold the 13:00 row only — this is the E25 defect: before the \
             fix the 13:00 first line landed in the 12:00 cell (IoU 0.195 vs 0.189)"
        );
        assert_eq!(
            outcome.kinds,
            vec![
                CellMatchKind::Single,
                CellMatchKind::Multiple,
                CellMatchKind::Single,
                CellMatchKind::Multiple,
            ]
        );
        assert_eq!(outcome.matched_lines_total, lines.len());
    }

    #[test]
    fn rows_come_from_the_grid_and_boundaries_from_the_text() {
        // 3 行 2 列：真实文本行带密（行内间隙 6–18 px），模型框整体下移。
        let cells = vec![
            cell(0, 0, [0.0, 4.0, 40.0, 20.0]),
            cell(0, 1, [60.0, 4.0, 200.0, 20.0]),
            cell(1, 0, [0.0, 26.0, 40.0, 44.0]),
            cell(1, 1, [60.0, 22.0, 200.0, 58.0]),
            cell(2, 0, [0.0, 60.0, 40.0, 80.0]),
            cell(2, 1, [60.0, 56.0, 200.0, 80.0]),
        ];
        let lines = vec![
            line([2.0, 0.0, 38.0, 16.0], "h0"),
            line([62.0, 0.0, 198.0, 16.0], "h1"),
            line([2.0, 24.0, 38.0, 40.0], "t1"),
            line([62.0, 20.0, 198.0, 32.0], "r1a"),
            line([62.0, 36.0, 198.0, 50.0], "r1b"),
            line([2.0, 62.0, 38.0, 78.0], "t2"),
            line([62.0, 60.0, 198.0, 74.0], "r2"),
        ];
        let outcome = match_cells(&cells, &lines);

        assert!(
            outcome.text_bands_applied,
            "the gaps are 24 px / 18 px apart from 3–11 px inside the rows"
        );
        assert_eq!(outcome.texts[0], "h0");
        assert_eq!(outcome.texts[1], "h1");
        assert_eq!(outcome.texts[2], "t1");
        assert_eq!(outcome.texts[3], "r1a\nr1b");
        assert_eq!(outcome.texts[4], "t2");
        assert_eq!(outcome.texts[5], "r2");
        assert_eq!(outcome.kinds[3], CellMatchKind::Multiple);
    }

    #[test]
    fn a_spanning_cell_collects_lines_from_both_of_its_rows() {
        // 左列一个 `rowspan=2` 的单元格跨行 0/1：两个行带的行都要能进得去它。
        let mut spanning = cell(0, 0, [0.0, 0.0, 40.0, 90.0]);
        spanning.rowspan = 2;
        spanning.logic.row_end = 1;
        let cells = vec![
            spanning,
            cell(0, 1, [60.0, 4.0, 200.0, 20.0]),
            cell(1, 1, [60.0, 40.0, 200.0, 56.0]),
            cell(2, 0, [0.0, 96.0, 40.0, 112.0]),
            cell(2, 1, [60.0, 96.0, 200.0, 112.0]),
        ];
        let lines = vec![
            line([2.0, 4.0, 38.0, 20.0], "merged-a"),
            line([62.0, 4.0, 198.0, 20.0], "a"),
            line([2.0, 40.0, 38.0, 56.0], "merged-b"),
            line([62.0, 40.0, 198.0, 56.0], "b"),
            line([2.0, 96.0, 38.0, 112.0], "c0"),
            line([62.0, 96.0, 198.0, 112.0], "c1"),
        ];
        let outcome = match_cells(&cells, &lines);

        assert!(outcome.text_bands_applied);
        assert_eq!(
            outcome.texts[0], "merged-a\nmerged-b",
            "a rowspan=2 cell must be a candidate in every row it covers"
        );
        assert_eq!(outcome.kinds[0], CellMatchKind::Multiple);
        assert_eq!(outcome.texts[3], "c0");
        assert_eq!(outcome.texts[4], "c1");
    }

    #[test]
    fn a_line_its_own_row_cannot_reach_is_still_matched_not_dropped() {
        // 行带法只许改变**归属**：第二带的那行文字够不着第 2 行的任何框，但够得着第 1 行的框。
        // 退回全集后它必须仍然被匹配 —— 丢文字是静默的信息损失。
        let cells = vec![
            cell(0, 0, [0.0, 0.0, 100.0, 55.0]),
            cell(1, 0, [0.0, 60.0, 100.0, 80.0]),
        ];
        let lines = vec![
            line([5.0, 0.0, 95.0, 12.0], "a"),
            line([5.0, 14.0, 95.0, 26.0], "b"),
            line([5.0, 40.0, 95.0, 50.0], "c"),
        ];
        let outcome = match_cells(&cells, &lines);

        assert!(outcome.text_bands_applied, "gaps 14 px / 21 px → rows = 2");
        assert_eq!(outcome.texts[0], "a\nb\nc");
        assert_eq!(outcome.kinds[0], CellMatchKind::Multiple);
        assert_eq!(outcome.kinds[1], CellMatchKind::Unmatched);
        assert_eq!(
            outcome.matched_lines_total, 3,
            "no line may be dropped by the band restriction"
        );
    }

    #[test]
    fn the_text_gap_replaces_the_models_drifting_boundary() {
        // 直接用生产图的真实坐标驱动：模型把 r6/r7 的边界算在 `(397.5 + 392.5)/2 = 395`，
        // 而文本的合格间隙只有一处（12:00 的最后一行下沿 `375` → 13:00 首行上沿 `383`，中点
        // `379`）。行边界取 379 之后，13:00 首行（中心 `395`）落到边界**下方**，整条 13:00 行归第 2 行。
        let (_, lines) = real_wrapped_rows();
        let kept: Vec<usize> = (0..lines.len()).collect();
        let table_top = 332.7;
        let table_bottom = 459.7;

        let assigned = assign_rows_from_text(&lines, &kept, table_top, table_bottom, 2)
            .expect("the single eligible gap is enough for rows = 2");

        assert_eq!(
            assigned,
            vec![Some(0), Some(0), Some(0), Some(1), Some(1), Some(1)],
            "if the model's boundary at 395 were kept, the 13:00 first line would stay on row 1"
        );
    }

    #[test]
    fn a_header_row_of_side_by_side_cells_is_not_split_by_its_own_noise() {
        // 6 列表头 `A..F` 在**同一条视觉行**上，相邻中心距只有 1–2 px（真实 OCR 的 y 抖动）；
        // 第 2 行是空行（没有文本行）。
        //
        // 这是 E25 首版的真实失效样本（`139659444_0_all-line.jpg`，全量 TEDS `0.875 → 0.25`）：
        // "相邻中心距最大的间隙"把这段噪声当成了行边界，`A` 留在第 1 行、`B..F` 被推到第 2 行。
        // 判据因此收紧为"**只有纵向不重叠的相邻两行才可能是行边界**"。
        // 槽位顺序 = 先整行 0、再整行 1（与 decode 产出的槽位序一致）。
        let mut cells = Vec::new();
        for row in 0..2 {
            for col in 0..6 {
                let left = col as f32 * 40.0;
                let top = row as f32 * 40.0;
                cells.push(cell(row, col, [left, top, left + 30.0, top + 20.0]));
            }
        }
        let lines: Vec<TextLine> = (0..6)
            .map(|col| {
                let left = col as f32 * 40.0;
                let top = 1.0 + col as f32 * 0.4;
                line(
                    [left + 2.0, top, left + 28.0, top + 18.0],
                    &format!("c{col}"),
                )
            })
            .collect();
        let outcome = match_cells(&cells, &lines);

        // 6 格的 y 区间**互相重叠** → 一个合格间隙都没有 → 行归属不可信（`rows - 1 = 1` 个边界
        // 无处可取），退回参考实现的逐格匹配：每格按 IoU 进自己那一格，整行表头留在第 1 行。
        //
        // 这是 E25 首版的真实失效样本（`139659444_0_all-line.jpg`，全量 TEDS `0.875 → 0.25`）：
        // 当时"取相邻中心距最大的间隙"把这段 1–2 px 的 y 抖动当成了行边界，`A` 留在第 1 行、
        // `B..F` 被推到第 2 行。判据因此收紧为"**只有纵向不重叠的相邻两行才可能是行边界**"。
        assert!(
            !outcome.text_bands_applied,
            "there is not a single eligible gap here, so the row assignment must stay off"
        );
        for col in 0..6 {
            assert_eq!(
                outcome.texts[col],
                format!("c{col}"),
                "the whole header row must stay on row 0"
            );
            assert_eq!(outcome.kinds[col], CellMatchKind::Single);
            assert_eq!(outcome.kinds[6 + col], CellMatchKind::Unmatched);
        }
    }

    #[test]
    fn a_cell_with_several_stacked_lines_keeps_all_of_them() {
        // 3 行 4 列，第 1 行第 1 格是**三条纵向堆叠**的文本行（标号 / 张数 / 箱），
        // 下面两行各有 4 个单元格。合格间隙共 4 处（`30 / 60 / 95 / 135`），
        // 取最大的两个作边界正好落在两条**真实行边界**上（`95` 与 `135`）→ 三条堆叠行都归第 1 行。
        //
        // 这是"堆叠表头"的**良性**版本。全量 TEDS 里有一个更难的版本会失败
        // （`14_0c01373ca6999ceae4af96ed037efaa9_segR__tR__yf_0_all-line.jpg`：修前 `0.860`、
        // 本条修法 `0.461` —— 那里单元格内的堆叠间隙比行边界还大）：已作为**已知局限**登记在
        // `docs/11` §4.12 的未覆盖项里，并附带"更复杂的两条设计（视觉分组 + 保序 DP `0.54395`、
        // 模型边界吸附 `0.57229`）都能修好它，但全量 TEDS 反而更差"的实测证据。
        let mut cells = Vec::new();
        for row in 0..3 {
            let (top, bottom) = match row {
                0 => (0.0, 95.0),
                1 => (100.0, 140.0),
                _ => (140.0, 180.0),
            };
            for col in 0..4 {
                let left = col as f32 * 40.0;
                cells.push(cell(row, col, [left, top, left + 36.0, bottom]));
            }
        }
        let mut lines = vec![
            line([5.0, 5.0, 55.0, 25.0], "标号"),
            line([5.0, 35.0, 55.0, 55.0], "张数"),
            line([5.0, 65.0, 55.0, 85.0], "箱"),
        ];
        for (index, text) in ["0", "1", "2"].iter().enumerate() {
            let left = 44.0 + index as f32 * 40.0;
            lines.push(line([left, 35.0, left + 32.0, 55.0], text));
        }
        for (row, texts) in [(1usize, ["A", "2", "1", "3"]), (2, ["B", "2", "1", "2"])] {
            let top = if row == 1 { 105.0 } else { 145.0 };
            for (index, text) in texts.iter().enumerate() {
                let left = 4.0 + index as f32 * 40.0;
                lines.push(line([left, top, left + 32.0, top + 20.0], text));
            }
        }
        let outcome = match_cells(&cells, &lines);

        assert!(outcome.text_bands_applied);
        assert_eq!(
            outcome.texts[0], "标号\n张数\n箱",
            "a tall cell must collect every stacked line inside it"
        );
        assert_eq!(outcome.kinds[0], CellMatchKind::Multiple);
        assert_eq!(outcome.texts[1], "0");
        assert_eq!(outcome.texts[2], "1");
        assert_eq!(outcome.texts[3], "2");
        assert_eq!(
            &outcome.texts[4..8],
            ["A", "2", "1", "3"],
            "the data rows must stay on their own rows"
        );
        assert_eq!(&outcome.texts[8..12], ["B", "2", "1", "2"]);
        assert_eq!(outcome.matched_lines_total, lines.len());
    }

    // ---- 辅助函数 ----

    #[test]
    fn the_polygon_reduction_matches_the_reference_min_max_form() {
        let polygon = [10.0, 30.0, 50.0, 5.0, 20.0, 40.0, 5.0, 12.0];
        assert_eq!(axis_aligned_bbox(&polygon), [5.0, 5.0, 50.0, 40.0]);
    }

    #[test]
    fn a_quad_walks_the_box_clockwise_from_the_top_left() {
        assert_eq!(
            bbox_to_quad([1.0, 2.0, 3.0, 4.0]),
            [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]]
        );
    }

    #[test]
    fn a_crop_that_collapses_after_clamping_is_skipped_rather_than_recognized() {
        // bbox 完全在图像右侧之外 → 夹到边界后没有正长度 → 不识别、不计数。
        let cells = vec![slot(0, Some([500.0, 500.0, 600.0, 600.0]))];
        let image = solid(200, 100);
        let mut recognizer = CountingRecognizer::default();
        let outcome = backfill(
            &cells,
            BackfillSource::Crop {
                image: &image,
                recognizer: &mut recognizer,
            },
        )
        .unwrap();

        assert_eq!(recognizer.calls, 0);
        assert_eq!(outcome.recognizer_calls, 0);
        // 它**不是**占位框（有几何），只是没能产出文本
        assert_eq!(outcome.blank_cells, 0);
        assert_eq!(outcome.unmatched_cells, 1);
        assert_eq!(outcome.kinds, vec![CellMatchKind::Unmatched]);
    }

    #[test]
    fn a_zero_sided_image_is_refused_rather_than_cropped() {
        // `RecImage` 本身不允许零边，这里直接验证谓词：零边 → 没有四边形。
        assert_eq!(clamped_quad([0.0, 0.0, 1.0, 1.0], 0, 10), None);
        assert_eq!(clamped_quad([0.0, 0.0, 1.0, 1.0], 10, 0), None);
        assert!(clamped_quad([0.0, 0.0, 1.0, 1.0], 10, 10).is_some());
    }
}
