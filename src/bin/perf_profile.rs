//! OCR 性能剖析工具：逐阶段计时 + 逐阶段内存采样。
//!
//! 用法：
//! ```text
//! cargo run --release --bin perf_profile -- --config models/configs/test-config.yaml --rounds 3
//! ```
//!
//! 默认从 `datasets/text` 加载所有图片，执行 warmup + 正式轮次，
//! 产出 JSON 报告 + stderr 人类可读摘要。

use clap::Parser;
use rapid_structure_rs::evaluation::stats::percentile;
use rapid_structure_rs::{
    ClassifierPlan, ClassifierPolicy, DetectionPolicy, EngineConfig, ImageInput, OcrEngine,
    OcrRequest, OutputPolicy, PreprocessPolicy, RapidOcrEngine, RecognitionPolicy, StagePlan,
    TimingLedger, WordOutputMode,
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Arc, time::Instant};

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long = "config")]
    config_path: Option<PathBuf>,
    #[arg(long, default_value = "datasets/text")]
    images_dir: PathBuf,
    #[arg(long, default_value_t = 3)]
    rounds: usize,
    #[arg(long, default_value_t = 1)]
    warmup: usize,
    #[arg(long, default_value = "off")]
    words: String,
    #[arg(long)]
    formula: Option<PathBuf>,
    #[arg(long)]
    table_model: Option<PathBuf>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    max_side_len: Option<usize>,
    #[arg(long)]
    intra_threads: Option<usize>,
    #[arg(long)]
    mem_probe: bool,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: perf_profile is running a debug build; use `cargo run --release` for accurate measurements"
        );
    }

    // --- 解析 --words ---
    let word_mode = match cli.words.as_str() {
        "off" => WordOutputMode::Off,
        "words" => WordOutputMode::Words,
        "chars" => WordOutputMode::Chars,
        other => return Err(format!("invalid --words value: {other}").into()),
    };

    // --- 收集图片 ---
    let mut images: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&cli.images_dir)? {
        let p = entry?.path();
        if p.is_file()
            && let Some(ext) = p.extension().and_then(|v| v.to_str())
            && matches!(
                ext.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "bmp" | "webp" | "tif" | "tiff"
            )
        {
            images.push(p);
        }
    }
    images.sort();
    if images.is_empty() {
        return Err(format!("no images found under {}", cli.images_dir.display()).into());
    }
    eprintln!(
        "perf_profile: {} images, {} warmup + {} rounds",
        images.len(),
        cli.warmup,
        cli.rounds
    );

    // --- 加载配置 ---
    let mut cfg = match cli.config_path {
        Some(ref p) => EngineConfig::from_yaml_file(p)?,
        None => EngineConfig::default(),
    };
    if let Some(v) = cli.max_side_len {
        cfg.global.max_side_len = v;
    }
    if let Some(v) = cli.intra_threads {
        cfg.runtime.intra_threads = Some(v);
        cfg.runtime.inter_threads = Some(1);
        cfg.runtime.auto_tune_threads = false;
    }
    let effective_max_side = cfg.global.max_side_len as u32;
    let config_fingerprint = serde_json::to_string(&cfg)?;

    // --- 初始化引擎 ---
    let engine_start = Instant::now();
    let mut engine = RapidOcrEngine::new(cfg)?;
    let init_ms = engine_start.elapsed().as_secs_f64() * 1000.0;
    let model_id = engine.model_id().to_string();
    // init working set after model loading
    let init_mem = rapid_structure_rs::sample_memory().unwrap_or_default();

    // Enable memory probe always (this IS the profiler tool)
    engine.enable_mem_probe();

    // --- ORT version ---
    let ort_version = rapid_structure_rs::ort_runtime_version();

    // --- Build request ---
    let table_policy = match &cli.table_model {
        Some(p) => rapid_structure_rs::TablePolicy {
            enabled: true,
            model_path: Some(p.clone()),
            ..Default::default()
        },
        None => Default::default(),
    };
    let formula_policy = match &cli.formula {
        Some(p) => rapid_structure_rs::FormulaPolicy {
            enabled: true,
            model_path: Some(p.clone()),
            ..Default::default()
        },
        None => Default::default(),
    };
    let make_request = |bytes: Vec<u8>| OcrRequest {
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
            max_side: Some(effective_max_side),
            ..PreprocessPolicy::default()
        },
        detection: DetectionPolicy::default(),
        recognition: RecognitionPolicy { words: word_mode },
        output: OutputPolicy::default(),
        formula: formula_policy.clone(),
        table: table_policy.clone(),
    };

    // --- Warmup ---
    for _ in 0..cli.warmup {
        for p in &images {
            let _ = engine.recognize(make_request(fs::read(p)?))?;
            let _ = engine.take_stage_mem_records();
        }
    }

    // --- 正式测量 ---
    let total_rounds = cli.rounds;
    let mut image_data: Vec<ImageProfile> = Vec::with_capacity(images.len());

    for path in &images {
        let bytes = fs::read(path)?;
        let size_bytes = bytes.len() as u64;
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // Get image dimensions from header (quick decode just for meta)
        let (dim_w, dim_h) = image_dimensions(&bytes);

        let mut rounds: Vec<RoundData> = Vec::with_capacity(total_rounds);
        for _r in 0..total_rounds {
            let wall_start = Instant::now();
            let result = engine.recognize(make_request(bytes.clone()))?;
            let wall_ms = wall_start.elapsed().as_secs_f64() * 1000.0;

            let timings_json = serde_json::to_value(&result.timings)?;
            let ledger = TimingLedger::from_timings(&result.timings);
            let ledger_json = serde_json::to_value(ledger)?;

            let mem_records = engine.take_stage_mem_records();
            let mem_json: Vec<Value> = {
                let mut prev_ws: Option<u64> = None;
                mem_records
                    .iter()
                    .map(|rec| {
                        let delta = if let Some(p) = prev_ws {
                            rec.sample.working_set_kb.saturating_sub(p)
                        } else {
                            0
                        };
                        prev_ws = Some(rec.sample.working_set_kb);
                        json!({
                            "name": rec.stage,
                            "working_set_kb": rec.sample.working_set_kb,
                            "peak_working_set_kb": rec.sample.peak_working_set_kb,
                            "private_bytes_kb": rec.sample.private_bytes_kb,
                            "delta_from_prev_kb": delta,
                        })
                    })
                    .collect()
            };

            rounds.push(RoundData {
                wall_ms,
                timings: timings_json,
                ledger: ledger_json,
                regions: result.regions.len(),
                mem_records: mem_json,
            });
        }
        image_data.push(ImageProfile {
            file: file_name,
            size_bytes,
            dimensions: (dim_w, dim_h),
            rounds,
        });
    }

    // --- 汇总统计 ---
    let all_wall: Vec<f64> = image_data
        .iter()
        .flat_map(|img| img.rounds.iter().map(|r| r.wall_ms))
        .collect();
    let all_total: Vec<f64> = image_data
        .iter()
        .flat_map(|img| {
            img.rounds
                .iter()
                .filter_map(|r| r.timings.get("total_ms").and_then(|v| v.as_f64()))
        })
        .collect();
    let total_samples = all_wall.len();
    let throughput = if total_samples > 0 {
        let total_secs = all_wall.iter().sum::<f64>() / 1000.0;
        if total_secs > 0.0 {
            total_samples as f64 / total_secs
        } else {
            0.0
        }
    } else {
        0.0
    };

    let stddev = |v: &[f64]| -> f64 {
        if v.is_empty() {
            return 0.0;
        }
        let m = v.iter().sum::<f64>() / v.len() as f64;
        ((v.iter().map(|x| (x - m) * (x - m)).sum::<f64>()) / v.len() as f64).sqrt()
    };
    let mean = |v: &[f64]| -> f64 {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let sorted = |v: &[f64]| -> Vec<f64> {
        let mut c = v.to_vec();
        c.sort_by(|a, b| a.partial_cmp(b).unwrap());
        c
    };

    let wall_s = sorted(&all_wall);
    let total_s = sorted(&all_total);

    // Aggregate per-stage timing from all OcrTimings
    let stage_keys = [
        "decode_ms",
        "resize_ms",
        "crop_ms",
        "detector_preprocess_ms",
        "detector_infer_ms",
        "detector_postprocess_ms",
        "classifier_preprocess_ms",
        "classifier_infer_ms",
        "classifier_postprocess_ms",
        "recognizer_preprocess_ms",
        "recognizer_infer_ms",
        "recognizer_postprocess_ms",
        "postprocess_ms",
        "total_ms",
    ];
    let stage_timing_summary: Value = {
        let mut obj = serde_json::Map::new();
        for key in &stage_keys {
            let vals: Vec<f64> = image_data
                .iter()
                .flat_map(|img| img.rounds.iter())
                .filter_map(|r| r.timings.get(*key).and_then(|v| v.as_f64()))
                .collect();
            let s = sorted(&vals);
            obj.insert(
                (*key).to_string(),
                json!({
                    "mean": format!("{:.3}", mean(&vals)),
                    "p50": format!("{:.3}", percentile(&s, 0.50)),
                    "p90": format!("{:.3}", percentile(&s, 0.90)),
                }),
            );
        }
        Value::Object(obj)
    };

    // Aggregate per-stage memory
    let stage_order = [
        "input",
        "input_end",
        "detect_pre",
        "detect_infer",
        "detect_post",
        "classifier",
        "rec_pre",
        "rec_infer",
        "rec_post",
        "page_post",
        "output_end",
    ];
    let stage_memory_summary: Value = {
        let mut obj = serde_json::Map::new();
        for stage in &stage_order {
            let deltas: Vec<u64> = image_data
                .iter()
                .flat_map(|img| img.rounds.iter())
                .flat_map(|r| r.mem_records.iter())
                .filter(|rec| rec.get("name").and_then(Value::as_str) == Some(stage))
                .filter_map(|rec| rec.get("delta_from_prev_kb").and_then(Value::as_u64))
                .collect();
            if deltas.is_empty() {
                continue;
            }
            let mean_delta = deltas.iter().sum::<u64>() as f64 / deltas.len() as f64;
            let peak_delta = *deltas.iter().max().unwrap_or(&0);
            obj.insert(
                (*stage).to_string(),
                json!({
                    "samples": deltas.len(),
                    "mean_delta_kb": mean_delta as u64,
                    "peak_delta_kb": peak_delta,
                }),
            );
        }
        Value::Object(obj)
    };

    // Top bottlenecks (sorted by mean time descending)
    let top_bottlenecks: Vec<Value> = {
        let mut items: Vec<(String, f64)> = stage_keys
            .iter()
            .filter(|k| **k != "total_ms")
            .map(|k| {
                let vals: Vec<f64> = image_data
                    .iter()
                    .flat_map(|img| img.rounds.iter())
                    .filter_map(|r| r.timings.get(*k).and_then(|v| v.as_f64()))
                    .collect();
                ((*k).to_string(), mean(&vals))
            })
            .filter(|(_, v)| *v > 0.001)
            .collect();
        items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        items
            .iter()
            .take(5)
            .map(|(name, ms)| json!({"stage": name, "mean_ms": format!("{:.3}", ms)}))
            .collect()
    };

    // --- Build JSON report ---
    let images_json: Vec<Value> = image_data
        .iter()
        .map(|img| {
            let rounds_json: Vec<Value> = img
                .rounds
                .iter()
                .map(|r| {
                    json!({
                        "wall_ms": format!("{:.3}", r.wall_ms),
                        "timings": r.timings,
                        "ledger": r.ledger,
                        "memory": { "stages": r.mem_records },
                        "regions": r.regions,
                    })
                })
                .collect();
            json!({
                "file": img.file,
                "size_bytes": img.size_bytes,
                "dimensions": [img.dimensions.0, img.dimensions.1],
                "rounds": rounds_json,
            })
        })
        .collect();

    let report = json!({
        "meta": {
            "tool": "perf_profile",
            "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "config_fingerprint": config_fingerprint,
            "ort_version": ort_version,
            "model_id": model_id,
            "images_dir": cli.images_dir.display().to_string(),
            "image_count": images.len(),
            "rounds": cli.rounds,
            "warmup_rounds": cli.warmup,
            "total_samples": total_samples,
            "init_ms": format!("{:.1}", init_ms),
            "init_working_set_kb": init_mem.working_set_kb,
            "mem_source": "windows:GetProcessMemoryInfo",
        },
        "images": images_json,
        "summary": {
            "throughput_images_per_sec": format!("{:.2}", throughput),
            "wall_ms": {
                "mean": format!("{:.1}", mean(&all_wall)),
                "p50": format!("{:.1}", percentile(&wall_s, 0.50)),
                "p90": format!("{:.1}", percentile(&wall_s, 0.90)),
                "max": format!("{:.1}", wall_s.last().copied().unwrap_or(0.0)),
                "min": format!("{:.1}", wall_s.first().copied().unwrap_or(0.0)),
                "stddev": format!("{:.1}", stddev(&all_wall)),
            },
            "ocr_total_ms": {
                "mean": format!("{:.1}", mean(&all_total)),
                "p50": format!("{:.1}", percentile(&total_s, 0.50)),
                "p90": format!("{:.1}", percentile(&total_s, 0.90)),
            },
            "stage_timing": stage_timing_summary,
            "stage_memory": stage_memory_summary,
            "top_bottlenecks": top_bottlenecks,
        },
    });

    // --- Output ---
    let out_path = cli
        .output
        .unwrap_or_else(|| PathBuf::from("Temp/perf-profile-report.json"));
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let pretty = serde_json::to_string_pretty(&report)?;
    fs::write(&out_path, &pretty)?;
    eprintln!("perf_profile: report written to {}", out_path.display());

    // --- Human-readable summary to stdout ---
    println!(
        "=== OCR Performance Profile ({} images x {} rounds) ===",
        images.len(),
        cli.rounds
    );
    println!(
        "Engine init: {:.1} ms | Working set after init: {} KB",
        init_ms, init_mem.working_set_kb
    );
    println!(
        "Model: {} | ORT: {}",
        model_id,
        ort_version.as_deref().unwrap_or("unknown")
    );
    println!("Throughput: {:.2} images/s", throughput);
    println!(
        "Wall: mean={:.1} p50={:.1} p90={:.1} min={:.1} max={:.1} stddev={:.1} ms",
        mean(&all_wall),
        percentile(&wall_s, 0.50),
        percentile(&wall_s, 0.90),
        wall_s.first().copied().unwrap_or(0.0),
        wall_s.last().copied().unwrap_or(0.0),
        stddev(&all_wall)
    );
    println!("\n--- Stage Timing (mean ms) ---");
    println!(
        "  decode={:.2}  resize={:.2}  crop={:.2}",
        stage_mean(&image_data, "decode_ms"),
        stage_mean(&image_data, "resize_ms"),
        stage_mean(&image_data, "crop_ms"),
    );
    println!(
        "  det_pre={:.2}  det_infer={:.2}  det_post={:.2}",
        stage_mean(&image_data, "detector_preprocess_ms"),
        stage_mean(&image_data, "detector_infer_ms"),
        stage_mean(&image_data, "detector_postprocess_ms"),
    );
    println!(
        "  cls_pre={:.2}  cls_infer={:.2}  cls_post={:.2}",
        stage_mean(&image_data, "classifier_preprocess_ms"),
        stage_mean(&image_data, "classifier_infer_ms"),
        stage_mean(&image_data, "classifier_postprocess_ms"),
    );
    println!(
        "  rec_pre={:.2}  rec_infer={:.2}  rec_post={:.2}",
        stage_mean(&image_data, "recognizer_preprocess_ms"),
        stage_mean(&image_data, "recognizer_infer_ms"),
        stage_mean(&image_data, "recognizer_postprocess_ms"),
    );
    println!(
        "  final_post={:.2}  total={:.2}",
        stage_mean(&image_data, "postprocess_ms"),
        stage_mean(&image_data, "total_ms"),
    );
    println!("\n--- Top Bottlenecks ---");
    for b in &top_bottlenecks {
        println!(
            "  {}: {} ms",
            b["stage"].as_str().unwrap_or("?"),
            b["mean_ms"].as_str().unwrap_or("?"),
        );
    }
    println!("\n--- Stage Memory Deltas (KB, mean) ---");
    if let Value::Object(map) = &stage_memory_summary {
        for (stage, stats) in map {
            println!(
                "  {:<16} delta={}  peak_delta={}",
                stage,
                stats
                    .get("mean_delta_kb")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                stats
                    .get("peak_delta_kb")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            );
        }
    }

    Ok(())
}

struct ImageProfile {
    file: String,
    size_bytes: u64,
    dimensions: (u32, u32),
    rounds: Vec<RoundData>,
}

struct RoundData {
    wall_ms: f64,
    timings: Value,
    ledger: Value,
    regions: usize,
    mem_records: Vec<Value>,
}

fn stage_mean(data: &[ImageProfile], key: &str) -> f64 {
    let vals: Vec<f64> = data
        .iter()
        .flat_map(|img| img.rounds.iter())
        .filter_map(|r| r.timings.get(key).and_then(|v| v.as_f64()))
        .collect();
    if vals.is_empty() {
        0.0
    } else {
        vals.iter().sum::<f64>() / vals.len() as f64
    }
}

/// 从图片文件头部读取尺寸（不做完整解码）。
fn image_dimensions(bytes: &[u8]) -> (u32, u32) {
    // PNG: bytes 16-19 = width, 20-23 = height (big-endian)
    if bytes.len() >= 24 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return (w, h);
    }
    // JPEG: scan for SOF0/SOF2 marker
    if bytes.len() > 2 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] == 0xFF && matches!(bytes[i + 1], 0xC0 | 0xC2) {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return (w, h);
            }
            if bytes[i] == 0xFF && i + 3 < bytes.len() {
                let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
                i += 2 + seg_len;
            } else {
                i += 1;
            }
        }
    }
    (0, 0)
}
