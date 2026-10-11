//! 公式识别结果的 Markdown 块与排序入口。
//!
//! 本模块只保留被生产路径实际调用的两项能力：
//!
//! - [`to_markdown_block`]：把 LaTeX 渲染成 `$$...$$` display math。模型输出先做行尾归一化
//!   与首尾裁剪；空 LaTeX 不产生任何输出块；块内 `$$` 被转义为 `\$\$`，保证 display math 的
//!   定界符不被提前闭合；空行被折叠，避免 Markdown 在 math 内分段；`truncated=true` 时在块后
//!   追加不可见的 HTML 注释标记 [`TRUNCATED_MARKER`]，既不污染渲染结果，又能被下游机械识别。
//! - [`order_formula_results`]：按显式区域顺序重排识别结果。
//!
//! JSON 与 HTML 不再在本模块给出独立函数，由页面级输出通道统一负责：JSON 走 `to_output_json`
//! （`FormulaOutcome` 直接序列化 latex/eos_index/truncated/model_id/token_ids），HTML 走
//! `render_report` 的公式分支（LaTeX 文本与 `data-latex` 属性分别转义，并给出 `data-truncated`）。

use crate::{error::Result, formula::recognizer::FormulaRecognition};

/// `truncated=true` 时 Markdown 追加的不可见标记。
pub const TRUNCATED_MARKER: &str = "<!-- formula truncated: no EOS token -->";

/// 把模型输出规范化为可以安全嵌入 display math 的单个块。
fn normalize_display_math(latex: &str) -> String {
    let normalized = latex.replace("\r\n", "\n").replace('\r', "\n");
    // 空行在 Markdown 中结束段落，也会在 LaTeX 数学模式里构成非法的段落分隔，
    // 因此这里直接丢弃空行，只保留单个换行做行分隔。
    let lines: Vec<&str> = normalized
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::trim_end)
        .collect();
    // display math 的定界符是 `$$`；LaTeX 中字面 `$` 应写作 `\$`。
    lines.join("\n").trim().replace("$$", "\\$\\$")
}

/// 把 LaTeX 渲染为 Markdown display math 块。
///
/// 空 LaTeX 返回空字符串（不产生空 math 块）；`truncated = true` 时在块后追加
/// 不可见的 HTML 注释标记，既不污染渲染结果，又能被下游机械识别。
pub fn to_markdown_block(latex: &str, truncated: bool) -> String {
    let latex = normalize_display_math(latex);
    if latex.is_empty() {
        return String::new();
    }
    let block = format!("$$\n{latex}\n$$");
    if truncated {
        format!("{block}\n\n{TRUNCATED_MARKER}")
    } else {
        block
    }
}

/// Reorders formula results by an explicit region index list, e.g. OCR reading order.
pub fn order_formula_results(
    results: Vec<FormulaRecognition>,
    order: &[usize],
) -> Result<Vec<FormulaRecognition>> {
    if order.len() != results.len() {
        return Err(crate::error::RapidOcrError::InvalidInput(format!(
            "formula result order length mismatch: results={}, order={}",
            results.len(),
            order.len()
        )));
    }
    let mut seen = vec![false; results.len()];
    let mut ordered = Vec::with_capacity(results.len());
    for &index in order {
        if index >= results.len() || seen[index] {
            return Err(crate::error::RapidOcrError::InvalidInput(format!(
                "formula result order contains invalid index {index}"
            )));
        }
        seen[index] = true;
        ordered.push(results[index].clone());
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_with(latex: &str, truncated: bool) -> FormulaRecognition {
        FormulaRecognition {
            latex: latex.to_string(),
            token_ids: vec![0, 82, 2],
            eos_index: if truncated { None } else { Some(2) },
            truncated,
            model_id: "test-model".to_string(),
            elapsed_ms: 1.5,
            batch_size: 1,
        }
    }

    fn result() -> FormulaRecognition {
        result_with("x < y & z", false)
    }

    // `to_formula_json` 已删除，其 `include_token_ids` 两态断言迁移到 `src/output/json.rs`
    // 的 `to_output_json`；`to_formula_markdown` 已删除，其 7 条用例改挂生产入口 `to_markdown_block`。
    #[test]
    fn markdown_uses_display_math_and_hides_tokens() {
        let markdown = to_markdown_block("x < y & z", false);
        assert_eq!(markdown, "$$\nx < y & z\n$$");
        assert!(!markdown.contains("82"));
    }

    #[test]
    fn markdown_escapes_inner_display_math_delimiters() {
        let markdown = to_markdown_block("a $$ b", false);
        assert_eq!(markdown, "$$\na \\$\\$ b\n$$");
        // 只有首尾两行可以是未转义的 `$$` 定界符。
        let delimiters = markdown.matches("$$").count();
        assert_eq!(delimiters, 2, "markdown: {markdown}");
    }

    #[test]
    fn markdown_keeps_fences_as_literal_math_content() {
        let markdown = to_markdown_block("```x```", false);
        assert_eq!(markdown, "$$\n```x```\n$$");
        assert!(!markdown.contains("````"), "markdown: {markdown}");
    }

    #[test]
    fn markdown_normalizes_newlines_and_drops_blank_lines() {
        let markdown = to_markdown_block("a\r\n b\r\n\r\n\r\nc", false);
        assert_eq!(markdown, "$$\na\n b\nc\n$$");
    }

    #[test]
    fn markdown_for_empty_latex_emits_nothing() {
        assert_eq!(to_markdown_block("", false), "");
        assert_eq!(to_markdown_block("   \n \n", false), "");
    }

    #[test]
    fn markdown_marks_truncated_results_with_invisible_comment() {
        let markdown = to_markdown_block("\\frac{1}{", true);
        assert!(markdown.starts_with("$$\n\\frac{1}{\n$$"));
        assert!(markdown.ends_with(TRUNCATED_MARKER));
        assert_eq!(
            markdown.matches(TRUNCATED_MARKER).count(),
            1,
            "markdown: {markdown}"
        );

        let complete = to_markdown_block("\\frac{1}{2}", false);
        assert!(!complete.contains(TRUNCATED_MARKER));
    }

    #[test]
    fn markdown_keeps_markdown_special_characters_verbatim() {
        let markdown = to_markdown_block("*a* _b_ [c](d) #e", false);
        assert_eq!(markdown, "$$\n*a* _b_ [c](d) #e\n$$");
    }

    // 被删除的 `to_formula_html` 的 4 条用例迁移到 `src/output/html.rs` 的报告公式分支
    // （生产 HTML 通道从不产出 `data-eos`，那条断言属于死函数独有的行为，不随迁移保留）。

    #[test]
    fn order_results_rejects_invalid_orders() {
        let results = vec![result(), result(), result()];
        let error = order_formula_results(results.clone(), &[0, 0, 2])
            .expect_err("duplicate index must fail");
        assert!(error.to_string().contains("invalid index"));

        let error =
            order_formula_results(results.clone(), &[0, 1]).expect_err("length mismatch must fail");
        assert!(error.to_string().contains("length mismatch"));

        let ordered = order_formula_results(results, &[2, 0, 1]).expect("valid order");
        assert_eq!(ordered.len(), 3);
    }
}
