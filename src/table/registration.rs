//! 表格模型的**清单登记**与加载期完整性校验。
//!
//! # 为什么需要单独一层
//!
//! 页级 det / rec 的模型哈希来自 `assets/default_models.yaml` 或模型目录里的
//! `manifest.json`，由 [`crate::model_source`] 的**单一权威来源**规则选出；表格域走的是
//! 另一条路 —— 模型路径是调用方直接给出的（评测 CLI 的 `--model`），没有"来源表"可以问。
//! 于是"这个文件是不是我们以为的那个文件"就没有人回答。
//!
//! 本模块把答案放在**模型自己旁边**：`<model_dir>/manifest.json`。这与
//! [`crate::model_source::ModelManifest::load_from_dir`] 的既有规则完全一致
//! （`<model_dir>/manifest.json` 是该目录的唯一权威描述），因此不需要第二套查找逻辑，
//! 也不会与默认表产生"缺字段就从别处补"的合并语义。
//!
//! # 为什么不校验是可接受的默认
//!
//! **不是**可接受的默认：只要模型旁边有清单，加载路径就**必须**用它校验。清单不存在时
//! 返回 `None`（"没有可用的哈希"），这只覆盖手工放置模型的情形；评测与服务这类
//! "模型必须可追溯"的路径改用 [`require_registered_model_hash`]，让缺失变成错误。
//!
//! # 与下载校验的区别
//!
//! 下载时校验只能保证**下载那一刻**的字节是对的；磁盘上的文件在两次运行之间可以被替换。
//! 因此校验必须发生在**加载入口**，且必须在打开 ONNX Runtime 会话之前 ——
//! 否则"校验过了"这句话覆盖的只是探针之后的路径。

use std::path::Path;

use crate::{
    error::{RapidOcrError, Result},
    model_set::ModelRole,
    model_source::ModelManifest,
    model_verify,
};

/// 表格结构模型在清单里的角色。
pub const TABLE_MODEL_ROLE: ModelRole = ModelRole::TableStructure;

/// 清单里登记的一个表格模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredTableModel {
    /// 登记的文件名；等于模型文件的 basename。
    pub name: String,
    /// 清单声明的 SHA-256（小写十六进制）。
    pub sha256: String,
    /// 清单声明的字节数；`None` 表示未声明。
    pub size_bytes: Option<u64>,
    /// 清单声明的下载来源；`None` 表示只能本地放置。
    pub source_url: Option<String>,
}

/// 在模型同目录的 `manifest.json` 里查这个模型。
///
/// - 目录里**没有** `manifest.json` → `Ok(None)`（没有可用的登记）；
/// - 有清单但**没有登记这个文件名** → 错误，并列出清单里实际登记的名字
///   （静默放行会让"模型受清单管辖"变成一句空话）；
/// - 有登记但角色不是 [`TABLE_MODEL_ROLE`] → 错误（把结构模型登记成 `detector`
///   是登记错误，不是"另一个用途"）；
/// - 有登记但 `sha256` 为空 → 错误（[`crate::model_source`] 规定本地清单是**完整**描述，
///   每个文件都必须携带哈希）。
pub fn registration_for(model_path: &Path) -> Result<Option<RegisteredTableModel>> {
    let directory = model_path.parent().ok_or_else(|| {
        RapidOcrError::Config(format!(
            "table structure model path `{}` has no parent directory, so its registration cannot be found",
            model_path.display()
        ))
    })?;
    let Some(manifest) = ModelManifest::load_from_dir(directory)? else {
        return Ok(None);
    };
    let file_name = model_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            RapidOcrError::Config(format!(
                "table structure model path `{}` has no usable file name",
                model_path.display()
            ))
        })?;

    let Some(entry) = manifest.files.iter().find(|file| file.name == file_name) else {
        let registered: Vec<&str> = manifest
            .files
            .iter()
            .map(|file| file.name.as_str())
            .collect();
        return Err(RapidOcrError::Config(format!(
            "the manifest in `{}` does not register `{file_name}` (registered: {}); \
             a model that sits next to a manifest is governed by it",
            directory.display(),
            if registered.is_empty() {
                "none".to_string()
            } else {
                registered.join(", ")
            }
        )));
    };

    if entry.role != TABLE_MODEL_ROLE {
        return Err(RapidOcrError::Config(format!(
            "`{file_name}` is registered as role `{}` in `{}`, but this loader needs `{}`",
            entry.role,
            directory.display(),
            TABLE_MODEL_ROLE
        )));
    }

    if entry.sha256.trim().is_empty() {
        return Err(RapidOcrError::Config(format!(
            "`{file_name}` is registered in `{}` without a SHA-256; a local manifest must describe \
             every file completely, otherwise the registration cannot be checked",
            directory.display()
        )));
    }

    Ok(Some(RegisteredTableModel {
        name: entry.name.clone(),
        sha256: entry.sha256.trim().to_ascii_lowercase(),
        size_bytes: entry.size_bytes,
        source_url: entry.source_url.clone(),
    }))
}

/// 登记里的期望哈希；没有登记时 `Ok(None)`。
pub fn registered_model_hash(model_path: &Path) -> Result<Option<String>> {
    Ok(registration_for(model_path)?.map(|registration| registration.sha256))
}

/// 同 [`registered_model_hash`]，但**没有登记时报错**。
///
/// 评测与服务用这一档：一个没有可追溯哈希的表格模型不应该被静默接受 ——
/// 那时"我们校验了模型"只是一句没人核对过的声明。
pub fn require_registered_model_hash(model_path: &Path) -> Result<String> {
    registered_model_hash(model_path)?.ok_or_else(|| {
        RapidOcrError::Config(format!(
            "no manifest registers the table structure model `{}`; add one next to the model so its \
             SHA-256 can be verified at load time (see models/table/manifest.json)",
            model_path.display()
        ))
    })
}

/// 加载期校验：把**有效期望哈希**（调用方给的优先，否则取登记）交给
/// [`model_verify::verify_sha256`]，返回实际摘要。
///
/// 返回值是**实际**算出的摘要（校验通过时等于期望值），调用方可以把它写进报告，
/// 让"这个结果出自哪个模型"可追溯，而不只是"某个哈希校验过"。
pub fn verify_registered_model(
    model_path: &Path,
    expected_sha256: Option<&str>,
) -> Result<Option<String>> {
    let expected = match expected_sha256 {
        Some(hash) if !hash.trim().is_empty() => Some(hash.trim().to_string()),
        _ => registered_model_hash(model_path)?,
    };
    model_verify::verify_sha256(model_path, expected.as_deref())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::error::RapidOcrError;

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    /// 每个测试一个独立目录：登记查找是**按目录**的，共用目录会让测试互相看见对方的清单。
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "rapid-ocr-table-registration-{label}-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("scratch dir should be creatable");
            Self { dir }
        }

        fn model(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.dir.join(name);
            fs::write(&path, contents).expect("model file should be writable");
            path
        }

        fn manifest(&self, json: &str) -> PathBuf {
            let path = self.dir.join("manifest.json");
            fs::write(&path, json).expect("manifest should be writable");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn manifest_json(name: &str, role: &str, sha256: &str, size_bytes: Option<u64>) -> String {
        let size = match size_bytes {
            Some(bytes) => format!(",\n      \"size_bytes\": {bytes}"),
            None => String::new(),
        };
        format!(
            r#"{{
  "schema_version": 1,
  "id": "scratch",
  "family": "SLANet",
  "version": "v2.0.0",
  "files": [
    {{ "name": "{name}", "role": "{role}", "sha256": "{sha256}"{size} }}
  ]
}}"#
        )
    }

    /// 已知摘要的固定输入：`sha256("hello")`。
    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn a_directory_without_a_manifest_reports_no_registration() {
        let scratch = Scratch::new("no-manifest");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        assert_eq!(
            registration_for(&model).expect("no manifest is not an error"),
            None
        );
        assert_eq!(
            registered_model_hash(&model).expect("no manifest is not an error"),
            None
        );
    }

    #[test]
    fn a_registered_model_is_found_by_its_file_name() {
        let scratch = Scratch::new("found");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "table_structure",
            HELLO_SHA256,
            Some(5),
        ));

        let registration = registration_for(&model)
            .expect("a well formed manifest should parse")
            .expect("the model is registered");
        assert_eq!(registration.name, "slanet-plus.onnx");
        assert_eq!(registration.sha256, HELLO_SHA256);
        assert_eq!(registration.size_bytes, Some(5));
        assert_eq!(registration.source_url, None);
    }

    #[test]
    fn a_model_that_sits_next_to_a_manifest_must_be_registered_in_it() {
        let scratch = Scratch::new("unregistered");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "some-other.onnx",
            "table_structure",
            HELLO_SHA256,
            Some(5),
        ));

        let error = registration_for(&model).expect_err("an unregistered model must be refused");
        let text = error.to_string();
        assert!(
            text.contains("does not register `slanet-plus.onnx`"),
            "{text}"
        );
        // 错误必须列出清单里**实际**登记了什么，否则调用方只能自己去翻文件。
        assert!(text.contains("some-other.onnx"), "{text}");
    }

    #[test]
    fn a_model_registered_under_another_role_is_refused() {
        let scratch = Scratch::new("wrong-role");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "detector",
            HELLO_SHA256,
            Some(5),
        ));

        let error = registration_for(&model).expect_err("a role mismatch must be refused");
        let text = error.to_string();
        assert!(text.contains("registered as role `detector`"), "{text}");
        assert!(text.contains("needs `table_structure`"), "{text}");
    }

    #[test]
    fn a_registration_without_a_hash_is_refused_rather_than_treated_as_unhashed() {
        let scratch = Scratch::new("no-hash");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "table_structure",
            "",
            Some(5),
        ));

        let error = registration_for(&model).expect_err("an empty hash must be refused");
        assert!(error.to_string().contains("without a SHA-256"), "{error}");
    }

    #[test]
    fn a_matching_hash_is_verified_at_load_time() {
        let scratch = Scratch::new("verify-ok");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "table_structure",
            HELLO_SHA256,
            Some(5),
        ));

        let actual = verify_registered_model(&model, None)
            .expect("a matching hash should verify")
            .expect("the registration supplies an expected hash");
        assert_eq!(actual, HELLO_SHA256);
    }

    #[test]
    fn a_tampered_model_is_refused_at_load_time() {
        let scratch = Scratch::new("verify-tampered");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "table_structure",
            HELLO_SHA256,
            Some(5),
        ));
        // 同一个路径，内容换了 —— 下载期校验看不出来，只有加载期校验能。
        scratch.model("slanet-plus.onnx", b"HELLO");

        let error = verify_registered_model(&model, None).expect_err("tampering must be refused");
        assert!(
            matches!(error, RapidOcrError::HashMismatch { .. }),
            "the error must be locatable, got {error:?}"
        );
    }

    #[test]
    fn an_explicit_hash_overrides_the_registration() {
        let scratch = Scratch::new("override");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "table_structure",
            "0000000000000000000000000000000000000000000000000000000000000000",
            Some(5),
        ));

        // 显式哈希正确 → 通过（登记里的错误哈希被忽略）。
        verify_registered_model(&model, Some(HELLO_SHA256)).expect("the explicit hash is right");

        // 显式哈希错误 → 拒绝，且错误里带的是**显式**那个期望值。
        let error = verify_registered_model(&model, Some("deadbeef"))
            .expect_err("an explicit mismatch must be refused");
        match error {
            RapidOcrError::HashMismatch { expected, .. } => assert_eq!(expected, "deadbeef"),
            other => panic!("expected a hash mismatch, got {other:?}"),
        }
    }

    #[test]
    fn an_unregistered_model_has_no_hash_to_require() {
        let scratch = Scratch::new("require-none");
        let model = scratch.model("slanet-plus.onnx", b"hello");

        let error =
            require_registered_model_hash(&model).expect_err("a missing registration must fail");
        assert!(
            error.to_string().contains("no manifest registers"),
            "{error}"
        );
    }

    #[test]
    fn a_registered_model_has_a_hash_to_require() {
        let scratch = Scratch::new("require-some");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "table_structure",
            HELLO_SHA256,
            Some(5),
        ));

        assert_eq!(
            require_registered_model_hash(&model).expect("the registration supplies a hash"),
            HELLO_SHA256
        );
    }

    /// 上游给的哈希大小写不该改变结论：清单是手写的，`SHA256` 与 `sha256` 是同一个值。
    #[test]
    fn the_registered_hash_is_normalized_to_lower_case() {
        let scratch = Scratch::new("upper-case");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "table_structure",
            &HELLO_SHA256.to_ascii_uppercase(),
            Some(5),
        ));

        let registration = registration_for(&model)
            .expect("manifest should parse")
            .expect("registered");
        assert_eq!(registration.sha256, HELLO_SHA256);
        verify_registered_model(&model, None).expect("case must not matter");
    }

    #[test]
    fn a_broken_manifest_is_an_error_rather_than_a_missing_registration() {
        let scratch = Scratch::new("broken-manifest");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest("{ not json");

        let error = registration_for(&model).expect_err("a broken manifest must not be ignored");
        assert!(
            error.to_string().contains("manifest"),
            "the error should name the manifest, got {error}"
        );
    }

    /// `model_source` 的单一来源规则规定：本地清单是**完整**描述，不参与默认表合并。
    /// 因此一个只登记了表格模型的目录，在要求 det/rec/dict 时会**报错**而不是回落到默认表。
    #[test]
    fn a_table_only_manifest_does_not_borrow_roles_from_the_default_table() {
        let scratch = Scratch::new("single-source");
        let model = scratch.model("slanet-plus.onnx", b"hello");
        scratch.manifest(&manifest_json(
            "slanet-plus.onnx",
            "table_structure",
            HELLO_SHA256,
            Some(5),
        ));

        let manifest = ModelManifest::load_from_dir(&scratch.dir)
            .expect("manifest should parse")
            .expect("the directory has a manifest");
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].role, TABLE_MODEL_ROLE);
        // 同一个清单既服务 `model_source` 的单一来源规则，也服务本模块的登记查找 ——
        // 两处必须看到同一个条目，否则"清单是唯一权威"就有一半是空的。
        assert_eq!(
            registration_for(&model)
                .expect("manifest should parse")
                .expect("registered")
                .name,
            manifest.files[0].name
        );
    }

    fn _assert_signature(_: fn(&Path, Option<&str>) -> Result<Option<String>>) {}

    #[test]
    fn the_public_surface_is_the_one_the_loader_calls() {
        _assert_signature(verify_registered_model);
    }
}
