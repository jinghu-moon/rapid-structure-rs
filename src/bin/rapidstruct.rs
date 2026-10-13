use clap::{Parser, Subcommand};
use rapid_structure_rs::evaluation::ocr::{
    EvaluationCase, EvaluationReport, EvaluationSummary, evaluate_case,
};
use rapid_structure_rs::{
    ClassifierPlan, ClassifierPolicy, DetectionPolicy, EngineConfig, FormulaPolicy, ImageInput,
    OcrEngine, OcrRequest, OutputPolicy, PreprocessPolicy, RapidOcrEngine, RecognitionPolicy,
    ReportMode, StagePlan, TablePolicy, TextOrder, WordOutputMode, render_output_report,
    to_output_items, to_output_json, to_output_markdown,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// `rapidstruct serve` 的服务端（M1：HTTP 层 + 静态页 + 双队列 + 有界结果存储）。
///
/// 只在启用 `serve` feature 时参与编译；`serve` **不在** `default` 里，
/// 因此默认构建的依赖图与代码路径都不受影响（§2.1 的硬性约束）。
#[cfg(feature = "serve")]
#[path = "serve/mod.rs"]
mod serve;

/// 页面级公式路由的 CLI 选项。
#[derive(Debug, Clone, Default, clap::Args)]
struct FormulaArgs {
    /// 公式识别模型（`pp_formulanet_plus_m.onnx`）；给出即启用公式路由。
    #[arg(long = "formula-model", value_name = "ONNX")]
    model: Option<PathBuf>,
    /// 页面公式检测模型（`pix2text-mfd-1.5.onnx`）；不给出时只处理显式区域。
    #[arg(long = "formula-detector", value_name = "ONNX")]
    detector: Option<PathBuf>,
    /// 公式识别模型 SHA-256 校验。
    #[arg(long = "formula-sha256")]
    sha256: Option<String>,
    /// 公式检测置信度阈值。
    #[arg(long = "formula-confidence", default_value_t = 0.25)]
    confidence: f32,
    /// 每页最多保留的公式区域数。
    #[arg(long = "formula-max-regions", default_value_t = 64)]
    max_regions: usize,
    /// 在 JSON 输出中保留公式原始 token IDs。
    #[arg(long = "formula-token-ids")]
    token_ids: bool,
}

impl FormulaArgs {
    fn policy(&self) -> FormulaPolicy {
        match &self.model {
            Some(model) => FormulaPolicy {
                enabled: true,
                model_path: Some(model.clone()),
                expected_model_sha256: self.sha256.clone(),
                detector_path: self.detector.clone(),
                confidence_threshold: self.confidence,
                max_regions: self.max_regions,
                include_token_ids: self.token_ids,
                ..FormulaPolicy::default()
            },
            None => FormulaPolicy::default(),
        }
    }
}

/// `recognize` 的输出通道（`docs/11` §3 E15.3 的人工比对入口）。
///
/// 三条通道共用同一份 `OcrOutput`，所以这个开关只影响**渲染**，不影响识别：
/// 需要对比表格语义一致性时，同一张图分别用 `--format json|html|markdown` 跑三次即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum OutputFormat {
    /// 纯文本：按阅读顺序的行（表格区域不贡献文本，见 `docs/11` §3 E15.4）。
    Plain,
    /// 完整 JSON：`items[]` + `tables[]`。
    Json,
    /// 自包含 HTML 报告（表格渲染为**真的** `<table>`，已净化）。
    Html,
    /// Markdown：表格渲染为逻辑网格，跨度损失有显式说明行。
    Markdown,
}

/// 表格路由参数。给 `--table-model` 才启用；不给时页面 OCR 行为与无表格功能完全一致。
#[derive(Debug, clap::Args)]
struct TableArgs {
    #[arg(long, value_name = "MODEL")]
    table_model: Option<PathBuf>,
    #[arg(long, value_name = "N")]
    table_intra_threads: Option<usize>,
}

impl TableArgs {
    fn policy(&self) -> TablePolicy {
        match &self.table_model {
            Some(model_path) => TablePolicy {
                enabled: true,
                model_path: Some(model_path.clone()),
                intra_threads: self.table_intra_threads,
                ..TablePolicy::default()
            },
            None => TablePolicy::default(),
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "rapidstruct", about = "Run PaddleOCR ONNX models")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Debug, Subcommand)]
enum Command {
    Run {
        #[arg(long, value_name = "IMAGE")]
        img_path: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        words: bool,
        #[arg(long)]
        chars: bool,
        /// 等价于 `--format json`（保留旧写法）。
        #[arg(long, conflicts_with = "format")]
        json: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Plain)]
        format: OutputFormat,
        #[command(flatten)]
        formula: FormulaArgs,
        #[command(flatten)]
        table: TableArgs,
    },
    Report {
        #[arg(long = "input-dir")]
        input_dir: PathBuf,
        #[arg(long = "output-dir")]
        output_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        #[command(flatten)]
        formula: FormulaArgs,
        #[command(flatten)]
        table: TableArgs,
    },
    Evaluate {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, default_value_t = 0.5)]
        iou_threshold: f32,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Check,
    /// 本地 Web 评估界面（docs/05）。需要 `--features serve` 构建。
    #[cfg(feature = "serve")]
    Serve(serve::ServeArgs),
    /// 未启用 `serve` feature 时仍然**可解析**，但给出可定位错误（§3）。
    #[cfg(not(feature = "serve"))]
    Serve(UnavailableServeArgs),
}

/// `serve` 在未启用 feature 的构建里：接收任意参数，只为了给出"重建提示"，
/// 而不是让用户看到 clap 的 `unexpected argument` 这种与真正原因无关的错误。
#[cfg(not(feature = "serve"))]
#[derive(Debug, clap::Args)]
struct UnavailableServeArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    rest: Vec<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Run {
            img_path,
            config,
            words,
            chars,
            json,
            format,
            formula,
            table,
        } => {
            let cfg = match config {
                Some(path) => EngineConfig::from_yaml_file(path)?,
                None => EngineConfig::default(),
            };
            let mut engine = RapidOcrEngine::new(cfg)?;
            let bytes = std::fs::read(&img_path)?;
            let word_mode = word_mode_from_flags(words, chars);
            let format = if json { OutputFormat::Json } else { format };
            let mut request = make_request(bytes, word_mode, table.policy());
            request.formula = formula.policy();
            let output = engine.recognize(request)?;
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&to_output_json(&output)?)?
                ),
                OutputFormat::Html => {
                    let source_name = img_path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("source.png");
                    let timing = format!("total {:.1} ms", output.timings.total_ms);
                    print!(
                        "{}",
                        render_output_report(
                            source_name,
                            source_name,
                            &output,
                            &timing,
                            ReportMode::Static,
                        )?
                    );
                }
                OutputFormat::Markdown => {
                    println!("{}", to_output_markdown(&output));
                    for (index, table) in output.tables(TextOrder::Reading) {
                        println!(
                            "table[{index}] rows={} cols={} cells={} confidence={}",
                            table.rows,
                            table.cols,
                            table.cells.len(),
                            table.confidence.as_str()
                        );
                    }
                }
                OutputFormat::Plain => {
                    println!("{}", output.plain_text(TextOrder::Reading));
                    for (index, latex) in output.formula_latex(TextOrder::Reading) {
                        println!("formula[{index}] = {latex}");
                    }
                    for (index, table) in output.tables(TextOrder::Reading) {
                        println!(
                            "table[{index}] rows={} cols={} cells={} confidence={}",
                            table.rows,
                            table.cols,
                            table.cells.len(),
                            table.confidence.as_str()
                        );
                    }
                    println!(
                        "regions={} formulas={} tables={} total_ms={:.1}",
                        output.regions.len(),
                        output.formula_count(),
                        output.table_count(),
                        output.timings.total_ms
                    );
                }
            }
        }
        Command::Check => println!("rapid-structure-rs ready"),
        Command::Report {
            input_dir,
            output_dir,
            config,
            formula,
            table,
        } => report_cmd(
            input_dir,
            output_dir,
            config,
            formula.policy(),
            table.policy(),
        )?,
        Command::Evaluate {
            manifest,
            config,
            iou_threshold,
            output,
        } => evaluate_cmd(manifest, config, iou_threshold, output)?,
        #[cfg(feature = "serve")]
        Command::Serve(args) => serve::serve_main(args)?,
        #[cfg(not(feature = "serve"))]
        Command::Serve(_) => {
            return Err(
                "`rapidstruct serve` is not compiled into this binary: rebuild with \
                 `cargo build --features serve` (docs/05 §2.1: the HTTP layer is an optional \
                 feature and is deliberately not part of the default build)"
                    .into(),
            );
        }
    }
    Ok(())
}

/// CLI 的两个开关 → 档位（**唯一**实现，便于测试）。
///
/// `--chars` 是 `--words` 的细化：两个都给时取更细的那一档（而不是报错也不是取 `words`），
/// 因为逐字框包含了一个词的全部信息。档位名不在这里重新发明：结果必须与
/// [`WordOutputMode::from_name`] 的三个写法逐一对应（serve 的 `?words=` 用的就是它）。
fn word_mode_from_flags(words: bool, chars: bool) -> WordOutputMode {
    if chars {
        WordOutputMode::Chars
    } else if words {
        WordOutputMode::Words
    } else {
        WordOutputMode::Off
    }
}

fn make_request(bytes: Vec<u8>, words: WordOutputMode, table: TablePolicy) -> OcrRequest {
    OcrRequest {
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
        preprocess: PreprocessPolicy::default(),
        detection: DetectionPolicy::default(),
        recognition: RecognitionPolicy { words },
        output: OutputPolicy::default(),
        formula: FormulaPolicy::default(),
        table,
    }
}

fn report_cmd(
    input_dir: PathBuf,
    output_dir: PathBuf,
    config: Option<PathBuf>,
    formula: FormulaPolicy,
    table: TablePolicy,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = match config {
        Some(path) => EngineConfig::from_yaml_file(path)?,
        None => EngineConfig::default(),
    };
    let mut engine = RapidOcrEngine::new(cfg)?;
    std::fs::create_dir_all(&output_dir)?;
    for entry in std::fs::read_dir(&input_dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !matches!(
            ext.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "bmp" | "webp"
        ) {
            continue;
        }
        let mut request = make_request(std::fs::read(&path)?, WordOutputMode::Off, table.clone());
        request.formula = formula.clone();
        let output = engine.recognize(request)?;
        let source_name = path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("source.png");
        let html_name = format!(
            "{}.html",
            path.file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("report")
        );
        std::fs::copy(&path, output_dir.join(source_name))?;
        let html = render_output_report(
            source_name,
            source_name,
            &output,
            &format!("total {:.1} ms", output.timings.total_ms),
            ReportMode::Full,
        )?;
        std::fs::write(output_dir.join(html_name), html)?;
    }
    Ok(())
}

fn evaluate_cmd(
    manifest: PathBuf,
    config: Option<PathBuf>,
    iou_threshold: f32,
    output: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(&manifest)?;
    let manifest_value: serde_json::Value = serde_json::from_str(&raw)?;
    // 域判别：裸数组 = OCR 域（现状）；对象且 `domain == "table"` = 表格域。
    // OCR 报告的形状因此**一个字节都不改**，既有 baseline 与 serve 契约不受影响。
    if rapid_structure_rs::evaluation::table::TableEvalManifest::matches(&manifest_value) {
        return evaluate_table_cmd(manifest, config, output);
    }
    let cases: Vec<EvaluationCase> = serde_json::from_value(manifest_value)?;
    let manifest_dir = manifest.parent().unwrap_or_else(|| Path::new("."));
    let cfg = match config {
        Some(path) => EngineConfig::from_yaml_file(path)?,
        None => EngineConfig::default(),
    };
    let mut engine = RapidOcrEngine::new(cfg)?;
    let mut reports = Vec::with_capacity(cases.len());
    for case in cases {
        let image_path = Path::new(&case.image);
        let resolved_image = if image_path.is_absolute() {
            image_path.to_path_buf()
        } else {
            manifest_dir.join(image_path)
        };
        let result = engine.recognize(make_request(
            std::fs::read(&resolved_image)?,
            WordOutputMode::Off,
            // 评测口径**不启用表格路由**：`--manifest` 量的是纯文本 CER 与 polygon IoU，
            // 表格会额外产出一个没有 `recognition` 的区域，从而改变区域数与 IoU 口径，
            // 与 `tests/baseline/windows-baseline/evaluation-cpu.json` 不可比（门禁 G6）。
            TablePolicy::default(),
        ))?;
        let metrics = evaluate_case(
            &case.text,
            &case.boxes,
            &to_output_items(&result),
            iou_threshold,
        );
        reports.push(EvaluationReport {
            image: case.image,
            metrics,
        });
    }
    let summary = EvaluationSummary::from_cases(reports);
    // 评估报告本身只描述质量指标；进程级峰值工作集与 ONNX Runtime 指纹由 CLI 附加
    // （口径与库内一致），不把平台/运行时信息塞进纯算法的报告类型。
    //
    // ORT 指纹必须随质量报告一起落地：没有它就无法证明两份评估跑在同一份运行库上，
    // 而阶段 0 的报告正是因为没有版本/路径/哈希而无法被严格比较。
    //
    // M4：`rapidstruct serve` 的 `POST /api/evaluate` 复用**同一个**函数，因此两个入口的
    // 报告字段（含运行时四字段）逐字段相同，而不是两处各拼一遍。
    let report = evaluation_report_value(&summary)?;
    let text = serde_json::to_string_pretty(&report)?;
    if let Some(path) = output {
        std::fs::write(path, &text)?;
    }
    println!("{text}");
    Ok(())
}

/// 表格域评测（`docs/09-table-recognition-execution.md` §9.2）。
///
/// 链路在库里（`evaluation::table::run`），与 `src/bin/table_eval.rs` 是**同一份**
/// 实现。这里只做两件事：把 manifest 合成 [`TableEvalConfig`]、把报告打出来。
///
/// `--config`（OCR 引擎配置）与 `--iou-threshold` 在表格域**不适用**：表格域不跑
/// 页级 OCR 引擎，也没有多边形 IoU 指标。给了它们说明调用方拿错了 manifest，
/// 因此明确报错而不是忽略 —— 静默忽略会让人以为阈值生效了。
fn evaluate_table_cmd(
    manifest: PathBuf,
    config: Option<PathBuf>,
    output: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    use rapid_structure_rs::evaluation::table::run::{TableEvalEvent, run_table_evaluation};

    if config.is_some() {
        return Err(
            "--config selects the page-level OCR engine, which the table domain does not \
                    run; remove it (or use the OCR manifest)"
                .into(),
        );
    }
    let parsed: rapid_structure_rs::evaluation::table::TableEvalManifest =
        serde_json::from_str(&std::fs::read_to_string(&manifest)?)?;
    let eval_config = parsed.into_config()?;
    let outcome = run_table_evaluation(&eval_config, &mut |event| match event {
        TableEvalEvent::Message(text) => eprintln!("{text}"),
        TableEvalEvent::Progress { done, total } => eprintln!("  {done}/{total}"),
    })?;
    let text = serde_json::to_string_pretty(&outcome.report)?;
    if let Some(path) = output {
        std::fs::write(path, &text)?;
    }
    println!("{text}");
    Ok(())
}

/// 质量指标（库）+ 进程级运行时字段 → 评估报告 JSON（**唯一**实现）。
///
/// 两个消费者：CLI 的 `rapidstruct evaluate`（`--output` 与 stdout）与 serve 的
/// `POST /api/evaluate`（HTTP 响应体）。字段名与顺序由这份实现决定。
///
/// 运行时那四个字段的实现已抽到 `evaluation::runtime_report`（域无关），表格域报告
/// 走的是同一个函数；这里只负责把 OCR 域的汇总序列化出来。**输出逐字段不变。**
pub(crate) fn evaluation_report_value(
    summary: &EvaluationSummary,
) -> Result<serde_json::Value, rapid_structure_rs::RapidOcrError> {
    let mut report = serde_json::to_value(summary)
        .map_err(|error| rapid_structure_rs::RapidOcrError::Decode(error.to_string()))?;
    rapid_structure_rs::evaluation::runtime_report::attach_runtime_fields(&mut report)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{make_request, word_mode_from_flags};
    use rapid_structure_rs::WordOutputMode;

    /// CLI 与 HTTP 共用**同一份**档位词表（用户复审清单第 7 条）。
    ///
    /// 三个开关组合的结果必须能用 [`WordOutputMode::from_name`] 同名解析回来：
    /// 哪天有人给 CLI 加了个 `--single-char` 之类的别名，而 serve 的 `?words=` 仍然只
    /// 认三个写法，这个断言就会失败。这比"两边各自测自己的默认值"强：它直接钉住
    /// 两个入口不可能给出不同档位。
    #[test]
    fn the_cli_flags_map_to_the_same_modes_as_the_query_parameter() {
        for (words, chars, name) in [
            (false, false, "off"),
            (true, false, "words"),
            (false, true, "chars"),
            // `--chars` 更细，两个都给时取它（而不是 `--words`）。
            (true, true, "chars"),
        ] {
            let from_flags = word_mode_from_flags(words, chars);
            assert_eq!(
                from_flags,
                WordOutputMode::from_name(name).expect("the documented query vocabulary"),
                "--words={words} --chars={chars}"
            );
            assert_eq!(from_flags.as_str(), name);
        }
    }

    /// 档位真的进了请求而不是在参数里被丢掉（否则上面那个断言只能证明"算对了"）。
    #[test]
    fn the_word_mode_reaches_the_request_recognition_policy() {
        for mode in WordOutputMode::ALL {
            let request = make_request(Vec::new(), mode, Default::default());
            assert_eq!(request.recognition.words, mode, "{}", mode.as_str());
        }
    }
}
