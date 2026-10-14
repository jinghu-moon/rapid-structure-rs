//! 字符定位基准 CLI（docs/13：逐字框几何的**改前基线**）。
//!
//! 本文件是**薄壳**：度量、闸门与报告都在 `rapid_structure_rs::evaluation::charbox` 里，
//! 因为契约测试与后续 CTC 对齐改造要调用的是同一份实现，不能各算各的。这里只负责
//! 参数解析、把进度打到 stderr、把报告写到 `--report`。
//!
//! 用法：
//!
//! ```text
//! python tools/build_charbox_dataset.py
//! cargo run --release --bin charbox_eval -- --report tests/baseline/windows-baseline/charbox-ctc-baseline.json
//! ```
//!
//! # 为什么默认要跑三档
//!
//! Step 1 接通的 `words=off|words|chars` 是有代价的：逐字框要额外做一遍 CTC 后处理
//! 与单应变换，并直接抬高响应体。只报 `chars` 一档无法回答"这个能力值多少"，
//! 所以三档各跑 `--repetitions` 遍，报告里给出**逐页配对**的两两差值。
//!
//! 配对的强度不是风格问题：整页 `total_ms` 由检测主导，档位差淹在跨页方差里
//! （`docs/13` §4.3：同一对档位在两次全量运行里符号都会翻）。可分离的那一项是
//! 识别阶段的 `rec_postprocess_ms`，逐字框就在那里面算。

use std::path::PathBuf;

use clap::Parser;

use rapid_structure_rs::evaluation::charbox::run::{
    CharBoxEvalConfig, CharBoxEvent, DEFAULT_DATASET_RELATIVE, run_charbox_evaluation,
};
use rapid_structure_rs::{RapidOcrError, WordOutputMode};

fn parse_mode(raw: &str) -> Result<WordOutputMode, String> {
    match raw.trim() {
        "off" => Ok(WordOutputMode::Off),
        "words" => Ok(WordOutputMode::Words),
        "chars" => Ok(WordOutputMode::Chars),
        other => Err(format!(
            "unknown words mode `{other}`; expected off, words or chars"
        )),
    }
}

/// `--modes` 的后置校验。与 `run_charbox_evaluation()` 里的拒绝同源，但在这里报错
/// 能立刻指出是哪个参数写错了，而不是让读者以为基准本身跑失败了。
fn require_chars(modes: &[WordOutputMode]) -> Result<(), String> {
    if modes.contains(&WordOutputMode::Chars) {
        Ok(())
    } else {
        Err(
            "`--modes` must include `chars`: this benchmark scores character geometry, \
             and without per-char boxes there is nothing to measure"
                .to_string(),
        )
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "charbox_eval",
    about = "Measure per-character box geometry (IoU, boundary/center error, selection P/R) and the cost of words=words|chars"
)]
struct Cli {
    /// 数据集根目录（由 `tools/build_charbox_dataset.py` 生成，含 `manifest.json`）。
    #[arg(long, default_value = DEFAULT_DATASET_RELATIVE)]
    dataset: PathBuf,

    /// 页级 OCR 引擎配置 YAML。
    #[arg(long, default_value = "models/configs/test-config.yaml")]
    config: PathBuf,

    /// 报告输出。
    #[arg(long, default_value = "Temp/charbox-baseline.json")]
    report: PathBuf,

    /// 每档模式重复遍数（取分位数用；1 遍无法区分第一张图的冷启动与稳定耗时）。
    #[arg(long, default_value_t = 3)]
    repetitions: usize,

    /// 行级匹配 IoU 阈值。
    #[arg(long, default_value_t = rapid_structure_rs::evaluation::charbox::run::DEFAULT_LINE_IOU)]
    line_iou: f32,

    /// 只跑前 N 页；0 = 全量。冒烟用，出基线必须全量。
    #[arg(long, default_value_t = 0)]
    limit: usize,

    /// 参与成本测量的档位，逗号分隔。必须含 `chars`。
    #[arg(
        long,
        default_value = "off,words,chars",
        value_delimiter = ',',
        value_parser = parse_mode
    )]
    modes: Vec<WordOutputMode>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    require_chars(&cli.modes).map_err(RapidOcrError::InvalidInput)?;
    let config = CharBoxEvalConfig {
        dataset: cli.dataset,
        engine_config: cli.config,
        repetitions: cli.repetitions,
        line_iou: cli.line_iou,
        limit: cli.limit,
        modes: cli.modes,
    };

    let outcome = run_charbox_evaluation(&config, &mut |event| match event {
        CharBoxEvent::Message(text) => eprintln!("{text}"),
        CharBoxEvent::Progress { done, total } => eprintln!("  {done}/{total}"),
    })?;

    if let Some(parent) = cli.report.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cli.report, serde_json::to_vec_pretty(&outcome.report)?)?;

    eprintln!(
        "wrote {} ({} page(s), {} line(s) matched, {} char(s) scored, {} gate failure(s))",
        cli.report.display(),
        outcome.pages,
        outcome.lines_matched,
        outcome.chars_scored,
        outcome.gate_failures
    );
    Ok(())
}
