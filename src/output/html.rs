use std::{fmt::Write as _, path::Path};

use crate::{OcrJsonItem, OcrOutput, error::Result};

/// 报告渲染模式（`docs/05` §9.5）。
///
/// 两个模式共享同一套样式与同一个 SVG/列表结构，**唯一**区别是脚本段：
///
/// - [`ReportMode::Full`]：CLI `report --output-dir` 使用。保留内联 `<script>`
///   （点击高亮、公式复制、滚动定位）与**相对路径** `image_href`——报告与同目录的原图
///   一起被写出，因此离线可用（`relative_image_name` 的注释即此意）；
/// - [`ReportMode::Static`]：Web 导出（`GET /api/jobs/{id}/export?format=html`）使用。
///   **正文不含任何 `<script`**，样式内联保留；图片由调用方以 `data:image/png;base64,…`
///   内嵌，因此导出的单文件脱离服务仍可查看。它由导出响应的独立 CSP
///   （`script-src 'none'`；`docs/05` §9.5 第 2 条）背书：静态模式里没有任何脚本可被执行，
///   也就不需要 nonce。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportMode {
    Full,
    Static,
}

/// HTML 报告里的注入点：脚本段**是**两个模式的唯一差异，因此它被抽成一个常量，
/// 由 `render_report` 按模式拼接。`Full` 的产物与引入本模式之前逐字节相同。
const REPORT_SCRIPT: &str = r#"  <script>
    const items = [...document.querySelectorAll('.result')];
    const polygons = [...document.querySelectorAll('polygon')];
    const labels = [...document.querySelectorAll('.polygon-label')];
    const formulaItems = [...document.querySelectorAll('.formulas li')];
    function select(id, source) {
      items.forEach(item => item.classList.toggle('active', item.dataset.target === id));
      polygons.forEach(poly => poly.classList.toggle('active', poly.id === id));
      labels.forEach(label => label.classList.toggle('active', label.dataset.target === id));
      formulaItems.forEach(item => item.classList.toggle('active', item.dataset.target === id));
      if (source === 'image') {
        document.querySelector(`.result[data-target="${id}"]`)?.scrollIntoView({ behavior:'smooth', block:'nearest' });
      }
    }
    items.forEach(item => item.addEventListener('click', () => select(item.dataset.target, 'result')));
    polygons.forEach(poly => poly.addEventListener('click', () => select(poly.id, 'image')));
    formulaItems.forEach(item => item.addEventListener('click', (event) => {
      if (event.target.closest('.copy')) return;
      select(item.dataset.target, 'formula');
    }));
    document.querySelectorAll('.formulas button.copy').forEach(button => {
      button.addEventListener('click', async (event) => {
        event.stopPropagation();
        try { await navigator.clipboard.writeText(button.dataset.latex); button.textContent = '已复制'; }
        catch (error) { button.textContent = '复制失败'; }
      });
    });
  </script>
"#;

/// Builds an offline report shell around an image and OCR polygons.
///
/// `image_href` is intentionally a relative path so the report works offline when the
/// report and the image are written into the same directory (**CLI**); the Web exporter
/// passes a `data:image/png;base64,…` URL instead, because the export document must
/// survive on its own (`docs/05` §9.5 item 5).
///
/// `mode` selects whether the interactive script is emitted at all; see [`ReportMode`].
pub fn render_report(
    title: &str,
    image_href: &str,
    width: u32,
    height: u32,
    items: &[OcrJsonItem],
    timing_summary: &str,
    mode: ReportMode,
) -> Result<String> {
    let mut polygons = String::new();
    let mut rows = String::new();
    let mut formula_rows = String::new();
    let mut formula_count = 0usize;
    let mut table_rows = String::new();
    let mut table_count = 0usize;
    for (index, item) in items.iter().enumerate() {
        let id = format!("ocr-{index}");
        let is_formula = item.kind == "formula";
        let is_table = item.kind == "table";
        if is_formula {
            formula_count += 1;
        }
        if is_table {
            table_count += 1;
        }
        let (points, label_x, label_y) = item
            .box_
            .map(|points| {
                let label_x = points
                    .iter()
                    .map(|point| point[0])
                    .fold(f64::INFINITY, f64::min);
                let label_y = points
                    .iter()
                    .map(|point| point[1])
                    .fold(f64::INFINITY, f64::min);
                let points_text = points
                    .iter()
                    .map(|point| format!("{},{}", point[0], point[1]))
                    .collect::<Vec<_>>()
                    .join(" ");
                (points_text, label_x, label_y)
            })
            .unwrap_or_else(|| (String::new(), 0.0, 0.0));
        if !points.is_empty() {
            let class = if is_formula {
                "polygon formula-polygon"
            } else if is_table {
                "polygon table-polygon"
            } else {
                "polygon"
            };
            writeln!(
                polygons,
                "<polygon id=\"{id}\" data-index=\"{index}\" class=\"{class}\" points=\"{}\" />\n<text class=\"polygon-label\" data-target=\"{id}\" x=\"{label_x}\" y=\"{label_y}\">{}</text>",
                escape_attr(&points),
                index + 1
            )
            .expect("writing a String cannot fail");
        }
        let row_class = if is_formula {
            "result formula-result"
        } else if is_table {
            "result table-result"
        } else {
            "result"
        };
        let kind_badge = if is_formula {
            "<span class=\"kind\">公式</span>"
        } else if is_table {
            "<span class=\"kind\">表格</span>"
        } else {
            ""
        };
        writeln!(
            rows,
            "<button class=\"{row_class}\" data-target=\"{id}\">{kind_badge}<span class=\"index\">{}</span><span class=\"text\">{}</span><span class=\"score\">{:.3}</span></button>",
            index + 1,
            escape_html(&item.txt),
            item.score
        )
        .expect("writing a String cannot fail");

        if let Some(latex) = item.latex.as_ref() {
            // LaTeX 文本与 HTML 属性分别转义：属性还需要额外转义换行/制表符。
            let truncated = item.truncated.unwrap_or(false);
            writeln!(
                formula_rows,
                "<li data-target=\"{id}\"{truncated}><code class=\"latex\">{}</code><button class=\"copy\" data-latex=\"{}\">复制</button></li>",
                escape_html(latex),
                escape_attr(latex),
                truncated = if truncated { " data-truncated=\"true\"" } else { "" }
            )
            .expect("writing a String cannot fail");
        }

        if is_table {
            // `item.table` 缺席时**不做任何猜测**：退化成一个只有标题的条目，而不是
            // 把 `txt`（Markdown）塞进 `<pre>` 冒充表格。
            let meta = match item.table.as_ref() {
                Some(view) => format!(
                    "{} × {} · {} 个单元格 · 置信度 {}{}",
                    view.rows,
                    view.cols,
                    view.cells,
                    escape_html(view.confidence),
                    if view.holes == 0 {
                        String::new()
                    } else {
                        format!(" · {} 个空洞", view.holes)
                    }
                ),
                None => "（缺少表格视图）".to_string(),
            };
            // **两次净化**：`item.table.table_html` 来自引擎出口（已净化），而
            // `render_report` 也会被测试与其它调用方用**手工构造**的 `OcrJsonItem` 调用。
            // 幂等保证第二次不会双重转义（见 `sanitizing_is_idempotent`）。
            let body = match item.table.as_ref() {
                Some(view) => sanitize_table_html(&view.table_html),
                None => String::new(),
            };
            writeln!(
                table_rows,
                "<li data-target=\"{id}\"><div class=\"table-meta\">{meta}</div><div class=\"table-scroll\">{body}</div></li>"
            )
            .expect("writing a String cannot fail");
        }
    }

    let formulas_section = if formula_rows.is_empty() {
        String::new()
    } else {
        format!(
            "<div class=\"formulas\"><h2>公式 ({formula_count})</h2><ol>{formula_rows}</ol></div>"
        )
    };

    // 表格区块**只在存在表格时出现**（样式也一起带上），因此没有表格的报告逐字节不变。
    let tables_section = if table_rows.is_empty() {
        String::new()
    } else {
        format!(
            "<style>{TABLES_STYLE}</style><div class=\"tables\"><h2>表格 ({table_count})</h2><p class=\"table-note\" style=\"margin:0 0 8px;color:#66737d;font-size:12px;\">表格为结构模型预测并经净化后的 HTML；跨行/跨列在 Markdown 里无法表达，Markdown 通道会显式标注。</p><ol>{table_rows}</ol></div>"
        )
    };

    // 表头里的表格计数同样只在存在表格时出现，理由同上。
    let table_meta = if table_count == 0 {
        String::new()
    } else {
        format!(" · {table_count} 个表格")
    };

    // 两个模式的**唯一**差异：`Static` 不发出任何脚本（`docs/05` §9.5 的硬要求）。
    let script = match mode {
        ReportMode::Full => REPORT_SCRIPT,
        ReportMode::Static => "",
    };

    Ok(format!(
        r##"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title}</title>
  <style>
    :root {{ color-scheme: light; --ink:#18212b; --muted:#66737d; --line:#d9e0e6; --accent:#176b87; }}
    * {{ box-sizing:border-box; }}
    body {{ margin:0; background:#f3f5f7; color:var(--ink); font-family:"Segoe UI","Microsoft YaHei",sans-serif; }}
    header {{ padding:18px 24px; border-bottom:1px solid var(--line); background:#fff; }}
    h1 {{ margin:0 0 5px; font-size:20px; }}
    .meta {{ color:var(--muted); font-size:13px; }}
    main {{ display:grid; grid-template-columns:minmax(0,1fr) 330px; gap:18px; padding:18px; align-items:start; }}
    .viewer {{ min-width:0; overflow:hidden; padding:14px; border:1px solid #c9d1d8; background:#dfe4e8; }}
    .image-stage {{ position:relative; width:100%; max-width:{width}px; aspect-ratio:{width} / {height}; margin:0 auto; line-height:0; background:#fff; }}
    .image-stage img {{ display:block; width:100%; height:100%; object-fit:contain; }}
    svg {{ position:absolute; inset:0; width:100%; height:100%; overflow:visible; pointer-events:none; }}
    polygon {{ fill:#176b87; fill-opacity:.08; stroke:#176b87; stroke-width:2; vector-effect:non-scaling-stroke; pointer-events:auto; cursor:pointer; }}
    polygon:hover, polygon.active {{ fill:#f08a24; fill-opacity:.25; stroke:#d46500; stroke-width:3; }}
    .polygon-label {{ fill:#fff; stroke:#176b87; stroke-width:6; paint-order:stroke; stroke-linejoin:round; font:700 22px Consolas,monospace; pointer-events:none; dominant-baseline: hanging; }}
    .polygon-label.active {{ fill:#fff; stroke:#d46500; }}
    aside {{ position:sticky; top:18px; max-height:calc(100vh - 36px); overflow:auto; border:1px solid var(--line); background:#fff; }}
    .summary {{ padding:14px 15px; border-bottom:1px solid var(--line); color:var(--muted); font-size:13px; line-height:1.6; }}
    .results {{ display:flex; flex-direction:column; }}
    .result {{ display:grid; grid-template-columns:30px minmax(0,1fr) 48px; gap:8px; align-items:start; padding:10px 12px; border:0; border-bottom:1px solid #edf0f2; background:#fff; color:var(--ink); text-align:left; font:inherit; cursor:pointer; }}
    .result:hover, .result.active {{ background:#eef6f8; }}
    .kind {{ color:#8a2be2; font:11px "Microsoft YaHei",sans-serif; }}
    .formula-polygon {{ fill:#8a2be2; fill-opacity:.08; stroke:#8a2be2; }}
    .formula-polygon:hover, .formula-polygon.active {{ fill:#f08a24; fill-opacity:.25; stroke:#d46500; }}
    .formulas {{ padding:12px 14px; border-top:1px solid var(--line); }}
    .formulas h2 {{ margin:0 0 8px; font-size:14px; }}
    .formulas ol {{ margin:0; padding-left:18px; }}
    .formulas li {{ margin-bottom:8px; line-height:1.5; }}
    .formulas li.active {{ background:#eef6f8; }}
    .formulas code.latex {{ display:block; overflow-wrap:anywhere; font:12px Consolas,monospace; }}
    .formulas button.copy {{ margin-top:2px; border:1px solid var(--line); background:#fff; cursor:pointer; font:11px inherit; }}
    .index {{ color:var(--accent); font:12px Consolas,monospace; }}
    .text {{ overflow-wrap:anywhere; line-height:1.45; }}
    .score {{ color:var(--muted); font:12px Consolas,monospace; text-align:right; }}
    @media (max-width:900px) {{ main {{ grid-template-columns:1fr; }} aside {{ position:static; max-height:none; }} }}
  </style>
</head>
<body>
  <header><h1>{title}</h1><div class="meta">原图 {width} × {height} px · {count} 个区域 · {formula_count} 个公式{table_meta} · {timing}</div></header>
  <main>
    <section class="viewer"><div class="image-stage"><img src="{image_href}" alt="OCR source"><svg viewBox="0 0 {width} {height}" aria-label="OCR polygons">{polygons}</svg></div></section>
    <aside><div class="summary">点击右侧文本或图片中的框，可检查原位坐标与识别结果是否对应。公式区域渲染为 LaTeX，不经过普通文本通道。</div><div class="results">{rows}</div>{formulas_section}{tables_section}</aside>
  </main>
{script}</body>
</html>
"##,
        title = escape_html(title),
        image_href = escape_attr(image_href),
        width = width,
        height = height,
        count = items.len(),
        formula_count = formula_count,
        timing = escape_html(timing_summary),
        polygons = polygons,
        rows = rows,
        formulas_section = formulas_section,
        tables_section = tables_section,
        table_meta = table_meta,
        script = script,
    ))
}

pub fn relative_image_name(image_path: &Path) -> String {
    let stem = image_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("source");
    let extension = image_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("png");
    format!("{stem}-source.{extension}")
}

/// `OcrOutput` → 报告 HTML（`mode` 决定是否发出脚本段，见 [`ReportMode`]）。
///
/// CLI 的 `report` 子命令固定传 [`ReportMode::Full`]（与引入本参数之前逐字节相同）；
/// `rapidstruct serve` 的导出固定传 [`ReportMode::Static`]。
pub fn render_output_report(
    title: &str,
    image_href: &str,
    output: &OcrOutput,
    timing_summary: &str,
    mode: ReportMode,
) -> Result<String> {
    let items = crate::output::json::to_output_items(output);
    render_report(
        title,
        image_href,
        output.image.original_size.width,
        output.image.original_size.height,
        &items,
        timing_summary,
        mode,
    )
}

pub(crate) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// 转义 HTML 属性值。
///
/// 属性值在 HTML 解析阶段会把换行与制表符规范化为空格，因此属性必须比文本节点
/// 多转义这些字符，否则 LaTeX 内容在往返后会发生变化。
pub(crate) fn escape_attr(value: &str) -> String {
    escape_html(value)
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
        .replace('\t', "&#9;")
}

/// 表格 HTML 的标签白名单（`docs/11` §3 E15.2 / G8）。
///
/// 含 `<html>` / `<body>`：引擎的 `TableOutcome::html` 带着这一层外壳，而官方
/// `metric.py` 的 `pred.xpath('body/table')` 依赖它 —— 净化不能把外壳吃掉。
const ALLOWED_TABLE_TAGS: &[&str] = &[
    "html", "body", "table", "thead", "tbody", "tfoot", "caption", "colgroup", "col", "tr", "th",
    "td", "br", "sub", "sup", "b", "i", "u", "em", "strong", "span", "p", "div", "ul", "ol", "li",
    "code", "small",
];

/// 表格 HTML 的**唯一**净化实现（`docs/11` §3 E15.2）。
///
/// # 它做什么
///
/// 只保留 [`ALLOWED_TABLE_TAGS`] 里的标签与取值合法的 `rowspan`/`colspan`/`align`/`valign`，
/// 丢掉 HTML 注释，其余标记一律从输出里移除。
///
/// **文本节点原样通过**：调用方必须已经用 `TextPolicy::Escape` 转义过单元格文本，
/// 否则这里再转义一次会把 `&lt;` 变成 `&amp;lt;`。这条前置条件由 `Escape` 保证 ——
/// 引擎只在 `TableTextPolicy::Escape`（默认，也是**面向用户**的口径）上调用本函数；
/// `Raw` 是评测口径，它的消费者是 `lxml` 而不是浏览器，净化会改动与 PubTabNet 真值的
/// 可比性，因此不作用于它。
///
/// # 为什么白名单就够
///
/// 危险的不是文本而是**可执行的标记**：`<script>` / `<iframe>` / `<img onerror>` /
/// `style=` / `href="javascript:"` 全都依赖某个标签或某个属性，而它们一个都不在白名单里。
/// 于是即使调用方误用了 `Raw`，注入的标记也只是**不出现**，不会被执行。
///
/// # 幂等
///
/// 输出只含白名单标签与白名单属性，所以再跑一遍结果不变
/// （`sanitizing_is_idempotent` 钉住这一点）。因此"引擎出口净化一次 + 渲染入口再净化一次"
/// 不会产生第二份实现，也不会双重转义。
///
/// # 已知的粗糙处
///
/// 标签以第一个 `>` 结束，因此属性值里的裸 `>`（`<td title="a>b">`）会把标签截短。这会
/// 丢掉一个本来就会被白名单拒绝的属性，并把 `b">` 当成文本 —— 既不改行为也不引入风险。
/// 发射器自己产出的属性只有数字与引号，不会触发这条路径。
pub fn sanitize_table_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some(length) = comment_length(after) {
            rest = &after[length..];
            continue;
        }
        let Some(end) = after.find('>') else {
            // 没有闭合的 `<`：不是标记，按文本处理。
            out.push_str(after);
            rest = "";
            break;
        };
        if let Some(tag) = sanitize_tag(&after[1..end]) {
            out.push_str(&tag);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// `<!-- … -->` 的长度（含两端定界符）。未闭合时吃掉剩余全部。
fn comment_length(input: &str) -> Option<usize> {
    let inner = input.strip_prefix("<!--")?;
    Some(match inner.find("-->") {
        Some(end) => 4 + end + 3,
        None => input.len(),
    })
}

/// 一个标签（不含尖括号）→ 净化后的标记；不在白名单里返回 `None`。
fn sanitize_tag(inner: &str) -> Option<String> {
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    let (closing, body) = match inner.strip_prefix('/') {
        Some(body) => (true, body.trim()),
        None => (false, inner),
    };
    let mut pieces = body.splitn(2, char::is_whitespace);
    let name = pieces.next().unwrap_or("").to_ascii_lowercase();
    if !ALLOWED_TABLE_TAGS.contains(&name.as_str()) {
        return None;
    }
    if closing {
        return Some(format!("</{name}>"));
    }
    let attrs = pieces.next().unwrap_or("").trim_end_matches('/');
    Some(format!("<{name}{}>", sanitize_attrs(attrs)))
}

/// 属性串 → 只保留白名单属性（`rowspan`/`colspan` 必须是 1..=1000 的整数，
/// `align`/`valign` 必须落在固定枚举里）。取值一律重新拼装，因此没有编码歧义。
fn sanitize_attrs(raw: &str) -> String {
    const ALIGNMENTS: [&str; 7] = [
        "left", "center", "right", "top", "middle", "bottom", "justify",
    ];
    let bytes = raw.as_bytes();
    let mut out = String::new();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let name_start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() && bytes[index] != b'=' {
            index += 1;
        }
        let name = raw[name_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let mut value = String::new();
        if index < bytes.len() && bytes[index] == b'=' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() && (bytes[index] == b'"' || bytes[index] == b'\'') {
                let quote = bytes[index];
                index += 1;
                let value_start = index;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
                value = raw[value_start..index].to_string();
                if index < bytes.len() {
                    index += 1;
                }
            } else {
                let value_start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                value = raw[value_start..index].to_string();
            }
        }
        match name.as_str() {
            "rowspan" | "colspan" => {
                // 只接受正整数，且拒绝荒唐的大值（`rowspan="999999"` 会让渲染器炸掉）。
                if let Ok(number) = value.parse::<u32>()
                    && (1..=1000).contains(&number)
                {
                    out.push_str(&format!(" {name}=\"{number}\""));
                }
            }
            "align" | "valign" => {
                let normalized = value.to_ascii_lowercase();
                if ALIGNMENTS.contains(&normalized.as_str()) {
                    out.push_str(&format!(" {name}=\"{normalized}\""));
                }
            }
            _ => {}
        }
    }
    out
}

/// 表格区块自带的样式。
///
/// **刻意不写进 `<style>` 里**：那段样式在每个报告里，改它会让"没有表格的报告"也变字节。
/// 放在这里意味着只有真的存在表格时才多出这几条规则 —— 于是 `docs/11` §3.2 第 ④ 条
/// "既有 text/formula 的输出逐字节不变"在报告通道上是**字面成立**的。
const TABLES_STYLE: &str = r#"
    .table-polygon { fill:#0f8a5f; fill-opacity:.08; stroke:#0f8a5f; }
    .table-polygon:hover, .table-polygon.active { fill:#f08a24; fill-opacity:.25; stroke:#d46500; }
    .tables { padding:12px 14px; border-top:1px solid var(--line); }
    .tables h2 { margin:0 0 8px; font-size:14px; }
    .tables ol { margin:0; padding-left:18px; }
    .tables li { margin-bottom:12px; line-height:1.5; }
    .tables li.active { background:#eef6f8; }
    .tables .table-meta { color:var(--muted); font:11px Consolas,monospace; margin-bottom:4px; }
    .tables .table-scroll { overflow-x:auto; border:1px solid var(--line); }
    .tables table { border-collapse:collapse; background:#fff; font-size:12px; }
    .tables th, .tables td { border:1px solid #d9e0e6; padding:3px 6px; text-align:left; vertical-align:top; }
"#;

#[cfg(test)]
mod tests {
    use super::{
        REPORT_SCRIPT, ReportMode, relative_image_name, render_report, sanitize_table_html,
    };
    use crate::{OcrJsonItem, OcrTableItem};

    /// 两个现有用例共用的样例：一个文本区域 + 一个公式区域（公式用于覆盖 LaTeX 分支）。
    fn sample_items() -> Vec<OcrJsonItem> {
        vec![
            OcrJsonItem {
                box_: Some([[1.0, 2.0], [30.0, 2.0], [30.0, 18.0], [1.0, 18.0]]),
                txt: "<hello> & world".into(),
                score: 0.875,
                kind: "text",
                latex: None,
                eos_index: None,
                truncated: None,
                table: None,
            },
            OcrJsonItem {
                box_: Some([[1.0, 40.0], [80.0, 40.0], [80.0, 70.0], [1.0, 70.0]]),
                txt: "a<b & c".into(),
                score: 0.71,
                kind: "formula",
                latex: Some("a<b & c\nd".into()),
                eos_index: None,
                truncated: Some(true),
                table: None,
            },
        ]
    }

    #[test]
    fn report_contains_original_coordinates_and_escaped_text() {
        let html = render_report(
            "测试 <报告>",
            "source.png",
            640,
            480,
            &[OcrJsonItem {
                box_: Some([[1.0, 2.0], [30.0, 2.0], [30.0, 18.0], [1.0, 18.0]]),
                txt: "<hello> & world".into(),
                score: 0.875,
                kind: "text",
                latex: None,
                eos_index: None,
                truncated: None,
                table: None,
            }],
            "total 12.3 ms",
            ReportMode::Full,
        )
        .expect("report should render");
        assert!(html.contains("points=\"1,2 30,2 30,18 1,18\""));
        assert!(html.contains("class=\"polygon-label\""));
        assert!(html.contains("aspect-ratio:640 / 480"));
        assert!(html.contains("scrollIntoView"));
        assert!(html.contains("&lt;hello&gt; &amp; world"));
        assert!(!html.contains("<hello> & world"));
    }

    #[test]
    fn report_renders_formula_regions_as_escaped_latex() {
        let html = render_report(
            "公式报告",
            "source.png",
            640,
            480,
            &[
                OcrJsonItem {
                    box_: Some([[1.0, 2.0], [30.0, 2.0], [30.0, 18.0], [1.0, 18.0]]),
                    txt: "text".into(),
                    score: 0.9,
                    kind: "text",
                    latex: None,
                    eos_index: None,
                    truncated: None,
                    table: None,
                },
                OcrJsonItem {
                    box_: Some([[1.0, 40.0], [80.0, 40.0], [80.0, 70.0], [1.0, 70.0]]),
                    txt: "a<b & c".into(),
                    score: 0.71,
                    kind: "formula",
                    latex: Some("a<b & c\nd".into()),
                    eos_index: None,
                    truncated: Some(true),
                    table: None,
                },
            ],
            "total 12.3 ms",
            ReportMode::Full,
        )
        .expect("report should render");

        assert!(html.contains("1 个公式"), "html: {html}");
        assert!(html.contains("formula-polygon"), "html: {html}");
        assert!(html.contains("class=\"formulas\""), "html: {html}");
        // LaTeX 文本节点与属性都必须转义，且属性里的换行被编码。
        assert!(html.contains("a&lt;b &amp; c"), "html: {html}");
        assert!(
            html.contains("data-latex=\"a&lt;b &amp; c&#10;d\""),
            "html: {html}"
        );
        assert!(html.contains("data-truncated=\"true\""), "html: {html}");
        assert!(
            !html.contains("a<b & c"),
            "raw LaTeX must never appear unescaped"
        );
    }

    /// §9.5 第 1 条：`Static` 正文**不含任何 `<script`**，且样式、多边形、列表、公式全部保留。
    #[test]
    fn static_mode_emits_no_script_at_all() {
        let full = render_report(
            "静态报告",
            "data:image/png;base64,AAAA",
            640,
            480,
            &sample_items(),
            "total 12.3 ms",
            ReportMode::Full,
        )
        .expect("full report");
        let stat = render_report(
            "静态报告",
            "data:image/png;base64,AAAA",
            640,
            480,
            &sample_items(),
            "total 12.3 ms",
            ReportMode::Static,
        )
        .expect("static report");

        assert!(
            full.contains("<script"),
            "the CLI report must stay interactive"
        );
        assert!(
            !stat.to_lowercase().contains("<script"),
            "the static export must not contain any <script: {stat}"
        );
        assert!(!stat.contains("</script"), "{stat}");
        assert!(!stat.contains("addEventListener"), "{stat}");
        assert!(!stat.contains("navigator.clipboard"), "{stat}");
        // 样式内联保留，结构（多边形 / 列表 / 公式 / 标题）一字不少。
        assert!(stat.contains("<style>"), "{stat}");
        assert!(stat.contains("points=\"1,2 30,2 30,18 1,18\""), "{stat}");
        assert!(stat.contains("class=\"result formula-result\""), "{stat}");
        assert!(stat.contains("class=\"formulas\""), "{stat}");
        assert!(stat.contains("data:image/png;base64,AAAA"), "{stat}");

        // 更强的等式：两个模式**只**差脚本段——把脚本段从 Full 里去掉就是 Static。
        assert_eq!(full.replace(REPORT_SCRIPT, ""), stat);
    }

    /// `Full` 的产物必须与引入 `ReportMode` 之前逐字节相同：脚本段的位置与内容都钉住。
    #[test]
    fn full_mode_still_renders_the_exact_cli_script_block() {
        let html = render_report(
            "钉住",
            "source.png",
            640,
            480,
            &sample_items(),
            "total 1.0 ms",
            ReportMode::Full,
        )
        .expect("full report");
        assert!(
            html.contains(REPORT_SCRIPT),
            "the CLI script block must be intact"
        );
        assert!(REPORT_SCRIPT.starts_with("  <script>\n"));
        assert!(REPORT_SCRIPT.ends_with("  </script>\n"));
        assert!(html.contains("  </main>\n  <script>\n"), "{html}");
        assert!(html.ends_with("  </script>\n</body>\n</html>\n"), "{html}");
        assert_eq!(html.matches("<script").count(), 1, "{html}");
    }

    /// 表格 HTML 的净化（`docs/11` §3 E15.2 / G8）：白名单内的结构一字不改。
    #[test]
    fn sanitize_keeps_the_whitelisted_table_structure() {
        let table = "<html><body><table><thead><tr><td>a</td><td rowspan=\"2\" colspan=\"3\">b</td></tr></thead><tbody><tr><td colspan=\"2\">c</td></tr></tbody></table></body></html>";
        assert_eq!(sanitize_table_html(table), table);
    }

    /// 净化必须**同时**挡住每一类"可执行的标记"。
    ///
    /// 每条用例都是一种真实的注入面：脚本标签、事件处理器属性、`style`、`javascript:`
    /// 协议、被替换的嵌入元素、条件注释。判据是"注入的标记不出现"，而不是"输出里没有
    /// 某些字符串" —— 后者会被编码绕过。
    #[test]
    fn sanitize_removes_every_active_construct() {
        let cases: [(&str, &[&str]); 7] = [
            (
                "<table><tr><td><script>alert(1)</script></td></tr></table>",
                &["<script", "</script"],
            ),
            (
                "<table><tr><td><img src=x onerror=alert(1)></td></tr></table>",
                &["<img", "onerror"],
            ),
            (
                "<table><tr><td style=\"background:url(javascript:alert(1))\">x</td></tr></table>",
                &["style=", "javascript:"],
            ),
            (
                "<table onclick=\"alert(1)\"><tr><td>x</td></tr></table>",
                &["onclick"],
            ),
            (
                "<table><tr><td><a href=\"javascript:alert(1)\">x</a></td></tr></table>",
                &["<a ", "href="],
            ),
            (
                "<table><tr><td><iframe src=\"//evil\"></iframe></td></tr></table>",
                &["<iframe"],
            ),
            (
                "<table><!--[if IE]><script>alert(1)</script><![endif]--><tr><td>x</td></tr></table>",
                &["<!--", "<script"],
            ),
        ];
        for (input, forbidden) in cases {
            let cleaned = sanitize_table_html(input);
            for needle in forbidden {
                assert!(
                    !cleaned.contains(needle),
                    "`{needle}` survived sanitizing of `{input}`:\n{cleaned}"
                );
            }
            // 净化不是"整段丢弃"：表格骨架必须还在，否则它只是把内容删了。
            assert!(
                cleaned.contains("<table>"),
                "sanitizing must keep the table skeleton:\n{cleaned}"
            );
        }
    }

    /// 引擎出口净化一次、渲染入口再净化一次 —— 第二次必须是空操作（`sanitize_table_html`
    /// 的文档承诺）。没有这条，双重净化会悄悄改字节。
    #[test]
    fn sanitizing_is_idempotent() {
        for input in [
            "<html><body><table><tr><td rowspan=\"2\">a</td></tr></table></body></html>",
            "<table><tr><td><script>x</script><b>keep</b></td></tr></table>",
            "<table><tr><td>plain</td></tr></table>",
        ] {
            let once = sanitize_table_html(input);
            assert_eq!(sanitize_table_html(&once), once, "input: {input}");
        }
    }

    /// 官方 `metric.py` 依赖 `pred.xpath('body/table')`，所以 `<html>`/`<body>` 外壳
    /// **不能**被净化吃掉。
    #[test]
    fn sanitize_keeps_the_page_shell_that_the_metric_requires() {
        let cleaned =
            sanitize_table_html("<html><body><table><tr><td>x</td></tr></table></body></html>");
        assert!(cleaned.starts_with("<html><body>"), "{cleaned}");
        assert!(cleaned.ends_with("</body></html>"), "{cleaned}");
        assert!(cleaned.contains("<body><table>"), "{cleaned}");
    }

    /// 表格区域渲染成**真的** `<table>`（而不是被转义的 `<pre>` 块）。
    #[test]
    fn report_renders_a_table_region_as_real_table_markup() {
        let html = render_report(
            "表格报告",
            "source.png",
            640,
            480,
            &[table_item()],
            "total 12.3 ms",
            ReportMode::Full,
        )
        .expect("report should render");

        assert!(html.contains("1 个表格"), "html: {html}");
        assert!(html.contains("class=\"tables\""), "html: {html}");
        assert!(html.contains("table-polygon"), "html: {html}");
        assert!(
            html.contains("class=\"result table-result\""),
            "html: {html}"
        );
        assert!(html.contains("2 × 2 · 4 个单元格 · 置信度 high"), "{html}");
        // 真的表格标签，而不是转义后的文本。
        assert!(html.contains("<td>c11</td>"), "html: {html}");
        assert!(!html.contains("&lt;td&gt;"), "html: {html}");
        assert!(
            html.contains(".tables table { border-collapse:collapse;"),
            "{html}"
        );
        // 表格的 Markdown 仍在结果列表里（`txt` 的口径），但它不是表格渲染的唯一载体。
        assert!(html.contains("| c00 | c01 |"), "html: {html}");
    }

    /// `docs/11` §3.2 第 ④ 条在报告通道上的**字面**形态。
    ///
    /// 一份只有文本与公式的报告，其表头、区块与标记必须与表格通道引入之前逐字节相同：
    /// 表格计数、表格区块、表格样式都只在存在表格时出现。
    #[test]
    fn a_report_without_tables_gains_no_table_markup_at_all() {
        let html = render_report(
            "无表格",
            "source.png",
            640,
            480,
            &sample_items(),
            "total 12.3 ms",
            ReportMode::Full,
        )
        .expect("report should render");
        // 表头原文（没有 ` · 0 个表格` 这种"新增字段就是空"的改动）。
        assert!(
            html.contains("原图 640 × 480 px · 2 个区域 · 1 个公式 · total 12.3 ms"),
            "html: {html}"
        );
        for needle in [
            "个表格",
            "class=\"tables\"",
            "table-polygon",
            "table-result",
            "table-scroll",
        ] {
            assert!(
                !html.contains(needle),
                "a table-free report must not contain `{needle}`"
            );
        }
    }

    /// `kind == "table"` 但缺 `table` 视图：**不许猜**。退化成一个只有标题的条目，
    /// 而不是把 `txt`（Markdown）塞进 `<pre>` 冒充表格。
    #[test]
    fn a_table_item_without_its_view_is_not_disguised_as_a_table() {
        let html = render_report(
            "缺视图",
            "source.png",
            640,
            480,
            &[OcrJsonItem {
                table: None,
                ..table_item()
            }],
            "total 1.0 ms",
            ReportMode::Full,
        )
        .expect("report should render");
        assert!(html.contains("（缺少表格视图）"), "html: {html}");
        assert!(!html.contains("<table"), "html: {html}");
    }

    fn table_item() -> OcrJsonItem {
        OcrJsonItem {
            box_: Some([[1.0, 6.0], [5.0, 6.0], [5.0, 9.0], [1.0, 9.0]]),
            txt: "| c00 | c01 |\n| --- | --- |\n| c10 | c11 |".into(),
            score: 0.71,
            kind: "table",
            latex: None,
            eos_index: None,
            truncated: None,
            table: Some(OcrTableItem {
                rows: 2,
                cols: 2,
                confidence: "high",
                degradations: Vec::new(),
                holes: 0,
                cells: 4,
                table_html:
                    "<table><tr><td>c00</td><td>c01</td></tr><tr><td>c10</td><td>c11</td></tr></table>"
                        .into(),
            }),
        }
    }

    #[test]
    fn image_name_preserves_extension() {
        assert_eq!(
            relative_image_name(std::path::Path::new("case 01.jpg")),
            "case 01-source.jpg"
        );
    }
}
