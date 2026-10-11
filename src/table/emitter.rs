//! 结构树 + 单元格文本 → HTML / Markdown，并给出**结构可信度档位**。
//! **纯函数、零外部资产**。
//!
//! # 与参考实现的关系
//!
//! 语义对应 `refer/RapidTable-main/rapid_table/table_matcher/main.py` 的
//! `TableMatch.get_pred_html`（`main.py:105-165`），但**输入形状不同**：参考实现拿到的是
//! **扁平 token 列表**（`<td …>` 开标签与 `</td>` 闭标签是两个独立条目），因此它靠
//! "`"</td>" not in tag` 来区分开标签与闭标签。本仓库的
//! [`crate::table::decode`] 已经把两者配成 [`StructureNode`]（`cell_index` 非空即为单元格
//! 叶子），所以这里直接按树遍历，不需要再猜哪个片段是闭标签。
//!
//! # 与参考实现的三处**刻意不同**
//!
//! 1. **保留 `<thead>` / `<tbody>`**。参考实现在 `get_pred_html` 末尾用
//!    `filter_elements = ["<thead>", "</thead>", "<tbody>", "</tbody>"]` 把它们删掉
//!    （`main.py:162-164`）。那是**输出策略**，不是解码语义；而 PubTabNet 的真值 HTML
//!    里**含**这两个元素，删掉预测里的它们会凭空制造结构差异。因此这里忠实输出模型
//!    预测到的结构，是否过滤留给评测口径与输出档去决定。
//! 2. **属性从数字重建，不回显标签文本**。参考实现原样吐出 `tag`。这里改为从
//!    [`CellSlot::rowspan`] / [`CellSlot::colspan`]（解码器解析并校验过的正整数）重建
//!    开标签，并**交叉核对**结构节点上记的值 —— 于是"属性只能来自解码器验证过的值"
//!    不再是约定，而是被强制执行的。顺序固定为 `colspan` 在前、`rowspan` 在后：
//!    `metric.py` 的 `load_html_tree` 把属性读进字典，**顺序不影响 TEDS**，固定顺序
//!    只是为了让输出可复现。
//! 3. **文本编码是显式策略**，见 [`TextPolicy`]。
//!
//! 参考实现里那段"多段 OCR 结果用空格拼接 + 拆 `<b>`"的逻辑属于**回填**阶段
//! （多个检测框落在同一个单元格里），不属于发射器：发射器只接收"这个单元格的文本已经
//! 拼好了"。把拼接逻辑塞进发射器会让它在只有一个文本时也走同一段分支，从而在
//! 首尾空格上产生难以解释的差异。
//!
//! # 为什么发射器要报告可信度
//!
//! `docs/07-table-recognition.md` §7.5 与 `docs/09-table-recognition-execution.md` §14.1②：
//! **发射器是唯一能察觉"span 超出词表"的位置**。模型无法输出词表里没有的跨度
//! （SLANet-plus 的 `rowspan`/`colspan` 片段只到 20，而真实数据里有 `colspan="24"`），
//! 于是它会给出一个更小的值，结果是逻辑网格**没有被铺满**。解码器看不到这个症状
//! （它只是照声明的跨度铺格子），只有把格子与网格对起来看才发现。
//!
//! 因此渲染入口返回 [`RenderedTable`]，它**同时**带着文本与可信度：调用方拿不到
//! 文本却不拿可信度。降级不是异常兜底，是**正常路径**（§14.1④）。

use crate::{
    error::{RapidOcrError, Result},
    table::decode::{CellSlot, GridSize, StructureNode, StructureResult},
};

/// 单元格文本的编码策略。
///
/// 这不是"风格选项"，而是**两个目的不同的调用方**：
///
/// - [`Raw`](Self::Raw)：评测口径必须用这个。PubTabNet 的单元格文本本身就是 HTML 片段
///   （`tools/table_detokenize.py` 的字符普查：128,667 个非空单元格里 `<` 与 `>` 各出现
///   4,171 次且**完全成对**），转义会改变与真值的可比性 —— 真值里是 `<sub>`，预测里
///   变成 `&lt;sub&gt;`，TEDS 会把它们判成不同的内容。
/// - [`Escape`](Self::Escape)：用户可见输出必须用这个。单元格文本来自 OCR，未经转义就
///   拼进 HTML 等于让被识别的内容注入标记。
///
/// 一个默认值无法同时满足这两者，所以这里不给默认值、由调用方明确选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextPolicy {
    /// 原样拼接，不转义。
    Raw,
    /// 转义 `&`、`<`、`>`、`"`、`'`。
    Escape,
}

/// 结构可信度档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureConfidence {
    /// 网格铺满、解码正常结束。
    High,
    /// 至少命中一条降级条件（见 [`StructureDegradation`]）。**输出仍然可用**，
    /// 但调用方必须把它标出来，不能当成一张正常表格。
    Low,
}

/// 结构降级的原因。
///
/// 每条都是**可复算**的：输入相同则结论相同，没有"模型觉得"这类主观判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureDegradation {
    /// 逻辑网格没有铺满：`holes` 个格子没有任何单元格覆盖。
    ///
    /// 模型侧最典型的成因是 **span 超出词表** —— 无法表达 `colspan="24"` 时模型给一个
    /// 更小的值，矩形就填不满。注意这里**不去读词表的上限**：`24` 这类值根本没有对应的
    /// token，唯一的证据就是网格有洞。
    GridHoles {
        /// 未被任何单元格覆盖的格子数。
        holes: usize,
    },
    /// 解码步数用尽仍未遇到 `eos`：结构被 `Loop` 的 trip 上限截断，尾部已经丢失。
    Truncated,
}

/// 一次表格渲染的产物。
///
/// 文本与可信度**绑在一起**返回，是为了让"静默产出看似正常的表格"在类型上做不到。
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedTable {
    /// 渲染结果：HTML 页面、`<table>` 片段或 Markdown，取决于调用的是哪个入口。
    pub rendered: String,
    /// 结构可信度档位。
    pub confidence: StructureConfidence,
    /// 命中了的降级原因（[`StructureConfidence::High`] 时为空）。
    pub degradations: Vec<StructureDegradation>,
}

/// 结构树 + 每个单元格的文本 → 完整 HTML 页面（含 `<html><body>` 外壳）。
///
/// `cell_texts` 必须与 [`StructureResult::cells`] **等长且同序**（下标即 `cell_index`）。
/// 结构预测（`TEDS-Struct`）把每个元素传空串即可 —— 传空切片是不允许的，那会让
/// "这个单元格没有文本"与"调用方忘了给文本"变成同一件事。
///
/// `policy` 选 [`TextPolicy::Raw`] 时与真值可比（评测口径），选 [`TextPolicy::Escape`]
/// 时用于用户可见输出。
pub fn emit_html(
    result: &StructureResult,
    cell_texts: &[String],
    policy: TextPolicy,
) -> Result<RenderedTable> {
    validate_cells(result, cell_texts.len())?;
    let mut out = String::with_capacity(64 + cell_texts.iter().map(String::len).sum::<usize>());
    out.push_str("<html><body>");
    emit_node(&result.root, result, cell_texts, policy, &mut out)?;
    out.push_str("</body></html>");
    Ok(rendered(result, out))
}

/// 只输出表格子树（不含 `<html>` / `<body>` 外壳）。
///
/// 用于需要把表格嵌进更大文档的场景；评测口径用 [`emit_html`]，因为官方 `metric.py`
/// 要求 `pred.xpath('body/table')` 非空。
pub fn emit_table_html(
    result: &StructureResult,
    cell_texts: &[String],
    policy: TextPolicy,
) -> Result<RenderedTable> {
    validate_cells(result, cell_texts.len())?;
    let mut out = String::with_capacity(cell_texts.iter().map(String::len).sum::<usize>() + 64);
    emit_node(&result.root, result, cell_texts, policy, &mut out)?;
    Ok(rendered(result, out))
}

/// Markdown 无法表达 `rowspan`/`colspan` 时插入的显式说明。
///
/// 为什么必须说，而不是"调用方自己看 HTML"：**带跨度的表格是良构的** ——
/// `confidence == High`、`degradations` 为空、每个格子都被覆盖。于是只看 `markdown`
/// 的消费者拿不到任何"这里丢了东西"的信号。用 HTML 注释是因为它在渲染后的 Markdown 里
/// 不可见（不污染阅读），但在原文里可见（可被 diff 与测试命中）。
const MARKDOWN_SPAN_NOTE: &str = "<!-- 表格含跨行/跨列，Markdown 无法表达；已按逻辑网格展开，跨过的位置为空位。无损表示见 HTML 字段。 -->";

/// 结构树 + 每个单元格的文本 → Markdown 表格。
///
/// # 为什么必须先把单元格放进逻辑网格
///
/// Markdown 表格是**矩形**的，没有 `rowspan`/`colspan`。因此不能"从拼好的 HTML 做替换"，
/// 只能反过来：把每个单元格的文本放到它在逻辑网格里的**左上角**位置，跨过的其余位置留空，
/// 然后按 `grid.cols` 逐行输出。跨行/跨列的信息**在 Markdown 里丢失**（这是格式的能力
/// 边界，不是实现的缺陷），所以无损输出必须保留 HTML。
///
/// # 跨度不是静默丢失的
///
/// 只要网格里存在 `rowspan > 1` 或 `colspan > 1` 的单元格，输出就会在表格**前后各加一行**
/// [`MARKDOWN_SPAN_NOTE`]。没有跨度时不加任何说明 —— 无病呻吟会让所有正常表格都带上噪声。
///
/// 单元格里的 `|` 会被转义（否则会凭空多出一列），换行变成 `<br>`（Markdown 表格不支持
/// 多行单元格）。其余字符原样保留 —— 与 [`TextPolicy::Raw`] 同理，单元格文本里的 HTML
/// 片段在 Markdown 里是合法且有意保留的。
pub fn emit_markdown(result: &StructureResult, cell_texts: &[String]) -> Result<RenderedTable> {
    validate_cells(result, cell_texts.len())?;
    let grid = result.grid;
    if grid.rows == 0 || grid.cols == 0 {
        return Ok(rendered(result, String::new()));
    }

    let mut matrix = vec![vec![String::new(); grid.cols]; grid.rows];
    for cell in &result.cells {
        let text = text_for(cell.cell_index, cell_texts)?;
        matrix[cell.logic.row_start][cell.logic.col_start] = escape_markdown_cell(text);
    }

    let has_span = result
        .cells
        .iter()
        .any(|cell| cell.rowspan > 1 || cell.colspan > 1);
    let mut out = String::new();
    if has_span {
        out.push_str(MARKDOWN_SPAN_NOTE);
        out.push('\n');
    }
    for (row_index, row) in matrix.iter().enumerate() {
        out.push('|');
        for value in row {
            out.push(' ');
            out.push_str(value);
            out.push_str(" |");
        }
        out.push('\n');
        if row_index == 0 {
            out.push('|');
            for _ in row {
                out.push_str(" --- |");
            }
            out.push('\n');
        }
    }
    if has_span {
        out.push_str(MARKDOWN_SPAN_NOTE);
        out.push('\n');
    }
    Ok(rendered(result, out))
}

/// 这条结构命中了哪些降级条件。空 = 没有降级。
pub fn structure_degradations(result: &StructureResult) -> Vec<StructureDegradation> {
    let mut out = Vec::new();
    if !result.is_grid_complete() {
        out.push(StructureDegradation::GridHoles {
            holes: result.coverage.holes(),
        });
    }
    if result.is_truncated() {
        out.push(StructureDegradation::Truncated);
    }
    out
}

fn rendered(result: &StructureResult, rendered: String) -> RenderedTable {
    let degradations = structure_degradations(result);
    let confidence = if degradations.is_empty() {
        StructureConfidence::High
    } else {
        StructureConfidence::Low
    };
    RenderedTable {
        rendered,
        confidence,
        degradations,
    }
}

fn emit_node(
    node: &StructureNode,
    result: &StructureResult,
    cell_texts: &[String],
    policy: TextPolicy,
    out: &mut String,
) -> Result<()> {
    if let Some(index) = node.cell_index {
        let cell = result.cells.get(index).ok_or_else(|| {
            RapidOcrError::Decode(format!(
                "table structure node refers to cell {index}, but the result only has {} cell(s)",
                result.cells.len()
            ))
        })?;
        check_node_matches_cell(node, cell)?;
        out.push_str(&cell_open_tag(cell, result.grid)?);
        out.push_str(&encode_text(text_for(index, cell_texts)?, policy));
        out.push_str("</td>");
        return Ok(());
    }

    let closing = closing_tag(&node.tag)?;
    out.push_str(&node.tag);
    for child in &node.children {
        emit_node(child, result, cell_texts, policy, out)?;
    }
    out.push_str(&closing);
    Ok(())
}

/// 单元格开标签，**从解码器验证过的 span 数字重建**。
///
/// 两条后置条件，任一不成立就返回错误而不是产出可疑的 HTML：跨度是正整数、
/// 跨度不越出逻辑网格。
fn cell_open_tag(cell: &CellSlot, grid: GridSize) -> Result<String> {
    if cell.rowspan == 0 || cell.colspan == 0 {
        return Err(RapidOcrError::Decode(format!(
            "table cell {} declares rowspan {} / colspan {}; spans are positive integers",
            cell.cell_index, cell.rowspan, cell.colspan
        )));
    }
    if cell.logic.row_start + cell.rowspan > grid.rows
        || cell.logic.col_start + cell.colspan > grid.cols
    {
        return Err(RapidOcrError::Decode(format!(
            "table cell {} spans {}x{} from row {} / column {}, which runs past the {}x{} logical \
             grid",
            cell.cell_index,
            cell.rowspan,
            cell.colspan,
            cell.logic.row_start,
            cell.logic.col_start,
            grid.rows,
            grid.cols
        )));
    }

    let mut tag = String::from("<td");
    if cell.colspan > 1 {
        tag.push_str(&format!(" colspan=\"{}\"", cell.colspan));
    }
    if cell.rowspan > 1 {
        tag.push_str(&format!(" rowspan=\"{}\"", cell.rowspan));
    }
    tag.push('>');
    Ok(tag)
}

/// 结构节点上记的跨度必须与单元格表一致。
///
/// [`crate::table::decode::tokens_to_structure`] 保证两者同源，所以这条在正常路径上永远
/// 成立；它挡的是**手工构造的 [`StructureResult`]**（公开结构体、字段全公开）——
/// 那种输入下"回显节点标签"和"从单元格表重建"会给出不同的 HTML，静默选一边就是数据损坏。
fn check_node_matches_cell(node: &StructureNode, cell: &CellSlot) -> Result<()> {
    let agrees = |declared: Option<usize>, resolved: usize| match declared {
        Some(value) => value == resolved,
        None => resolved <= 1,
    };
    if !agrees(node.rowspan, cell.rowspan) || !agrees(node.colspan, cell.colspan) {
        return Err(RapidOcrError::Decode(format!(
            "table structure cell {} has rowspan {:?} / colspan {:?} in the tree but {} / {} in \
             the cell table; the tree and the cell table disagree",
            cell.cell_index, node.rowspan, node.colspan, cell.rowspan, cell.colspan
        )));
    }
    Ok(())
}

/// 单元格表自身的整体校验：文本数量、跨度合法性、网格边界、起始位置唯一。
///
/// 三个渲染入口都先跑它，于是 HTML 与 Markdown 对同一份结构的接受/拒绝**完全一致** ——
/// 否则"HTML 能出、Markdown 报错"会变成调用方难以解释的差异。
fn validate_cells(result: &StructureResult, text_count: usize) -> Result<()> {
    if text_count != result.cells.len() {
        return Err(RapidOcrError::Decode(format!(
            "table structure has {} cell(s) but {} text(s) were supplied; the emitter cannot \
             guess which cell a text belongs to",
            result.cells.len(),
            text_count
        )));
    }

    let grid = result.grid;
    let mut claimed = vec![false; grid.cell_count()];
    for cell in &result.cells {
        cell_open_tag(cell, grid)?;
        let start = cell.logic.row_start * grid.cols + cell.logic.col_start;
        if claimed[start] {
            return Err(RapidOcrError::Decode(format!(
                "table cell {} starts at row {} / column {}, where another cell already starts; \
                 the logical grid cannot place both",
                cell.cell_index, cell.logic.row_start, cell.logic.col_start
            )));
        }
        claimed[start] = true;
    }
    Ok(())
}

fn text_for(index: usize, cell_texts: &[String]) -> Result<&str> {
    cell_texts.get(index).map(String::as_str).ok_or_else(|| {
        RapidOcrError::Decode(format!(
            "table structure cell {index} is outside the supplied text slice (len {})",
            cell_texts.len()
        ))
    })
}

fn encode_text(text: &str, policy: TextPolicy) -> String {
    match policy {
        TextPolicy::Raw => text.to_string(),
        // 与页面/报告出口共用同一份 HTML 转义（`crate::output::html::escape_html`）：
        // 同形五映射（`& < > " '`），合并前后逐字节相同，避免两处转义漂移。
        TextPolicy::Escape => crate::output::html::escape_html(text),
    }
}

/// Markdown 单元格转义。
fn escape_markdown_cell(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            '\n' => out.push_str("<br>"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// `<tr>` → `</tr>`。标签里带属性时（`<td colspan="2">`）也成立，但单元格不走这条路径。
fn closing_tag(tag: &str) -> Result<String> {
    let inner = tag
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
        .ok_or_else(|| {
            RapidOcrError::Decode(format!(
                "table structure container tag `{tag}` is not of the form `<name>`"
            ))
        })?;
    let name = inner.split_whitespace().next().unwrap_or(inner);
    if name.is_empty() || name.starts_with('/') {
        return Err(RapidOcrError::Decode(format!(
            "table structure container tag `{tag}` has no element name"
        )));
    }
    Ok(format!("</{name}>"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::decode::{GridCoverage, GridSize, LogicCell, StructureTermination};

    fn container(tag: &str, children: Vec<StructureNode>) -> StructureNode {
        StructureNode {
            tag: tag.to_string(),
            rowspan: None,
            colspan: None,
            cell_index: None,
            bbox: None,
            children,
        }
    }

    /// 与 [`spanned`] 配套的标签文本：真实解码器产出的就是这个形状
    /// （`colspan` 在前、`rowspan` 在后，1 的属性不出现）。
    fn cell_tag(rowspan: usize, colspan: usize) -> String {
        let mut tag = String::from("<td");
        if colspan > 1 {
            tag.push_str(&format!(" colspan=\"{colspan}\""));
        }
        if rowspan > 1 {
            tag.push_str(&format!(" rowspan=\"{rowspan}\""));
        }
        tag.push('>');
        tag
    }

    /// 一个单元格：**节点 + 槽位成对给出，跨度写在两处** —— 与
    /// [`crate::table::decode::tokens_to_structure`] 的真实产物同形。测试想制造
    /// "树与单元格表不一致"时，只需改其中一处。
    fn spanned(
        index: usize,
        row: usize,
        col: usize,
        rowspan: usize,
        colspan: usize,
    ) -> (StructureNode, CellSlot) {
        let node = StructureNode {
            tag: cell_tag(rowspan, colspan),
            rowspan: (rowspan > 1).then_some(rowspan),
            colspan: (colspan > 1).then_some(colspan),
            cell_index: Some(index),
            bbox: None,
            children: Vec::new(),
        };
        let cell = CellSlot {
            cell_index: index,
            tag: cell_tag(rowspan, colspan),
            rowspan,
            colspan,
            score: 1.0,
            bbox: None,
            logic: LogicCell {
                row_start: row,
                row_end: row + rowspan - 1,
                col_start: col,
                col_end: col + colspan - 1,
            },
        };
        (node, cell)
    }

    /// 一张 3×2 的表，铺满 6 个格子：
    ///
    /// ```text
    /// <table><thead><tr><td></td><td></td></tr></thead>
    ///        <tbody><tr><td></td><td rowspan="2"></td></tr>
    ///               <tr><td></td></tr></tbody></table>
    /// ```
    ///
    /// 第三行的那个单元格是关键：没有它 `(2,0)` 就是空洞，网格就不完整。
    fn sample() -> StructureResult {
        let (n0, c0) = spanned(0, 0, 0, 1, 1);
        let (n1, c1) = spanned(1, 0, 1, 1, 1);
        let (n2, c2) = spanned(2, 1, 0, 1, 1);
        let (n3, c3) = spanned(3, 1, 1, 2, 1);
        let (n4, c4) = spanned(4, 2, 0, 1, 1);
        StructureResult {
            root: container(
                "<table>",
                vec![
                    container("<thead>", vec![container("<tr>", vec![n0, n1])]),
                    container(
                        "<tbody>",
                        vec![container("<tr>", vec![n2, n3]), container("<tr>", vec![n4])],
                    ),
                ],
            ),
            cells: vec![c0, c1, c2, c3, c4],
            grid: GridSize { rows: 3, cols: 2 },
            coverage: GridCoverage {
                cells: 6,
                covered: 6,
            },
            termination: StructureTermination::Eos,
            confidence: 0.9,
        }
    }

    fn five_texts(values: [&str; 5]) -> Vec<String> {
        values.map(str::to_string).to_vec()
    }

    fn four_texts(values: [&str; 4]) -> Vec<String> {
        values.map(str::to_string).to_vec()
    }

    /// 一张**没有跨度**的 2×2 表：用于断言"没有丢失就不要加说明"。
    fn flat_two_by_two() -> StructureResult {
        let (n0, c0) = spanned(0, 0, 0, 1, 1);
        let (n1, c1) = spanned(1, 0, 1, 1, 1);
        let (n2, c2) = spanned(2, 1, 0, 1, 1);
        let (n3, c3) = spanned(3, 1, 1, 1, 1);
        StructureResult {
            root: container(
                "<table>",
                vec![
                    container("<tr>", vec![n0, n1]),
                    container("<tr>", vec![n2, n3]),
                ],
            ),
            cells: vec![c0, c1, c2, c3],
            grid: GridSize { rows: 2, cols: 2 },
            coverage: GridCoverage {
                cells: 4,
                covered: 4,
            },
            termination: StructureTermination::Eos,
            confidence: 1.0,
        }
    }

    fn blanks() -> Vec<String> {
        five_texts(["", "", "", "", ""])
    }

    fn raw(result: &StructureResult, texts: &[String]) -> RenderedTable {
        emit_html(result, texts, TextPolicy::Raw).expect("must emit")
    }

    // ---- 基本发射 ----

    #[test]
    fn an_empty_cell_round_trips_to_the_self_closing_form() {
        let out = raw(&sample(), &blanks());
        assert_eq!(
            out.rendered,
            "<html><body><table><thead><tr><td></td><td></td></tr></thead>\
             <tbody><tr><td></td><td rowspan=\"2\"></td></tr><tr><td></td></tr></tbody>\
             </table></body></html>"
        );
    }

    #[test]
    fn cell_texts_land_in_their_own_cells() {
        let out = raw(&sample(), &five_texts(["a", "b", "c", "d", "e"]));
        assert!(out.rendered.contains("<td>a</td>"));
        assert!(out.rendered.contains("<td>b</td>"));
        assert!(out.rendered.contains("<td>c</td>"));
        assert!(out.rendered.contains("<td rowspan=\"2\">d</td>"));
        assert!(out.rendered.contains("<td>e</td>"));
    }

    #[test]
    fn a_missing_text_slice_is_rejected_rather_than_padded() {
        let error = emit_html(&sample(), &[], TextPolicy::Raw).expect_err("must reject");
        assert!(
            error.to_string().contains("5 cell(s) but 0 text(s)"),
            "{error}"
        );
    }

    #[test]
    fn inline_markup_in_cell_text_is_not_escaped_under_the_raw_policy() {
        let out = raw(&sample(), &five_texts(["SiO<sub>2</sub>", "", "", "", ""]));
        assert!(
            out.rendered.contains("<td>SiO<sub>2</sub></td>"),
            "the raw policy must preserve markup so TEDS stays comparable: {}",
            out.rendered
        );
    }

    #[test]
    fn the_escape_policy_neutralizes_markup_in_cell_text() {
        let out = emit_html(
            &sample(),
            &five_texts(["<script>alert(1)</script>", "", "", "", ""]),
            TextPolicy::Escape,
        )
        .expect("must emit");
        assert!(
            out.rendered
                .contains("<td>&lt;script&gt;alert(1)&lt;/script&gt;</td>"),
            "the escape policy must neutralize injected markup: {}",
            out.rendered
        );
        assert!(!out.rendered.contains("<script>"));
    }

    #[test]
    fn the_escape_policy_covers_the_five_xml_entities() {
        let out = emit_html(
            &sample(),
            &five_texts(["&<>\"'", "", "", "", ""]),
            TextPolicy::Escape,
        )
        .expect("must emit");
        assert!(
            out.rendered.contains("<td>&amp;&lt;&gt;&quot;&#39;</td>"),
            "{}",
            out.rendered
        );
    }

    #[test]
    fn section_tags_are_kept_because_the_ground_truth_has_them() {
        let out = raw(&sample(), &blanks());
        assert!(out.rendered.contains("<thead>"));
        assert!(out.rendered.contains("<tbody>"));
    }

    #[test]
    fn the_table_only_variant_drops_the_page_shell() {
        let out = emit_table_html(&sample(), &blanks(), TextPolicy::Raw).expect("must emit");
        assert!(out.rendered.starts_with("<table>"));
        assert!(out.rendered.ends_with("</table>"));
        assert!(!out.rendered.contains("<html>"));
    }

    #[test]
    fn a_malformed_container_tag_is_rejected() {
        assert!(closing_tag("<tr").is_err());
        assert!(closing_tag("<>").is_err());
    }

    #[test]
    fn closing_tag_ignores_attributes() {
        assert_eq!(closing_tag("<tr class=\"x\">").unwrap(), "</tr>");
        assert_eq!(closing_tag("<tr>").unwrap(), "</tr>");
    }

    #[test]
    fn a_cell_index_outside_the_cell_table_is_rejected() {
        let mut result = sample();
        result.root = container("<table>", vec![container("<tr>", vec![cell_node(99)])]);
        let error =
            emit_html(&result, &blanks(), TextPolicy::Raw).expect_err("cell 99 does not exist");
        assert!(error.to_string().contains("cell 99"), "{error}");
    }

    fn cell_node(index: usize) -> StructureNode {
        StructureNode {
            tag: "<td>".to_string(),
            rowspan: None,
            colspan: None,
            cell_index: Some(index),
            bbox: None,
            children: Vec::new(),
        }
    }

    // ---- 属性重建 ----

    #[test]
    fn both_attributes_are_emitted_with_colspan_first() {
        let (node, cell) = spanned(0, 0, 0, 2, 3);
        let result = StructureResult {
            root: container("<table>", vec![container("<tr>", vec![node])]),
            cells: vec![cell],
            grid: GridSize { rows: 2, cols: 3 },
            coverage: GridCoverage {
                cells: 6,
                covered: 6,
            },
            termination: StructureTermination::Eos,
            confidence: 1.0,
        };
        let out = emit_table_html(&result, &[String::new()], TextPolicy::Raw).expect("must emit");
        assert_eq!(
            out.rendered,
            "<table><tr><td colspan=\"3\" rowspan=\"2\"></td></tr></table>"
        );
    }

    #[test]
    fn a_tree_that_disagrees_with_the_cell_table_is_rejected() {
        let mut result = sample();
        // 节点说"跨一列"（None），单元格表说"跨两列"。
        result.cells[1].colspan = 2;
        result.cells[1].logic.col_end = 2;
        result.grid.cols = 3;
        result.coverage = GridCoverage {
            cells: 9,
            covered: 7,
        };
        let error = emit_html(&result, &blanks(), TextPolicy::Raw)
            .expect_err("the tree and the cell table disagree");
        assert!(error.to_string().contains("disagree"), "{error}");
    }

    #[test]
    fn a_span_that_runs_past_the_grid_is_rejected() {
        let mut result = sample();
        result.cells[3].rowspan = 99;
        let error = emit_html(&result, &blanks(), TextPolicy::Raw)
            .expect_err("a span past the grid must not be emitted");
        assert!(error.to_string().contains("runs past"), "{error}");
    }

    #[test]
    fn a_zero_span_is_rejected() {
        let mut result = sample();
        result.cells[0].colspan = 0;
        let error = emit_html(&result, &blanks(), TextPolicy::Raw).expect_err("spans are 1-based");
        assert!(error.to_string().contains("positive integers"), "{error}");
    }

    // ---- 降级 ----

    #[test]
    fn a_complete_grid_is_high_confidence() {
        let out = raw(&sample(), &blanks());
        assert_eq!(out.confidence, StructureConfidence::High);
        assert!(out.degradations.is_empty());
    }

    #[test]
    fn grid_holes_lower_the_confidence_instead_of_emitting_a_plausible_table() {
        let mut result = sample();
        // 少覆盖一个格子 —— 正是"span 超出词表"的症状。
        result.coverage = GridCoverage {
            cells: 6,
            covered: 5,
        };
        let out = raw(&result, &blanks());
        assert_eq!(out.confidence, StructureConfidence::Low);
        assert_eq!(
            out.degradations,
            vec![StructureDegradation::GridHoles { holes: 1 }]
        );
        // 文本照样产出（降级是正常路径），但可信度必须跟着一起给出来。
        assert!(out.rendered.contains("<table>"));
    }

    #[test]
    fn truncation_lowers_the_confidence_even_when_the_grid_is_full() {
        let mut result = sample();
        result.termination = StructureTermination::Exhausted;
        let out = raw(&result, &blanks());
        assert_eq!(out.confidence, StructureConfidence::Low);
        assert_eq!(out.degradations, vec![StructureDegradation::Truncated]);
    }

    #[test]
    fn both_degradations_are_reported_together() {
        let mut result = sample();
        result.coverage = GridCoverage {
            cells: 6,
            covered: 4,
        };
        result.termination = StructureTermination::Exhausted;
        let out = raw(&result, &blanks());
        assert_eq!(
            out.degradations,
            vec![
                StructureDegradation::GridHoles { holes: 2 },
                StructureDegradation::Truncated
            ]
        );
    }

    #[test]
    fn an_empty_result_is_high_confidence_rather_than_degraded() {
        let mut result = sample();
        result.cells.clear();
        result.root = container("<table>", Vec::new());
        result.grid = GridSize { rows: 0, cols: 0 };
        result.coverage = GridCoverage {
            cells: 0,
            covered: 0,
        };
        let out = raw(&result, &[]);
        assert_eq!(out.confidence, StructureConfidence::High);
        assert!(out.degradations.is_empty());
    }

    // ---- Markdown ----

    #[test]
    fn markdown_places_cells_into_the_logical_grid() {
        let out = emit_markdown(&flat_two_by_two(), &four_texts(["a", "b", "c", "d"]))
            .expect("must emit");
        assert_eq!(out.rendered, "| a | b |\n| --- | --- |\n| c | d |\n");
    }

    /// 跨度在 Markdown 里必然丢失，所以必须**说**出来。
    ///
    /// 这条测试的关键前提：`sample()` 是**良构**的（网格铺满、`confidence == High`、
    /// `degradations` 为空），也就是说只看 `TableOutcome::confidence`/`degradations`
    /// 的消费者不可能发现跨度没了。说明行因此不是"额外的噪声"，而是唯一的信号。
    #[test]
    fn markdown_warns_explicitly_that_spans_are_lost() {
        let out =
            emit_markdown(&sample(), &five_texts(["a", "b", "c", "d", "e"])).expect("must emit");
        assert_eq!(out.confidence, StructureConfidence::High);
        assert!(
            out.degradations.is_empty(),
            "a spanned table can still be perfectly well-formed: {:?}",
            out.degradations
        );
        let lines: Vec<&str> = out.rendered.lines().collect();
        assert!(lines[0].starts_with("<!--"), "lines: {lines:?}");
        assert!(lines[0].contains("跨行/跨列"), "lines: {lines:?}");
        assert_eq!(
            lines[lines.len() - 1],
            lines[0],
            "the note must close the table too, not only open it"
        );
        assert_eq!(lines[1], "| a | b |", "the note must not break the table");
    }

    #[test]
    fn markdown_says_nothing_extra_when_nothing_is_lost() {
        let out = emit_markdown(&flat_two_by_two(), &four_texts(["a", "b", "c", "d"]))
            .expect("must emit");
        assert!(!out.rendered.contains("<!--"), "{}", out.rendered);
        assert_eq!(out.rendered.lines().count(), 3);
    }

    #[test]
    fn markdown_leaves_spanned_over_positions_empty() {
        // 左格跨两行、右格跨两列：两个跨过的位置在 Markdown 里只能是空格。
        let (n0, c0) = spanned(0, 0, 0, 2, 1);
        let (n1, c1) = spanned(1, 0, 1, 1, 2);
        let (n2, c2) = spanned(2, 1, 1, 1, 2);
        let result = StructureResult {
            root: container(
                "<table>",
                vec![container("<tr>", vec![n0, n1]), container("<tr>", vec![n2])],
            ),
            cells: vec![c0, c1, c2],
            grid: GridSize { rows: 2, cols: 3 },
            coverage: GridCoverage {
                cells: 6,
                covered: 6,
            },
            termination: StructureTermination::Eos,
            confidence: 1.0,
        };
        let texts = ["a", "b", "c"].map(str::to_string).to_vec();
        let out = emit_markdown(&result, &texts).expect("must emit");
        assert_eq!(
            out.rendered,
            format!(
                "{note}\n| a | b |  |\n| --- | --- | --- |\n|  | c |  |\n{note}\n",
                note = super::MARKDOWN_SPAN_NOTE
            )
        );
    }

    #[test]
    fn markdown_escapes_pipes_and_newlines_so_the_table_keeps_its_shape() {
        let out = emit_markdown(
            &flat_two_by_two(),
            &four_texts(["a|b", "line\nbreak", "back\\slash", ""]),
        )
        .expect("must emit");
        let first = out.rendered.lines().next().unwrap();
        assert_eq!(first, "| a\\|b | line<br>break |");
        assert!(out.rendered.contains("back\\\\slash"));
        // 分隔行恰好两列 —— 文本里注入的 `|` 没有撑开表格（否则这里会出现第三个 `---`）。
        assert_eq!(out.rendered.lines().nth(1).unwrap(), "| --- | --- |");
    }

    #[test]
    fn markdown_of_an_empty_grid_is_empty() {
        let mut result = sample();
        result.cells.clear();
        result.grid = GridSize { rows: 0, cols: 0 };
        result.coverage = GridCoverage {
            cells: 0,
            covered: 0,
        };
        let out = emit_markdown(&result, &[]).expect("must emit");
        assert_eq!(out.rendered, "");
        assert_eq!(out.confidence, StructureConfidence::High);
    }

    #[test]
    fn markdown_rejects_a_text_count_that_does_not_match_the_cells() {
        let error = emit_markdown(&sample(), &[]).expect_err("must reject");
        assert!(
            error.to_string().contains("5 cell(s) but 0 text(s)"),
            "{error}"
        );
    }

    #[test]
    fn markdown_rejects_two_cells_claiming_the_same_start() {
        let mut result = sample();
        result.cells[4].logic = result.cells[2].logic;
        let error = emit_markdown(&result, &blanks()).expect_err("two cells cannot share a start");
        assert!(error.to_string().contains("already starts"), "{error}");
    }

    #[test]
    fn html_and_markdown_agree_on_what_they_reject() {
        // 同一份坏结构，两个入口必须给出同一条拒绝理由 —— 否则调用方无法解释
        // "为什么 HTML 能出而 Markdown 报错"。
        let mut result = sample();
        result.cells[0].colspan = 0;
        let html_error = emit_html(&result, &blanks(), TextPolicy::Raw).unwrap_err();
        let md_error = emit_markdown(&result, &blanks()).unwrap_err();
        assert_eq!(html_error.to_string(), md_error.to_string());
    }
}
