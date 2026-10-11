//! P0-0 小型文本回归 fixture：把"没丢文字"这条判据从本机 `datasets/` 依赖里解出来。
//!
//! 评审 §8.2 的四图字符多重集门禁（real / region61 / region62 / table.jpg）依赖**未入库**
//! 的 `datasets/text`、`datasets/table`，因此只能当本机证据。这里改用一个**随仓库提交**的
//! 小尺寸裁图（`tests/fixtures/text-min/*.png`）+ 一份**期望字符多重集**
//! （`expected_char_multiset.json`），并强制 CPU provider（同一模型逐位确定）：
//!
//! - 判据是"**只增不减**"：当前管线在每张裁图上识别出的每个字符，出现次数必须
//!   **不少于**期望值。任何让文字丢失的重构（分位/编排/输入物化改动意外丢区域、丢字符）
//!   都会让它变红，而多识别出来的字符不算回归。
//! - 模型是仓库外资产（见 `src/test_support.rs` 的约定）：`models/text/small` 不在时
//!   **skip**，绝不 panic、也不回落开发机绝对路径。带模型的机器上它就是硬门禁。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rapid_structure_rs::{
    ClassifierPlan, ClassifierPolicy, DetectionPolicy, EngineConfig, ImageInput, OcrEngine,
    OcrRequest, OutputPolicy, PreprocessPolicy, RapidOcrEngine, RecognitionPolicy, StagePlan,
    TablePolicy, WordOutputMode,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// 期望字符多重集：`{ "<image>.png": { "<char>": count } }`。
fn load_expected(path: &Path) -> BTreeMap<String, BTreeMap<String, u64>> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read expected multiset {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "expected multiset {} must be valid JSON: {error}",
            path.display()
        )
    })
}

/// 跑一次 CPU 文本管线，返回识别文本的非空白字符计数。
fn recognized_multiset(
    engine: &mut RapidOcrEngine,
    image: &Path,
    max_side: u32,
) -> BTreeMap<char, u64> {
    let bytes = std::fs::read(image)
        .unwrap_or_else(|error| panic!("read crop {}: {error}", image.display()));
    let request = OcrRequest {
        input: ImageInput::Encoded(Arc::from(bytes)),
        roi: None,
        scale_hint: None,
        stages: StagePlan {
            detect: true,
            classify: ClassifierPlan {
                policy: ClassifierPolicy::IfAvailable,
                apply_rotation: true,
            },
            recognize: true,
        },
        preprocess: PreprocessPolicy {
            max_side: Some(max_side),
            ..PreprocessPolicy::default()
        },
        detection: DetectionPolicy::default(),
        recognition: RecognitionPolicy {
            words: WordOutputMode::Off,
        },
        output: OutputPolicy::default(),
        formula: rapid_structure_rs::FormulaPolicy::default(),
        table: TablePolicy::default(),
    };
    let output = engine
        .recognize(request)
        .unwrap_or_else(|error| panic!("recognize {}: {error}", image.display()));
    let mut counts: BTreeMap<char, u64> = BTreeMap::new();
    for region in &output.regions {
        if let Some(recognition) = &region.recognition {
            for ch in recognition.text.chars() {
                if !ch.is_whitespace() {
                    *counts.entry(ch).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}

#[test]
fn text_min_fixture_recognition_does_not_lose_characters() {
    let root = repo_root();
    let fixtures = root.join("tests/fixtures/text-min");
    let config_path = fixtures.join("config-small-cpu.yaml");

    // 模型是仓库外资产：任一缺失即 skip（与 `src/test_support.rs` 同规则，不回落绝对路径）。
    let missing = |rel: &str| !root.join(rel).is_file();
    let required = [
        "models/text/small/PP-OCRv6_det_small.onnx",
        "models/text/small/PP-OCRv6_rec_small.onnx",
        "models/text/small/ppocrv6_dict.txt",
    ];
    if required.iter().any(|rel| missing(rel)) {
        eprintln!(
            "skipping test: text-min regression needs models/text/small (PP-OCRv6 det/rec + dict); \
             place them under `models/text/small` to run this character-multiset gate"
        );
        return;
    }

    let config = EngineConfig::from_yaml_file(&config_path)
        .unwrap_or_else(|error| panic!("load {}: {error}", config_path.display()));
    let max_side = config.global.max_side_len as u32;
    let mut engine =
        RapidOcrEngine::new(config).unwrap_or_else(|error| panic!("build text engine: {error}"));

    let expected = load_expected(&fixtures.join("expected_char_multiset.json"));
    assert!(
        !expected.is_empty(),
        "the committed expectation must cover at least one crop"
    );

    for (image_name, expected_counts) in &expected {
        let image = fixtures.join(image_name);
        let actual = recognized_multiset(&mut engine, &image, max_side);

        assert!(
            !actual.is_empty(),
            "{image_name}: the pipeline recognized no text at all — the page read path is broken"
        );

        // 只增不减：期望里每个字符的计数都必须被当前结果覆盖。
        let mut lost: Vec<String> = Vec::new();
        for (ch, want) in expected_counts {
            assert_eq!(
                ch.chars().count(),
                1,
                "expectation keys must be single chars"
            );
            let have = ch
                .chars()
                .next()
                .map(|c| actual.get(&c).copied().unwrap_or(0))
                .unwrap_or(0);
            if have < *want {
                lost.push(format!("'{ch}': want {want}, got {have}"));
            }
        }
        assert!(
            lost.is_empty(),
            "{image_name}: the refactor lost text characters (multiset must only grow): {lost:?}"
        );
    }
}
