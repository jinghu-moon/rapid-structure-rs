//! SLANet-plus 结构解码的**纯函数层**。
//!
//! 这一层不依赖 ONNX、不读文件、不碰任何外部资产 —— 干净 clone 就能跑它的全部单测。
//! 模型推理与 session 在后续阶段加入（`docs/09-table-recognition-execution.md` §6）。
//!
//! # 为什么必须独立成层
//!
//! `docs/09-table-recognition-execution.md` §4 的理由：cell bbox 的坐标系（`rescale_cell_bboxes`）
//! 是回填路径（R1）的前置阻塞项，而坐标系必须有一个**可以单独拿 golden 钉住**的实测对象。
//! 只要解码器还缠在 ONNX session 里，"坐标系不对"与"模型推理不对"就无法区分。拆开之后，
//! E4 只需用固定输入跑一次真实模型，就能把坐标系单独锁死。
//!
//! # 与参考实现的对应关系
//!
//! 本模块是 `refer/RapidTable-main/rapid_table/table_structure/pp_structure/post_process.py`
//! 的 `TableLabelDecode` 与 `refer/RapidTable-main/rapid_table/table_matcher/main.py` 的
//! `decode_one_logic_points` 的 Rust 重写，语义**逐条对齐**，包括那些看起来像怪癖的地方 ——
//! 因为 E4 要拿参考实现的 Python 输出做逐 cell 比对，此处"顺手修正"会让比对结果无法解释。
//!
//! 三处刻意保留的怪癖：
//!
//! 1. `eos` 只在 `idx > 0` 时终止序列；`idx == 0` 的 `eos` 被忽略后继续解码；
//! 2. `sos` 与 `eos` 都会从 token 流里被**跳过**（`ignored_tokens`）；
//! 3. 逻辑网格推导里 `<tr>` 重置列号、`</tr>` 递增行号，两者是**独立事件** ——
//!    所以缺 `</tr>` 的行不会推进行号，这是参考实现的真实行为。
//!
//! # 一条硬约束：token 是片段，必须先拼接
//!
//! 词表里的 `<td`（下标 6）与 `>`（下标 7）是两个独立 token（见 [`super::metadata`]）。
//! 因此单元格的标签必须由**连续片段拼接**得到，绝不能用"token == `<td>`"这种相等匹配。
//! 拼接是自然发生的：`<td` + ` colspan="2"` + `>` → `<td colspan="2">`。

use crate::error::{RapidOcrError, Result};

/// `sos` 在类别空间中的下标。
///
/// 依据 `TableLabelDecode.add_special_char`：`[beg_str] + dict_character + [end_str]`，
/// 即 `sos` 被**前置**、`eos` 被**追加**。所以 `sos == 0`、`eos == token 数 + 1`。
///
/// ⚠️ 这是本阶段唯一**未经模型实测**的约定（E2 只验证了宽度 `tokens.len() + 2 == 50`）。
/// 它来自参考实现的编解码契约，须由 E4 的 golden 最终确认 —— 若 E4 发现相反，
/// 只需改这两个函数，其余代码全部通过它们取下标。
pub const SOS_CLASS_INDEX: usize = 0;

/// `eos` 在类别空间中的下标（追加在词表之后）。
pub fn eos_class_index(token_count: usize) -> usize {
    token_count + 1
}

/// 结构词表：`character` metadata 的 48 个片段 + `sos`/`eos` 两个特殊类别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureVocabulary {
    tokens: Vec<String>,
}

impl StructureVocabulary {
    /// 从 `character` metadata 解析出的片段构造词表。
    ///
    /// 拒绝空词表，以及**自身就含 `sos`/`eos` 字面量**的 metadata —— 后者会让特殊类别
    /// 与普通 token 撞名，是"不报错但结果全错"的典型来源。
    pub fn new(tokens: Vec<String>) -> Result<Self> {
        if tokens.is_empty() {
            return Err(RapidOcrError::Config(
                "table structure vocabulary must not be empty".to_string(),
            ));
        }
        for (index, token) in tokens.iter().enumerate() {
            if token == "sos" || token == "eos" {
                return Err(RapidOcrError::Config(format!(
                    "table structure vocabulary token {index} is the reserved special token \
                     `{token}`; sos/eos are added by the decoder and must not appear in the \
                     model's `character` metadata"
                )));
            }
        }
        Ok(Self { tokens })
    }

    /// 普通片段（不含 `sos`/`eos`）。
    pub fn tokens(&self) -> &[String] {
        &self.tokens
    }

    /// 类别总数 = 片段数 + `sos`/`eos`，应等于结构输出末维。
    pub fn class_count(&self) -> usize {
        self.tokens.len() + 2
    }

    pub fn sos_class_index(&self) -> usize {
        SOS_CLASS_INDEX
    }

    pub fn eos_class_index(&self) -> usize {
        eos_class_index(self.tokens.len())
    }

    /// 类别下标 → 片段。
    ///
    /// 越界是**必须报错**而不是 `unwrap_or_default()` 的情况：结构输出的末维与词表宽度
    /// 在契约层已经交叉校验过（`super::metadata::validate_vocabulary_width`），
    /// 走到这里还越界意味着张量本身被读错了，静默返回空串会把读错变成错结果。
    pub fn token_at(&self, class_index: usize) -> Result<&str> {
        if class_index == self.sos_class_index() {
            return Err(RapidOcrError::Decode(
                "table structure class `sos` reached the token mapping; sos must be skipped, not \
                 mapped to a fragment"
                    .to_string(),
            ));
        }
        if class_index == self.eos_class_index() {
            return Err(RapidOcrError::Decode(
                "table structure class `eos` reached the token mapping; eos must terminate the \
                 sequence, not be mapped to a fragment"
                    .to_string(),
            ));
        }
        // `sos` 占 0、普通片段从 1 开始，因此减去 1 得到词表下标。
        self.tokens
            .get(class_index - 1)
            .map(String::as_str)
            .ok_or_else(|| {
                RapidOcrError::Decode(format!(
                    "table structure class index {class_index} is out of range for a vocabulary of \
                 {} fragments ({} classes)",
                    self.tokens.len(),
                    self.class_count()
                ))
            })
    }

    /// 该类别是否是单元格起始 token。
    ///
    /// 依据参考实现的 `self.td_token = ["<td>", "<td", "<td></td>"]` —— 三个都算，
    /// 而 `>`、`</td>` 不算。字形上 `<td>` 是 `<td` + `>` 拼出来的，官方
    /// 词表里只有后两者，但这里仍然把 `<td>` 一并收进来：判定集合与参考实现**逐字一致**
    /// 才不会有"某个未来的模型把 `<td>` 直接放进 metadata"时静默走岔的余地。
    pub fn is_cell_start(&self, token: &str) -> bool {
        token == "<td>" || token == "<td" || token == "<td></td>"
    }
}

/// 沿类别维做 argmax，同时保留最大概率作为该步的分数。
///
/// `logits` 是 `steps × classes` 的**行优先**扁平切片，与 `(N, L, C)` 张量里单个 batch
/// 的 `L × C` 视图一致。
///
/// 与参考实现的 `structure_probs.argmax(axis=2)` + `structure_probs.max(axis=2)` 等价。
///
/// 返回 `Result` 而不是"静默截断"：`logits.len()` 不能整除 `steps` 时继续解码，
/// 会让每一步的类别下标整体错位，而错位后的结果看起来**完全合法**。
pub fn argmax_structure(logits: &[f32], steps: usize) -> Result<Vec<(usize, f32)>> {
    if steps == 0 {
        return Err(RapidOcrError::Decode(
            "table structure argmax needs at least one step".to_string(),
        ));
    }
    if !logits.len().is_multiple_of(steps) {
        return Err(RapidOcrError::Decode(format!(
            "table structure logits have {} values which is not a multiple of {steps} steps",
            logits.len()
        )));
    }
    let classes = logits.len() / steps;
    if classes == 0 {
        return Err(RapidOcrError::Decode(
            "table structure argmax got logits with zero classes".to_string(),
        ));
    }

    let mut steps_out = Vec::with_capacity(steps);
    for step in 0..steps {
        let row = &logits[step * classes..(step + 1) * classes];
        let mut best_index = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for (index, &value) in row.iter().enumerate() {
            // `>` 而非 `>=`：平局时取**较小**下标。参考实现的 `argmax` 同样取首个最大值，
            // 两者必须在平局上一致，否则 E4 的逐 cell 比对会随机失败。
            if value > best_score {
                best_score = value;
                best_index = index;
            }
        }
        if !best_score.is_finite() {
            return Err(RapidOcrError::Decode(format!(
                "table structure step {step} has no finite logit; the structure output looks \
                 uninitialised"
            )));
        }
        steps_out.push((best_index, best_score));
    }
    Ok(steps_out)
}

/// argmax 之后、绑定 bbox 之前的一步。
#[derive(Debug, Clone, PartialEq)]
pub struct TokenStep {
    /// 该步在**原始** `argmax` 序列中的下标（`sos`/`eos` 造成的空档**已计入**，
    /// 也就是说它等于参考实现 `decode` 循环里的 `idx`）。
    ///
    /// 必须随步一起携带、不能在别处重新推导：`scale_0.tmp_0`（bbox）与
    /// `scale_1.tmp_0`（structure）是**逐 step 对齐**的两路输出，参考实现取的正是
    /// `bbox_preds[batch_idx, idx]`。若丢掉它、改用"过滤后的第 n 个"，一旦 `sos`
    /// 不出现在下标 0（或出现多次），bbox 就会整体错位，而错位后的结果
    /// **看起来完全合法**——每个单元格都拿到一个别的单元格的框。
    pub step_index: usize,
    /// `argmax` 得到的类别下标。
    pub class_index: usize,
    /// 该类别对应的词表片段（`sos`/`eos` 不会出现在这里，它们已被跳过）。
    pub fragment: String,
    /// 该步的最大概率。
    pub score: f32,
    /// 该步是否是单元格起始（`<td` / `<td></td>`），决定它是否消费一个 bbox。
    pub is_cell_start: bool,
}

/// 类别序列 → token 步序列，落实参考实现的 `sos`/`eos` 规则。
///
/// 逐条对应 `TableLabelDecode.decode` 的循环：
///
/// ```text
/// if idx > 0 and char_idx == end_idx: break         ← eos 终止
/// if char_idx in ignored_tokens: continue           ← sos/eos 都被跳过
/// text = self.character[char_idx]
/// ```
///
/// 特别注意第一行的 `idx > 0`：`idx == 0` 处的 `eos` **不会**终止序列。这是参考实现的
/// 真实行为（用 `idx > 0` 代替了较新版本里的 `initialized` 布尔），此处照抄。
///
/// 返回值里的 [`StructureTermination`] 记录序列是**怎么结束的**：走 `break` 是
/// [`StructureTermination::Eos`]，把 `argmaxed` 跑完是 [`StructureTermination::Exhausted`]。
/// 后者意味着结构被 `Loop` 的 trip 上限截断 —— 这一点必须随序列一起交给
/// [`tokens_to_structure`]，因为**只看 token 序列本身分辨不出来**。
pub fn decode_token_steps(
    vocabulary: &StructureVocabulary,
    argmaxed: &[(usize, f32)],
) -> Result<(Vec<TokenStep>, StructureTermination)> {
    let sos = vocabulary.sos_class_index();
    let eos = vocabulary.eos_class_index();
    let mut out = Vec::with_capacity(argmaxed.len());
    let mut termination = StructureTermination::Exhausted;
    for (index, &(class_index, score)) in argmaxed.iter().enumerate() {
        if index > 0 && class_index == eos {
            termination = StructureTermination::Eos;
            break;
        }
        if class_index == sos || class_index == eos {
            continue;
        }
        let fragment = vocabulary.token_at(class_index)?.to_string();
        let is_cell_start = vocabulary.is_cell_start(&fragment);
        out.push(TokenStep {
            step_index: index,
            class_index,
            fragment,
            score,
            is_cell_start,
        });
    }
    Ok((out, termination))
}

/// 一个单元格在逻辑网格中的位置（0 基，**闭区间**）。
///
/// 与参考实现 `decode_one_logic_points` 返回的 `[r_start, r_end, col_start, col_end]` 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicCell {
    pub row_start: usize,
    pub row_end: usize,
    pub col_start: usize,
    pub col_end: usize,
}

/// 逻辑网格尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub rows: usize,
    pub cols: usize,
}

impl GridSize {
    /// 网格里的格子总数。
    pub const fn cell_count(&self) -> usize {
        self.rows * self.cols
    }
}

/// 逻辑网格的**占满性**。
///
/// # 为什么这是一个独立的量，而不是"由 span 求和推出来"
///
/// `covered` 是**互异**被覆盖格子的个数，不是 `Σ rowspan × colspan`：矩形**可以重叠**。
/// 反例：行 0 的第二个单元格带 `rowspan=2` 时占掉 `(0,1)` 与 `(1,1)`；随后行 1 的第一个
/// 单元格若带 `colspan=2`，它会从 `(1,0)` 起占 `(1,0)` 与 `(1,1)` —— 两者在 `(1,1)` 重叠。
/// 因此这个量只能在**持有占用集合的那一处**算出来，也就是 [`decode_logic_points`]。
///
/// # 空洞说明什么
///
/// 填充器本身不会留下空洞（每个单元格都从一个空闲格起、向右下铺开），所以
/// `holes > 0` 只能来自**声明与网格不一致**：某个单元格声明的 span 没有平铺满整个矩形。
/// 模型侧最典型的成因是 **span 超出词表** —— SLANet-plus 的 `rowspan`/`colspan` 片段只到
/// 20，而真实表格里存在 `colspan="24"`，模型只能给一个更小的值，于是矩形填不满。
///
/// 这正是 `docs/07-table-recognition.md` §7.5 要求 emitter 校验的量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridCoverage {
    /// 网格里的格子总数（`rows × cols`）。
    pub cells: usize,
    /// 被某个单元格矩形覆盖的格子数（去重）。
    pub covered: usize,
}

impl GridCoverage {
    /// 未被任何单元格覆盖的格子数。
    pub const fn holes(&self) -> usize {
        self.cells.saturating_sub(self.covered)
    }

    /// 网格是否被完全铺满。
    pub const fn is_complete(&self) -> bool {
        self.holes() == 0
    }
}

/// 结构 token 序列的**终止方式**。
///
/// 参考实现的循环有两个出口：遇到 `eos` 正常退出，或者步数用尽（`Loop` 的 trip 上限，
/// 常量 `assign_0.tmp_0.0 = 500`）。两者产出的 token 序列**看起来都合法**，
/// 只有"有没有见到 `eos`"能把它们分开 —— 因此这个量必须随序列一起携带。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureTermination {
    /// 在序列里遇到了 `eos`（正常退出）。
    Eos,
    /// 序列用尽仍未遇到 `eos`：结构被 `Loop` 的 trip 上限截断，尾部信息已经丢失。
    Exhausted,
}

impl StructureTermination {
    pub const fn is_truncated(self) -> bool {
        matches!(self, Self::Exhausted)
    }
}

/// 单元格槽位：结构解码得到、内容由后续阶段回填。
#[derive(Debug, Clone, PartialEq)]
pub struct CellSlot {
    /// 在 token 流中出现的次序（0 基），也是回填时与 `matched[cell_idx]` 对应的键。
    pub cell_index: usize,
    /// 该单元格的完整标签，由连续片段**拼接**得到（如 `<td colspan="2">` 或 `<td></td>`）。
    pub tag: String,
    /// 已解析的 `rowspan`（缺省为 1）。
    pub rowspan: usize,
    /// 已解析的 `colspan`（缺省为 1）。
    pub colspan: usize,
    /// 结构分支在该步的概率。
    pub score: f32,
    /// 反归一化后的 bbox（`[x_min, y_min, x_max, y_max]`）。全零占位框被丢弃，记为 `None`。
    pub bbox: Option<[f32; 4]>,
    /// 逻辑网格坐标。`cells` 与 `logic` 一一对应。
    pub logic: LogicCell,
}

/// 结构树节点。
///
/// 相比 `docs/07-table-recognition.md` §7.3 的示例，这里多一个 [`children`](Self::children)：
/// 少了它 `StructureResult::root` 就不是树。标签、span 与 bbox 的字段语义与文档一致。
#[derive(Debug, Clone, PartialEq)]
pub struct StructureNode {
    /// 该节点的完整标签（`table` / `tr` / `<td colspan="2">` / …）。
    pub tag: String,
    /// 仅当标签显式带该属性时为 `Some`。
    pub rowspan: Option<usize>,
    pub colspan: Option<usize>,
    /// 单元格节点在 [`StructureResult::cells`] 中的下标；非单元格节点为 `None`。
    pub cell_index: Option<usize>,
    /// 与 [`CellSlot::bbox`] 相同的 bbox；非单元格或占位框为 `None`。
    pub bbox: Option<[f32; 4]>,
    pub children: Vec<StructureNode>,
}

/// 一次结构解码的全部产物。
#[derive(Debug, Clone, PartialEq)]
pub struct StructureResult {
    /// 合成的 `<table>` 根节点（词表本身不含 `<table>`，见参考实现的 `wrap_with_html_struct`）。
    pub root: StructureNode,
    /// 按 token 顺序排列的单元格槽位。
    pub cells: Vec<CellSlot>,
    /// 逻辑网格尺寸。
    pub grid: GridSize,
    /// 逻辑网格的占满性。`!coverage.is_complete()` 说明某处的 span 没有平铺满矩形 ——
    /// 这是 emitter 判定"结构降级档"的依据之一。
    pub coverage: GridCoverage,
    /// token 序列的终止方式。`StructureTermination::Exhausted` 说明结构被 trip 上限截断。
    pub termination: StructureTermination,
    /// 结构分支的平均概率（参考实现 `np.mean(score_list)`）。
    pub confidence: f32,
}

impl StructureResult {
    /// 逻辑网格是否被完全铺满。
    pub const fn is_grid_complete(&self) -> bool {
        self.coverage.is_complete()
    }

    /// 结构是否被解码步数上限截断。
    pub const fn is_truncated(&self) -> bool {
        self.termination.is_truncated()
    }
}

/// 全零占位 bbox 判定。
///
/// 参考实现 `filter_blank_bbox` 的是 `~np.all(cell_bboxes == 0, axis=1)` ——
/// 判据是**八个坐标全为 0**，不是"面积为零"。一个 `(0,0,0,0)` 的退化框不会被丢弃。
pub fn is_blank_polygon(polygon: &[f32; 8]) -> bool {
    polygon.iter().all(|&value| value == 0.0)
}

/// 8 点多边形 → 轴对齐 bbox `[x_min, y_min, x_max, y_max]`。
///
/// 与参考实现 `TableMatch.match_result` 里的 `np.min(pred_box[0::2])` 等一致 ——
/// 后续的 IoU 与距离**全部**建立在轴对齐框上，因此这里就把它降维，不再保留多边形。
pub fn polygon_to_bbox(polygon: &[f32; 8]) -> [f32; 4] {
    crate::vision::geometry::aabb_from_flat8(polygon)
}

/// 预处理形状表，对应参考实现 `TablePreprocess` 的 `shape_list`。
///
/// 参考实现的 `shape_list = [h, w, ratio, ratio, 488, 488]`（`resize_image` 产出前四项、
/// `pad_img` 追加后两项）。`_bbox_decode` 只用前两项，`rescale_cell_bboxes` 只用原图。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableShape {
    /// 原图（表格 crop）高。
    pub original_height: f32,
    /// 原图（表格 crop）宽。
    pub original_width: f32,
    /// `488 / max(h, w)` —— 参考实现 `resize_image` 的 `ratio`。
    pub ratio: f32,
    /// 参考实现的 `max_len`，实测为 488。
    pub max_len: f32,
}

impl TableShape {
    /// 由 crop 尺寸构造。`ratio` 与参考实现的 `resize_image` 逐字一致。
    pub fn new(original_height: f32, original_width: f32, max_len: f32) -> Result<Self> {
        if !(original_height > 0.0 && original_width > 0.0 && max_len > 0.0) {
            return Err(RapidOcrError::Decode(format!(
                "table shape needs positive dimensions, got h={original_height} w={original_width} \
                 max_len={max_len}"
            )));
        }
        let longest = original_height.max(original_width);
        Ok(Self {
            original_height,
            original_width,
            ratio: max_len / longest,
            max_len,
        })
    }

    /// 反归一化：`bbox[0::2] *= w; bbox[1::2] *= h`（参考实现 `_bbox_decode`）。
    ///
    /// 模型输出的是**相对原图 crop** 的归一化坐标，因此这里乘的是 `original_*` 而不是
    /// resize 后的尺寸 —— 这与直觉相反（模型看到的明明是 letterbox 后的 488×488），
    /// 但参考实现就是这么写的，而训练侧的标注也确实按原图归一化。
    pub fn denormalize_polygon(&self, normalized: &[f32; 8]) -> [f32; 8] {
        let mut out = *normalized;
        for x in out.iter_mut().step_by(2) {
            *x *= self.original_width;
        }
        for y in out.iter_mut().skip(1).step_by(2) {
            *y *= self.original_height;
        }
        out
    }
}

/// 参考实现 `rescale_cell_bboxes` 的两个比例。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RescaleRatios {
    pub w_ratio: f32,
    pub h_ratio: f32,
}

/// 计算 `rescale_cell_bboxes` 的比例。
///
/// 参考实现原文：
///
/// ```text
/// ratio   = min(488 / h, 488 / w)
/// w_ratio = 488 / (w * ratio)
/// h_ratio = 488 / (h * ratio)
/// ```
///
/// # 坐标系（**E3 只落接口 + 代数结论，E4 用真实模型 golden 复核**）
///
/// 模型输出（框与结构 token 概率都是）归一化到**补零后的 488×488 画布**：图像占据画布
/// **左上角**的 `rw×rh` 区块，右侧与下方补零（`TablePreprocess.pad_img` 是左上对齐，
/// 不是居中）。
///
/// 两步复合后 x 与 y 的总因子都是
///
/// ```text
/// w * w_ratio = w * 488 / (w * ratio) = 488 / ratio = max(h, w)
/// ```
///
/// 而 `488 / ratio` 正是**画布坐标 → 原图像素**的比例。也就是说：
///
/// - [`TableShape::denormalize_polygon`] 与 [`rescale_polygon`] **复合**的结果是原图
///   （crop）**像素**坐标，**不是** 488 画布坐标；
/// - 中间的 `_bbox_decode` 在代数上被完全抵消，所以它乘原图尺寸还是别的尺寸都不影响
///   最终结果 —— 反过来，**只调用其中一个**才是错的。
///
/// 参考实现 `RapidTable` 把结构模型直接跑在**整张图**上（`rapid_table/main.py` 里
/// `imgs` 未经裁剪就同时喂给 `table_structure` 与 `ocr_engine`），因此它的 `cell_bboxes`
/// 与页面级 `dt_boxes` 同处一个像素坐标系，回填时的几何匹配不需要额外换算。本仓库走
/// 的是"页面 crop 里的小表"路线，**crop 偏移量必须另外加上**（E9 的职责）。
///
/// E4 仍须用真实模型 + Python 参考实现的逐 cell 比对来复核这条代数结论 —— 代数只能证明
/// "参考实现算出来是 `normalized × max(h,w)`"，不能证明"模型确实输出画布归一化坐标"。
pub fn rescale_ratios(original_width: f32, original_height: f32, max_len: f32) -> RescaleRatios {
    let ratio = (max_len / original_height).min(max_len / original_width);
    RescaleRatios {
        w_ratio: max_len / (original_width * ratio),
        h_ratio: max_len / (original_height * ratio),
    }
}

/// 对 8 点多边形施加 `rescale_cell_bboxes`。
pub fn rescale_polygon(polygon: &[f32; 8], ratios: RescaleRatios) -> [f32; 8] {
    let mut out = *polygon;
    for x in out.iter_mut().step_by(2) {
        *x *= ratios.w_ratio;
    }
    for y in out.iter_mut().skip(1).step_by(2) {
        *y *= ratios.h_ratio;
    }
    out
}

/// 逻辑网格推导，对应参考实现 `decode_one_logic_points`。
///
/// 输入是**按行分组的 span 序列**（每行一个 `Vec<(rowspan, colspan)>`）。分组来自 token 流里
/// 的 `<tr>` / `</tr>` 事件 —— 那一步在 [`tokens_to_structure`] 的单次遍历里完成，这里只做
/// 网格落位。
///
/// 算法与参考实现逐条一致：
///
/// - 每行从 `current_col = 0` 起（对应 `<tr>` 把列号重置为 0）；
/// - 单元格从 `(current_row, current_col)` 起占 `rowspan × colspan`；
/// - 落点前先跳过已被占用的格子（`is_occupied` 循环）；
/// - 落点后用 `mark_occupied` 把整个矩形标占用；
/// - 结束后 `current_col += colspan`。
///
/// ⚠️ 参考实现在遇到**非单元格 token** 时不推进列号，因此 `thead`/`tbody` 的存在与否
/// 不影响网格。
///
/// 第三个返回值是**占满性**。它只能在这里算：`occupied` 是唯一知道"哪些格子被覆盖过"
/// 的地方，而矩形可以重叠，所以"被覆盖的格子数"不能由 `Σ rowspan × colspan` 还原
/// （见 [`GridCoverage`]）。
pub fn decode_logic_points(
    rows: &[Vec<(usize, usize)>],
) -> Result<(Vec<LogicCell>, GridSize, GridCoverage)> {
    let total: usize = rows.iter().map(Vec::len).sum();
    let mut logic = Vec::with_capacity(total);
    let mut occupied: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut max_rows = 0usize;
    let mut max_cols = 0usize;

    for (row_index, row) in rows.iter().enumerate() {
        let mut current_col = 0usize;
        for &(rowspan, colspan) in row {
            if rowspan == 0 || colspan == 0 {
                return Err(RapidOcrError::Decode(
                    "table cell spans must be at least 1; rowspan/colspan attributes in the \
                     vocabulary start at 2, so a 0 means the tag was parsed wrong"
                        .to_string(),
                ));
            }

            // 「找到下一个未被占用的列」。参考实现无上限，这里同样不设上限：
            // 占用格总是有限的，而 current_col 每轮必增，循环必然终止。
            while occupied.contains(&(row_index, current_col)) {
                current_col += 1;
            }

            let cell = LogicCell {
                row_start: row_index,
                row_end: row_index + rowspan - 1,
                col_start: current_col,
                col_end: current_col + colspan - 1,
            };
            for r in cell.row_start..=cell.row_end {
                for c in cell.col_start..=cell.col_end {
                    occupied.insert((r, c));
                }
            }
            max_rows = max_rows.max(cell.row_end + 1);
            max_cols = max_cols.max(cell.col_end + 1);
            logic.push(cell);

            current_col += colspan;
        }
    }

    let cells = max_rows * max_cols;
    let coverage = GridCoverage {
        cells,
        covered: occupied.len(),
    };
    Ok((
        logic,
        GridSize {
            rows: max_rows,
            cols: max_cols,
        },
        coverage,
    ))
}

/// 从拼接出的单元格标签解析 `rowspan` / `colspan`。
///
/// 输入形如 `<td>`、`<td colspan="2">`、`<td rowspan="3" colspan="2">`、`<td></td>`。
/// 只接受**正整数**；词表里的值从 2 到 20，但这里不硬编码上界 —— 上界是模型表达能力问题，
/// 属于 emitter 的降级判据（`docs/09-table-recognition-execution.md` §4.3），
/// 解码器不做、也不假装能做这个检测。
pub fn parse_cell_tag(tag: &str) -> Result<(Option<usize>, Option<usize>)> {
    if !tag.starts_with("<td") {
        return Err(RapidOcrError::Decode(format!(
            "expected a cell tag starting with `<td`, got `{tag}`"
        )));
    }
    let mut rowspan = None;
    let mut colspan = None;
    for attribute in ["rowspan", "colspan"] {
        let Some(position) = tag.find(attribute) else {
            continue;
        };
        let rest = &tag[position + attribute.len()..];
        let Some(equals) = rest.find('=') else {
            return Err(RapidOcrError::Decode(format!(
                "table cell tag `{tag}` has `{attribute}` without a value"
            )));
        };
        let rest = rest[equals + 1..].trim_start();
        let quote = rest.chars().next();
        let digits: String = match quote {
            Some(q @ ('"' | '\'')) => {
                let after = &rest[q.len_utf8()..];
                after.chars().take_while(|c| c.is_ascii_digit()).collect()
            }
            _ => rest.chars().take_while(|c| c.is_ascii_digit()).collect(),
        };
        let value: usize = digits.parse().map_err(|_| {
            RapidOcrError::Decode(format!(
                "table cell tag `{tag}` has a non-numeric `{attribute}` value"
            ))
        })?;
        if value == 0 {
            return Err(RapidOcrError::Decode(format!(
                "table cell tag `{tag}` has `{attribute}=0`; spans are 1-based"
            )));
        }
        match attribute {
            "rowspan" => rowspan = Some(value),
            _ => colspan = Some(value),
        }
    }
    Ok((rowspan, colspan))
}

/// token 步序列 → 完整结构结果。
///
/// `cell_polygons` 必须**按 token 顺序**给出每个单元格起始步的 8 点 bbox，长度等于
/// `steps` 中 `is_cell_start == true` 的个数。这一点由 [`decode_token_steps`] 的
/// `is_cell_start` 与模型输出的 `scale_0` 输出（每步一行 8 个值）一一对应。
///
/// 返回的 [`StructureResult::cells`] 与 [`CellSlot::logic`] 通过 `cell_index` 对齐，
/// 那些被丢弃的全零占位框**不会**出现在 `cells` 里 —— 与参考实现 `filter_blank_bbox`
/// 的位置一致（它也是在 `decode` 末尾过滤）。因此 `cells.len() <= logic.len()`
/// 是本函数可能出现的正常状态…… **但本实现刻意让两者严格相等**：占位框保留在
/// `cells` 里、只把 `bbox` 置 `None`。理由是回填路径要用 `cell_index` 作为与
/// `matched[cell_idx]` 的键，若在这里过滤，下标就会与 token 位置脱钩，而参考实现的
/// `td_index` 是按 token 位置递增的。这是一次**有意的偏离**，已在
/// `docs/09-table-recognition-execution.md` §4 记录。
///
/// `termination` 由 [`decode_token_steps`] 给出，并原样记进
/// [`StructureResult::termination`]。它**不能**在这里从 `steps` 反推：截断后的序列
/// 与正常结束的序列在 token 层面完全同形。
pub fn tokens_to_structure(
    vocabulary: &StructureVocabulary,
    steps: &[TokenStep],
    termination: StructureTermination,
    cell_polygons: &[[f32; 8]],
) -> Result<StructureResult> {
    let expected_polygons = steps.iter().filter(|step| step.is_cell_start).count();
    if cell_polygons.len() != expected_polygons {
        return Err(RapidOcrError::Decode(format!(
            "table structure has {expected_polygons} cell-start tokens but {} cell boxes were \
             supplied; the box output and the structure output are not aligned",
            cell_polygons.len()
        )));
    }

    let mut root = StructureNode {
        // 词表不含 `<table>`；参考实现的 `wrap_with_html_struct` 负责补上它，
        // 这里在解析阶段就建出来，让 `StructureResult::root` 名副其实。
        tag: "<table>".to_string(),
        rowspan: None,
        colspan: None,
        cell_index: None,
        bbox: None,
        children: Vec::new(),
    };
    // 栈里存的是**每一层的子下标**（`root` 是第 0 层、不入栈），不是完整路径 ——
    // 完整路径只要把整个栈读出来就是，存两份会互相打架。
    let mut stack: Vec<usize> = Vec::new();

    let mut cells: Vec<CellSlot> = Vec::new();
    // 网格落位用的**按行分组**的 span。分组规则忠实于参考实现：只有 `</tr>` 推进行号，
    // `<tr>` 不推进（它只重置列号，而"每行从 0 列起"由 `decode_logic_points` 自己保证）。
    let mut row_spans: Vec<Vec<(usize, usize)>> = vec![Vec::new()];
    let mut polygon_cursor = 0usize;
    // 参考实现为**每一步**（跳过 sos/eos 之后）都记一个分数，包括 `>` 与 `</td>`。
    // 因此这里对全部步骤求和，而不是只对遍历路径上"被访问到"的步骤求和 ——
    // 单元格标签的属性片段被 `concat_cell_tag` 一次消费掉，但它们仍然计分。
    let score_sum: f64 = steps.iter().map(|step| f64::from(step.score)).sum();

    // 单次遍历同时产出「单元格槽位」与「结构树」。必须是单次：单元格的属性片段
    // （` colspan="2"`、`>`）在拼成标签后就已经被消费，第二遍再看它们会误判成错位。
    let mut position = 0usize;
    while position < steps.len() {
        let step = &steps[position];
        // 每一步都必须真的是词表里的一个片段。`TokenStep` 通常是 `decode_token_steps`
        // 的产物，但这是公开 API，不能让手工拼出的步骤绕开词表把任意字符串喂进树里。
        match vocabulary.token_at(step.class_index) {
            Ok(token) if token == step.fragment => {}
            _ => {
                return Err(RapidOcrError::Decode(format!(
                    "table structure step {position} claims class {} with fragment `{}`, which \
                     is not that class in the model vocabulary",
                    step.class_index, step.fragment
                )));
            }
        }
        // 参考实现为**每一步**（跳过 sos/eos 之后）都记一个分数，包括 `>` 与 `</td>`；
        // 分数已在循环外一次求和。

        if step.is_cell_start {
            let (tag, consumed) = concat_cell_tag(steps, position)?;
            let (rowspan, colspan) = parse_cell_tag(&tag)?;
            let polygon = cell_polygons[polygon_cursor];
            polygon_cursor += 1;

            let cell_index = cells.len();
            cells.push(CellSlot {
                cell_index,
                tag: tag.clone(),
                rowspan: rowspan.unwrap_or(1),
                colspan: colspan.unwrap_or(1),
                score: step.score,
                bbox: if is_blank_polygon(&polygon) {
                    None
                } else {
                    Some(polygon_to_bbox(&polygon))
                },
                // 占位，稍后由 `decode_logic_points` 回填。
                logic: LogicCell {
                    row_start: 0,
                    row_end: 0,
                    col_start: 0,
                    col_end: 0,
                },
            });
            row_spans
                .last_mut()
                .expect("row_spans always keeps at least the current row")
                .push((cells[cell_index].rowspan, cells[cell_index].colspan));

            push_child(
                &mut root,
                &stack,
                StructureNode {
                    tag,
                    rowspan: rowspan.filter(|&value| value > 1),
                    colspan: colspan.filter(|&value| value > 1),
                    cell_index: Some(cell_index),
                    bbox: cells[cell_index].bbox,
                    children: Vec::new(),
                },
            );

            position += consumed;
            continue;
        }

        match step.fragment.as_str() {
            // 单元格是**叶子**：`</td>` 只标记内容的结束位置，而内容要到回填阶段才存在。
            // 因此它不弹栈 —— 弹出会把外层 `<tr>` 误关掉。
            "</td>" => {}
            fragment if fragment.starts_with("</") => {
                let Some(open_index) = stack.pop() else {
                    return Err(RapidOcrError::Decode(format!(
                        "table structure closes `{fragment}` without a matching open tag"
                    )));
                };
                let open_tag = node_at(&root, &stack).children[open_index].tag.as_str();
                if !closes(open_tag, fragment) {
                    return Err(RapidOcrError::Decode(format!(
                        "table structure closes `{fragment}` but the innermost open tag is \
                         `{open_tag}`"
                    )));
                }
                // 只有 `</tr>` 推进行号（参考实现里 `</tr>` 让 `current_row += 1`）。
                if fragment == "</tr>" {
                    row_spans.push(Vec::new());
                }
            }
            "<thead>" | "<tbody>" | "<tr>" => {
                // `thead`/`tbody` **照原样保留**。参考实现在 `get_pred_html` 末尾用
                // `filter_elements` 把它们滤掉，但那是**输出策略**，属于 emitter
                // （`docs/07-table-recognition.md` §7.5）；解码器丢掉它们就是丢信息。
                let node = StructureNode {
                    tag: step.fragment.clone(),
                    rowspan: None,
                    colspan: None,
                    cell_index: None,
                    bbox: None,
                    children: Vec::new(),
                };
                let index = push_child(&mut root, &stack, node);
                stack.push(index);
            }
            fragment => {
                return Err(RapidOcrError::Decode(format!(
                    "table structure has a fragment `{fragment}` that is neither a cell start nor \
                     a tag this decoder knows; a bare `>` or ` colspan=…` here means the `<td` \
                     that should have opened it is missing"
                )));
            }
        }
        position += 1;
    }

    if !stack.is_empty() {
        return Err(RapidOcrError::Decode(format!(
            "table structure ends with {} unclosed tag(s)",
            stack.len()
        )));
    }

    let (logic, grid, coverage) = decode_logic_points(&row_spans)?;
    for (cell, cell_logic) in cells.iter_mut().zip(logic.iter()) {
        cell.logic = *cell_logic;
    }

    let confidence = if steps.is_empty() {
        0.0
    } else {
        (score_sum / steps.len() as f64) as f32
    };

    Ok(StructureResult {
        root,
        cells,
        grid,
        coverage,
        termination,
        confidence,
    })
}

/// 从 `steps[start]` 处的单元格起始片段开始，向后拼接直到控制片段 `>` 出现。
///
/// 返回拼好的标签与**被消费的片段个数**（含起始片段本身）。这就是"token 是片段"的直接
/// 后果：`<td` 之后的 ` colspan="…"` / ` rowspan="…"` 片段都属于同一个标签，直到 `>`
/// 收尾。`<td></td>` 是自闭合的单 token，消费 1 个片段。
fn concat_cell_tag(steps: &[TokenStep], start: usize) -> Result<(String, usize)> {
    let start_fragment = &steps[start].fragment;
    if start_fragment == "<td></td>" {
        return Ok((start_fragment.clone(), 1));
    }
    if start_fragment != "<td" {
        return Err(RapidOcrError::Decode(format!(
            "table cell start token must be `<td` or `<td></td>`, got `{start_fragment}`"
        )));
    }

    let mut tag = String::from("<td");
    for (offset, step) in steps[start + 1..].iter().enumerate() {
        tag.push_str(&step.fragment);
        if step.fragment.starts_with('>') {
            return Ok((tag, offset + 2));
        }
        // 片段流里出现另一个单元格起始或行边界，说明 `>` 丢了 —— 结构串已经被截断
        // （例如撞上 `Loop` 的 500 步上限）。报错而不是猜一个闭合标签。
        if step.is_cell_start
            || step.fragment.starts_with("<tr>")
            || step.fragment.starts_with("</tr>")
        {
            return Err(RapidOcrError::Decode(format!(
                "table cell tag starting at `<td` was never closed by a `>` fragment; the \
                 structure sequence looks truncated (got `{tag}`)"
            )));
        }
    }
    Err(RapidOcrError::Decode(format!(
        "table cell tag `{tag}` runs off the end of the structure sequence without a `>` fragment"
    )))
}

/// 按「每层的子下标」把节点插到树上，返回它在父节点里的下标。
fn push_child(root: &mut StructureNode, stack: &[usize], node: StructureNode) -> usize {
    let parent = node_at_mut(root, stack);
    parent.children.push(node);
    parent.children.len() - 1
}

/// 按「每层的子下标」取节点。`stack` 必须全部来自 [`push_child`]（内部不变量）。
fn node_at<'a>(root: &'a StructureNode, stack: &[usize]) -> &'a StructureNode {
    let mut node = root;
    for &index in stack {
        node = &node.children[index];
    }
    node
}

/// [`node_at`] 的可变版本。
fn node_at_mut<'a>(root: &'a mut StructureNode, stack: &[usize]) -> &'a mut StructureNode {
    let mut node = root;
    for &index in stack {
        node = &mut node.children[index];
    }
    node
}

/// `</tr>` 是否关闭 `<tr>`。用于把闭合片段钉到正确的开标签上：没有这一层校验，
/// `</td>` 会把外层 `<tr>` 关掉、`</tr>` 会把 `<thead>` 关掉，而树看起来仍然"平衡"。
fn closes(open_tag: &str, close_fragment: &str) -> bool {
    match close_fragment.strip_prefix("</") {
        Some(name_with_bracket) => open_tag.strip_prefix('<') == Some(name_with_bracket),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实模型的 48 个片段（与 `super::super::metadata` 的 `REAL_METADATA` 同源）。
    fn real_vocabulary() -> StructureVocabulary {
        let tokens: Vec<String> = vec![
            "<thead>", "</thead>", "<tbody>", "</tbody>", "<tr>", "</tr>", "<td", ">", "</td>",
        ]
        .into_iter()
        .map(str::to_string)
        .chain((2..=20).map(|n| format!(" colspan=\"{n}\"")))
        .chain((2..=20).map(|n| format!(" rowspan=\"{n}\"")))
        .chain(std::iter::once("<td></td>".to_string()))
        .collect();
        assert_eq!(tokens.len(), 48);
        StructureVocabulary::new(tokens).expect("real vocabulary must build")
    }

    /// 把片段序列编译成 `TokenStep`，跳过 `sos`/`eos` 的规则由 `decode_token_steps` 负责，
    /// 这里直接从片段构造，方便构造非法序列。
    fn steps_of(vocabulary: &StructureVocabulary, fragments: &[&str]) -> Vec<TokenStep> {
        fragments
            .iter()
            .enumerate()
            .map(|(step_index, fragment)| {
                let class_index = vocabulary
                    .tokens()
                    .iter()
                    .position(|token| token == fragment)
                    .map(|index| index + 1)
                    .unwrap_or_else(|| panic!("`{fragment}` is not in the vocabulary"));
                TokenStep {
                    step_index,
                    class_index,
                    fragment: (*fragment).to_string(),
                    score: 1.0,
                    is_cell_start: vocabulary.is_cell_start(fragment),
                }
            })
            .collect()
    }

    fn polygon(x0: f32, y0: f32, x1: f32, y1: f32) -> [f32; 8] {
        [x0, y0, x1, y0, x1, y1, x0, y1]
    }

    /// 测试用的结构解码入口。这些用例测的是标签拼接、网格与 bbox，终止方式一律取
    /// **正常退出**；截断那条路径由 `truncation_is_carried_separately_from_the_tokens`
    /// 单独覆盖 —— 否则每个用例都要重复一个与它无关的参数。
    fn structure_of(
        vocabulary: &StructureVocabulary,
        steps: &[TokenStep],
        polygons: &[[f32; 8]],
    ) -> Result<StructureResult> {
        tokens_to_structure(vocabulary, steps, StructureTermination::Eos, polygons)
    }

    /// 参考实现用「八个坐标全为 0」表示该单元格没有框（`filter_blank_bbox` 的判据）。
    fn blank_polygon() -> [f32; 8] {
        [0.0; 8]
    }

    // ---- 词表与类别空间 ----

    #[test]
    fn special_tokens_surround_the_vocabulary() {
        let vocabulary = real_vocabulary();
        assert_eq!(vocabulary.class_count(), 50);
        assert_eq!(vocabulary.sos_class_index(), 0);
        assert_eq!(vocabulary.eos_class_index(), 49);
    }

    #[test]
    fn class_index_one_maps_to_the_first_fragment() {
        let vocabulary = real_vocabulary();
        assert_eq!(vocabulary.token_at(1).unwrap(), "<thead>");
        assert_eq!(vocabulary.token_at(7).unwrap(), "<td");
        assert_eq!(vocabulary.token_at(8).unwrap(), ">");
        assert_eq!(vocabulary.token_at(9).unwrap(), "</td>");
        assert_eq!(vocabulary.token_at(10).unwrap(), " colspan=\"2\"");
        assert_eq!(vocabulary.token_at(48).unwrap(), "<td></td>");
    }

    #[test]
    fn special_classes_cannot_be_mapped_to_fragments() {
        let vocabulary = real_vocabulary();
        let sos = vocabulary
            .token_at(0)
            .expect_err("sos must not map to a fragment");
        assert!(sos.to_string().contains("sos"), "{sos}");
        let eos = vocabulary
            .token_at(49)
            .expect_err("eos must not map to a fragment");
        assert!(eos.to_string().contains("eos"), "{eos}");
        let out_of_range = vocabulary.token_at(50).expect_err("50 is out of range");
        assert!(
            out_of_range.to_string().contains("out of range"),
            "{out_of_range}"
        );
    }

    #[test]
    fn vocabulary_rejects_reserved_special_tokens() {
        let error = StructureVocabulary::new(vec!["<td".to_string(), "sos".to_string()])
            .expect_err("`sos` must not appear in the metadata vocabulary");
        assert!(error.to_string().contains("reserved"), "{error}");
        assert!(StructureVocabulary::new(Vec::new()).is_err());
    }

    // ---- `sos`/`eos` 边界 ----

    #[test]
    fn eos_terminates_the_sequence() {
        let vocabulary = real_vocabulary();
        // <td + > + </td> 然后 eos 然后一堆本该被忽略的 token。
        let argmaxed = vec![(7, 0.9), (8, 0.8), (9, 0.7), (49, 0.6), (7, 0.5), (8, 0.4)];
        let (steps, termination) = decode_token_steps(&vocabulary, &argmaxed).unwrap();
        assert_eq!(termination, StructureTermination::Eos);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2].fragment, "</td>");
    }

    #[test]
    fn eos_at_step_zero_does_not_terminate() {
        let vocabulary = real_vocabulary();
        // 参考实现用 `idx > 0` 而不是 `initialized`，所以第 0 步的 eos 只是被跳过。
        let argmaxed = vec![(49, 0.6), (7, 0.9), (8, 0.8)];
        let (steps, termination) = decode_token_steps(&vocabulary, &argmaxed).unwrap();
        assert_eq!(termination, StructureTermination::Exhausted);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].fragment, "<td");
        assert_eq!(steps[1].fragment, ">");
    }

    #[test]
    fn sos_is_skipped_wherever_it_appears() {
        let vocabulary = real_vocabulary();
        let argmaxed = vec![(0, 0.5), (7, 0.9), (0, 0.4), (8, 0.8), (0, 0.3)];
        let (steps, _termination) = decode_token_steps(&vocabulary, &argmaxed).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(steps.iter().all(|step| step.class_index != 0));
    }

    // ---- argmax ----

    #[test]
    fn argmax_picks_the_largest_class_and_keeps_its_score() {
        // 3 步 × 4 类
        let logits = vec![
            0.1, 0.9, 0.0, 0.0, // step 0 → class 1
            0.5, 0.2, 0.7, 0.1, // step 1 → class 2
            0.0, 0.0, 0.0, 0.0, // step 2 → 全零，取 class 0
        ];
        let result = argmax_structure(&logits, 3).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].0, 1);
        assert!((result[0].1 - 0.9).abs() < 1e-6);
        assert_eq!(result[1].0, 2);
        assert!((result[1].1 - 0.7).abs() < 1e-6);
        assert_eq!(result[2].0, 0);
    }

    #[test]
    fn argmax_breaks_ties_towards_the_lower_class() {
        // 参考实现是 numpy 的 argmax，平局取首个最大值的下标。
        let result = argmax_structure(&[0.5, 0.5, 0.5], 1).unwrap();
        assert_eq!(result[0].0, 0);
    }

    #[test]
    fn argmax_rejects_misaligned_logits() {
        let error = argmax_structure(&[0.1, 0.2, 0.3, 0.4, 0.5], 2)
            .expect_err("5 values cannot split into 2 steps");
        assert!(error.to_string().contains("not a multiple"), "{error}");
        assert!(argmax_structure(&[0.1], 0).is_err());
    }

    #[test]
    fn argmax_rejects_all_negative_infinity() {
        let error = argmax_structure(&[f32::NEG_INFINITY, f32::NEG_INFINITY], 1)
            .expect_err("no finite logit means the tensor was not initialised");
        assert!(error.to_string().contains("finite"), "{error}");
    }

    // ---- 单元格标签拼接 ----

    #[test]
    fn plain_cell_tag_is_assembled_from_fragments() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["<tr>", "<td", ">", "</td>", "</tr>"]);
        let result = structure_of(&vocabulary, &steps, &[polygon(1.0, 2.0, 3.0, 4.0)]).unwrap();
        assert_eq!(result.cells.len(), 1);
        assert_eq!(result.cells[0].tag, "<td>");
        assert_eq!(result.cells[0].rowspan, 1);
        assert_eq!(result.cells[0].colspan, 1);
    }

    #[test]
    fn spanned_cell_tag_carries_both_attributes() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(
            &vocabulary,
            &[
                "<tr>",
                "<td",
                " colspan=\"2\"",
                " rowspan=\"3\"",
                ">",
                "</td>",
                "</tr>",
            ],
        );
        let result = structure_of(&vocabulary, &steps, &[polygon(0.0, 0.0, 2.0, 2.0)]).unwrap();
        assert_eq!(result.cells[0].tag, "<td colspan=\"2\" rowspan=\"3\">");
        assert_eq!(result.cells[0].colspan, 2);
        assert_eq!(result.cells[0].rowspan, 3);
    }

    #[test]
    fn empty_cell_is_a_single_fragment() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["<tr>", "<td></td>", "</tr>"]);
        let result = structure_of(&vocabulary, &steps, &[blank_polygon()]).unwrap();
        assert_eq!(result.cells[0].tag, "<td></td>");
        // 全零占位框被丢弃，但槽位本身保留（见 `tokens_to_structure` 的文档）。
        assert!(result.cells[0].bbox.is_none());
    }

    #[test]
    fn unclosed_cell_tag_is_rejected() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["<tr>", "<td", " colspan=\"2\"", "</tr>"]);
        let error = structure_of(&vocabulary, &steps, &[blank_polygon()])
            .expect_err("a `<td` without `>` must not be silently closed");
        assert!(error.to_string().contains("never closed"), "{error}");
    }

    #[test]
    fn cell_tag_running_off_the_end_is_rejected() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["<td", " colspan=\"2\""]);
        let error = structure_of(&vocabulary, &steps, &[blank_polygon()]).expect_err("truncated");
        assert!(error.to_string().contains("runs off the end"), "{error}");
    }

    #[test]
    fn cell_box_count_must_match_cell_start_count() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["<td", ">", "</td>", "<td", ">", "</td>"]);
        let error = structure_of(&vocabulary, &steps, &[blank_polygon()])
            .expect_err("two cell starts need two boxes");
        assert!(error.to_string().contains("not aligned"), "{error}");
    }

    #[test]
    fn truncation_is_carried_separately_from_the_tokens() {
        // 同一串 token，唯一区别是"有没有见过 eos"。序列本身**分辨不出**这两者，
        // 所以终止方式必须作为独立输入传进来、独立记在结果上。
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["<tr>", "<td", ">", "</tr>"]);
        let complete = tokens_to_structure(
            &vocabulary,
            &steps,
            StructureTermination::Eos,
            &[blank_polygon()],
        )
        .unwrap();
        let cut_off = tokens_to_structure(
            &vocabulary,
            &steps,
            StructureTermination::Exhausted,
            &[blank_polygon()],
        )
        .unwrap();

        assert_eq!(complete.cells, cut_off.cells);
        assert_eq!(complete.coverage, cut_off.coverage);
        assert!(!complete.is_truncated());
        assert!(cut_off.is_truncated());
    }

    // ---- bbox ----

    #[test]
    fn polygon_collapses_to_its_axis_aligned_box() {
        let bbox = polygon_to_bbox(&[4.0, 1.0, 9.0, 1.0, 9.0, 7.0, 4.0, 7.0]);
        assert_eq!(bbox, [4.0, 1.0, 9.0, 7.0]);
    }

    #[test]
    fn blank_polygon_needs_all_eight_zero_coordinates() {
        assert!(is_blank_polygon(&[0.0; 8]));
        // 退化的 `(0,0,0,0)` 框**不是**占位框 —— 参考实现的判据是八个坐标，不是面积。
        assert!(!is_blank_polygon(&[
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0001
        ]));
    }

    #[test]
    fn denormalization_multiplies_by_the_original_crop_size() {
        let shape = TableShape::new(300.0, 600.0, 488.0).unwrap();
        assert!((shape.ratio - 488.0 / 600.0).abs() < 1e-6);
        let polygon = shape.denormalize_polygon(&[0.5, 0.5, 1.0, 0.5, 1.0, 1.0, 0.5, 1.0]);
        assert_eq!(
            polygon,
            [300.0, 150.0, 600.0, 150.0, 600.0, 300.0, 300.0, 300.0]
        );
    }

    #[test]
    fn table_shape_rejects_degenerate_sizes() {
        assert!(TableShape::new(0.0, 10.0, 488.0).is_err());
        assert!(TableShape::new(10.0, 10.0, 0.0).is_err());
    }

    // ---- 坐标系比例（E4 的钉死对象，这里先把可证部分固化） ----

    #[test]
    fn wide_tables_leave_the_horizontal_axis_unscaled() {
        // docs/07 §12 R1：w >= h 时 w_ratio == 1。这条反直觉，单独断言。
        let ratios = rescale_ratios(600.0, 300.0, 488.0);
        assert!((ratios.w_ratio - 1.0).abs() < 1e-6, "{ratios:?}");
        assert!((ratios.h_ratio - 2.0).abs() < 1e-6, "{ratios:?}");
    }

    #[test]
    fn tall_tables_leave_the_vertical_axis_unscaled() {
        let ratios = rescale_ratios(300.0, 600.0, 488.0);
        assert!((ratios.h_ratio - 1.0).abs() < 1e-6, "{ratios:?}");
        assert!((ratios.w_ratio - 2.0).abs() < 1e-6, "{ratios:?}");
    }

    #[test]
    fn square_tables_are_unscaled_on_both_axes() {
        let ratios = rescale_ratios(400.0, 400.0, 488.0);
        assert!((ratios.w_ratio - 1.0).abs() < 1e-6);
        assert!((ratios.h_ratio - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rescale_scales_the_two_axes_independently() {
        let ratios = rescale_ratios(600.0, 300.0, 488.0);
        let polygon = rescale_polygon(&[1.0, 2.0, 3.0, 2.0, 3.0, 4.0, 1.0, 4.0], ratios);
        assert_eq!(polygon, [1.0, 4.0, 3.0, 4.0, 3.0, 8.0, 1.0, 8.0]);
    }

    #[test]
    fn the_two_coordinate_stages_compose_into_one_uniform_scale() {
        // 参考实现分两步：先按原图 crop 尺寸反归一化（`_bbox_decode`），再乘
        // `rescale_cell_bboxes` 的 `w_ratio`/`h_ratio`。两步复合后 x 与 y 的总因子都是
        // `max_len / ratio = max(h, w)` —— 即结果是**各向同性**缩放，而"横表 w_ratio == 1"
        // 只是中间量的性质。
        //
        // 这是 E4 的锚点：E4 若实测出别的坐标系，说明参考实现另有语义（例如 det 侧
        // 用同一份 crop 做了别的换算），而不是"我们代数算错了"。
        let normalized = [0.25, 0.5, 0.75, 0.5, 0.75, 0.9, 0.25, 0.9];
        for &(height, width) in &[
            (300.0f32, 600.0f32),
            (600.0, 300.0),
            (400.0, 400.0),
            (100.0, 977.0),
        ] {
            let max_side = height.max(width);
            let shape = TableShape::new(height, width, 488.0).unwrap();
            let ratios = rescale_ratios(width, height, 488.0);
            let composed = rescale_polygon(&shape.denormalize_polygon(&normalized), ratios);
            for (index, (got, unit)) in composed.iter().zip(normalized.iter()).enumerate() {
                let want = unit * max_side;
                assert!(
                    (got - want).abs() < 1e-3,
                    "coordinate {index} composed to {got}, expected {want} \
                     (normalized {unit} * max({height}, {width}))"
                );
            }
        }
    }

    // ---- 逻辑网格 ----

    #[test]
    fn a_simple_grid_walks_left_to_right() {
        let (logic, grid, coverage) = decode_logic_points(&[vec![(1, 1), (1, 1), (1, 1)]]).unwrap();
        assert_eq!(
            logic,
            vec![
                LogicCell {
                    row_start: 0,
                    row_end: 0,
                    col_start: 0,
                    col_end: 0
                },
                LogicCell {
                    row_start: 0,
                    row_end: 0,
                    col_start: 1,
                    col_end: 1
                },
                LogicCell {
                    row_start: 0,
                    row_end: 0,
                    col_start: 2,
                    col_end: 2
                },
            ]
        );
        assert_eq!(grid, GridSize { rows: 1, cols: 3 });
        assert_eq!(
            coverage,
            GridCoverage {
                cells: 3,
                covered: 3
            }
        );
        assert!(coverage.is_complete());
    }

    #[test]
    fn colspan_pushes_later_cells_to_the_right() {
        let (logic, grid, coverage) = decode_logic_points(&[vec![(1, 2), (1, 1)]]).unwrap();
        assert_eq!(logic[0].col_end, 1);
        assert_eq!(logic[1].col_start, 2);
        assert_eq!(grid.cols, 3);
        assert_eq!(coverage.holes(), 0);
    }

    #[test]
    fn rowspan_blocks_the_cell_below_it() {
        // 2×2 网格：第一行是「跨两行的左上格 + 右上格」，第二行只剩左下这一个位置。
        let (logic, grid, coverage) =
            decode_logic_points(&[vec![(2, 1), (1, 1)], vec![(1, 1)]]).unwrap();
        assert_eq!(
            logic[0],
            LogicCell {
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 0
            }
        );
        assert_eq!(
            logic[1],
            LogicCell {
                row_start: 0,
                row_end: 0,
                col_start: 1,
                col_end: 1
            }
        );
        // 第三个格子必须跳过被占用的 (1,0)，落到 (1,1)。
        assert_eq!(
            logic[2],
            LogicCell {
                row_start: 1,
                row_end: 1,
                col_start: 1,
                col_end: 1
            }
        );
        assert_eq!(grid, GridSize { rows: 2, cols: 2 });
        assert_eq!(
            coverage,
            GridCoverage {
                cells: 4,
                covered: 4
            }
        );
    }

    #[test]
    fn a_declared_span_that_does_not_tile_the_grid_leaves_a_hole() {
        // 这是"模型把 span 给错/截断到词表内"的最小复现：
        // 第一行声明「跨 3 行的左格 + 一个右格」，第二行只有一个格子，
        // 于是 max_rows=3（来自 rowspan）而 (2,1) 没有任何人覆盖。
        let (logic, grid, coverage) =
            decode_logic_points(&[vec![(3, 1), (1, 1)], vec![(1, 1)]]).unwrap();
        assert_eq!(logic.len(), 3);
        assert_eq!(grid, GridSize { rows: 3, cols: 2 });
        assert_eq!(
            coverage,
            GridCoverage {
                cells: 6,
                covered: 5
            }
        );
        assert_eq!(coverage.holes(), 1);
        assert!(!coverage.is_complete());
    }

    #[test]
    fn overlapping_rectangles_are_counted_once_not_by_span_area() {
        // `covered` 必须按**互异**格子数算：行 0 的右格跨 2 行占掉 (0,1)/(1,1)，
        // 行 1 的左格又跨 2 列占掉 (1,0)/(1,1) —— 两者在 (1,1) 重叠。
        // `Σ rowspan × colspan` 会得到 1 + 2 + 2 = 5，而真实被覆盖的格子只有 4 个，
        // 网格恰好是满的。这正是"不能把 covered 换成 span 求和"的判据。
        let (logic, grid, coverage) =
            decode_logic_points(&[vec![(1, 1), (2, 1)], vec![(1, 2)]]).unwrap();
        assert_eq!(logic.len(), 3);
        assert_eq!(grid, GridSize { rows: 2, cols: 2 });
        assert_eq!(
            coverage,
            GridCoverage {
                cells: 4,
                covered: 4
            }
        );
        let span_area: usize = logic
            .iter()
            .map(|cell| (cell.row_end - cell.row_start + 1) * (cell.col_end - cell.col_start + 1))
            .sum();
        assert_eq!(span_area, 5, "the rectangles really do overlap");
        assert_ne!(coverage.covered, span_area);
        assert!(coverage.is_complete());
    }

    #[test]
    fn single_diagonal_cell_yields_a_one_by_one_grid() {
        let (logic, grid, coverage) = decode_logic_points(&[vec![(1, 1)]]).unwrap();
        assert_eq!(logic.len(), 1);
        assert_eq!(grid, GridSize { rows: 1, cols: 1 });
        assert!(coverage.is_complete());
    }

    #[test]
    fn zero_spans_are_rejected() {
        let error = decode_logic_points(&[vec![(0, 1)]]).expect_err("spans are 1-based");
        assert!(error.to_string().contains("at least 1"), "{error}");
    }

    #[test]
    fn empty_cell_list_yields_an_empty_grid() {
        let (logic, grid, coverage) = decode_logic_points(&[]).unwrap();
        assert!(logic.is_empty());
        assert_eq!(grid, GridSize { rows: 0, cols: 0 });
        // 空网格没有洞：0 个格子、0 个被覆盖 —— 这是"没有表格"而不是"表格有缺口"。
        assert_eq!(
            coverage,
            GridCoverage {
                cells: 0,
                covered: 0
            }
        );
        assert!(coverage.is_complete());
    }

    // ---- 标签解析 ----

    #[test]
    fn cell_tag_parsing_reads_spans_in_either_order() {
        assert_eq!(parse_cell_tag("<td>").unwrap(), (None, None));
        assert_eq!(parse_cell_tag("<td></td>").unwrap(), (None, None));
        assert_eq!(
            parse_cell_tag("<td colspan=\"2\">").unwrap(),
            (None, Some(2))
        );
        assert_eq!(
            parse_cell_tag("<td rowspan=\"3\">").unwrap(),
            (Some(3), None)
        );
        assert_eq!(
            parse_cell_tag("<td rowspan=\"3\" colspan=\"4\">").unwrap(),
            (Some(3), Some(4))
        );
    }

    #[test]
    fn cell_tag_parsing_rejects_malformed_attributes() {
        let error = parse_cell_tag("<tr>").expect_err("only `<td` tags are cells");
        assert!(error.to_string().contains("<td"), "{error}");
        assert!(parse_cell_tag("<td colspan=\"0\">").is_err());
        assert!(parse_cell_tag("<td colspan=\"\">").is_err());
        assert!(parse_cell_tag("<td colspan>").is_err());
    }

    // ---- 结构树 ----

    #[test]
    fn tree_nests_rows_under_the_table_and_cells_under_rows() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(
            &vocabulary,
            &[
                "<tr>", "<td", ">", "</td>", "<td", ">", "</td>", "</tr>", "<tr>", "<td", ">",
                "</td>", "</tr>",
            ],
        );
        let boxes = [
            polygon(0.0, 0.0, 1.0, 1.0),
            polygon(1.0, 0.0, 2.0, 1.0),
            polygon(0.0, 1.0, 1.0, 2.0),
        ];
        let result = structure_of(&vocabulary, &steps, &boxes).unwrap();

        assert_eq!(result.root.tag, "<table>");
        assert_eq!(result.root.children.len(), 2);
        assert_eq!(result.root.children[0].tag, "<tr>");
        assert_eq!(result.root.children[0].children.len(), 2);
        assert_eq!(result.root.children[0].children[0].tag, "<td>");
        assert_eq!(result.root.children[0].children[0].cell_index, Some(0));
        assert_eq!(result.root.children[1].children[0].cell_index, Some(2));
        assert_eq!(result.cells.len(), 3);
        assert_eq!(result.grid, GridSize { rows: 2, cols: 2 });
    }

    #[test]
    fn thead_and_tbody_are_preserved_in_the_tree() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(
            &vocabulary,
            &[
                "<thead>", "<tr>", "<td", ">", "</td>", "</tr>", "</thead>", "<tbody>", "</tbody>",
            ],
        );
        let result = structure_of(&vocabulary, &steps, &[polygon(0.0, 0.0, 1.0, 1.0)]).unwrap();
        assert_eq!(result.root.children[0].tag, "<thead>");
        assert_eq!(result.root.children[0].children[0].tag, "<tr>");
        assert_eq!(result.root.children[1].tag, "<tbody>");
    }

    #[test]
    fn an_unmatched_close_tag_is_rejected() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["</tr>", "<tr>"]);
        let error = structure_of(&vocabulary, &steps, &[]).expect_err("unmatched `</tr>`");
        assert!(
            error.to_string().contains("without a matching open tag"),
            "{error}"
        );
    }

    #[test]
    fn unclosed_tags_at_the_end_are_rejected() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["<tr>", "<td", ">", "</td>"]);
        let error = structure_of(&vocabulary, &steps, &[polygon(0.0, 0.0, 1.0, 1.0)]).unwrap_err();
        assert!(error.to_string().contains("unclosed"), "{error}");
    }

    #[test]
    fn a_stray_close_fragment_is_rejected() {
        let vocabulary = real_vocabulary();
        let steps = steps_of(&vocabulary, &["<tr>", ">"]);
        let error = structure_of(&vocabulary, &steps, &[]).expect_err("stray `>`");
        assert!(
            error.to_string().contains("neither a cell start"),
            "{error}"
        );
    }

    #[test]
    fn confidence_is_the_mean_of_every_step_score() {
        let vocabulary = real_vocabulary();
        let mut steps = steps_of(&vocabulary, &["<tr>", "<td", ">", "</td>", "</tr>"]);
        for (index, step) in steps.iter_mut().enumerate() {
            step.score = index as f32;
        }
        let result = structure_of(&vocabulary, &steps, &[polygon(0.0, 0.0, 1.0, 1.0)]).unwrap();
        // 0+1+2+3+4 = 10，除以 5 步 = 2.0
        assert!(
            (result.confidence - 2.0).abs() < 1e-6,
            "{}",
            result.confidence
        );
    }

    #[test]
    fn an_empty_sequence_decodes_to_an_empty_table() {
        let vocabulary = real_vocabulary();
        let result = structure_of(&vocabulary, &[], &[]).unwrap();
        assert_eq!(result.root.tag, "<table>");
        assert!(result.root.children.is_empty());
        assert!(result.cells.is_empty());
        assert_eq!(result.confidence, 0.0);
    }

    #[test]
    fn full_pipeline_from_logits_to_cells() {
        // 端到端：logits → argmax → 片段 → 结构。
        // 目标串：<tr> <td colspan="2"> </td> </tr>
        let vocabulary = real_vocabulary();
        let wanted: Vec<&str> = vec!["<tr>", "<td", " colspan=\"2\"", ">", "</td>", "</tr>"];
        let classes = vocabulary.class_count();
        let mut logits = vec![0.0f32; wanted.len() * classes];
        for (step, fragment) in wanted.iter().enumerate() {
            let class_index = vocabulary
                .tokens()
                .iter()
                .position(|token| token == fragment)
                .unwrap()
                + 1;
            // 让唯一的最大值落在目标类别上。
            for value in logits[step * classes..(step + 1) * classes].iter_mut() {
                *value = 0.1;
            }
            let slot = &mut logits[step * classes + class_index];
            *slot = 0.9;
        }

        let argmaxed = argmax_structure(&logits, wanted.len()).unwrap();
        let (steps, _termination) = decode_token_steps(&vocabulary, &argmaxed).unwrap();
        assert_eq!(steps.len(), wanted.len());
        let result = structure_of(&vocabulary, &steps, &[polygon(0.0, 0.0, 10.0, 5.0)]).unwrap();

        assert_eq!(result.cells.len(), 1);
        assert_eq!(result.cells[0].tag, "<td colspan=\"2\">");
        assert_eq!(result.cells[0].colspan, 2);
        assert_eq!(result.cells[0].bbox, Some([0.0, 0.0, 10.0, 5.0]));
        assert_eq!(result.grid, GridSize { rows: 1, cols: 2 });
    }
}

/// 真实模型 golden 测试（doc 09 §5）。
///
/// 上面的 `tests` 模块用手工构造的 logits 验证**语义**；这里用
/// `tools/build_table_golden.py` 从官方参考实现（`refer/RapidTable-main` 里的
/// PaddleOCR `TableLabelDecode` 与 `TableMatch.decode_one_logic_points`）逐字导出的
/// **真实模型输出**验证**契约**：类别布局、token 流、逐 cell 的 8 点坐标、逻辑网格、
/// 置信度、结构树形状。
///
/// manifest 里每个单元格存了三套坐标，三套都比：
///
/// - `raw_normalized` —— 模型 `scale_0.tmp_0` 的原始输出，未做任何变换
/// - `after_bbox_decode` —— `× [原图 w, 原图 h]`（参考实现的 `_bbox_decode`）
/// - `after_rescale` —— 再 `× [w_ratio, h_ratio]`（参考实现的 `rescale_cell_bboxes`）
///
/// 只比最终值无法区分"两步各自正确"与"两步的误差互相抵消"，所以三套都要比。
///
/// **容差来源**：两侧都是 `f32` 乘法、顺序完全相同（先 `w` 再 `w_ratio`），
/// 实测**逐位一致**（偏差 `0.0`）；[`COORD_TOLERANCE`] 只为吸收 JSON 十进制往返。
/// 唯一的例外见 [`UNIFORM_SCALE_TOLERANCE`]（代数等价但算式不同，差一个 ulp）。
/// 完整的实测记录见 `docs/09-table-recognition-execution.md` §5.4。
#[cfg(test)]
mod golden_tests {
    use super::*;
    use crate::test_support::fixture_dir;
    use serde_json::Value;
    use std::path::{Path, PathBuf};

    /// 逐阶段复现（`_bbox_decode` → `rescale_cell_bboxes`）与 golden 的绝对容差。
    ///
    /// 实测偏差 **0.0**（逐位一致）：两侧都是"每个坐标一次 `f32` 乘法"，IEEE-754 下
    /// 结果唯一。给到 `1e-6` 只是为了吸收 JSON 十进制往返（`float(np.float32)` → 文本
    /// → `f32`）可能带来的末位差异。参照：在 371 这个量级上一个 `f32` ulp ≈ 3.05e-5，
    /// 所以 `1e-6` 仍然**远小于一个 ulp** —— 这条断言实际上是在要求逐位相等。
    const COORD_TOLERANCE: f32 = 1e-6;
    /// "参考实现两步复合 ≡ 各向同性缩放 `max(h, w)`"这条**代数**不变量的绝对容差。
    ///
    /// 它与逐阶段复现不是同一个算式（一次乘法 vs 两次相乘），实测最大偏差
    /// **3.0517578125e-05**，恰好是 371 量级上的一个 `f32` ulp —— 纯舍入差、
    /// 不含任何逻辑含义，因此容差必须比 [`COORD_TOLERANCE`] 宽。
    const UNIFORM_SCALE_TOLERANCE: f32 = 1e-4;
    /// 置信度与逐 cell 分数的绝对容差。
    const CONFIDENCE_TOLERANCE: f32 = 1e-5;
    /// 参考实现的 `max_len`；表长边会被缩放到这个值然后补零到 `488×488`。
    const MAX_LEN: f32 = 488.0;

    fn golden_dir() -> PathBuf {
        fixture_dir("table-golden")
    }

    fn manifest() -> Value {
        let path = golden_dir().join("manifest.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        serde_json::from_str(&text).expect("golden manifest is valid json")
    }

    /// 与 `src/formula/preprocess.rs` 里那份 golden 读取器同一套 npy 解析（`<f4`、C 序）。
    fn load_npy(path: &Path) -> (Vec<usize>, Vec<f32>) {
        let bytes =
            std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert!(
            bytes.starts_with(b"\x93NUMPY"),
            "{} is not a npy file",
            path.display()
        );
        let major = bytes[6];
        let header_len = match major {
            1 => u16::from_le_bytes([bytes[8], bytes[9]]) as usize,
            2 | 3 => u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            other => panic!("unsupported npy version {other}"),
        };
        let header_start = if major == 1 { 10 } else { 12 };
        let header = std::str::from_utf8(&bytes[header_start..header_start + header_len])
            .expect("npy header should be utf-8");
        assert!(header.contains("'descr': '<f4'"), "golden npy must be f32");
        assert!(
            header.contains("'fortran_order': False"),
            "golden npy must be C order"
        );

        let shape_start = header
            .find("'shape': (")
            .expect("npy header should contain shape")
            + "'shape': (".len();
        let shape_end = header[shape_start..]
            .find(')')
            .expect("npy shape should end")
            + shape_start;
        let shape: Vec<usize> = header[shape_start..shape_end]
            .split(',')
            .filter_map(|part| {
                let part = part.trim();
                if part.is_empty() {
                    None
                } else {
                    Some(part.parse().expect("npy shape item"))
                }
            })
            .collect();

        let data_start = header_start + header_len;
        let values: Vec<f32> = bytes[data_start..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        assert_eq!(
            values.len(),
            shape.iter().product::<usize>(),
            "npy payload length disagrees with its header"
        );
        (shape, values)
    }

    fn number(value: &Value, context: &str) -> f64 {
        value
            .as_f64()
            .unwrap_or_else(|| panic!("{context} should be a json number"))
    }

    fn count(value: &Value, context: &str) -> usize {
        value
            .as_u64()
            .unwrap_or_else(|| panic!("{context} should be a json integer")) as usize
    }

    /// 把 manifest 里某个键下的 8 点坐标读成逐 cell 的 `[[f32; 8]]`。
    fn reference_polygons(manifest: &Value, key: &str) -> Vec<[f32; 8]> {
        manifest["reference"]["cells"]
            .as_array()
            .expect("golden reference cells")
            .iter()
            .map(|cell| {
                let items = cell[key]
                    .as_array()
                    .unwrap_or_else(|| panic!("golden cell should carry `{key}`"));
                assert_eq!(items.len(), 8, "golden cell `{key}` should hold 8 values");
                let mut polygon = [0.0f32; 8];
                for (slot, item) in polygon.iter_mut().zip(items) {
                    *slot = number(item, key) as f32;
                }
                polygon
            })
            .collect()
    }

    fn golden_vocabulary(manifest: &Value) -> StructureVocabulary {
        let tokens: Vec<String> = manifest["vocabulary"]["tokens"]
            .as_array()
            .expect("golden vocabulary tokens")
            .iter()
            .map(|token| {
                token
                    .as_str()
                    .expect("vocabulary token should be a json string")
                    .to_string()
            })
            .collect();
        StructureVocabulary::new(tokens).expect("golden vocabulary builds")
    }

    /// 走一遍完整解码：真实模型 logits → argmax → token 步 → 结构结果。
    ///
    /// bbox 的坐标变换**由调用方做** —— 这正是生产路径的形状：`scale_0.tmp_0` 的原始行
    /// → [`TableShape::denormalize_polygon`] → [`rescale_polygon`] → 按 **token 顺序**
    /// 交给 [`tokens_to_structure`]。`step_index` 是这里的关键：`scale_0` 与 `scale_1`
    /// 是逐 step 对齐的两路输出，取框必须用原始 step 下标而不是"第几个单元格"。
    fn decode_golden() -> (Value, Vec<TokenStep>, StructureResult) {
        let manifest = manifest();
        let vocabulary = golden_vocabulary(&manifest);

        let (structure_shape, structure_logits) = load_npy(&golden_dir().join("structure.npy"));
        assert_eq!(
            structure_shape.len(),
            3,
            "structure.npy should be (N, L, C)"
        );
        let steps_count = structure_shape[1];
        assert_eq!(
            structure_shape[2],
            vocabulary.class_count(),
            "model class count disagrees with the golden vocabulary"
        );

        let argmaxed = argmax_structure(&structure_logits, steps_count).expect("golden argmax");
        let (steps, _termination) =
            decode_token_steps(&vocabulary, &argmaxed).expect("golden token steps");

        let (boxes_shape, boxes) = load_npy(&golden_dir().join("boxes.npy"));
        assert_eq!(
            boxes_shape,
            vec![structure_shape[0], steps_count, 8],
            "boxes.npy should be (N, L, 8) and share L with structure.npy"
        );

        let height = number(&manifest["source_image"]["height"], "source height") as f32;
        let width = number(&manifest["source_image"]["width"], "source width") as f32;
        let shape = TableShape::new(height, width, MAX_LEN).expect("golden table shape");
        let ratios = rescale_ratios(width, height, MAX_LEN);

        let cell_polygons: Vec<[f32; 8]> = steps
            .iter()
            .filter(|step| step.is_cell_start)
            .map(|step| {
                let offset = step.step_index * 8;
                let mut raw = [0.0f32; 8];
                raw.copy_from_slice(&boxes[offset..offset + 8]);
                rescale_polygon(&shape.denormalize_polygon(&raw), ratios)
            })
            .collect();

        let result = tokens_to_structure(
            &vocabulary,
            &steps,
            StructureTermination::Eos,
            &cell_polygons,
        )
        .expect("golden structure should decode");
        (manifest, steps, result)
    }

    fn assert_polygon_close(actual: &[f32; 8], expected: &[f32; 8], context: &str) {
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            let delta = (actual - expected).abs();
            assert!(
                delta <= COORD_TOLERANCE,
                "{context}: coordinate {index} differs by {delta} (actual {actual} vs expected \
                 {expected})"
            );
        }
    }

    #[test]
    fn golden_vocabulary_matches_the_reference_class_layout() {
        let manifest = manifest();
        assert_eq!(count(&manifest["vocabulary"]["token_count"], "tokens"), 48);
        assert_eq!(count(&manifest["vocabulary"]["class_count"], "classes"), 50);
        assert_eq!(count(&manifest["vocabulary"]["sos_class_index"], "sos"), 0);
        assert_eq!(count(&manifest["vocabulary"]["eos_class_index"], "eos"), 49);

        let vocabulary = golden_vocabulary(&manifest);
        assert_eq!(vocabulary.tokens().len(), 48);
        assert_eq!(vocabulary.class_count(), 50);
        assert_eq!(vocabulary.sos_class_index(), SOS_CLASS_INDEX);
        assert_eq!(vocabulary.eos_class_index(), 49);
        assert_eq!(eos_class_index(vocabulary.tokens().len()), 49);

        // `sos`/`eos` 是特殊类别，不是可解码片段。
        assert!(vocabulary.token_at(0).is_err());
        assert!(vocabulary.token_at(49).is_err());

        // 单元格起始的判定集合与参考实现 `td_token` 逐字一致。
        assert!(vocabulary.is_cell_start("<td>"));
        assert!(vocabulary.is_cell_start("<td"));
        assert!(vocabulary.is_cell_start("<td></td>"));
        assert!(!vocabulary.is_cell_start(">"));
        assert!(!vocabulary.is_cell_start("</td>"));
        assert!(!vocabulary.is_cell_start("<tr>"));
    }

    #[test]
    fn golden_preprocess_constants_match_the_reference() {
        let manifest = manifest();
        let values: Vec<f32> = manifest["preprocess"]["mean"]
            .as_array()
            .expect("preprocess mean")
            .iter()
            .map(|value| number(value, "mean") as f32)
            .collect();
        assert_eq!(values, vec![0.485, 0.456, 0.406]);
        let values: Vec<f32> = manifest["preprocess"]["std"]
            .as_array()
            .expect("preprocess std")
            .iter()
            .map(|value| number(value, "std") as f32)
            .collect();
        assert_eq!(values, vec![0.229, 0.224, 0.225]);

        let scale = number(&manifest["preprocess"]["scale"], "scale") as f32;
        assert!((scale - 1.0 / 255.0).abs() <= f32::EPSILON);
        assert_eq!(
            number(&manifest["preprocess"]["max_len"], "max_len") as f32,
            MAX_LEN
        );
        // 顺序是"先归一化再补零"：补的零是**归一化空间**的零，不是像素零。
        assert_eq!(
            manifest["preprocess"]["order"].as_str().expect("order"),
            "resize -> normalize -> pad(top-left, zeros) -> to_chw"
        );
        assert_eq!(manifest["input"]["channel_order"].as_str(), Some("BGR"));
        assert_eq!(count(&manifest["input"]["height"], "input height"), 385);
        assert_eq!(count(&manifest["input"]["width"], "input width"), 488);
    }

    #[test]
    fn golden_token_stream_matches_the_reference_decoder() {
        let (manifest, steps, _) = decode_golden();

        let expected: Vec<&str> = manifest["reference"]["tokens"]
            .as_array()
            .expect("reference tokens")
            .iter()
            .map(|token| token.as_str().expect("token should be a string"))
            .collect();
        let fragments: Vec<&str> = steps.iter().map(|step| step.fragment.as_str()).collect();
        assert_eq!(fragments, expected);
        assert_eq!(steps.len(), 114);

        // `step_index` 必须与原始序列一一对应且单调（`sos`/`eos` 占掉的空档被计入），
        // 否则 `scale_0` 的取框会整体错位。
        assert!(
            steps
                .windows(2)
                .all(|pair| pair[0].step_index < pair[1].step_index)
        );
        assert_eq!(steps[0].step_index, 0);

        // 真实模型**不在**下标 0 放 `sos`：第 0 步是 `<tbody>`，`sos` 被排在最后一位。
        // 因此"过滤后的第 n 个"与"原始下标"在这个样本里恰好相等 —— 但契约上必须用后者
        // 取框，否则任何一个 `sos` 位置不同的模型都会静默错位。
        assert_eq!(steps[0].fragment, "<tbody>");
        assert_eq!(steps.last().unwrap().fragment, "</tbody>");

        // 原始 116 步的尾部：`eos` 在下标 114、`sos` 在下标 115。
        // `decode` 的 `if idx > 0 and char_idx == end_idx: break` 在这里正常触发；
        // 若模型把 `eos` 摆在 0，那条 `idx > 0` 的例外就会生效 —— 这正是参考实现里
        // "用 `idx > 0` 代替较新版本的 `initialized` 布尔"那个怪癖所保护的情形。
        let (structure_shape, structure_logits) = load_npy(&golden_dir().join("structure.npy"));
        let raw = argmax_structure(&structure_logits, structure_shape[1]).expect("golden argmax");
        assert_eq!(raw.len(), 116);
        assert_eq!(raw[114].0, eos_class_index(48));
        assert_eq!(raw[115].0, SOS_CLASS_INDEX);
        assert_eq!(raw[0].0, steps[0].class_index);
    }

    #[test]
    fn golden_logic_grid_matches_the_reference_decoder() {
        let (manifest, _, result) = decode_golden();
        let points = manifest["reference"]["logic_points"]
            .as_array()
            .expect("reference logic points");
        assert_eq!(result.cells.len(), points.len());

        for (index, (cell, point)) in result.cells.iter().zip(points).enumerate() {
            assert_eq!(point.as_array().expect("logic point").len(), 4);
            let expected = LogicCell {
                row_start: count(&point[0], "row_start"),
                row_end: count(&point[1], "row_end"),
                col_start: count(&point[2], "col_start"),
                col_end: count(&point[3], "col_end"),
            };
            assert_eq!(cell.logic, expected, "cell {index} logic cell");
        }

        assert_eq!(
            result.grid,
            GridSize {
                rows: count(&manifest["reference"]["grid"]["rows"], "rows"),
                cols: count(&manifest["reference"]["grid"]["cols"], "cols"),
            }
        );
        assert_eq!(result.grid, GridSize { rows: 16, cols: 5 });
    }

    #[test]
    fn golden_cell_boxes_match_the_reference_pipeline_cell_by_cell() {
        let (manifest, steps, result) = decode_golden();

        let raw = reference_polygons(&manifest, "raw_normalized");
        let decoded = reference_polygons(&manifest, "after_bbox_decode");
        let rescaled = reference_polygons(&manifest, "after_rescale");
        assert_eq!(raw.len(), 80);
        assert_eq!(result.cells.len(), rescaled.len());
        assert_eq!(
            count(&manifest["reference"]["dropped_blank_count"], "dropped"),
            0
        );

        let height = number(&manifest["source_image"]["height"], "height") as f32;
        let width = number(&manifest["source_image"]["width"], "width") as f32;
        let shape = TableShape::new(height, width, MAX_LEN).expect("table shape");
        let ratios = rescale_ratios(width, height, MAX_LEN);

        let cell_steps: Vec<&TokenStep> = steps.iter().filter(|step| step.is_cell_start).collect();
        assert_eq!(cell_steps.len(), result.cells.len());

        for (index, cell) in result.cells.iter().enumerate() {
            // 每一段**单独**比：只比最终值无法区分"两步各自正确"与"两步的误差互相抵消"。
            assert_polygon_close(
                &shape.denormalize_polygon(&raw[index]),
                &decoded[index],
                &format!("cell {index} after `_bbox_decode`"),
            );
            assert_polygon_close(
                &rescale_polygon(&decoded[index], ratios),
                &rescaled[index],
                &format!("cell {index} after `rescale_cell_bboxes`"),
            );

            // 生成阶段 / 概率
            let record = &manifest["reference"]["cells"][index];
            assert_eq!(
                cell.tag,
                record["tag"].as_str().expect("cell tag"),
                "cell {index} tag"
            );
            assert_eq!(cell.rowspan, 1, "cell {index} rowspan");
            assert_eq!(cell.colspan, 1, "cell {index} colspan");
            let score = number(&record["score"], "cell score") as f32;
            assert!(
                (cell.score - score).abs() <= CONFIDENCE_TOLERANCE,
                "cell {index} score {} vs {score}",
                cell.score
            );

            // 下标对齐：`cells[i]` 就是参考实现里第 i 个 `<td` 步，取框必须用同一个 step。
            assert_eq!(
                cell_steps[index].step_index,
                count(&record["step"], "cell step"),
                "cell {index} token step"
            );
            assert_eq!(
                record["blank"].as_bool().expect("blank flag"),
                cell.bbox.is_none(),
                "cell {index} blank flag"
            );

            match cell.bbox {
                None => {}
                Some(bbox) => {
                    let expected = polygon_to_bbox(&rescaled[index]);
                    for (axis, (actual, expected)) in bbox.iter().zip(&expected).enumerate() {
                        let delta = (actual - expected).abs();
                        assert!(
                            delta <= COORD_TOLERANCE,
                            "cell {index} bbox axis {axis} differs by {delta} (actual {actual} vs \
                             expected {expected})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn golden_confidence_is_the_mean_of_every_step_score() {
        let (manifest, steps, result) = decode_golden();
        let expected = number(&manifest["reference"]["confidence"], "confidence") as f32;
        assert!(
            (result.confidence - expected).abs() <= CONFIDENCE_TOLERANCE,
            "confidence {} vs {expected}",
            result.confidence
        );

        // 独立复算一次：参考实现为**每一个非跳过步**计分（含 `>` 与 `</td>`），
        // 不是只算结构树遍历路径上被"访问到"的那些步。
        let mean = steps.iter().map(|step| f64::from(step.score)).sum::<f64>() / steps.len() as f64;
        assert!((f64::from(result.confidence) - mean).abs() <= f64::from(CONFIDENCE_TOLERANCE));
    }

    #[test]
    fn golden_structure_tree_has_one_tbody_with_sixteen_rows_of_five_cells() {
        let (_, _, result) = decode_golden();
        assert_eq!(result.root.tag, "<table>");
        assert_eq!(result.root.children.len(), 1);

        let body = &result.root.children[0];
        assert_eq!(body.tag, "<tbody>");
        assert_eq!(body.children.len(), 16);
        for (row_index, row) in body.children.iter().enumerate() {
            assert_eq!(row.tag, "<tr>", "row {row_index}");
            assert_eq!(row.children.len(), 5, "row {row_index}");
            for (cell_index, cell) in row.children.iter().enumerate() {
                assert_eq!(cell.tag, "<td></td>", "row {row_index} cell {cell_index}");
                assert!(cell.children.is_empty(), "cells are leaves");
                assert!(cell.cell_index.is_some());
            }
        }
    }

    /// doc 09 §5 的核心结论：参考实现的 cell 坐标空间是**原图（crop）像素**，
    /// 等于"模型归一化输出 × `max(h, w)`"。
    #[test]
    fn golden_pins_the_coordinate_space_to_the_original_crop() {
        let manifest = manifest();
        let height = number(&manifest["source_image"]["height"], "height") as f32;
        let width = number(&manifest["source_image"]["width"], "width") as f32;
        assert_eq!(
            (height, width),
            (293.0, 371.0),
            "golden source image changed"
        );
        let uniform = height.max(width);
        assert_eq!(uniform, 371.0);

        // 横表：`w_ratio == 1`，纵轴的 `h_ratio` 补上 371/293 —— 两步复合后两个轴的
        // 总因子都等于 `max(h, w)`，也就是说中间的 `_bbox_decode` 被代数抵消。
        let w_ratio = number(&manifest["geometry"]["w_ratio"], "w_ratio") as f32;
        let h_ratio = number(&manifest["geometry"]["h_ratio"], "h_ratio") as f32;
        assert_eq!(w_ratio, 1.0, "横表的 `w_ratio` 应为 1");
        assert!((h_ratio - uniform / height).abs() <= UNIFORM_SCALE_TOLERANCE);
        assert!((width * w_ratio - uniform).abs() <= UNIFORM_SCALE_TOLERANCE);
        assert!((height * h_ratio - uniform).abs() <= UNIFORM_SCALE_TOLERANCE);
        assert_eq!(
            number(&manifest["geometry"]["uniform_scale_max_h_w"], "uniform") as f32,
            uniform
        );

        let raw = reference_polygons(&manifest, "raw_normalized");
        let rescaled = reference_polygons(&manifest, "after_rescale");

        // 不变量：`after_rescale == raw_normalized × max(h, w)`，逐 cell 逐坐标。
        // 这一步是**代数**等价而非同一算式（一次乘法 vs 两次相乘），所以要允许一个 ulp。
        for (index, (raw, rescaled)) in raw.iter().zip(&rescaled).enumerate() {
            for (axis, expected) in raw.iter().enumerate() {
                let expected = expected * uniform;
                let delta = (rescaled[axis] - expected).abs();
                assert!(
                    delta <= UNIFORM_SCALE_TOLERANCE,
                    "cell {index} coordinate {axis}: {} vs {expected} (×max(h,w), delta {delta})",
                    rescaled[axis]
                );
            }
        }

        // 反事实假设"坐标 = 归一化 × 488 画布边长"必须能**被证伪** —— 它会让框跑出原图。
        let canvas_overflow = raw.iter().any(|polygon| {
            polygon.iter().enumerate().any(|(axis, value)| {
                let bound = if axis % 2 == 0 { width } else { height };
                value * MAX_LEN > bound
            })
        });
        assert!(
            canvas_overflow,
            "「×488」假设应当在原图外，否则它与 crop 空间无法区分"
        );

        // 而参考结果本身**全部落在原图内** —— 这是 crop 空间才有的性质。
        for (index, polygon) in rescaled.iter().enumerate() {
            for (axis, value) in polygon.iter().enumerate() {
                let bound = if axis % 2 == 0 { width } else { height };
                assert!(
                    *value >= -COORD_TOLERANCE && *value <= bound + COORD_TOLERANCE,
                    "cell {index} coordinate {axis} = {value} leaves the {bound} crop"
                );
            }
        }
    }
}
