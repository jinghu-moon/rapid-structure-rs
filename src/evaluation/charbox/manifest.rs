//! 字符定位基准数据集的读取与自检（docs/13）。
//!
//! # 为什么读取时要重新校验一遍
//!
//! 数据集是**生成**的，不入库（渲染结果绑定本机字体版本）。于是"报告里的数字"与
//! "当时那批图"之间唯一的纽带就是 manifest 里每张图的 sha256。不校验它，一次改文本、
//! 改字号或换了字体版本的重新生成会让旧报告继续被引用，而图已经换了 —— 这正是
//! `docs/12` 里反复出现的"静默复用另一份东西的产物"这一类错误。
//!
//! # 结构自检而不是像素自检
//!
//! "GT 框是不是真的框住了墨迹"由生成器负责（它量的是自己刚画出来的像素），不在这里
//! 用第二种语言再实现一遍 —— 那只会留下"两份实现谁对"这个无法回答的问题。
//! 这里查的是**与像素无关**的不变量：字符顺序、框是否落在自己那一行里、拼接是否等于
//! 行文本、计数是否自洽。这些一旦破了，后面所有指标都会静默失真。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{RapidOcrError, Result};
use crate::evaluation::charbox::metrics::{LineFrame, Quad};
use crate::evaluation::digest::digest_hex;

pub const DEFAULT_DATASET_RELATIVE: &str = "datasets/text/charbox";
pub const SUPPORTED_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub generator: String,
    pub case_count: usize,
    pub line_count: usize,
    pub char_count: usize,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub image: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub lines: Vec<GroundLine>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroundLine {
    pub text: String,
    pub group: String,
    pub font: String,
    pub script: String,
    pub px: u32,
    pub angle: f32,
    pub layout: String,
    pub spacing: f32,
    /// 行四边形（原图坐标）。字段名与 manifest 一致，不是 Rust 风格，因此显式 rename。
    #[serde(rename = "line")]
    pub quad: Quad,
    pub chars: Vec<GroundChar>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroundChar {
    #[serde(rename = "char")]
    pub ch: String,
    /// `ink` = 由列窗口内的墨迹像素导出；`metric` = 该窗口没有墨迹、退回字体度量。
    /// 比例本身是基准可信度的一部分，因此报告要把它算出来、写出来。
    #[serde(default)]
    pub derived: Option<String>,
    #[serde(rename = "box")]
    pub quad: Quad,
}

impl Manifest {
    /// 读取并校验 manifest 与它点名的每一张图。
    ///
    /// 任何一处不符都是**错误**：一份悄悄过期的一角，比一次跑不完的运行危险得多。
    pub fn load_verified(root: &Path) -> Result<Self> {
        let path = root.join("manifest.json");
        let text = std::fs::read_to_string(&path).map_err(|error| {
            RapidOcrError::InvalidInput(format!(
                "cannot read the charbox manifest {}: {error}. Generate it with \
                 `python tools/build_charbox_dataset.py --out {}`",
                path.display(),
                root.display()
            ))
        })?;
        let manifest: Manifest = serde_json::from_str(&text).map_err(|error| {
            RapidOcrError::InvalidInput(format!(
                "the charbox manifest {} is not the shape this harness reads: {error}",
                path.display()
            ))
        })?;
        manifest.verify_root(root)?;
        Ok(manifest)
    }

    fn verify_root(&self, root: &Path) -> Result<()> {
        let mut problems: Vec<String> = Vec::new();
        if self.schema != SUPPORTED_SCHEMA {
            problems.push(format!(
                "manifest schema is {}, this harness reads {SUPPORTED_SCHEMA}",
                self.schema
            ));
        }
        if self.cases.len() != self.case_count {
            problems.push(format!(
                "case_count says {} but the manifest carries {} case(s)",
                self.case_count,
                self.cases.len()
            ));
        }
        let lines = self
            .cases
            .iter()
            .map(|case| case.lines.len())
            .sum::<usize>();
        if lines != self.line_count {
            problems.push(format!(
                "line_count says {} but the cases hold {lines}",
                self.line_count
            ));
        }
        let chars = self
            .cases
            .iter()
            .flat_map(|case| &case.lines)
            .map(|line| line.chars.len())
            .sum::<usize>();
        if chars != self.char_count {
            problems.push(format!(
                "char_count says {} but the cases hold {chars}",
                self.char_count
            ));
        }
        let mut names = BTreeSet::new();
        for case in &self.cases {
            if !names.insert(case.image.as_str()) {
                problems.push(format!("{} is listed twice", case.image));
            }
            let path: PathBuf = root.join(&case.image);
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    problems.push(format!("cannot read {}: {error}", path.display()));
                    continue;
                }
            };
            let digest = digest_hex(&bytes);
            if digest != case.sha256 {
                problems.push(format!(
                    "{} was regenerated or edited after the manifest was written: sha256 is \
                     {digest}, the manifest says {}",
                    case.image, case.sha256
                ));
            }
            let (width, height) = match image::image_dimensions(&path) {
                Ok(size) => size,
                Err(error) => {
                    problems.push(format!("{} cannot be decoded: {error}", case.image));
                    continue;
                }
            };
            if (width, height) != (case.width, case.height) {
                problems.push(format!(
                    "{} is {width}x{height}, the manifest says {}x{}",
                    case.image, case.width, case.height
                ));
            }
            case.verify(case.width, case.height, &mut problems);
        }
        if !problems.is_empty() {
            return Err(RapidOcrError::InvalidInput(format!(
                "the charbox benchmark dataset at {} is not usable ({} problem(s)):\n  {}",
                root.display(),
                problems.len(),
                problems.join("\n  ")
            )));
        }
        Ok(())
    }

    /// 数据集里出现过的分组名（排序后），报告的分组段落直接按它展开。
    pub fn groups(&self) -> Vec<String> {
        let mut groups: BTreeSet<String> = BTreeSet::new();
        for case in &self.cases {
            for line in &case.lines {
                groups.insert(line.group.clone());
            }
        }
        groups.into_iter().collect()
    }

    pub fn total_gt_chars(&self) -> usize {
        self.cases
            .iter()
            .flat_map(|case| &case.lines)
            .map(|line| line.chars.len())
            .sum()
    }
}

impl Case {
    fn verify(&self, width: u32, height: u32, problems: &mut Vec<String>) {
        for (line_index, line) in self.lines.iter().enumerate() {
            let located = format!("{} line[{line_index}] ({:?})", self.image, line.text);
            if line.chars.is_empty() {
                problems.push(format!("{located} has no ground-truth characters"));
                continue;
            }
            let joined: String = line.chars.iter().map(|ch| ch.ch.as_str()).collect();
            let expected: String = line.text.chars().filter(|ch| !ch.is_whitespace()).collect();
            if joined != expected {
                problems.push(format!(
                    "{located}: concatenating the character boxes gives {joined:?}, the line text \
                     without spaces is {expected:?}"
                ));
            }
            for ch in &line.chars {
                if ch.ch.chars().count() != 1 {
                    problems.push(format!("{}: {:?} is not one character", located, ch.ch));
                }
                if let Some(derived) = &ch.derived
                    && derived != "ink"
                    && derived != "metric"
                {
                    problems.push(format!(
                        "{located}: derived is {derived:?}, which is neither \"ink\" nor \"metric\""
                    ));
                }
            }
            let Some(frame) = LineFrame::from_quad(&line.quad, &line.layout) else {
                problems.push(format!("{located}: its line quad is degenerate"));
                continue;
            };
            let line_extent = frame.project(&line.quad);
            if !line.quad.iter().flatten().all(|value| value.is_finite()) {
                problems.push(format!("{located}: its line quad has a non-finite point"));
                continue;
            }
            let mut previous_center: Option<f32> = None;
            for ch in &line.chars {
                if !ch.quad.iter().flatten().all(|value| value.is_finite()) {
                    problems.push(format!("{}: {:?} has a non-finite box", located, ch.ch));
                    continue;
                }
                for [x, y] in ch.quad {
                    if x < -0.5 || y < -0.5 || x > width as f32 + 0.5 || y > height as f32 + 0.5 {
                        problems.push(format!(
                            "{}: {:?} box point ({x}, {y}) is outside the {}x{} image",
                            located, ch.ch, width, height
                        ));
                    }
                }
                let extent = frame.project(&ch.quad);
                // 1px 容差：墨迹框是从整数像素量出来的，行四边形是浮点旋转后的结果。
                if extent.along.0 < line_extent.along.0 - 1.0
                    || extent.along.1 > line_extent.along.1 + 1.0
                    || extent.cross.0 < line_extent.cross.0 - 1.0
                    || extent.cross.1 > line_extent.cross.1 + 1.0
                {
                    problems.push(format!(
                        "{}: {:?} box escapes its own line quad",
                        located, ch.ch
                    ));
                }
                let center = (extent.along.0 + extent.along.1) / 2.0;
                if let Some(previous) = previous_center
                    && center <= previous
                {
                    problems.push(format!(
                        "{}: {:?} is not ordered along the line (center {center} after \
                         {previous}); a benchmark cannot score a character order the \
                         generator did not draw",
                        located, ch.ch
                    ));
                }
                previous_center = Some(center);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Case, GroundChar, GroundLine, Manifest};

    fn line(text: &str, chars: &[&str]) -> GroundLine {
        let width = 20.0 * chars.len() as f32;
        GroundLine {
            text: text.to_string(),
            group: "unit".to_string(),
            font: "simsun".to_string(),
            script: "cn".to_string(),
            px: 20,
            angle: 0.0,
            layout: "horizontal".to_string(),
            spacing: 0.0,
            quad: [[0.0, 0.0], [width, 0.0], [width, 30.0], [0.0, 30.0]],
            chars: chars
                .iter()
                .enumerate()
                .map(|(index, ch)| GroundChar {
                    ch: (*ch).to_string(),
                    derived: Some("ink".to_string()),
                    quad: [
                        [20.0 * index as f32, 2.0],
                        [20.0 * index as f32 + 18.0, 2.0],
                        [20.0 * index as f32 + 18.0, 28.0],
                        [20.0 * index as f32, 28.0],
                    ],
                })
                .collect(),
        }
    }

    fn manifest(cases: Vec<Case>) -> Manifest {
        let line_count = cases.iter().map(|case| case.lines.len()).sum();
        let char_count = cases
            .iter()
            .flat_map(|case| &case.lines)
            .map(|line| line.chars.len())
            .sum();
        Manifest {
            schema: super::SUPPORTED_SCHEMA,
            generator: "test".to_string(),
            case_count: cases.len(),
            line_count,
            char_count,
            cases,
        }
    }

    fn case(lines: Vec<GroundLine>) -> Case {
        Case {
            image: "pages/none.png".to_string(),
            sha256: "0".repeat(64),
            width: 400,
            height: 200,
            lines,
        }
    }

    #[test]
    fn a_well_formed_line_passes_the_structural_checks() {
        let mut problems = Vec::new();
        case(vec![line("ab", &["a", "b"])]).verify(400, 200, &mut problems);
        assert!(problems.is_empty(), "unexpected problems: {problems:?}");
    }

    #[test]
    fn character_boxes_must_rebuild_the_line_text() {
        let mut problems = Vec::new();
        case(vec![line("ax", &["a", "b"])]).verify(400, 200, &mut problems);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("concatenating the character boxes")),
            "a mismatched concatenation must be named, got {problems:?}"
        );
    }

    #[test]
    fn characters_must_be_listed_in_the_order_they_were_drawn() {
        let mut line = line("ab", &["a", "b"]);
        line.chars.swap(0, 1);
        let mut problems = Vec::new();
        case(vec![line]).verify(400, 200, &mut problems);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("not ordered")),
            "a swapped character order must be refused, got {problems:?}"
        );
    }

    #[test]
    fn a_box_outside_its_line_is_a_generator_bug_not_a_measurement() {
        let mut line = line("ab", &["a", "b"]);
        line.chars[1].quad = [[300.0, 2.0], [318.0, 2.0], [318.0, 28.0], [300.0, 28.0]];
        let mut problems = Vec::new();
        case(vec![line]).verify(400, 200, &mut problems);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("escapes its own line quad")),
            "an escaping box must be reported, got {problems:?}"
        );
    }

    #[test]
    fn the_declared_counts_have_to_match_the_payload() {
        let mut subject = manifest(vec![case(vec![line("ab", &["a", "b"])])]);
        subject.char_count += 1;
        let error = subject
            .verify_root(std::path::Path::new("."))
            .expect_err("a wrong count must stop the run");
        assert!(
            error.to_string().contains("char_count says 3"),
            "the error must quote the counts, got: {error}"
        );
    }

    #[test]
    fn an_unsupported_schema_is_refused_before_anything_is_scored() {
        let mut subject = manifest(Vec::new());
        subject.schema = 99;
        let error = subject
            .verify_root(std::path::Path::new("."))
            .expect_err("a future schema must not be read as this one");
        assert!(
            error.to_string().contains("schema is 99"),
            "the error must name the schema, got: {error}"
        );
    }
}
