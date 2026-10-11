// 这一行覆盖的是 **遗留入口** `to_markdown` / `to_markdown_texts`（按 `boxes`/`txts`
// 入参，与 `to_output_markdown(&OcrOutput)` 并存，`json.rs` 的 `to_json_items` 是同类）。
// E15 试过删掉这一行：`-D warnings` 立刻报出「`to_markdown` is never used」「`to_markdown_texts`
// is never used」「field `center_y` is never read」——第三条是前两条的连带（`center_y` 只在
// `to_markdown` 的换行判定里读）。它们不是 E15 引入的死代码，删不删是"遗留入口统一清理"的
// 决定（要不要连带删 `to_json_items` 等），不属于输出三通道这一阶段，故保留并在此登记。
#![allow(dead_code)]

use crate::{OcrOutput, Quad, TextOrder, error::Result};

#[derive(Debug, Clone, Copy)]
struct BoxProps {
    top: f32,
    bottom: f32,
    left: f32,
    height: f32,
    center_y: f32,
}

impl BoxProps {
    fn merge(self, other: Self) -> Self {
        let top = self.top.min(other.top);
        let bottom = self.bottom.max(other.bottom);
        Self {
            top,
            bottom,
            left: self.left.min(other.left),
            height: bottom - top,
            center_y: (top + bottom) * 0.5,
        }
    }
}

pub fn to_markdown(boxes: &[Quad], txts: &[String]) -> Result<String> {
    if boxes.len() != txts.len() {
        return Err(crate::error::RapidOcrError::InvalidInput(format!(
            "markdown output length mismatch: boxes={}, txts={}",
            boxes.len(),
            txts.len()
        )));
    }
    if boxes.is_empty() {
        return Ok("No text detected.".to_string());
    }

    let mut combined: Vec<(Quad, String)> = (0..boxes.len())
        .map(|i| (boxes[i], txts[i].clone()))
        .collect();
    combined.sort_by(|(a_box, _), (b_box, _)| {
        let a = get_box_properties(a_box);
        let b = get_box_properties(b_box);
        a.top
            .partial_cmp(&b.top)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.left
                    .partial_cmp(&b.left)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let mut output_lines: Vec<String> = Vec::new();
    let mut current_line_parts: Vec<String> = vec![combined[0].1.clone()];
    let mut prev_props = get_box_properties(&combined[0].0);

    for (box_, text) in combined.iter().skip(1) {
        let current_props = get_box_properties(box_);

        let min_height = current_props.height.min(prev_props.height);
        let centers_are_close =
            (current_props.center_y - prev_props.center_y).abs() < (min_height * 0.5);

        let overlap_top = prev_props.top.max(current_props.top);
        let overlap_bottom = prev_props.bottom.min(current_props.bottom);
        let has_vertical_overlap = overlap_bottom > overlap_top;

        if centers_are_close || has_vertical_overlap {
            current_line_parts.push("   ".to_string());
            current_line_parts.push(text.clone());
        } else {
            output_lines.push(current_line_parts.join(""));

            let vertical_gap = current_props.top - prev_props.bottom;
            if vertical_gap > prev_props.height * 0.7 {
                output_lines.push(String::new());
            }

            current_line_parts = vec![text.clone()];
        }

        prev_props = current_props;
    }

    output_lines.push(current_line_parts.join(""));
    Ok(output_lines.join("\n"))
}

pub fn to_markdown_texts(txts: &[String]) -> String {
    if txts.is_empty() {
        return "No text detected.".to_string();
    }
    txts.join("\n")
}

/// 被表格区域消费掉的行（`TableOutcome::source_line_indices`）。
///
/// 这些行**已经**以单元格文本的形式出现在表格里。Markdown 通道若再输出一次，
/// 同一段文字就会出现两遍（`docs/11` §3 E15.4）。返回与 `regions` 等长的位图，
/// 避免为几次查表引入 `HashSet` 依赖。
///
/// 注意：`plain_text`（`src/api.rs:1036`）**不**做这个扣除 —— 它只取 `recognition`，
/// 而表格区域没有 `recognition`，所以"被消费的行恰好出现一次"是它的自然结果。
/// 两条通道的差异是刻意的：纯文本要的是页面文字内容，Markdown 要的是结构。
fn consumed_by_tables(output: &OcrOutput) -> Vec<bool> {
    let mut consumed = vec![false; output.regions.len()];
    for region in &output.regions {
        let Some(table) = region.table.as_ref() else {
            continue;
        };
        for &id in &table.source_line_indices {
            if let Some(slot) = consumed.get_mut(id) {
                *slot = true;
            }
        }
    }
    consumed
}

/// 按阅读顺序取出表格的 Markdown 网格（空表格的块为空串，由调用方过滤）。
fn table_markdown_blocks(output: &OcrOutput, order: TextOrder) -> Vec<String> {
    output
        .tables(order)
        .into_iter()
        .map(|(_, table)| table.markdown.clone())
        .collect()
}

/// 与 [`OcrOutput::plain_text`] 同口径，但跳过被表格消费的行。
fn plain_text_without_consumed(output: &OcrOutput, consumed: &[bool]) -> String {
    output
        .reading_order()
        .into_iter()
        .filter(|id| !consumed.get(*id).copied().unwrap_or(false))
        .filter_map(|id| output.regions.get(id)?.recognition.as_ref())
        .map(|recognition| recognition.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn to_output_markdown(output: &OcrOutput) -> String {
    let consumed = consumed_by_tables(output);
    // 整图文本输入没有多边形，无法参与阅读顺序分组；此时退回“纯文本 + 公式块”，
    // 仍保证公式 LaTeX 不丢失。
    let polygonless_text = output
        .regions
        .iter()
        .any(|region| region.recognition.is_some() && region.polygon.is_none());
    if polygonless_text {
        // 这个退化分支按“先文本、再公式、再表格”分段，组间顺序已经与原文一致地丢失了
        // （本分支的目的只是别丢内容），表格块同样追加在最后。
        let mut sections: Vec<String> = Vec::new();
        let text = plain_text_without_consumed(output, &consumed);
        if !text.is_empty() {
            sections.push(text);
        }
        for (_, latex) in output.formula_latex(TextOrder::Reading) {
            let block = crate::formula::output::to_markdown_block(latex, false);
            if !block.is_empty() {
                sections.push(block);
            }
        }
        for block in table_markdown_blocks(output, TextOrder::Reading) {
            if !block.is_empty() {
                sections.push(block);
            }
        }
        return if sections.is_empty() {
            "No text detected.".to_string()
        } else {
            sections.join("\n\n")
        };
    }

    let mut blocks: Vec<String> = Vec::new();
    let mut previous_props: Option<BoxProps> = None;

    for group in output.reading_order_groups() {
        let mut parts = Vec::new();
        let mut group_props: Option<BoxProps> = None;
        // 公式与表格都是多行块：同一组里出现它们时必须换行拼接，不能用行内分隔符。
        let mut group_has_block = false;

        for id in group {
            let Some(region) = output.regions.get(id) else {
                continue;
            };
            let Some(polygon) = region.polygon else {
                continue;
            };
            let props = get_box_properties(&polygon.points);
            group_props = Some(match group_props {
                Some(current) => current.merge(props),
                None => props,
            });

            if let Some(table) = &region.table {
                // 表格区域用 Markdown 表格表示自己，并顺带说明跨度损失（说明行由
                // `src/table/emitter.rs` 的 `emit_markdown` 注入，见 docs/11 §3.5）。
                group_has_block = true;
                parts.push(table.markdown.clone());
                continue;
            }
            if let Some(formula) = &region.formula {
                // 公式区域使用 display math，不当作普通文本转义。
                let block =
                    crate::formula::output::to_markdown_block(&formula.latex, formula.truncated);
                group_has_block = true;
                parts.push(block);
                continue;
            }
            if consumed.get(id).copied().unwrap_or(false) {
                // 这一行已经被表格的单元格吸收，不再重复输出。
                continue;
            }
            let Some(recognition) = &region.recognition else {
                continue;
            };
            parts.push(recognition.text.clone());
        }

        parts.retain(|part| !part.is_empty());
        if parts.is_empty() {
            continue;
        }
        if let (Some(previous), Some(current)) = (previous_props, group_props) {
            let min_height = previous.height.min(current.height);
            let vertical_gap = current.top - previous.bottom;
            if vertical_gap > min_height * 0.7 {
                blocks.push(String::new());
            }
        }
        // 含公式或表格的分组按行分隔，避免 display math / 表格与正文挤在同一行。
        blocks.push(if group_has_block {
            parts.join("\n")
        } else {
            parts.join("   ")
        });
        previous_props = group_props;
    }

    if blocks.is_empty() {
        output.plain_text(TextOrder::Reading)
    } else {
        blocks.join("\n")
    }
}

fn get_box_properties(box_: &Quad) -> BoxProps {
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    let mut left = f32::INFINITY;
    for point in box_ {
        top = top.min(point[1]);
        bottom = bottom.max(point[1]);
        left = left.min(point[0]);
    }
    let height = bottom - top;
    BoxProps {
        top,
        bottom,
        left,
        height,
        center_y: top + height / 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::to_markdown;
    use crate::{
        CoordinateSpace, EngineInfo, FormulaOutcome,
        GenericProviderPreference as ProviderPreference, ImageInfo, ImageSize, InputTimings,
        OcrOutput, OcrRegion, OcrTimings, Polygon, ProviderInfo, ProviderResolutionInfo,
        RecognitionOutcome, RegionKind, RegionSource, ResolvedProvider, StageReport, StageReports,
        StageState, TextOrder,
    };

    #[test]
    fn markdown_empty_message() {
        assert_eq!(
            to_markdown(&[], &[]).expect("empty should be valid"),
            "No text detected."
        );
    }

    fn formula_region(id: usize, points: [[f32; 2]; 4], latex: &str, truncated: bool) -> OcrRegion {
        OcrRegion {
            source: RegionSource::Detected { detector_index: id },
            kind: RegionKind::Formula,
            polygon: Some(Polygon { points }),
            detection: Some(crate::DetectionOutcome { score: 0.8 }),
            classification: None,
            recognition: None,
            formula: Some(FormulaOutcome {
                latex: latex.to_string(),
                eos_index: if truncated { None } else { Some(1) },
                truncated,
                model_id: "pp_formulanet_plus_m".to_string(),
                token_ids: None,
            }),
            table: None,
        }
    }

    fn text_region(id: usize, points: [[f32; 2]; 4], text: &str) -> OcrRegion {
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

    #[test]
    fn formula_regions_render_as_display_math_in_reading_order() {
        let output = output_with_regions(vec![
            text_region(
                0,
                [[10.0, 10.0], [90.0, 10.0], [90.0, 30.0], [10.0, 30.0]],
                "above",
            ),
            formula_region(
                1,
                [[10.0, 40.0], [90.0, 40.0], [90.0, 70.0], [10.0, 70.0]],
                "\\frac{a}{b}",
                false,
            ),
            text_region(
                2,
                [[10.0, 80.0], [90.0, 80.0], [90.0, 95.0], [10.0, 95.0]],
                "below",
            ),
        ]);
        let markdown = super::to_output_markdown(&output);
        assert!(markdown.contains("above"), "markdown: {markdown}");
        assert!(markdown.contains("$$"), "markdown: {markdown}");
        assert!(markdown.contains("\\frac{a}{b}"), "markdown: {markdown}");
        assert!(markdown.contains("below"), "markdown: {markdown}");
        assert!(
            markdown.find("$$").unwrap() > markdown.find("above").unwrap(),
            "formula must keep reading order: {markdown}"
        );
        assert!(
            markdown.find("below").unwrap() > markdown.find("$$").unwrap(),
            "formula must keep reading order: {markdown}"
        );
    }

    #[test]
    fn truncated_formula_markdown_carries_the_invisible_marker() {
        let output = output_with_regions(vec![formula_region(
            0,
            [[10.0, 10.0], [90.0, 10.0], [90.0, 40.0], [10.0, 40.0]],
            "\\frac{1}{",
            true,
        )]);
        let markdown = super::to_output_markdown(&output);
        assert!(
            markdown.contains(crate::formula::output::TRUNCATED_MARKER),
            "markdown: {markdown}"
        );
    }

    fn output_with_regions(regions: Vec<OcrRegion>) -> OcrOutput {
        let resolution = ProviderResolutionInfo {
            requested: ProviderPreference::Cpu,
            selected_ep: ResolvedProvider::Cpu,
            fallback_to_cpu: false,
        };
        OcrOutput {
            schema_version: 1,
            image: ImageInfo {
                original_size: ImageSize {
                    width: 200,
                    height: 100,
                },
                processed_size: ImageSize {
                    width: 200,
                    height: 100,
                },
                coordinate_space: CoordinateSpace::Image,
            },
            stages: StageReports {
                input: InputTimings::default(),
                detector: StageReport {
                    state: StageState::Completed {
                        items: regions.len(),
                    },
                    timing: None,
                },
                classifier: StageReport {
                    state: StageState::Disabled,
                    timing: None,
                },
                recognizer: StageReport {
                    state: StageState::Completed {
                        items: regions.len(),
                    },
                    timing: None,
                },
                formula: StageReport::default(),
                table: StageReport::default(),
            },
            regions,
            timings: OcrTimings::default(),
            engine: EngineInfo {
                model_id: "test".to_string(),
                provider: ProviderInfo {
                    detector: resolution,
                    classifier: None,
                    recognizer: resolution,
                },
            },
        }
    }

    fn table_region(
        id: usize,
        points: [[f32; 2]; 4],
        markdown: &str,
        source_line_indices: Vec<usize>,
    ) -> OcrRegion {
        OcrRegion {
            source: RegionSource::Detected { detector_index: id },
            kind: RegionKind::Table,
            polygon: Some(Polygon { points }),
            detection: Some(crate::DetectionOutcome { score: 0.7 }),
            classification: None,
            // 表格区域**没有** recognition：它的文字表示就是表格本身。
            recognition: None,
            formula: None,
            table: Some(crate::TableOutcome {
                html: "<html><body><table></table></body></html>".to_string(),
                table_html: "<table></table>".to_string(),
                markdown: markdown.to_string(),
                rows: 1,
                cols: 2,
                confidence: crate::TableStructureConfidence::High,
                degradations: Vec::new(),
                holes: 0,
                cells: Vec::new(),
                source_line_indices,
                model_id: "slanet-plus".to_string(),
            }),
        }
    }

    fn count_occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    /// 一页三行文字，其中前两行被表格消费。
    fn page_with_a_consuming_table() -> OcrOutput {
        output_with_regions(vec![
            text_region(
                0,
                [[10.0, 10.0], [90.0, 10.0], [90.0, 30.0], [10.0, 30.0]],
                "A",
            ),
            text_region(
                1,
                [[10.0, 40.0], [90.0, 40.0], [90.0, 60.0], [10.0, 60.0]],
                "B",
            ),
            table_region(
                2,
                [[0.0, 0.0], [200.0, 0.0], [200.0, 100.0], [0.0, 100.0]],
                "| h1 | h2 |\n| --- | --- |\n| A | B |",
                vec![0, 1],
            ),
            text_region(
                3,
                [[10.0, 150.0], [90.0, 150.0], [90.0, 170.0], [10.0, 170.0]],
                "C",
            ),
        ])
    }

    /// `docs/11` §3 E15.4：Markdown 通道把表格渲染成网格，**并且**不再重复它消费掉的行。
    #[test]
    fn a_table_region_replaces_the_lines_it_consumed() {
        let markdown = super::to_output_markdown(&page_with_a_consuming_table());
        assert!(markdown.contains("| h1 | h2 |"), "markdown: {markdown}");
        assert!(markdown.contains("| --- | --- |"), "markdown: {markdown}");
        assert!(markdown.contains("| A | B |"), "markdown: {markdown}");
        // A 与 B 只作为单元格出现一次，不再作为独立行出现第二次。
        assert_eq!(count_occurrences(&markdown, "A"), 1, "markdown: {markdown}");
        assert_eq!(count_occurrences(&markdown, "B"), 1, "markdown: {markdown}");
        // 没有被消费的行照常输出。
        assert_eq!(count_occurrences(&markdown, "C"), 1, "markdown: {markdown}");
        // 表格块是独立块：不与相邻文本挤在同一行。
        assert!(
            markdown.lines().any(|line| line == "| A | B |"),
            "the grid must keep its own line: {markdown}"
        );
    }

    /// `plain_text` 与 Markdown 通道的**刻意**差异。
    ///
    /// 纯文本是"页面的文字内容"（CER 评测、网页复制全文、搜索）：被表格消费的行必须
    /// 出现且只出现一次，而 Markdown 的 `|` / `---` 是标记，不得混进去。
    #[test]
    fn plain_text_keeps_the_consumed_lines_and_ignores_the_table() {
        let output = page_with_a_consuming_table();
        let text = output.plain_text(TextOrder::Reading);
        assert_eq!(text.lines().count(), 3, "plain_text: {text:?}");
        for needle in ["A", "B", "C"] {
            assert_eq!(count_occurrences(&text, needle), 1, "plain_text: {text:?}");
        }
        assert!(!text.contains('|'), "plain_text: {text:?}");
        assert!(!text.contains("---"), "plain_text: {text:?}");
    }

    /// 表格的跨度说明行（由 `src/table/emitter.rs` 的 `emit_markdown` 注入）必须原样
    /// 穿过 Markdown 通道 —— 否则"跨度被静默丢失"就回到未修复状态。
    #[test]
    fn the_span_note_survives_the_markdown_channel() {
        let output = page_with_a_consuming_table();
        let markdown = super::to_output_markdown(&output);
        // 本夹具的 markdown 里没有说明行；换成带说明行的表格再断言它不被吃掉。
        let _ = markdown;
        let with_note = output_with_regions(vec![table_region(
            0,
            [[0.0, 0.0], [200.0, 0.0], [200.0, 100.0], [0.0, 100.0]],
            "<!-- 表格含跨行/跨列 -->\n| a |  |\n| --- | --- |\n|  | c |\n<!-- 表格含跨行/跨列 -->",
            vec![],
        )]);
        let markdown = super::to_output_markdown(&with_note);
        assert!(
            markdown.contains("<!-- 表格含跨行/跨列 -->"),
            "markdown: {markdown}"
        );
    }

    /// `docs/11` §3.2 第 ④ 条：没有表格的页面，Markdown 输出与表格通道引入之前相同。
    #[test]
    fn a_page_without_tables_renders_exactly_as_before() {
        let output = output_with_regions(vec![
            text_region(
                0,
                [[10.0, 10.0], [60.0, 10.0], [60.0, 30.0], [10.0, 30.0]],
                "L1",
            ),
            text_region(
                1,
                [[10.0, 40.0], [60.0, 40.0], [60.0, 60.0], [10.0, 60.0]],
                "L2",
            ),
        ]);
        assert_eq!(super::to_output_markdown(&output), "L1\nL2");
    }

    #[derive(serde::Deserialize)]
    struct RealLayoutFixture {
        boxes: Vec<RealLayoutBox>,
        expected: Vec<usize>,
    }

    #[derive(serde::Deserialize)]
    struct RealLayoutBox {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        text: String,
    }

    #[test]
    fn output_markdown_uses_real_10_columns_order() {
        let fixture: RealLayoutFixture = serde_json::from_str(include_str!(
            "../../tests/fixtures/reading_order_10_columns.json"
        ))
        .expect("real layout fixture should parse");
        let regions = fixture
            .boxes
            .iter()
            .enumerate()
            .map(|(id, b)| {
                text_region(
                    id,
                    [[b.x0, b.y0], [b.x1, b.y0], [b.x1, b.y1], [b.x0, b.y1]],
                    &b.text,
                )
            })
            .collect();
        let output = output_with_regions(regions);
        let markdown = super::to_output_markdown(&output);
        let lines: Vec<&str> = markdown
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let expected: Vec<&str> = fixture
            .expected
            .iter()
            .map(|index| fixture.boxes[*index].text.as_str())
            .collect();
        assert_eq!(lines, expected);
    }

    #[test]
    fn output_markdown_uses_reading_order_for_columns() {
        let output = output_with_regions(vec![
            text_region(
                0,
                [[10.0, 10.0], [60.0, 10.0], [60.0, 30.0], [10.0, 30.0]],
                "L1",
            ),
            text_region(
                1,
                [[10.0, 40.0], [60.0, 40.0], [60.0, 60.0], [10.0, 60.0]],
                "L2",
            ),
            text_region(
                2,
                [[100.0, 10.0], [150.0, 10.0], [150.0, 30.0], [100.0, 30.0]],
                "R1",
            ),
            text_region(
                3,
                [[100.0, 40.0], [150.0, 40.0], [150.0, 60.0], [100.0, 60.0]],
                "R2",
            ),
        ]);

        assert_eq!(super::to_output_markdown(&output), "L1\nL2\nR1\nR2");
    }
}
