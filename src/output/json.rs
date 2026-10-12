use serde::Serialize;

use crate::{OcrOutput, error::Result};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OcrJsonItem {
    #[serde(rename = "box", skip_serializing_if = "Option::is_none")]
    pub box_: Option<[[f64; 2]; 4]>,
    /// 区域的**唯一文本表示**：文本区域是 CTC 文本，公式区域是 LaTeX，表格区域是 Markdown。
    ///
    /// 三种区域都通过这一个字段进入"按顺序拼文本"的消费者（`plain_text` 除外 ——
    /// 它只取 `recognition`，见 `OcrOutput::plain_text`）。
    pub txt: String,
    pub score: f64,
    /// 区域类型：`text`、`formula` 或 `table`。
    pub kind: &'static str,
    /// 公式区域才有的 LaTeX；文本区域为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eos_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// 表格区域才有的紧凑视图；文本与公式区域为 `None`。
    ///
    /// 完整内容仍在 `regions[i].table`（[`crate::TableOutcome`]），这里只是让"只看
    /// `items` 数组"的消费者不必回头再进 `regions` 里翻。`markdown` **不在这里重复** ——
    /// 它就是 `txt`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<OcrTableItem>,
}

/// 表格区域在 `items` 数组里的紧凑视图。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OcrTableItem {
    pub rows: usize,
    pub cols: usize,
    /// `high` 或 `low`（[`crate::TableStructureConfidence::as_str`]）。
    pub confidence: &'static str,
    /// `confidence == "low"` 的原因；空表示良构。
    pub degradations: Vec<crate::TableDegradation>,
    /// 逻辑网格里没被任何单元格覆盖的格子数。
    pub holes: usize,
    /// 单元格数，与 `TableOutcome::cells.len()` 相同。
    pub cells: usize,
    /// **只有 `<table>` 外壳**的 HTML（`TableOutcome::table_html`）；带
    /// `<html><body>` 包装的完整变体在 `regions[i].table.html`。
    pub table_html: String,
}

fn polygon_box(region: &crate::OcrRegion) -> Option<[[f64; 2]; 4]> {
    region.polygon.map(|polygon| {
        polygon
            .points
            .map(|point| [point[0] as f64, point[1] as f64])
    })
}

/// 文本、公式与表格区域按 region 顺序统一为输出项。
///
/// 三类区域的 `txt` 都是它们**唯一的文本表示**：文本是 CTC 文本，公式是 LaTeX，表格是
/// Markdown。因此普通文本消费者不会丢失公式或表格内容，也不会把它们伪装成 CTC 文本；
/// 判定"是不是真的识别出了文字"要看 `region.recognition`，不是 `txt` 非空。
pub fn to_output_items(output: &OcrOutput) -> Vec<OcrJsonItem> {
    output
        .regions
        .iter()
        .filter_map(|region| {
            if let Some(table) = region.table.as_ref() {
                return Some(OcrJsonItem {
                    box_: polygon_box(region),
                    txt: table.markdown.clone(),
                    score: region
                        .detection
                        .as_ref()
                        .map(|detection| detection.score as f64)
                        .unwrap_or(0.0),
                    kind: "table",
                    latex: None,
                    eos_index: None,
                    truncated: None,
                    table: Some(OcrTableItem {
                        rows: table.rows,
                        cols: table.cols,
                        confidence: table.confidence.as_str(),
                        degradations: table.degradations.clone(),
                        holes: table.holes,
                        cells: table.cells.len(),
                        table_html: table.table_html.clone(),
                    }),
                });
            }
            if let Some(formula) = region.formula.as_ref() {
                return Some(OcrJsonItem {
                    box_: polygon_box(region),
                    txt: formula.latex.clone(),
                    score: region
                        .detection
                        .as_ref()
                        .map(|detection| detection.score as f64)
                        .unwrap_or(0.0),
                    kind: "formula",
                    latex: Some(formula.latex.clone()),
                    eos_index: formula.eos_index,
                    truncated: Some(formula.truncated),
                    table: None,
                });
            }
            let recognition = region.recognition.as_ref()?;
            Some(OcrJsonItem {
                box_: polygon_box(region),
                txt: recognition.text.clone(),
                score: recognition.score as f64,
                kind: "text",
                latex: None,
                eos_index: None,
                truncated: None,
                table: None,
            })
        })
        .collect()
}

pub fn to_output_json(output: &OcrOutput) -> Result<Value> {
    let mut value = serde_json::to_value(output)
        .map_err(|error| crate::error::RapidOcrError::InvalidInput(error.to_string()))?;
    if let Value::Object(object) = &mut value {
        object.insert(
            "text".into(),
            Value::String(output.plain_text(crate::TextOrder::Reading)),
        );
        object.insert(
            "items".into(),
            serde_json::to_value(to_output_items(output))
                .map_err(|error| crate::error::RapidOcrError::InvalidInput(error.to_string()))?,
        );
        // 公式 LaTeX 单独给出，避免调用方必须从 items 里再筛一遍 kind。
        object.insert(
            "formulas".into(),
            serde_json::to_value(
                output
                    .formula_latex(crate::TextOrder::Reading)
                    .into_iter()
                    .map(|(region_id, latex)| {
                        serde_json::json!({ "region": region_id, "latex": latex })
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| crate::error::RapidOcrError::InvalidInput(error.to_string()))?,
        );
        // 表格同理，但字段更多：`html` / `table_html` / `markdown` 是同一份结构的三种呈现，
        // 全部给出以免调用方自己去猜该用哪个。
        object.insert(
            "tables".into(),
            serde_json::to_value(
                output
                    .tables(crate::TextOrder::Reading)
                    .into_iter()
                    .map(|(region_id, table)| {
                        serde_json::json!({
                            "region": region_id,
                            "rows": table.rows,
                            "cols": table.cols,
                            "confidence": table.confidence.as_str(),
                            "degradations": table.degradations,
                            "holes": table.holes,
                            "cells": table.cells.len(),
                            "html": table.html,
                            "table_html": table.table_html,
                            "markdown": table.markdown,
                            "model_id": table.model_id,
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| crate::error::RapidOcrError::InvalidInput(error.to_string()))?,
        );
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{to_output_items, to_output_json};
    use crate::{
        CoordinateSpace, EngineInfo, FormulaOutcome,
        GenericProviderPreference as ProviderPreference, ImageInfo, InputTimings, OcrOutput,
        OcrRegion, OcrTimings, Polygon, ProviderInfo, ProviderResolutionInfo, RecognitionOutcome,
        RegionKind, RegionSource, ResolvedProvider, StageReport, StageReports, StageState,
        TableOutcome, TableStructureConfidence,
    };

    #[test]
    fn output_items_empty_for_a_page_with_no_regions() {
        assert_eq!(to_output_items(&output_with(Vec::new())), Vec::new());
    }

    /// 迁移自被删除的 `to_formula_json`（`include_token_ids` 两态）：改挂生产入口 `to_output_json`。
    ///
    /// 与旧实现的区别是**如实**的：`FormulaOutcome.token_ids` 没有 `skip_serializing_if`，所以在
    /// 真实序列化里两态表现为"数组"与"null"（而非键缺失）；`eos_index`/`truncated`/`model_id`
    /// 一并钉在 `regions[i].formula` 上 —— 这正是被删函数当年从未接入的那条真实路径。
    #[test]
    fn formula_json_token_ids_are_serialized_only_when_present() {
        let formula_region = |token_ids: Option<Vec<i64>>| OcrRegion {
            source: RegionSource::Detected { detector_index: 0 },
            kind: RegionKind::Formula,
            polygon: Some(Polygon {
                points: [[1.0, 6.0], [5.0, 6.0], [5.0, 9.0], [1.0, 9.0]],
            }),
            detection: Some(crate::DetectionOutcome { score: 0.71 }),
            classification: None,
            recognition: None,
            formula: Some(FormulaOutcome {
                latex: "x < y & z".into(),
                eos_index: Some(2),
                truncated: false,
                model_id: "test-model".into(),
                token_ids,
            }),
            table: None,
        };

        let document =
            to_output_json(&output_with(vec![formula_region(None)])).expect("json document");
        let formula = &document["regions"][0]["formula"];
        assert_eq!(formula["latex"], "x < y & z");
        assert_eq!(formula["eos_index"], 2);
        assert_eq!(formula["truncated"], false);
        assert_eq!(formula["model_id"], "test-model");
        assert_eq!(formula["token_ids"], serde_json::Value::Null);

        let document = to_output_json(&output_with(vec![formula_region(Some(vec![0, 82, 2]))]))
            .expect("json document");
        assert_eq!(
            document["regions"][0]["formula"]["token_ids"],
            serde_json::json!([0, 82, 2])
        );
    }

    #[test]
    fn formula_region_uses_latex_and_stays_out_of_plain_text() {
        let output = output_with(vec![
            OcrRegion::text(
                RegionSource::Detected { detector_index: 0 },
                Some(Polygon {
                    points: [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]],
                }),
                None,
                None,
                Some(RecognitionOutcome {
                    text: "ok".into(),
                    score: 0.9,
                    words: None,
                }),
            ),
            OcrRegion {
                source: RegionSource::Detected { detector_index: 0 },
                kind: RegionKind::Formula,
                polygon: Some(Polygon {
                    points: [[1.0, 6.0], [5.0, 6.0], [5.0, 9.0], [1.0, 9.0]],
                }),
                detection: Some(crate::DetectionOutcome { score: 0.71 }),
                classification: None,
                recognition: None,
                formula: Some(FormulaOutcome {
                    latex: "\\frac{a}{b}".into(),
                    eos_index: Some(3),
                    truncated: false,
                    model_id: "pp_formulanet_plus_m".into(),
                    token_ids: None,
                }),
                table: None,
            },
        ]);

        let items = to_output_items(&output);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, "text");
        assert_eq!(items[0].latex, None);
        assert_eq!(items[1].kind, "formula");
        assert_eq!(items[1].txt, "\\frac{a}{b}");
        assert_eq!(items[1].latex.as_deref(), Some("\\frac{a}{b}"));
        assert_eq!(items[1].truncated, Some(false));

        let document = to_output_json(&output).expect("json document");
        assert_eq!(document["text"], "ok");
        assert_eq!(document["formulas"][0]["latex"], "\\frac{a}{b}");
        assert_eq!(document["regions"][1]["kind"], "formula");
        assert_eq!(document["regions"][1]["formula"]["eos_index"], 3);
        assert_eq!(output.len(), 2, "formula regions are recognition results");
        assert_eq!(output.text_len(), 1);
        assert!(!output.is_empty(), "a formula-only page is not empty");
    }

    #[test]
    fn formula_only_output_is_not_empty() {
        let output = output_with(vec![OcrRegion {
            source: RegionSource::Input,
            kind: RegionKind::Formula,
            polygon: Some(Polygon {
                points: [[1.0, 1.0], [9.0, 1.0], [9.0, 4.0], [1.0, 4.0]],
            }),
            detection: None,
            classification: None,
            recognition: None,
            formula: Some(FormulaOutcome {
                latex: "x".into(),
                eos_index: Some(1),
                truncated: false,
                model_id: "m".into(),
                token_ids: None,
            }),
            table: None,
        }]);
        output
            .validate()
            .expect("formula-only output must validate");
        assert_eq!(output.len(), 1);
        assert_eq!(output.text_len(), 0);
        assert!(!output.is_empty());
        assert!(output.plain_text(crate::TextOrder::Reading).is_empty());
    }

    #[test]
    fn invalid_region_kinds_are_rejected_by_validate() {
        let mut output = output_with(vec![OcrRegion {
            source: RegionSource::Input,
            kind: RegionKind::Formula,
            polygon: None,
            detection: None,
            classification: None,
            recognition: Some(RecognitionOutcome {
                text: "leaked".into(),
                score: 1.0,
                words: None,
            }),
            formula: None,
            table: None,
        }]);
        assert!(
            output.validate().is_err(),
            "formula region without formula outcome must be rejected"
        );
        output.regions[0].formula = Some(FormulaOutcome {
            latex: "x".into(),
            eos_index: None,
            truncated: true,
            model_id: "m".into(),
            token_ids: None,
        });
        assert!(
            output.validate().is_err(),
            "formula region carrying CTC text must be rejected"
        );
    }

    /// 表格区域的三条线契约（`docs/11` §2.1 E14.1）。
    ///
    /// 每一条都对应一个**具体的错误**而不是"字段缺失"：没有 table outcome 是空表格，
    /// 带 CTC 文本是伪造了"识别出来的文本"，带公式结果是两个域串了线。
    #[test]
    fn the_table_region_arms_are_enforced_by_validate() {
        let table = || TableOutcome {
            html: "<html><body><table></table></body></html>".into(),
            table_html: "<table></table>".into(),
            markdown: String::new(),
            rows: 0,
            cols: 0,
            confidence: TableStructureConfidence::High,
            degradations: Vec::new(),
            holes: 0,
            cells: Vec::new(),
            source_line_indices: Vec::new(),
            model_id: "slanet-plus".into(),
        };
        let region = |kind, recognition, formula, table_outcome| OcrRegion {
            source: RegionSource::Input,
            kind,
            polygon: None,
            detection: None,
            classification: None,
            recognition,
            formula,
            table: table_outcome,
        };

        // ① 正常的表格区域必须通过。
        output_with(vec![region(RegionKind::Table, None, None, Some(table()))])
            .validate()
            .expect("a well-formed table region must validate");

        // ② 表格区域没有 table outcome。
        let error = output_with(vec![region(RegionKind::Table, None, None, None)])
            .validate()
            .expect_err("a table region without an outcome must be rejected");
        assert!(
            error.to_string().contains("table"),
            "the message must name the table outcome: {error}"
        );

        // ③ 表格区域带 CTC 文本。
        assert!(
            output_with(vec![region(
                RegionKind::Table,
                Some(RecognitionOutcome {
                    text: "leaked".into(),
                    score: 1.0,
                    words: None,
                }),
                None,
                Some(table()),
            )])
            .validate()
            .is_err(),
            "a table region carrying CTC text must be rejected"
        );

        // ④ 表格区域带公式结果。
        assert!(
            output_with(vec![region(
                RegionKind::Table,
                None,
                Some(FormulaOutcome {
                    latex: "x".into(),
                    eos_index: None,
                    truncated: false,
                    model_id: "m".into(),
                    token_ids: None,
                }),
                Some(table()),
            )])
            .validate()
            .is_err(),
            "a table region carrying a formula outcome must be rejected"
        );

        // ⑤ 文本区域不得夹带 table outcome（反向对照，防止 ② 的检查被写成"只要有就行"）。
        assert!(
            output_with(vec![region(
                RegionKind::Text,
                Some(RecognitionOutcome {
                    text: "ok".into(),
                    score: 1.0,
                    words: None,
                }),
                None,
                Some(table()),
            )])
            .validate()
            .is_err(),
            "a text region carrying a table outcome must be rejected"
        );
    }

    /// E15.1：表格区域在 `items` 里的形态，以及它在三个通道里的可独立断言性。
    ///
    /// 关键判据不是"三个通道都非空"（那种断言对"把 Markdown 当 CTC 文本吐出去"同样成立），
    /// 而是：`kind == "table"`、`txt` 是 Markdown、`table.cells` 与 `regions[i].table.cells.len()`
    /// 相等、而 `plain_text` **不含** Markdown 标记 —— 表格不是"识别出来的文字"。
    #[test]
    fn a_table_region_reports_its_grid_without_faking_ctc_text() {
        let table = table_outcome();
        let output = output_with(vec![
            OcrRegion::text(
                RegionSource::Input,
                None,
                None,
                None,
                Some(RecognitionOutcome {
                    text: "ok".into(),
                    score: 0.9,
                    words: None,
                }),
            ),
            table_region(table.clone()),
        ]);
        output.validate().expect("a table page must validate");
        assert_eq!(output.table_count(), 1);
        assert_eq!(output.text_len(), 1);

        let items = to_output_items(&output);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, "text");
        assert_eq!(
            items[0].table, None,
            "a text item must not carry a table view"
        );
        assert_eq!(items[1].kind, "table");
        assert_eq!(
            items[1].txt, table.markdown,
            "`txt` is the table's markdown"
        );
        assert_eq!(items[1].latex, None);
        let view = items[1]
            .table
            .as_ref()
            .expect("the compact table view must be present");
        assert_eq!(view.rows, table.rows);
        assert_eq!(view.cols, table.cols);
        assert_eq!(view.cells, table.cells.len());
        assert_eq!(view.confidence, "high");
        assert_eq!(view.holes, 0);
        assert!(view.degradations.is_empty());
        assert_eq!(view.table_html, table.table_html);

        let document = to_output_json(&output).expect("json document");
        assert_eq!(document["regions"][1]["kind"], "table");
        assert_eq!(
            document["regions"][1]["table"]["markdown"], table.markdown,
            "the full table outcome stays reachable through `regions`"
        );
        let tables = document["tables"]
            .as_array()
            .expect("`tables` must always be an array");
        assert_eq!(tables.len(), 1);
        assert_eq!(
            tables[0]["region"], 1,
            "the region id must be the item's id"
        );
        assert_eq!(tables[0]["rows"], table.rows);
        assert_eq!(tables[0]["cols"], table.cols);
        assert_eq!(tables[0]["confidence"], "high");
        assert_eq!(tables[0]["cells"], table.cells.len());
        assert_eq!(tables[0]["model_id"], "slanet-plus");
        assert_eq!(tables[0]["html"], table.html);
        assert_eq!(tables[0]["table_html"], table.table_html);
        assert_eq!(tables[0]["markdown"], table.markdown);

        // 表格不贡献纯文本：Markdown 的 `|` 与 `---` 是标记，不是页面上的文字。
        assert_eq!(document["text"], "ok");
        assert!(!document["text"].as_str().unwrap().contains('|'));
    }

    /// 一份**不含表格**的页面，`items` 里文本与公式项的形态必须与 E15 之前逐字节相同。
    ///
    /// 这是 `docs/11` §3.2 第 ④ 条在这里能被拿到的形态：新增的是文档级的 `tables` 数组
    /// （恒存在，照 `formulas` 的先例），而 `items[]` 与 `regions[]` 的既有条目不变。
    #[test]
    fn a_page_without_tables_keeps_the_existing_item_shape() {
        let output = output_with(vec![OcrRegion::text(
            RegionSource::Detected { detector_index: 0 },
            Some(Polygon {
                points: [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]],
            }),
            None,
            None,
            Some(RecognitionOutcome {
                text: "ok".into(),
                score: 0.9,
                words: None,
            }),
        )]);
        let items = serde_json::to_value(to_output_items(&output)).expect("items must serialize");
        assert_eq!(
            items,
            serde_json::json!([{
                "box": [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]],
                "txt": "ok",
                "score": f64::from(0.9f32),
                "kind": "text",
            }]),
            "a text item must serialize to exactly the pre-E15 shape"
        );
        let document = to_output_json(&output).expect("json document");
        assert_eq!(
            document["tables"].as_array().map(Vec::len),
            Some(0),
            "`tables` is always present, like `formulas`"
        );
        assert_eq!(
            document["regions"][0].get("table"),
            None,
            "an absent table outcome must not be serialized"
        );
    }

    fn text_region_with(id: usize, points: [[f32; 2]; 4], text: &str) -> OcrRegion {
        OcrRegion::text(
            RegionSource::Detected { detector_index: id },
            Some(Polygon { points }),
            None,
            None,
            Some(RecognitionOutcome {
                text: text.to_string(),
                score: 0.99,
                words: None,
            }),
        )
    }

    fn count_occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    /// `docs/11` §3.2 第 ① 条：三通道对同一份 `OcrOutput` 的输出语义一致。
    ///
    /// 判据**不是**"三条通道都不为空"，而是"表格的网格规模与每个单元格文本在
    /// JSON / HTML / Markdown 里都能被独立断言"。这条测试同时是 `TableOutcome`
    /// 与三条渲染路径之间唯一的横切契约 —— 任何一条通道悄悄丢掉表格都会红。
    #[test]
    fn the_three_channels_agree_on_the_table_grid_and_its_cells() {
        let table = table_outcome();
        let cells: Vec<String> = table.cells.iter().map(|cell| cell.text.clone()).collect();
        let output = output_with(vec![
            text_region_with(
                0,
                [[10.0, 1.0], [50.0, 1.0], [50.0, 3.0], [10.0, 3.0]],
                "被消费的行",
            ),
            table_region(table),
        ]);

        // ---------- JSON：`items` 里有一个 `kind == "table"` 的条目，顶层有 `tables` ----------
        let document = to_output_json(&output).expect("json should render");
        let items = document["items"].as_array().expect("items is an array");
        let table_item = items
            .iter()
            .find(|item| item["kind"] == "table")
            .expect("a `kind: table` item must exist");
        assert_eq!(table_item["table"]["rows"], 2);
        assert_eq!(table_item["table"]["cols"], 2);
        assert_eq!(table_item["table"]["cells"], 4);
        assert_eq!(table_item["table"]["confidence"], "high");
        // `txt` 就是区域的唯一文本表示（Markdown），与顶层 `tables[].markdown` 同源。
        assert_eq!(
            table_item["txt"], document["tables"][0]["markdown"],
            "the item text and the tables[] markdown must be the same string"
        );
        let json_text = table_item["txt"].as_str().expect("txt is a string");
        for cell in &cells {
            assert_eq!(
                count_occurrences(json_text, cell),
                1,
                "json txt must carry `{cell}` exactly once: {json_text}"
            );
        }
        // 纯文本口径：被消费的行出现一次，表格标记不混进页面文字。
        let json_plain = document["text"].as_str().expect("text is a string");
        assert_eq!(
            count_occurrences(json_plain, "被消费的行"),
            1,
            "{json_plain}"
        );
        assert!(!json_plain.contains('|'), "{json_plain}");

        // ---------- HTML：真的 `<table>`，网格规模与单元格都在报告里 ----------
        let html = crate::output::html::render_report(
            "三通道",
            "source.png",
            640,
            480,
            &to_output_items(&output),
            "total 1.0 ms",
            crate::output::html::ReportMode::Full,
        )
        .expect("report should render");
        assert!(html.contains("2 × 2 · 4 个单元格 · 置信度 high"), "{html}");
        for cell in &cells {
            assert!(
                html.contains(&format!("<td>{cell}</td>")),
                "html must render `{cell}` as a real cell: {html}"
            );
        }

        // ---------- Markdown：逻辑网格，且被消费的行不再重复 ----------
        let markdown = crate::output::markdown::to_output_markdown(&output);
        assert!(markdown.contains("| c00 | c01 |"), "{markdown}");
        assert!(markdown.contains("| c10 | c11 |"), "{markdown}");
        for cell in &cells {
            assert_eq!(
                count_occurrences(&markdown, cell),
                1,
                "markdown must carry `{cell}` exactly once: {markdown}"
            );
        }
        assert_eq!(
            count_occurrences(&markdown, "被消费的行"),
            0,
            "the consumed line is represented by the grid, not repeated: {markdown}"
        );

        // 三通道对"几个单元格"这件事给出同一个数。
        assert_eq!(document["tables"][0]["cells"], cells.len());
        assert!(
            html.contains(&format!("{} 个单元格", cells.len())),
            "{html}"
        );
    }

    /// P0-1 / P1-7 共用：**三通道快照对拍**。
    ///
    /// 对报告者真实捕获的 `/result` 体（42 个区域，字节由 serve 侧的 SHA-256 钉死）反序列化出
    /// `OcrOutput`，把 JSON / HTML / Markdown 三条通道的输出各自取 SHA-256 钉在这里。任何一次
    /// "行为等价重构"若悄悄改了任一通道的字节，这条就会红 —— 它是死代码清理（P0-1）与后续
    /// 编排重写（P1-7）共同的回归护栏。`OcrOutput` 同时是 `Serialize`/`Deserialize`，而
    /// `to_output_json` 就是 `serde_json::to_value(output)` 加四个派生键，因此这份捕获体能无损往返。
    #[test]
    fn the_three_output_channels_are_frozen_for_the_captured_real_page() {
        use sha2::{Digest as _, Sha256};

        fn sha256_hex(bytes: &[u8]) -> String {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        }

        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/serve/result-real-42-regions.json");
        let raw = std::fs::read(&path).unwrap_or_else(|error| {
            panic!("captured body must exist at {}: {error}", path.display())
        });
        let output: OcrOutput = serde_json::from_slice(&raw)
            .expect("the captured /result body round-trips back into OcrOutput");

        let json = serde_json::to_string(&to_output_json(&output).expect("json channel"))
            .expect("json channel serializes");
        let html = crate::output::html::render_output_report(
            "snapshot",
            "source.png",
            &output,
            "total 0.0 ms",
            crate::output::html::ReportMode::Full,
        )
        .expect("html channel renders");
        let markdown = crate::output::markdown::to_output_markdown(&output);

        let actual = format!(
            "json={}\nhtml={}\nmarkdown={}",
            sha256_hex(json.as_bytes()),
            sha256_hex(html.as_bytes()),
            sha256_hex(markdown.as_bytes()),
        );
        // 改前基线（P0-1 删除死代码之前生成）；删除后必须逐字节不变。
        let expected = "json=63c1beb1a3e9140a911a69de29f981bdfd17ba546f10cf3dae1f56b9e53b6914\n\
            html=d092c814a99e00dcc4f4890ce0bb2da1a0aaf258f3c5a8421b49c39e7eb64278\n\
            markdown=3f99e617c337a90eb5cd27a41703ecede28352558f6922565ec9e4801183a0a2";
        assert_eq!(actual, expected, "the three output channels changed bytes");
    }

    fn table_outcome() -> TableOutcome {
        let cell = |row, col| crate::TableCellOutcome {
            row,
            col,
            rowspan: 1,
            colspan: 1,
            text: format!("c{row}{col}"),
            score: Some(0.9),
            match_kind: crate::TableCellMatch::Single,
            polygon: None,
        };
        TableOutcome {
            html: "<html><body><table><tr><td>c00</td><td>c01</td></tr><tr><td>c10</td><td>c11</td></tr></table></body></html>"
                .into(),
            table_html:
                "<table><tr><td>c00</td><td>c01</td></tr><tr><td>c10</td><td>c11</td></tr></table>"
                    .into(),
            markdown: "| c00 | c01 |\n| --- | --- |\n| c10 | c11 |".into(),
            rows: 2,
            cols: 2,
            confidence: TableStructureConfidence::High,
            degradations: Vec::new(),
            holes: 0,
            cells: vec![cell(0, 0), cell(0, 1), cell(1, 0), cell(1, 1)],
            source_line_indices: vec![0],
            model_id: "slanet-plus".into(),
        }
    }

    fn table_region(table: TableOutcome) -> OcrRegion {
        OcrRegion {
            source: RegionSource::Detected { detector_index: 1 },
            kind: RegionKind::Table,
            polygon: Some(Polygon {
                points: [[1.0, 6.0], [5.0, 6.0], [5.0, 9.0], [1.0, 9.0]],
            }),
            detection: Some(crate::DetectionOutcome { score: 0.71 }),
            classification: None,
            recognition: None,
            formula: None,
            table: Some(table),
        }
    }

    fn output_with(regions: Vec<OcrRegion>) -> OcrOutput {
        let provider = ProviderResolutionInfo {
            requested: ProviderPreference::Cpu,
            selected_ep: ResolvedProvider::Cpu,
            fallback_to_cpu: false,
        };
        OcrOutput {
            schema_version: 1,
            image: ImageInfo {
                original_size: crate::ImageSize {
                    width: 10,
                    height: 10,
                },
                processed_size: crate::ImageSize {
                    width: 10,
                    height: 10,
                },
                coordinate_space: CoordinateSpace::Image,
            },
            stages: StageReports {
                input: InputTimings::default(),
                detector: StageReport {
                    state: StageState::Completed { items: 1 },
                    timing: None,
                },
                classifier: StageReport {
                    state: StageState::Disabled,
                    timing: None,
                },
                recognizer: StageReport {
                    state: StageState::Completed { items: 1 },
                    timing: None,
                },
                formula: StageReport::default(),
                table: StageReport::default(),
            },
            regions,
            timings: OcrTimings::default(),
            engine: EngineInfo {
                model_id: "test".into(),
                provider: ProviderInfo {
                    detector: provider,
                    classifier: None,
                    recognizer: provider,
                },
            },
        }
    }

    /// P1-7 表格测量夹具的守卫（非 `#[ignore]`）：入库资产不能只服务于一个手动探针。
    ///
    /// 只钉三件事：夹具仍能反序列化成 `OcrOutput`（协议未漂移）、表格区域仍在、
    /// `to_output_json` 仍产出四个派生视图键。量测本身走下面的 `--ignored` 探针。
    #[test]
    fn p17_table_fixtures_still_load_and_carry_the_table_view() {
        for (name, table_regions) in [
            ("p17-table-real.json", 1usize),
            ("p17-table-synthetic.json", 8usize),
        ] {
            let raw = std::fs::read(crate::test_support::fixture_dir("json-output").join(name))
                .expect("fixture");
            let output: OcrOutput = serde_json::from_slice(&raw).expect("fixture must load");
            assert_eq!(
                output
                    .regions
                    .iter()
                    .filter(|region| region.table.is_some())
                    .count(),
                table_regions,
                "{name} must keep its table regions"
            );
            let doc = to_output_json(&output).expect("doc");
            for key in ["items", "tables", "formulas", "text"] {
                assert!(doc.get(key).is_some(), "{name} loses derived key `{key}`");
            }
            assert_eq!(
                doc["tables"].as_array().expect("tables").len(),
                table_regions
            );
        }
    }

    /// P1-7 成本分项记录的入库路径（相对仓库根）。
    const P17_COST_RECORD: &str = "tests/fixtures/json-output/p17-cost-record.json";

    /// `to_output_json` 插入的四个派生视图键（与函数体内的插入集合一一对应）。
    const P17_VIEW_KEYS: [&str; 4] = ["text", "items", "formulas", "tables"];

    /// 读一份 P1-7 样本（入库夹具，任何环境都必须可用）。
    fn p17_read_sample(fixture: &str) -> OcrOutput {
        let path = crate::test_support::fixture_dir("json-output").join(fixture);
        let raw = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("{fixture} must be committed: {error}"));
        serde_json::from_slice(&raw).expect("fixture must load as OcrOutput")
    }

    /// 样本的**字节与计数**事实：这两个量在任何机器上都一致，因此守卫用例可以直接重算比对。
    fn p17_payload_facts(output: &OcrOutput) -> serde_json::Value {
        let source = serde_json::to_string(&serde_json::to_value(output).expect("source value"))
            .expect("compact source")
            .len();
        let doc = to_output_json(output).expect("doc");
        let full = serde_json::to_string(&doc).expect("compact doc").len();
        let mut views = serde_json::Map::new();
        let mut views_total = 0usize;
        for key in P17_VIEW_KEYS {
            let bytes = serde_json::to_string(&doc[key])
                .expect("view slice bytes")
                .len();
            views_total += bytes;
            views.insert(key.to_string(), serde_json::json!(bytes));
        }
        // 每个被插入的键额外占 `,"key":`，即 `key.len() + 4` 字节；因此派生体积必须逐项复原总体积。
        let key_overhead: usize = P17_VIEW_KEYS.iter().map(|key| key.len() + 4).sum();
        serde_json::json!({
            "regions": output.regions.len(),
            "text_regions": output
                .regions
                .iter()
                .filter(|region| region.recognition.is_some())
                .count(),
            "formula_regions": output
                .regions
                .iter()
                .filter(|region| region.formula.is_some())
                .count(),
            "table_regions": output
                .regions
                .iter()
                .filter(|region| region.table.is_some())
                .count(),
            "payload_bytes": {
                "source": source,
                "full": full,
                "views": views,
                "views_total": views_total,
                "key_overhead": key_overhead,
            }
        })
    }

    /// 把 `f` 跑 `runs` 次取**最快**（min-of-N）；最后一份结果留在手里防止整段被优化掉。
    fn p17_time_min_ns<T>(runs: u32, mut f: impl FnMut() -> T) -> u64 {
        let mut best = u64::MAX;
        let mut last = None;
        for _ in 0..runs {
            let started = std::time::Instant::now();
            let value = f();
            best = best.min(started.elapsed().as_nanos() as u64);
            last = Some(value);
        }
        std::hint::black_box(&last);
        best
    }

    /// 百分比（两位小数）：记录里只存这个精度，守卫用例按同一精度重算。
    fn p17_percent(part: u64, whole: u64) -> f64 {
        (part as f64 * 100.0 / whole as f64 * 100.0).round() / 100.0
    }

    /// 一份样本的时间分项（min-of-`runs`×`rounds`）。绝对值是**机器相关**的，
    /// 因此它们只作为“分项比例”的分子分母参与判据，不当作阈值本身。
    fn p17_time_facts(output: &OcrOutput, runs: u32, rounds: u32) -> serde_json::Value {
        let mut best = [u64::MAX; 6];
        for round in 0..rounds {
            let measured = [
                p17_time_min_ns(runs, || serde_json::to_value(output).expect("to_value")),
                p17_time_min_ns(runs, || to_output_json(output).expect("doc")),
                p17_time_min_ns(runs, || to_output_items(output)),
                p17_time_min_ns(runs, || output.plain_text(crate::TextOrder::Reading)),
                p17_time_min_ns(runs, || {
                    output
                        .tables(crate::TextOrder::Reading)
                        .into_iter()
                        .map(|(region_id, _)| region_id)
                        .collect::<Vec<_>>()
                }),
                p17_time_min_ns(runs, || {
                    output
                        .formula_latex(crate::TextOrder::Reading)
                        .into_iter()
                        .map(|(region_id, _)| region_id)
                        .collect::<Vec<_>>()
                }),
            ];
            for (slot, value) in best.iter_mut().zip(measured) {
                *slot = (*slot).min(value);
            }
            println!(
                "P17 round={round} to_value_ns={} to_output_json_ns={}",
                measured[0], measured[1]
            );
        }
        let [
            source_value_ns,
            full_path_ns,
            items_view_ns,
            text_view_ns,
            tables_view_ns,
            formulas_view_ns,
        ] = best;
        let views_body_ns = items_view_ns + text_view_ns + tables_view_ns + formulas_view_ns;
        let increment_ns = full_path_ns - source_value_ns;
        let payload_full = serde_json::to_string(&to_output_json(output).expect("doc"))
            .expect("compact doc")
            .len() as u64;
        let payload_source =
            serde_json::to_string(&serde_json::to_value(output).expect("source value"))
                .expect("compact source")
                .len() as u64;
        // 写盘前自检：时间分项必须彼此相容（否则重生成的记述会把守卫用例带坑）。
        assert!(
            full_path_ns > source_value_ns,
            "派生视图的构造必须真的花时间：to_output_json={full_path_ns} to_value={source_value_ns}"
        );
        assert!(
            increment_ns >= views_body_ns,
            "视图本体之和 ({views_body_ns}) 不能大于整体增量 ({increment_ns})：本轮测量噪声过大，请重跑"
        );
        serde_json::json!({
            "time_ns": {
                "to_value": source_value_ns,
                "to_output_json": full_path_ns,
                "items_view": items_view_ns,
                "text_view": text_view_ns,
                "tables_view": tables_view_ns,
                "formulas_view": formulas_view_ns,
            },
            "derived": {
                "increment_ns": increment_ns,
                "views_body_ns": views_body_ns,
                "eliminable_share_percent": p17_percent(views_body_ns, full_path_ns),
                "redundancy_share_percent": p17_percent(payload_full - payload_source, payload_full),
            }
        })
    }

    /// P1-7 的**取消投影重构**结论不能再靠人工解释一段打印输出（复审 P2-4）。
    ///
    /// 本用例不重新计时（绝对耗时是机器相关的，不应该在 CI 里钉阈值），它做四件事：
    /// ① 记述里声明的样本必须**存在**且仍能加载，结构计数（区域数 / 文本与公式与表格区域数）
    ///    与记述一致；② 负载分项（字节）在当前代码上重算，必须逐项相等 —— 协议一飘就红；
    /// ③ 成本分项的**算式**必须自洽（增量 = 全路径 − 基础路径、本体 = 四个视图之和、
    ///    本体 ≤ 增量、占比 = 本体/声明的分母、分项能拼回总体积）；④ “取消”结论的前提必须在数据里成立
    ///    （字节冗余非破坏性不可消、未声明机器无关的绝对阈值）。
    ///
    /// 故意**不**断言任何绝对耗时阈值：那只能在一个机器上成立，写死了就是伪判据。
    #[test]
    fn the_saved_p17_cost_record_is_recomputable_and_self_consistent() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read_to_string(root.join(P17_COST_RECORD))
            .expect("成本分项记录必须入库（由 --ignored 探针重生成）");
        let record: serde_json::Value = serde_json::from_str(&raw).expect("记录是合法 JSON");
        assert_eq!(record["schema_version"].as_u64(), Some(1));
        assert!(
            record["method"]["timings_are_machine_dependent"]
                .as_bool()
                .unwrap_or(false),
            "记录必须自己声明“绝对耗时是机器相关的”，否则下面的占比会被当成全局阈值"
        );
        let judgement = record["judgement"]
            .as_object()
            .expect("judgement 必须是一个对象");
        let mut judgement_keys = judgement.keys().map(String::as_str).collect::<Vec<_>>();
        judgement_keys.sort_unstable();
        assert_eq!(
            judgement_keys,
            [
                "absolute_time_threshold_declared",
                "byte_redundancy_reachable_without_breaking_the_output_protocol",
                "decision",
                "eliminable_time_denominator",
                "question",
                "rationale",
            ],
            "判据块字段集变了：守卫与记述必须一起改"
        );
        let judgement_of = |key: &str| judgement.get(key).expect("已断言存活的字段");
        let decision = judgement_of("decision").as_str().expect("decision");
        assert!(
            matches!(
                decision,
                "cancel-projection-refactor" | "proceed-projection-refactor"
            ),
            "未知的结论写法: {decision:?}"
        );
        assert!(
            !judgement_of("rationale").as_str().unwrap_or("").is_empty(),
            "结论必须自述理由"
        );
        assert_eq!(
            judgement_of("eliminable_time_denominator").as_str(),
            Some("to_output_json"),
            "占比必须写明分母，否则不同机器、不同口径的数字会被当成同一个判据（旧 §9 的 2.2–5.9% 就死在这上面）"
        );
        let byte_redundancy_reachable =
            judgement_of("byte_redundancy_reachable_without_breaking_the_output_protocol")
                .as_bool()
                .expect("byte_redundancy_reachable_without_breaking_the_output_protocol");
        let absolute_threshold_declared = judgement_of("absolute_time_threshold_declared")
            .as_bool()
            .expect("绝对阈值声明");

        let samples = record["samples"].as_array().expect("samples array");
        assert!(samples.len() >= 2, "至少一份真实 + 一份放大样本");
        for sample in samples {
            let id = sample["id"].as_str().expect("sample id");
            let fixture = sample["fixture"].as_str().expect("sample fixture");
            let output = p17_read_sample(fixture);
            let facts = p17_payload_facts(&output);
            for key in [
                "regions",
                "text_regions",
                "formula_regions",
                "table_regions",
            ] {
                assert_eq!(
                    sample[key].as_u64().unwrap_or(u64::MAX),
                    facts[key].as_u64().unwrap_or(u64::MAX),
                    "样本 {id} 的 `{key}` 与当前夹具重算不一致（样本已不在、或记述陈旧）"
                );
            }
            let recorded_bytes = &sample["payload_bytes"];
            let live_bytes = &facts["payload_bytes"];
            assert_eq!(
                recorded_bytes["source"], live_bytes["source"],
                "样本 {id} 的源体积变了（输出协议漂移）"
            );
            assert_eq!(
                recorded_bytes["full"], live_bytes["full"],
                "样本 {id} 的完整体积变了（输出协议漂移）"
            );
            for key in P17_VIEW_KEYS {
                assert_eq!(
                    recorded_bytes["views"][key], live_bytes["views"][key],
                    "样本 {id} 的派生视图 `{key}` 体积变了"
                );
            }
            // 分项必须能拼回总体积：派生视图字节 + 四个键名开销 == 完整 − 源。
            let sum_of_parts = live_bytes["views_total"].as_u64().expect("views_total")
                + live_bytes["key_overhead"].as_u64().expect("key_overhead");
            assert_eq!(
                sum_of_parts,
                live_bytes["full"].as_u64().expect("full")
                    - live_bytes["source"].as_u64().expect("source"),
                "样本 {id} 的成本分项拼不回总体积"
            );
            assert_eq!(
                recorded_bytes["views_total"], live_bytes["views_total"],
                "样本 {id} 的派生视图总体积变了"
            );

            let time = &sample["time_ns"];
            let got = |name: &str| {
                time[name]
                    .as_u64()
                    .unwrap_or_else(|| panic!("样本 {id} 缺 time_ns.{name}"))
            };
            let derived = &sample["derived"];
            let claims = |name: &str| {
                derived[name]
                    .as_f64()
                    .unwrap_or_else(|| panic!("样本 {id} 缺 derived.{name}"))
            };
            let increment = derived["increment_ns"]
                .as_u64()
                .unwrap_or_else(|| panic!("样本 {id} 缺 derived.increment_ns"));
            let views_body = derived["views_body_ns"]
                .as_u64()
                .unwrap_or_else(|| panic!("样本 {id} 缺 derived.views_body_ns"));
            assert_eq!(
                increment,
                got("to_output_json") - got("to_value"),
                "样本 {id} 的增量不是 `to_output_json − to_value`"
            );
            assert_eq!(
                views_body,
                got("items_view") + got("text_view") + got("tables_view") + got("formulas_view"),
                "样本 {id} 的视图本体不是四个视图耗时之和"
            );
            assert!(
                increment >= views_body,
                "样本 {id} 里可消除部分 ({views_body}) 大于整体增量 ({increment})：记述不自洽"
            );
            let full_path = got("to_output_json");
            let eliminable = claims("eliminable_share_percent");
            assert!(
                (eliminable - p17_percent(views_body, full_path)).abs() < 0.05,
                "样本 {id} 的可消除占比不等于本体/全路径的重算值"
            );
            let redundancy = claims("redundancy_share_percent");
            let live_full = live_bytes["full"].as_u64().expect("full");
            let live_source = live_bytes["source"].as_u64().expect("source");
            assert!(
                (redundancy - p17_percent(live_full - live_source, live_full)).abs() < 0.05,
                "样本 {id} 的冗余占比不等于当前字节体积的重算值"
            );
            assert!(
                live_bytes["views_total"].as_u64().expect("views_total") > 0,
                "样本 {id} 没任何派生负载，不能用来支撑“冗余只能靠破坏协议变更消除”这句前提"
            );
        }
        if decision == "cancel-projection-refactor" {
            assert!(
                !byte_redundancy_reachable,
                "取消结论拿“字节不变约束下消不掉冗余”当前提，但记述说冗余能在不破坏协议的前提下消掉"
            );
            assert!(
                !absolute_threshold_declared,
                "取消结论不得依赖一个机器无关的绝对耗时阈值（记述已声明耗时是机器相关的）"
            );
        }
        assert!(
            samples
                .iter()
                .any(|sample| sample["payload_bytes"]["full"].as_u64().unwrap_or(0) > 500_000),
            "记述必须含一份大负载样本，不能默默退回小页面"
        );
    }

    /// P1-7 表格场景测量探针（审核点 3：旧测量样本无表格，不能支撑取消结论）。
    ///
    /// 输入是入库的可复现夹具（`tests/fixtures/json-output/`）：真实捕获页（含 1 张
    /// 10 格表）与合成放大变体（8 张 100 格表、每表 31 KB 级 HTML）。只量三件事：
    /// 派生视图构造时间、序列化时间、负载体积；不依赖模型，`--ignored` 手动跑。
    ///
    /// 重生成入库记述（上一条守卫用例比对的产物）：
    /// 先设环境变量 `RAPID_STRUCTURE_P17_WRITE_RECORD` 为 `1`，再跑
    /// `cargo test --lib --features serve p17_measure_json_table_views_payload_and_time -- --ignored --nocapture`。
    /// 写入的绝对耗时来自**当前机器**，记述里已声明这一事实；守卫用例只校算术与体积。
    #[test]
    #[ignore = "P1-7 量测探针（会跑上千次序列化），手动 --ignored 运行"]
    fn p17_measure_json_table_views_payload_and_time() {
        use serde_json::json;

        const RUNS: u32 = 300;
        const ROUNDS: u32 = 3;
        let write_record = std::env::var("RAPID_STRUCTURE_P17_WRITE_RECORD")
            .map(|value| value.trim() == "1")
            .unwrap_or(false);
        let samples = [
            (
                "real",
                "p17-table-real.json",
                "真实捕获页：多区域正文 + 1 张 10 格表格",
            ),
            (
                "synthetic",
                "p17-table-synthetic.json",
                "合成放大页：8 张 100 格表格，每张 HTML 与 table_html 均 ≈31 KB",
            ),
        ];

        let mut records = Vec::new();
        for (id, fixture, description) in samples {
            let output = p17_read_sample(fixture);
            println!("P17 sample={id} fixture={fixture} runs={RUNS} rounds={ROUNDS}");
            let mut record = p17_payload_facts(&output);
            record["id"] = json!(id);
            record["fixture"] = json!(fixture);
            record["description"] = json!(description);
            let times = p17_time_facts(&output, RUNS, ROUNDS);
            record["time_ns"] = times["time_ns"].clone();
            record["derived"] = times["derived"].clone();
            println!(
                "P17 {id}: {}",
                serde_json::to_string(&record).expect("record json")
            );
            records.push(record);
        }

        if !write_record {
            println!("P17 打印模式：设 RAPID_STRUCTURE_P17_WRITE_RECORD=1 才会写入入库记述");
            return;
        }
        let doc = json!({
            "schema_version": 1,
            "note": "P1-7（投影重构）的成本分项记述。守卫用例 the_saved_p17_cost_record_is_recomputable_and_self_consistent \
                     会重算样本存在性、字节分项与算术关系；绝对耗时是机器相关的，不当作阈值。",
            "method": {
                "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
                "statistic": "min-of-runs，取 rounds 轮的最小值",
                "runs_per_round": RUNS,
                "rounds": ROUNDS,
                "timings_are_machine_dependent": true,
            },
            "judgement": {
                "question": "把 items/text/tables/formulas 四个派生视图改成投影（不再重复走一遍源数据）能消掉多少成本？",
                "eliminable_time_denominator": "to_output_json",
                "byte_redundancy_reachable_without_breaking_the_output_protocol": false,
                "absolute_time_threshold_declared": false,
                "decision": "cancel-projection-refactor",
                "rationale": "两条前提，一条口径：字节冗余恰好就是 `full − source = views_total + key_overhead` \
                              这一项，输出字节不变约束下只能靠破坏性协议变更消除（按裁决排除在本重构外）；时间收益的上界就是 \
                              `views_body_ns`，它只作用于 JSON 构造路径（分母已在 `eliminable_time_denominator` 里写明）。\
                              占比的绝对值随机器与 CPU 负载浮动，所以本记述不声明任何机器无关的绝对阈值。",
            },
            "samples": records,
        });
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(P17_COST_RECORD);
        std::fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&doc).expect("pretty record json")
            ),
        )
        .expect("write the P1-7 cost record");
        println!("wrote {} samples to {}", records.len(), path.display());
    }
}
