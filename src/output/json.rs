use serde::Serialize;

use crate::{OcrOutput, Quad, error::Result};
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

#[allow(dead_code)]
pub fn to_json_items(
    boxes: Option<&[Quad]>,
    txts: &[String],
    scores: &[f32],
) -> Result<Vec<OcrJsonItem>> {
    if txts.len() != scores.len() {
        return Err(crate::error::RapidOcrError::InvalidInput(format!(
            "json output length mismatch: txts={}, scores={}",
            txts.len(),
            scores.len()
        )));
    }

    if let Some(boxes) = boxes
        && boxes.len() != txts.len()
    {
        return Err(crate::error::RapidOcrError::InvalidInput(format!(
            "json output length mismatch: boxes={}, txts={}",
            boxes.len(),
            txts.len()
        )));
    }

    let mut out = Vec::with_capacity(txts.len());
    for i in 0..txts.len() {
        let out_box = boxes.map(|all| {
            let src_box = all[i];
            let mut out_box = [[0.0_f64; 2]; 4];
            for (dst, src) in out_box.iter_mut().zip(src_box.iter()) {
                dst[0] = src[0] as f64;
                dst[1] = src[1] as f64;
            }
            out_box
        });
        out.push(OcrJsonItem {
            box_: out_box,
            txt: txts[i].clone(),
            score: scores[i] as f64,
            kind: "text",
            latex: None,
            eos_index: None,
            truncated: None,
            table: None,
        });
    }
    Ok(out)
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
    use super::{to_json_items, to_output_items, to_output_json};
    use crate::{
        CoordinateSpace, EngineInfo, FormulaOutcome,
        GenericProviderPreference as ProviderPreference, ImageInfo, InputTimings, OcrOutput,
        OcrRegion, OcrTimings, Polygon, ProviderInfo, ProviderResolutionInfo, RecognitionOutcome,
        RegionKind, RegionSource, ResolvedProvider, StageReport, StageReports, StageState,
        TableOutcome, TableStructureConfidence,
    };

    #[test]
    fn json_items_none_for_empty_inputs() {
        assert_eq!(
            to_json_items(Some(&[]), &[], &[]).expect("empty should be valid"),
            Vec::new()
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
}
