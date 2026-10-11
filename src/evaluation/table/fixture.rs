//! TabRecSet val 真值记录的加载、图像路径解析与"构造性上界"统计。
//!
//! # 真值从哪来
//!
//! `tools/table_detokenize.py` 把 TabRecSet 的两份原生标注（PubTabNet 格式的
//! `CAPTIONS_STRUCTURE` token 流 + LabelMe 多边形）交叉验证后，产出
//! `datasets/table/tabrecset/derived/pubtabnet_val.jsonl`。本模块**只读**它，
//! 不重新解释标注格式 —— 标注语义的权威记录在
//! `docs/09-table-recognition-execution.md` §7.4。
//!
//! # 为什么"span 超词表"必须单独统计
//!
//! SLANet-plus 的 `character` 词表里 `rowspan` / `colspan` 的取值上限是模型**见过的**
//! 最大值（实测 20）。TabRecSet val 里存在 `colspan="24"`，这类表格**不可能**被
//! 词表表达，模型输出得再好也拿不到满分 —— 这是**构造性上界**，不是模型缺陷。
//! 上限定在**表格**粒度（§8.1 ③ 要求的是"必然出错的表格占比"），不是单元格粒度：
//! 一张表只要有一个超限单元格，它的 TEDS 就不可能等于 1。
//!
//! 上限从**词表本身**解析出来，不硬编码 20：硬编码会在换模型时静默失效。

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    error::{RapidOcrError, Result},
    evaluation::digest::digest_hex,
    table::decode::StructureVocabulary,
};

/// 数据集根目录（相对仓库根）。
pub const DEFAULT_DATASET_RELATIVE: &str = "datasets/table/tabrecset";
/// `tools/table_detokenize.py` 产出的 val 真值记录。
pub const DEFAULT_MANIFEST_RELATIVE: &str = "datasets/table/tabrecset/derived/pubtabnet_val.jsonl";
/// 官方 `metric.py` 的 `true_json`（`{file_name: {"html": ...}}`）。
pub const DEFAULT_GT_HTML_RELATIVE: &str =
    "datasets/table/tabrecset/derived/pubtabnet_val_gt_html.json";

/// `pubtabnet_val.jsonl` 的一行。
///
/// 字段与 `tools/table_detokenize.py` 的 `build_records` 一一对应；
/// `#[serde(deny_unknown_fields)]` **刻意不加**：工具新增诊断字段时不应该让评测直接报错，
/// 但也因此**不能**靠"反序列化成功"证明字段完整 —— 下面 `TableRecord::validate` 逐项检查。
#[derive(Debug, Clone, Deserialize)]
pub struct TableRecord {
    /// 图像文件名（在 PubTabNet 名单内，唯一）。
    pub file_name: String,
    /// 完整 HTML 真值（`<html><body><table>…`）。
    pub html: String,
    /// `html` 的 SHA-256，用于证明评测用的真值与工具产出的是同一份。
    pub html_sha256: String,
    /// 相对数据集根目录的图像路径（`/` 分隔）。
    pub image_path: String,
    /// 真值单元格数（与 `<td` 出现次数一致）。
    pub n_cells: usize,
    /// 分片名。
    pub split: String,
    /// 结构 token 流长度（**只是计数**，token 本身在 `cropped_val_structure.jsonl`）。
    pub structure_tokens: usize,
    /// 单元格文本集合的 SHA-256。
    pub texts_sha256: String,
}

impl TableRecord {
    fn validate(&self) -> Result<()> {
        if self.file_name.is_empty() {
            return Err(RapidOcrError::Config(
                "table record has an empty file_name".to_string(),
            ));
        }
        if !self.html.starts_with("<html><body><table>")
            || !self.html.ends_with("</table></body></html>")
        {
            return Err(RapidOcrError::Config(format!(
                "table record {} has an html that is not a full page: {:?}",
                self.file_name,
                self.html.chars().take(48).collect::<String>()
            )));
        }
        if self.html_sha256.len() != 64 || !self.html_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(RapidOcrError::Config(format!(
                "table record {} has a malformed html_sha256: {:?}",
                self.file_name, self.html_sha256
            )));
        }
        Ok(())
    }
}

/// 一条样本：真值记录 + 解析后的图像绝对路径。
#[derive(Debug, Clone)]
pub struct TableSample {
    record: TableRecord,
    image_path: PathBuf,
}

impl TableSample {
    /// 数据集相对路径（`/` 分隔）。抽样键与报告都用它，**不是**绝对路径。
    pub fn relative_path(&self) -> &str {
        &self.record.image_path
    }

    /// 真值 HTML。
    pub fn ground_truth(&self) -> &str {
        &self.record.html
    }

    pub fn file_name(&self) -> &str {
        &self.record.file_name
    }

    pub fn n_cells(&self) -> usize {
        self.record.n_cells
    }

    pub fn record(&self) -> &TableRecord {
        &self.record
    }

    /// 图像文件的绝对路径。
    pub fn image_path(&self) -> &Path {
        &self.image_path
    }
}

/// 加载后的 val 分片。
#[derive(Debug, Clone)]
pub struct TableFixture {
    dataset_root: PathBuf,
    manifest_path: PathBuf,
    samples: Vec<TableSample>,
}

impl TableFixture {
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn samples(&self) -> &[TableSample] {
        &self.samples
    }

    pub fn dataset_root(&self) -> &Path {
        &self.dataset_root
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// 按"数据集相对路径 + 真值"的稳定哈希升序取前 `limit` 条（`limit == 0` 表示全量）。
    ///
    /// 与 `evaluation::formula::sampling::select_samples` 用**同一个键**，因此两个域的
    /// "第 N 个样本"口径一致；也**不是**"取前 N 张"（那条测试
    /// `hash_sampling_is_not_first_n` 守的就是这个）。
    pub fn select(&self, limit: usize) -> Vec<&TableSample> {
        let mut keyed: Vec<(String, &TableSample)> = self
            .samples
            .iter()
            .map(|s| {
                (
                    digest_hex(
                        format!("{}\u{1f}{}", s.relative_path(), s.ground_truth()).as_bytes(),
                    ),
                    s,
                )
            })
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        let take = if limit == 0 { usize::MAX } else { limit };
        keyed.into_iter().take(take).map(|(_, s)| s).collect()
    }

    /// 逐条核对 `html_sha256` 与文件存在性。
    ///
    /// 单列一个方法而不是在 `load` 里做，是因为"加载"要在毫秒级完成（CLI 的
    /// `--help`/参数校验路径也会走到它），而 7,104 次文件读取值得显式调用一次。
    pub fn verify(&self) -> Result<TableVerifyReport> {
        let mut hashed = 0_usize;
        let mut missing = Vec::new();
        let mut mismatched = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut duplicates = Vec::new();

        for sample in &self.samples {
            if !seen.insert(sample.file_name()) {
                duplicates.push(sample.file_name().to_string());
            }
            if !sample.image_path.is_file() {
                missing.push(sample.relative_path().to_string());
            }
            let actual = digest_hex(sample.ground_truth().as_bytes());
            if actual != sample.record.html_sha256 {
                mismatched.push(sample.relative_path().to_string());
            } else {
                hashed += 1;
            }
        }

        if !missing.is_empty() {
            return Err(RapidOcrError::InvalidImage(format!(
                "{} of {} table images are missing; first: {}",
                missing.len(),
                self.samples.len(),
                missing[0]
            )));
        }
        if !mismatched.is_empty() {
            return Err(RapidOcrError::Config(format!(
                "{} table records have an html_sha256 that does not match their html; first: {}",
                mismatched.len(),
                mismatched[0]
            )));
        }
        if !duplicates.is_empty() {
            return Err(RapidOcrError::Config(format!(
                "table manifest repeats {} file_name(s); first: {}",
                duplicates.len(),
                duplicates[0]
            )));
        }

        Ok(TableVerifyReport {
            samples: self.samples.len(),
            hashed,
        })
    }

    /// 选中样本集的稳定摘要（doc 09 §8.1④ 的 `sample_set_sha256`）。
    ///
    /// 口径与 `evaluation::formula::sampling::Manifest::sample_set_sha256` 一致：按
    /// `relative_path` 排序后（顺序无关），用 `\u{1f}` 分隔字段、`\u{1e}` 分隔条目。
    /// **不含图像内容**，因此同一份 manifest 在任何机器上得到同一个值。
    pub fn sample_set_digest(samples: &[&TableSample]) -> String {
        let mut ordered: Vec<&TableSample> = samples.to_vec();
        ordered.sort_by(|a, b| a.relative_path().cmp(b.relative_path()));
        let canonical = ordered
            .iter()
            .map(|s| format!("{}\u{1f}{}", s.relative_path(), s.ground_truth()))
            .collect::<Vec<_>>()
            .join("\u{1e}");
        digest_hex(canonical.as_bytes())
    }

    /// 选中样本集的**图像内容**摘要（doc 09 §8.1④ 的 `content_sha256`）。
    ///
    /// 只要有一张图读不出来就返回 `Ok(None)` —— 这条摘要存在的意义是"证明我们真的
    /// 看的是这些像素"，读不全时**不得伪造"已校验"**（同 `sampling.rs` 的约定）。
    pub fn content_digest(&self, samples: &[&TableSample]) -> Result<Option<String>> {
        let mut ordered: Vec<&TableSample> = samples.to_vec();
        ordered.sort_by(|a, b| a.relative_path().cmp(b.relative_path()));
        let mut entries = Vec::with_capacity(ordered.len());
        for sample in ordered {
            let Ok(bytes) = std::fs::read(&sample.image_path) else {
                return Ok(None);
            };
            entries.push(format!(
                "{}\u{1f}{}",
                sample.relative_path(),
                digest_hex(&bytes)
            ));
        }
        Ok(Some(digest_hex(entries.join("\u{1e}").as_bytes())))
    }
}

/// [`TableFixture::verify`] 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableVerifyReport {
    pub samples: usize,
    pub hashed: usize,
}

/// 从数据集根目录加载 val 真值。
///
/// `dataset_root` 是 `datasets/table/tabrecset`；manifest 固定为
/// `derived/pubtabnet_val.jsonl`（工具的唯一产物名）。
pub fn load_pubtabnet_val(dataset_root: impl AsRef<Path>) -> Result<TableFixture> {
    let dataset_root = dataset_root.as_ref().to_path_buf();
    let manifest_path = dataset_root.join("derived").join("pubtabnet_val.jsonl");
    if !manifest_path.is_file() {
        return Err(RapidOcrError::Config(format!(
            "table val manifest is missing: {} (run `python tools/table_detokenize.py` first)",
            manifest_path.display()
        )));
    }

    let text = fs::read_to_string(&manifest_path).map_err(|e| {
        RapidOcrError::Config(format!("cannot read {}: {e}", manifest_path.display()))
    })?;

    let mut samples = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: TableRecord = serde_json::from_str(line).map_err(|e| {
            RapidOcrError::Config(format!(
                "{}: line {} is not a table record: {e}",
                manifest_path.display(),
                index + 1
            ))
        })?;
        record.validate()?;

        let relative = Path::new(&record.image_path);
        if relative.is_absolute() || record.image_path.contains("..") {
            return Err(RapidOcrError::Config(format!(
                "table record {} has an image_path outside the dataset root: {:?}",
                record.file_name, record.image_path
            )));
        }
        let image_path = dataset_root.join(relative);
        samples.push(TableSample { record, image_path });
    }

    if samples.is_empty() {
        return Err(RapidOcrError::Config(format!(
            "{} contains no table records",
            manifest_path.display()
        )));
    }

    Ok(TableFixture {
        dataset_root,
        manifest_path,
        samples,
    })
}

/// span 的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpanKind {
    Row,
    Col,
}

impl SpanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Row => "rowspan",
            Self::Col => "colspan",
        }
    }
}

/// 词表能表达的 span 上限（`None` = 词表里根本没有这种 span token）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpanLimits {
    pub max_rowspan: Option<usize>,
    pub max_colspan: Option<usize>,
}

impl SpanLimits {
    fn max(&self, kind: SpanKind) -> Option<usize> {
        match kind {
            SpanKind::Row => self.max_rowspan,
            SpanKind::Col => self.max_colspan,
        }
    }
}

/// 一份 HTML 里出现的 span 取值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpanUsage {
    pub cells: usize,
    pub rowspans: BTreeSet<usize>,
    pub colspans: BTreeSet<usize>,
    pub beyond: Vec<(SpanKind, usize)>,
}

impl SpanUsage {
    pub fn max(&self, kind: SpanKind) -> Option<usize> {
        match kind {
            SpanKind::Row => self.rowspans.iter().next_back().copied(),
            SpanKind::Col => self.colspans.iter().next_back().copied(),
        }
    }

    pub fn exceeds(&self) -> bool {
        !self.beyond.is_empty()
    }
}

/// 从词表解析 span 上限。
///
/// 词表里 span 片段写作 ` colspan="2"` / ` rowspan="2"`（**带前导空格**，
/// 见 `docs/09-table-recognition-execution.md` §7.4.1）。
pub fn span_limits(vocabulary: &StructureVocabulary) -> SpanLimits {
    let mut limits = SpanLimits::default();
    for token in vocabulary.tokens() {
        if let Some((kind, value)) = parse_span_token(token) {
            let slot = match kind {
                SpanKind::Row => &mut limits.max_rowspan,
                SpanKind::Col => &mut limits.max_colspan,
            };
            *slot = Some(slot.map_or(value, |current: usize| current.max(value)));
        }
    }
    limits
}

fn parse_span_token(token: &str) -> Option<(SpanKind, usize)> {
    let token = token.trim();
    let (kind, rest) = if let Some(rest) = token.strip_prefix("rowspan=") {
        (SpanKind::Row, rest)
    } else {
        (SpanKind::Col, token.strip_prefix("colspan=")?)
    };
    let digits = rest.strip_prefix('"')?.strip_suffix('"')?;
    digits.parse::<usize>().ok().map(|v| (kind, v))
}

/// 统计一份 HTML 里的 `<td …>` 及其 span 取值；`limits` 为 `None` 时不做超限判定。
pub fn span_usage(html: &str, limits: Option<&SpanLimits>) -> SpanUsage {
    let mut usage = SpanUsage::default();
    for tag in td_tags(html) {
        usage.cells += 1;
        for kind in [SpanKind::Row, SpanKind::Col] {
            let Some(value) = attribute_value(tag, kind.as_str()) else {
                continue;
            };
            let slot = match kind {
                SpanKind::Row => &mut usage.rowspans,
                SpanKind::Col => &mut usage.colspans,
            };
            slot.insert(value);
            if let Some(limit) = limits.and_then(|l| l.max(kind))
                && value > limit
            {
                usage.beyond.push((kind, value));
            }
        }
    }
    usage
}

/// 逐样本枚举 `<td …>` 标签（不含 `</td>`，也不含 `>`）。
fn td_tags(html: &str) -> Vec<&str> {
    let mut tags = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<td") {
        let after = &rest[start..];
        let Some(end) = after.find('>') else { break };
        tags.push(&after[..end]);
        rest = &after[end + 1..];
    }
    tags
}

fn attribute_value(tag: &str, name: &str) -> Option<usize> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    rest[..end].parse::<usize>().ok()
}

/// "因 span 超词表而**必然**出错"的表格统计。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpanExceedance {
    /// 参与统计的表格数。
    pub samples: usize,
    /// 至少有一个 span 超限的表格数。
    pub samples_exceeding: usize,
    /// 超限的单元格数。
    pub cells_exceeding: usize,
    /// 全部单元格数。
    pub cells_total: usize,
    /// 观测到的最大 rowspan（与词表上限比较）。
    pub max_rowspan: Option<usize>,
    /// 观测到的最大 colspan。
    pub max_colspan: Option<usize>,
    /// 词表上限。
    pub limits: SpanLimits,
    /// 超限取值的样例（用于报告里说明"超了多少"）。
    pub examples: Vec<(String, SpanKind, usize)>,
}

impl SpanExceedance {
    /// 超限表格占比（0..1）。
    pub fn fraction(&self) -> f64 {
        if self.samples == 0 {
            return 0.0;
        }
        self.samples_exceeding as f64 / self.samples as f64
    }
}

/// 统计样本集合里的 span 超限情况。`examples` 最多保留 `max_examples` 条。
pub fn summarize_span_exceedance(
    samples: &[&TableSample],
    limits: &SpanLimits,
    max_examples: usize,
) -> SpanExceedance {
    let mut report = SpanExceedance {
        limits: *limits,
        ..SpanExceedance::default()
    };
    for sample in samples {
        report.samples += 1;
        let usage = span_usage(sample.ground_truth(), Some(limits));
        report.cells_total += usage.cells;
        report.cells_exceeding += usage.beyond.len();
        report.max_rowspan = max_opt(report.max_rowspan, usage.max(SpanKind::Row));
        report.max_colspan = max_opt(report.max_colspan, usage.max(SpanKind::Col));
        if usage.exceeds() {
            report.samples_exceeding += 1;
            for (kind, value) in &usage.beyond {
                if report.examples.len() < max_examples {
                    report
                        .examples
                        .push((sample.file_name().to_string(), *kind, *value));
                }
            }
        }
    }
    report
}

fn max_opt(a: Option<usize>, b: Option<usize>) -> Option<usize> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(file_name: &str, html: &str) -> TableSample {
        TableSample {
            record: TableRecord {
                file_name: file_name.to_string(),
                html: html.to_string(),
                html_sha256: digest_hex(html.as_bytes()),
                image_path: format!("cropped_datasets_1Tab_per_img/val/{file_name}"),
                n_cells: 0,
                split: "val".to_string(),
                structure_tokens: 0,
                texts_sha256: String::new(),
            },
            image_path: PathBuf::from("unused.jpg"),
        }
    }

    fn limits(max_rowspan: usize, max_colspan: usize) -> SpanLimits {
        SpanLimits {
            max_rowspan: Some(max_rowspan),
            max_colspan: Some(max_colspan),
        }
    }

    #[test]
    fn span_usage_counts_every_td_and_collects_values() {
        let html = "<html><body><table><tr><td>a</td><td colspan=\"2\">b</td></tr>\
                    <tr><td rowspan=\"3\">c</td></tr></table></body></html>";
        let usage = span_usage(html, Some(&limits(20, 20)));
        assert_eq!(usage.cells, 3);
        assert_eq!(usage.colspans.iter().copied().collect::<Vec<_>>(), vec![2]);
        assert_eq!(usage.rowspans.iter().copied().collect::<Vec<_>>(), vec![3]);
        assert!(!usage.exceeds(), "3 and 2 are both inside the limits");
    }

    #[test]
    fn a_span_beyond_the_vocabulary_is_flagged_with_its_value() {
        // TabRecSet val 里真实存在 colspan="24"，而 SLANet-plus 词表只到 20。
        let html = "<table><tr><td colspan=\"24\">x</td></tr></table>";
        let usage = span_usage(html, Some(&limits(16, 20)));
        assert!(usage.exceeds());
        assert_eq!(usage.beyond, vec![(SpanKind::Col, 24)]);
    }

    #[test]
    fn closing_tags_are_not_counted_as_cells() {
        let html = "<table><tr><td></td><td></td></tr></table>";
        assert_eq!(span_usage(html, None).cells, 2);
    }

    #[test]
    fn span_tokens_are_parsed_from_the_leading_space_form() {
        assert_eq!(parse_span_token(" colspan=\"7\""), Some((SpanKind::Col, 7)));
        assert_eq!(
            parse_span_token(" rowspan=\"12\""),
            Some((SpanKind::Row, 12))
        );
        assert_eq!(parse_span_token("<td>"), None);
        assert_eq!(parse_span_token(" colspan=\"\""), None);
        assert_eq!(parse_span_token(" colspan=\"x\""), None);
    }

    #[test]
    fn exceedance_is_reported_per_table_not_per_cell() {
        let clean = sample("a.jpg", "<table><tr><td>x</td></tr></table>");
        let dirty = sample(
            "b.jpg",
            "<table><tr><td colspan=\"24\">x</td><td colspan=\"21\">y</td></tr></table>",
        );
        let report = summarize_span_exceedance(&[&clean, &dirty], &limits(16, 20), 8);
        assert_eq!(report.samples, 2);
        assert_eq!(
            report.samples_exceeding, 1,
            "only one *table* is necessarily wrong"
        );
        assert_eq!(
            report.cells_exceeding, 2,
            "but two cells are beyond the vocabulary"
        );
        assert_eq!(report.cells_total, 3);
        assert_eq!(report.max_colspan, Some(24));
        assert!((report.fraction() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn selection_is_hash_based_and_reproducible() {
        let samples: Vec<TableSample> = (0..32)
            .map(|i| {
                sample(
                    &format!("s{i}.jpg"),
                    &format!("<table><tr><td>{i}</td></tr></table>"),
                )
            })
            .collect();
        let fixture = TableFixture {
            dataset_root: PathBuf::new(),
            manifest_path: PathBuf::new(),
            samples,
        };
        let first = fixture.select(4);
        let second = fixture.select(4);
        let names = |v: &[&TableSample]| {
            v.iter()
                .map(|s| s.file_name().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&first), names(&second));
        // 不是"取前 4 张"：那会让 s0..s3 恰好被选中，从而掩盖"顺序被当成随机性"的退化。
        assert_ne!(names(&first), vec!["s0.jpg", "s1.jpg", "s2.jpg", "s3.jpg"]);
    }

    #[test]
    fn a_record_without_a_page_shell_is_rejected() {
        let record = TableRecord {
            file_name: "a.jpg".to_string(),
            html: "<table><tr><td>x</td></tr></table>".to_string(),
            html_sha256: "0".repeat(64),
            image_path: "x.jpg".to_string(),
            n_cells: 1,
            split: "val".to_string(),
            structure_tokens: 1,
            texts_sha256: String::new(),
        };
        let error = record.validate().expect_err("must reject");
        assert!(error.to_string().contains("not a full page"), "{error}");
    }

    #[test]
    fn the_sample_set_digest_is_order_independent_but_not_content_blind() {
        let samples: Vec<TableSample> = (0..4)
            .map(|i| {
                sample(
                    &format!("s{i}.jpg"),
                    &format!("<table><tr><td>{i}</td></tr></table>"),
                )
            })
            .collect();
        let forward: Vec<&TableSample> = samples.iter().collect();
        let mut reversed: Vec<&TableSample> = samples.iter().collect();
        reversed.reverse();
        assert_eq!(
            TableFixture::sample_set_digest(&forward),
            TableFixture::sample_set_digest(&reversed),
            "排序后取哈希，顺序不得影响结果"
        );
        let smaller = TableFixture::sample_set_digest(&forward[..3]);
        assert_ne!(
            TableFixture::sample_set_digest(&forward),
            smaller,
            "样本集变了摘要就必须变，否则它证明不了任何事"
        );
    }

    #[test]
    fn a_content_digest_is_withheld_rather_than_faked_when_an_image_is_unreadable() {
        let temp = crate::test_support::TempDir::new("table-content-digest");
        temp.write("a.jpg", b"first");
        temp.write("b.jpg", b"second");

        let readable: Vec<TableSample> = ["a.jpg", "b.jpg"]
            .iter()
            .map(|name| {
                let mut s = sample(name, "<table><tr><td>x</td></tr></table>");
                s.image_path = temp.path().join(name);
                s
            })
            .collect();
        let fixture = TableFixture {
            dataset_root: temp.path().to_path_buf(),
            manifest_path: PathBuf::new(),
            samples: readable.clone(),
        };
        let refs: Vec<&TableSample> = readable.iter().collect();
        let digest = fixture
            .content_digest(&refs)
            .expect("reading two small files must not fail")
            .expect("both images are readable");
        assert_eq!(digest.len(), 64);

        // 少一张图 → `None`，不是"用剩下的算一个"。
        let mut broken = readable.clone();
        broken[1].image_path = temp.path().join("does-not-exist.jpg");
        let fixture = TableFixture {
            dataset_root: temp.path().to_path_buf(),
            manifest_path: PathBuf::new(),
            samples: broken.clone(),
        };
        let refs: Vec<&TableSample> = broken.iter().collect();
        assert_eq!(fixture.content_digest(&refs).expect("must not error"), None);
    }
}
