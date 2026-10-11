//! 评估报告里与**域无关**的运行时字段。
//!
//! # 为什么需要单独一个模块
//!
//! `src/bin/rapidstruct.rs` 的 `evaluation_report_value(&EvaluationSummary)` 是 OCR 域
//! 报告的**唯一**序列化入口（CLI `rapidstruct evaluate` 与 serve 的 `POST /api/evaluate`
//! 共用）。表格域的报告形状完全不同（`metric_scope` / `span_exceedance` /
//! `upper_bound` / `failures`…），把 TEDS 塞进 `EvaluationSummary` 会违反
//! `src/evaluation/mod.rs` 的"三类指标不得混入默认汇总"。
//!
//! 但两份报告里**确实有一块是共享的**：进程级峰值工作集、内存口径来源、ORT 指纹与
//! 版本。这四个字段的存在理由是"没有它就无法证明两份评估跑在同一份运行库上"
//! （`docs/03-windows-only-optimization-tasks.md` 阶段 0 的报告正是因为缺少版本/路径/哈希
//! 而无法被严格比较）。字段名与取值口径只有一份实现，因此两个域的报告不会出现
//! "同一天跑的两份报告写着不同的 ORT 版本"。
//!
//! 所以这里抽出的不是"报告序列化"，而是**域无关的那一层**：
//! [`attach_runtime_fields`] 决定字段名与来源，域自己的指标由各自的报告类型决定。

use serde_json::Value;

use crate::{
    error::Result,
    runtime::{
        memory::{peak_memory_source, peak_working_set_bytes},
        ort_runtime::ort_runtime_fingerprint,
        provider::ort_runtime_version,
    },
};

/// 报告里由本模块负责的字段名。
///
/// 单独列出来是为了让"哪些字段来自运行时而与域无关"成为一个可被测试断言的事实，
/// 而不是散落在四行 `object.insert` 里的约定。
pub const RUNTIME_FIELD_NAMES: [&str; 4] = [
    "peak_working_set_bytes",
    "memory_source",
    "ort_runtime",
    "ort_runtime_version",
];

/// 把运行时字段并入一份已经是 JSON 对象的报告。
///
/// 传进来的值必须是对象：数组（例如 OCR 域的 `cases` 列表）**不是**报告，
/// 给它挂运行时字段只会制造一个形状不明的产物，因此直接报错而不是静默返回原值。
pub fn attach_runtime_fields(report: &mut Value) -> Result<()> {
    let object = report.as_object_mut().ok_or_else(|| {
        crate::error::RapidOcrError::InvalidInput(
            "an evaluation report must be a JSON object before the runtime fields can be \
             attached; an array has no place to put them"
                .to_string(),
        )
    })?;
    object.insert(
        "peak_working_set_bytes".to_string(),
        serde_json::json!(peak_working_set_bytes()),
    );
    object.insert(
        "memory_source".to_string(),
        serde_json::json!(peak_memory_source()),
    );
    object.insert(
        "ort_runtime".to_string(),
        serde_json::to_value(ort_runtime_fingerprint())
            .map_err(|error| crate::error::RapidOcrError::Config(error.to_string()))?,
    );
    object.insert(
        "ort_runtime_version".to_string(),
        serde_json::json!(ort_runtime_version()),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{RUNTIME_FIELD_NAMES, attach_runtime_fields};
    use serde_json::json;

    #[test]
    fn every_named_field_is_actually_attached() {
        let mut report = json!({ "mean_cer": 0.4476 });
        attach_runtime_fields(&mut report).expect("an object report accepts runtime fields");
        for name in RUNTIME_FIELD_NAMES {
            assert!(
                report.get(name).is_some(),
                "the field {name} is declared as a runtime field but was not attached"
            );
        }
        // 域自己的字段必须原样保留：这个函数只**增加**字段。
        assert_eq!(report["mean_cer"], 0.4476);
    }

    #[test]
    fn the_attachment_does_not_rename_or_drop_domain_fields() {
        let mut report = json!({ "domain": "table", "metric_scope": { "structure_only": true } });
        let before = report.clone();
        attach_runtime_fields(&mut report).expect("an object report accepts runtime fields");
        assert_eq!(report["domain"], before["domain"]);
        assert_eq!(report["metric_scope"], before["metric_scope"]);
    }

    #[test]
    fn an_array_is_rejected_rather_than_silently_left_alone() {
        let mut report = json!([{ "image": "a.png", "cer": 0.0 }]);
        let error = attach_runtime_fields(&mut report)
            .expect_err("an array is not a report and cannot carry runtime fields");
        assert!(
            error.to_string().contains("must be a JSON object"),
            "the error must say why an array is refused, got: {error}"
        );
        // 拒绝时不能留下半改状态。
        assert!(report.is_array());
    }

    #[test]
    fn the_ort_fingerprint_is_an_object_not_a_bare_version_string() {
        // 阶段 0 的教训：只有版本号不足以证明两份报告跑在同一份运行库上，
        // 因此 `ort_runtime` 必须是携带路径/哈希的结构。
        let mut report = json!({});
        attach_runtime_fields(&mut report).expect("an empty object accepts runtime fields");
        assert!(
            report["ort_runtime"].is_object() || report["ort_runtime"].is_null(),
            "ort_runtime must stay a structured fingerprint, got {}",
            report["ort_runtime"]
        );
    }
}
