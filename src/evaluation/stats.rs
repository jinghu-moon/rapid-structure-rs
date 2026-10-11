//! 共享的耗时采样统计。
//!
//! benchmark 与评测工具都要报告“不只看平均值”的延迟分布，因此把统计口径放在
//! 共享层，避免每个 bin 各写一套 percentile。

use serde::{Deserialize, Serialize};

/// 分位口径的**唯一标记值**（P0-2 / N1 裁决）。
///
/// 三份报告（页面基准 / 表格报告）都在报告顶层写 `percentile_method: PERCENTILE_METHOD`，
/// 让跨版本比较能识别“这份数字是线性插值口径”。公式评测的 `Stats` 结构不变，其分位
/// 本就是线性，无需标记。旧报告没有这个键，按“无标记 = 旧口径”解读。
pub const PERCENTILE_METHOD: &str = "linear";

/// 一个指标的采样统计（毫秒）。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub samples: usize,
    pub min_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub stddev_ms: f64,
}

impl Stats {
    pub fn from_samples(mut values: Vec<f64>) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        values.sort_by(f64::total_cmp);
        let samples = values.len();
        let mean = values.iter().sum::<f64>() / samples as f64;
        let variance = if samples > 1 {
            values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / (samples - 1) as f64
        } else {
            0.0
        };
        Self {
            samples,
            min_ms: values[0],
            max_ms: values[samples - 1],
            mean_ms: mean,
            p50_ms: percentile(&values, 0.50),
            p95_ms: percentile(&values, 0.95),
            stddev_ms: variance.sqrt(),
        }
    }

    /// 需要额外分位时的通用入口：**不给 `Stats` 增加任何序列化字段**，只取数。
    ///
    /// 报告层要哪些分位由该报告自己的适配器声明（见 `table::run::summarize`、
    /// `bench_warm_e2e::stats`），口径统一走 [`percentile`]。
    pub fn quantile(values: &[f64], q: f64) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        percentile(&sorted, q)
    }
}

/// 线性插值百分位，与 numpy `percentile` 默认插值一致。
///
/// 入参必须**已按升序排列**且全部有限；含 `NaN`/`Inf` 的样本一律拒绝（P0-2 定义用例），
/// 避免非有限值经插值悄悄污染分位读数。
pub fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    assert!(
        sorted.iter().all(|value| value.is_finite()),
        "percentile samples must be finite; found a non-finite value"
    );
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = fraction * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        return sorted[lower];
    }
    let weight = position - lower as f64;
    sorted[lower] * (1.0 - weight) + sorted[upper] * weight
}

#[cfg(test)]
mod tests {
    use super::{PERCENTILE_METHOD, Stats, percentile};

    /// 公式评测报告把 `Stats` 当作字段整体序列化（`formula_eval.rs` 的
    /// `per_image_ms`/`per_batch_ms`，`formula_bench.rs` 同理），所以这三份报告里
    /// 公式评测这一份的键集合就是 `Stats` 的序列化键集合。P0-0 把它钉成契约：
    /// 任何改动分位口径/字段名的重构都必须让这份键集合显式变更，而不是静默漂移。
    #[test]
    fn stats_serializes_the_frozen_contract_key_set() {
        let value = serde_json::to_value(Stats::from_samples(vec![4.0, 1.0, 3.0, 2.0]))
            .expect("Stats must serialize");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("Stats serializes as a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "max_ms",
                "mean_ms",
                "min_ms",
                "p50_ms",
                "p95_ms",
                "samples",
                "stddev_ms"
            ],
            "the formula report's Stats block must keep exactly these keys"
        );
        // 同一份定值样本的快照：键集合之外还要钉住线性插值分位的读数，
        // 这样批 3 把三份报告统一接到 `stats::percentile` 时能对拍到逐值一致。
        assert_eq!(value["samples"], 4);
        assert!((value["min_ms"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert!((value["max_ms"].as_f64().unwrap() - 4.0).abs() < 1e-12);
        assert!((value["mean_ms"].as_f64().unwrap() - 2.5).abs() < 1e-12);
        assert!((value["p50_ms"].as_f64().unwrap() - 2.5).abs() < 1e-12);
        assert!((value["p95_ms"].as_f64().unwrap() - 3.85).abs() < 1e-12);
    }

    #[test]
    fn percentile_matches_linear_interpolation() {
        let values = vec![1.0, 2.0, 3.0, 4.0];
        assert!((percentile(&values, 0.0) - 1.0).abs() < 1e-12);
        assert!((percentile(&values, 1.0) - 4.0).abs() < 1e-12);
        assert!((percentile(&values, 0.5) - 2.5).abs() < 1e-12);
        // P0-2 钉住的新读数：[1,2,3,4] 的 p90 = 3.7（position = 0.9*3 = 2.7）。
        assert!((percentile(&values, 0.9) - 3.7).abs() < 1e-12);
        assert!((percentile(&values, 0.95) - 3.85).abs() < 1e-12);
    }

    #[test]
    fn percentile_of_an_empty_sample_is_zero() {
        assert_eq!(percentile(&[], 0.5), 0.0);
    }

    #[test]
    fn percentile_of_a_single_sample_ignores_the_fraction() {
        assert_eq!(percentile(&[7.0], 0.0), 7.0);
        assert_eq!(percentile(&[7.0], 0.5), 7.0);
        assert_eq!(percentile(&[7.0], 1.0), 7.0);
    }

    #[test]
    #[should_panic(expected = "must be finite")]
    fn percentile_rejects_non_finite_samples() {
        percentile(&[1.0, f64::NAN, 3.0], 0.5);
    }

    #[test]
    fn quantile_matches_percentile_without_touching_the_serialized_shape() {
        let values = [4.0, 1.0, 3.0, 2.0];
        // 通用入口自行排序，结果与预先排序后调用 `percentile` 一致。
        assert!((Stats::quantile(&values, 0.9) - 3.7).abs() < 1e-12);
        assert_eq!(Stats::quantile(&[], 0.5), 0.0);
        // `quantile` 不得往 `Stats` 的序列化里添加任何字段。
        let value = serde_json::to_value(Stats::from_samples(values.to_vec())).unwrap();
        assert!(value.get("quantile").is_none());
    }

    #[test]
    fn the_percentile_method_marker_is_locked_to_linear() {
        assert_eq!(PERCENTILE_METHOD, "linear");
    }

    #[test]
    fn stats_reports_min_max_mean_and_stddev() {
        let stats = Stats::from_samples(vec![4.0, 1.0, 3.0, 2.0]);
        assert_eq!(stats.samples, 4);
        assert!((stats.min_ms - 1.0).abs() < 1e-12);
        assert!((stats.max_ms - 4.0).abs() < 1e-12);
        assert!((stats.mean_ms - 2.5).abs() < 1e-12);
        assert!((stats.p50_ms - 2.5).abs() < 1e-12);
        assert!(stats.stddev_ms > 0.0);
    }

    #[test]
    fn empty_stats_are_zeroed() {
        let stats = Stats::from_samples(Vec::new());
        assert_eq!(stats.samples, 0);
        assert_eq!(stats.mean_ms, 0.0);
        assert_eq!(stats.p95_ms, 0.0);
    }
}
