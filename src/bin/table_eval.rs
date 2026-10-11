//! 表格结构评测 CLI（结构口径 TEDS-Struct 的预测产物）。
//!
//! 本文件是**薄壳**：链路在 `rapidstruct_rs::evaluation::table::run` 里，因为
//! `rapidstruct evaluate`（`src/bin/rapidstruct.rs`）需要跑**同一份**实现
//! （`docs/09-table-recognition-execution.md` §9.2）。这里只负责参数解析、
//! 把进度打到 stderr、把报告写到 `--report`。
//!
//! 用法：
//!
//! ```text
//! cargo run --release --bin table_eval -- --report Temp/table-report-val.json
//! python tools/table_teds.py --true datasets/table/tabrecset/derived/pubtabnet_val_gt_html.json \
//!     --pred Temp/table-pred-val.json --structure-only
//! ```
//!
//! # 打分不在 Rust 里做
//!
//! 官方 `metric.py` 的树编辑距离语义**就是** TEDS 的定义（见
//! `tools/table_teds.py` 里固定了字节数与 sha256 的那份实现）。在 Rust 里再实现
//! 一遍会制造"两个实现谁对"这个无法回答的问题。

use std::path::PathBuf;

use clap::Parser;

use rapid_structure_rs::TableOcrResolution;
use rapid_structure_rs::evaluation::table::run::{
    DEFAULT_MODEL_RELATIVE, TableContentSource, TableEvalConfig, TableEvalEvent,
    run_table_evaluation,
};

#[derive(Debug, Parser)]
#[command(
    name = "table_eval",
    about = "Run the table-structure model over TabRecSet val and emit TEDS predictions"
)]
struct Cli {
    /// 数据集根目录（含 `derived/pubtabnet_val.jsonl`）。
    #[arg(long, default_value = "datasets/table/tabrecset")]
    dataset: PathBuf,

    /// 结构模型路径。
    #[arg(long, default_value = DEFAULT_MODEL_RELATIVE)]
    model: PathBuf,

    /// 预测输出（官方 `metric.py` 的 `pred_json`：`{file_name: html}`）。
    #[arg(long, default_value = "Temp/table-pred-val.json")]
    predictions: PathBuf,

    /// 报告输出。
    #[arg(long, default_value = "Temp/table-report-val.json")]
    report: PathBuf,

    /// 只跑前 N 个稳定哈希样本；0 = 全量。
    #[arg(long, default_value_t = 0)]
    limit: usize,

    /// 结构模型输入边长。
    #[arg(long, default_value_t = 488)]
    side_len: usize,

    /// 不固定输入 H/W（只用于对照，会失去 E5 的逐位可复现性）。
    #[arg(long)]
    no_fix_shape: bool,

    /// ORT intra 线程数（同时把 inter 定为 1 并关掉自动调优）。
    #[arg(long)]
    intra_threads: Option<usize>,

    /// 遇到第一个失败样本就退出。
    #[arg(long)]
    fail_fast: bool,

    /// 单元格文本来源：`structure-only`（只报 TEDS-Struct）、`page-reuse`（复用整页 OCR，
    /// 报完整 TEDS）或 `crop-control`（逐单元格裁剪喂识别器 —— 对照口径，不是生产路径）。
    #[arg(
        long,
        default_value = "structure-only",
        value_parser = ["structure-only", "page-reuse", "crop-control"]
    )]
    content: String,

    /// `--content page-reuse` 时的页级 OCR 引擎配置 YAML；
    /// `--content crop-control` 时的识别器引擎配置 YAML。
    #[arg(long)]
    ocr_config: Option<PathBuf>,

    /// `--content page-reuse` 时的页级 OCR 结果缓存（命中就不重跑 OCR）。
    ///
    /// 缓存里记着产出它的**引擎指纹**（含解析后的分辨率与 provider）。换分辨率或换
    /// 配置后指纹不符会直接报错，而不是静默复用另一份引擎产出的文本 ——
    /// 所以做分辨率对照实验时，每个档位给一个自己的缓存文件。
    #[arg(long)]
    ocr_cache: Option<PathBuf>,

    /// 表格路径自己的 det `limit_side_len`（不继承 `--ocr-config` 里的值）。
    ///
    /// 默认值取自 `TableOcrResolution::default()`，不写第二遍字面量 —— 否则改了库里的
    /// 默认值之后 CLI 会继续用旧值，而报告里只会记 CLI 传进去的那个数。
    #[arg(long, default_value_t = TableOcrResolution::default().limit_side_len)]
    ocr_limit_side_len: usize,

    /// 表格路径自己的 det `limit_type`：`min`（把短边放大到 `limit_side_len`）
    /// 或 `max`（把长边压到它）。
    #[arg(long, default_value_t = TableOcrResolution::default().limit_type, value_parser = ["min", "max"])]
    ocr_limit_type: String,

    /// 表格路径自己的整页缩放上限（覆盖 `--ocr-config` 的 `global.max_side_len`）。
    #[arg(long, default_value_t = TableOcrResolution::default().max_side_len)]
    ocr_max_side_len: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let content = match cli.content.as_str() {
        "page-reuse" => TableContentSource::PageReuse {
            config: cli.ocr_config,
            cache: cli.ocr_cache,
        },
        "crop-control" => TableContentSource::CropControl {
            config: cli.ocr_config,
        },
        _ => TableContentSource::StructureOnly,
    };
    let config = TableEvalConfig {
        dataset: cli.dataset,
        model: cli.model,
        predictions: cli.predictions,
        limit: cli.limit,
        side_len: cli.side_len,
        fix_input_shape: !cli.no_fix_shape,
        intra_threads: cli.intra_threads,
        fail_fast: cli.fail_fast,
        content,
        ocr_resolution: TableOcrResolution {
            limit_side_len: cli.ocr_limit_side_len,
            limit_type: cli.ocr_limit_type,
            max_side_len: cli.ocr_max_side_len,
        },
    };

    let outcome = run_table_evaluation(&config, &mut |event| match event {
        TableEvalEvent::Message(text) => eprintln!("{text}"),
        TableEvalEvent::Progress { done, total } => eprintln!("  {done}/{total}"),
    })?;

    if let Some(parent) = cli.report.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cli.report, serde_json::to_vec_pretty(&outcome.report)?)?;

    eprintln!(
        "wrote {} ({} selected, {} failed)",
        cli.report.display(),
        outcome.selected,
        outcome.failed
    );
    eprintln!(
        "score it with: python tools/table_teds.py --true {} --pred {}{}",
        rapid_structure_rs::evaluation::table::DEFAULT_GT_HTML_RELATIVE,
        config.predictions.display(),
        // 结构口径要显式告诉 `metric.py` 只比结构；完整口径（复用/对照）不能加这个开关。
        if config.content.uses_engine() {
            ""
        } else {
            " --structure-only"
        }
    );
    Ok(())
}
