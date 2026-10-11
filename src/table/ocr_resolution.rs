//! 表格路径的 OCR 分辨率策略（`docs/09-table-recognition-execution.md` §11 = `docs/07` 的 R2）。
//!
//! # 为什么表格路径需要自己的一份，而不是继承页面级配置
//!
//! det 是整页稠密卷积网络，计算量正比于输入像素数，而**决定像素数的是三处代码**：
//!
//! - `global.max_side_len`：进 det 之前把整页缩到长边不超过它（`src/ocr/pipeline/image_ops.rs`
//!   的 `resize_image_within_bounds`）。**只缩不放。**
//! - `det.limit_side_len` + `det.limit_type`：`src/ocr/det/preprocess.rs:72-99` 的
//!   `compute_resize_hw`。`limit_type == "min"` 时 `ratio = limit_side_len / min(h, w)`
//!   —— 这个分支**只放大不缩小**，`min(h, w) >= limit_side_len` 时 `ratio = 1.0`。
//! - `src/ocr/det/detector.rs:210-224` 的 `resolve_limit_side_len_like_python`：**在
//!   `compute_resize_hw` 之前把 `limit_side_len` 换掉**。
//!
//! # ⚠️ `limit_side_len` 只在 `limit_type == "min"` 时生效
//!
//! 第三条容易被忽略：`detector.rs:215-223` 对非 `"min"` 的 `limit_type` **丢弃配置值**，
//! 改成按 `max_wh` 取 `960 / 1500 / 2000` 三档（这是复刻 PaddleOCR 的行为，由
//! `src/ocr/det/detector.rs:236-240` 的 `limit_side_len_for_non_min_matches_python_buckets` 钉死）。
//! 所以 `limit_type = "max"` 下 **`limit_side_len` 是死旋钮**：写什么都不影响结果，真正生效的
//! 只有 `global.max_side_len`（以及那个阶梯，而阶梯只在长边 > 960 时才缩）。
//! `limit_type = "max"` 的语义因此是"**永不放大**，长边超过 960/1500/2000 才缩"。
//!
//! 这两件事被 [`tests::the_limit_side_len_is_ignored_under_the_max_policy`] 绑在一起断言：
//! 若有人把 `max` 分支改成"用配置值"，那条测试会先失败，提醒同步本段文档与
//! [`TableOcrResolution::describe`] 的措辞。
//!
//! 页面级配置的这几个值是为"整页正文"选的；表格路径的目标是"单元格里的小字"，
//! 两者的最优输入尺度不是同一个数。因此本模块把这些值显式化，并在
//! [`TableOcrResolution::apply`] 里**无条件覆盖**引擎配置里的同名值 ——
//! 不是"配置里的值优先、缺了才用默认"。覆盖而不是合并的理由：合并会让
//! "表格路径到底跑在什么分辨率上"取决于一份本来为别的用途写的 YAML。
//!
//! # 为什么默认把 `limit_type` 从 `"min"` 换成 `"max"`
//!
//! 官方表格脚本用的是 `--det_limit_side_len=736 --det_limit_type=min`，那个值本身没错，
//! 但它隐含一个前提：**输入是整页，短边本来就在 736 附近**，于是 `min` 分支实际上不缩放。
//! `TabRecSet` 是**裁剪后的小表**：val 全量 7,104 张的长边中位数是 **448 px**、短边中位数是
//! **150 px**，其中 **7,055 / 7,104（99.3 %）的短边小于 736**。前提不成立时，`min` 会把一张
//! 448×150 的图放大到约 2208×736（1.6 Mpx，约 24 倍面积）再喂 det —— 既贵，又把输入推到
//! 训练分布之外。
//!
//! 实测（`docs/09` §11.4 有完整表；200 样本同子集 `sample_set_sha256 = 5cc64d54…`）：
//! 官方 `736/min/2000` 的完整 TEDS 是 **0.573928**、`page_ocr.ocr_ms` p50 **504.7 ms**；
//! `736/max/2000` 是 **0.592182** / **117.3 ms**；`320/min/2000` 是 **0.593718** / **190.4 ms**。
//! **降尺度同时更快且更准**，所以默认取"永不放大"这一族。全量 7,104 上
//! `max_side_len=736 + limit_type=max` 的完整 TEDS 是 **0.583404**（基线 0.570710）、
//! `page_ocr.ocr_ms` p50 **108.9 ms**（基线 549.159，**5.04×**）、`regions_seen` **+3.56 %**。
//!
//! 默认里 `limit_side_len` 仍留 736（官方的数），但它**在 `max` 下不参与计算**；
//! 之所以保留这个字段而不是删掉，是为了让六个档位的对照实验仍能通过 CLI/manifest
//! 完整复现（`min480` / `min320` 两个档位需要它）。

use serde::{Deserialize, Serialize};

use crate::{
    error::{RapidOcrError, Result},
    ocr::pipeline::config::EngineConfig,
};

/// 表格路径拿单元格文本时用的 det 分辨率。
///
/// 默认是 `limit_side_len = 736 / limit_type = "max" / max_side_len = 736`：
/// 数字沿用 PaddleOCR 官方表格脚本的 `--det_limit_side_len=736`
/// （来源：[ppstructure/table/README.md §4.3](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/f8b41a62/ppstructure/table/README.md)），
/// 但方向改成 `"max"` —— 官方脚本的 `"min"` 只对"整页"成立，见模块文档。
///
/// **默认下真正生效的只有 `max_side_len`**：`limit_type = "max"` 时
/// `src/ocr/det/detector.rs:215-223` 会丢弃 `limit_side_len`。把 `limit_side_len` 也设成 736
/// 只是让默认值自述成"官方的那个数"，它不参与计算。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TableOcrResolution {
    /// det 的 `limit_side_len`。**只在 `limit_type == "min"` 时被读取**（见模块文档）。
    pub limit_side_len: usize,
    /// det 的 `limit_type`，只接受 `"min"` 与 `"max"`（见 `src/ocr/det/preprocess.rs:81-91`）。
    ///
    /// `"min"` = 保证短边不小于 `limit_side_len`（只放大）；`"max"` = 永不放大。
    pub limit_type: String,
    /// 进 det 之前整页缩放的长边上限（`global.max_side_len`）。**默认策略下唯一生效的旋钮。**
    pub max_side_len: usize,
}

impl Default for TableOcrResolution {
    fn default() -> Self {
        Self {
            limit_side_len: 736,
            limit_type: "max".to_string(),
            max_side_len: 736,
        }
    }
}

impl TableOcrResolution {
    /// 覆盖引擎配置里的分辨率字段。
    ///
    /// 三个值都是**无条件**写入：`limit_side_len` 与 `limit_type` 写进 `det`，
    /// `max_side_len` 写进 `global`。调用方传进来的配置里这三个字段的原有取值
    /// 会被丢弃 —— 这正是"不继承页面级配置"的含义。
    pub fn apply(&self, config: &mut EngineConfig) -> Result<()> {
        if self.limit_side_len == 0 {
            return Err(RapidOcrError::Config(
                "table OCR resolution: limit_side_len must be greater than zero; a zero here \
                 would make every det input collapse to the 32-pixel floor"
                    .to_string(),
            ));
        }
        if self.max_side_len == 0 {
            return Err(RapidOcrError::Config(
                "table OCR resolution: max_side_len must be greater than zero".to_string(),
            ));
        }
        match self.limit_type.as_str() {
            "min" | "max" => {}
            other => {
                return Err(RapidOcrError::Config(format!(
                    "table OCR resolution: limit_type must be \"min\" or \"max\", got {other:?}; \
                     src/ocr/det/preprocess.rs treats anything other than \"max\" as the min \
                     branch, so an unknown value would silently behave like \"min\""
                )));
            }
        }
        config.global.max_side_len = self.max_side_len;
        config.det.limit_side_len = self.limit_side_len;
        config.det.limit_type = self.limit_type.clone();
        Ok(())
    }

    /// 报告里自述口径用的一行文本。
    ///
    /// 分辨率必须写进报告：完整 TEDS 的绝对值绑定在产出文本的那份引擎配置上
    /// （`docs/09` §10.5.8 第 3 条），只记配置文件的路径不足以自证口径 ——
    /// 同一份 YAML 可以被 [`TableOcrResolution::apply`] 覆盖成不同的分辨率。
    ///
    /// 措辞按 `limit_type` 分支：`"max"` 下必须**明确写出 `limit_side_len` 不参与计算**，
    /// 否则报告会让人以为改那个数会有效果。
    pub fn describe(&self) -> String {
        match self.limit_type.as_str() {
            "min" => format!(
                "table OCR resolution: limit_type=min limit_side_len={} max_side_len={} \
                 (enlarges until the short side reaches limit_side_len; never shrinks)",
                self.limit_side_len, self.max_side_len
            ),
            _ => format!(
                "table OCR resolution: limit_type=max max_side_len={} (never enlarges; the live \
                 long-side cap is max_side_len; limit_side_len={} is ignored under limit_type=max)",
                self.max_side_len, self.limit_side_len
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TableOcrResolution;
    use crate::ocr::pipeline::config::EngineConfig;

    #[test]
    fn the_default_keeps_the_official_number_but_never_upscales() {
        // 736 来自官方表格脚本；`"max"` 是本仓库实测之后的改动（见模块文档）。
        // 这两件事必须一起断言：只断言 736 会让"方向被改回去"这种回归悄悄通过。
        let resolution = TableOcrResolution::default();
        assert_eq!(resolution.limit_side_len, 736);
        assert_eq!(resolution.limit_type, "max");
        assert_eq!(resolution.max_side_len, 736);
    }

    #[test]
    fn applying_overwrites_instead_of_filling_in_gaps() {
        // 配置里三个字段都**已经是别的值**：覆盖必须无条件发生，否则"表格路径
        // 不继承页面级配置"就退化成"配置里的值优先"。
        let mut config = EngineConfig::default();
        config.global.max_side_len = 2000;
        config.det.limit_side_len = 960;
        config.det.limit_type = "max".to_string();

        TableOcrResolution {
            limit_side_len: 736,
            limit_type: "min".to_string(),
            max_side_len: 1280,
        }
        .apply(&mut config)
        .expect("the default resolution is valid");

        assert_eq!(config.global.max_side_len, 1280);
        assert_eq!(config.det.limit_side_len, 736);
        assert_eq!(config.det.limit_type, "min");
    }

    #[test]
    fn an_unknown_limit_type_is_refused_rather_than_treated_as_min() {
        let error = TableOcrResolution {
            limit_type: "smallest".to_string(),
            ..TableOcrResolution::default()
        }
        .apply(&mut EngineConfig::default())
        .expect_err("an unknown limit_type must not be accepted");
        let text = error.to_string();
        assert!(text.contains("smallest"), "{text}");
        assert!(text.contains("\"min\" or \"max\""), "{text}");
    }

    #[test]
    fn a_zero_side_len_is_refused() {
        let error = TableOcrResolution {
            limit_side_len: 0,
            ..TableOcrResolution::default()
        }
        .apply(&mut EngineConfig::default())
        .expect_err("limit_side_len = 0 must not be accepted");
        assert!(error.to_string().contains("limit_side_len"), "{error}");
    }

    #[test]
    fn the_description_names_all_three_knobs() {
        // 默认（`max`）分支：三个数都要出现在自述里，且必须写明 `limit_side_len` 被忽略。
        let described = TableOcrResolution::default().describe();
        assert!(described.contains("limit_type=max"), "{described}");
        assert!(described.contains("max_side_len=736"), "{described}");
        assert!(described.contains("limit_side_len=736"), "{described}");
        assert!(
            described.contains("ignored under limit_type=max"),
            "{described}"
        );
    }

    #[test]
    fn the_min_description_says_it_enlarges_rather_than_ignoring() {
        let described = TableOcrResolution {
            limit_side_len: 320,
            limit_type: "min".to_string(),
            max_side_len: 2000,
        }
        .describe();
        assert!(described.contains("limit_type=min"), "{described}");
        assert!(described.contains("limit_side_len=320"), "{described}");
        assert!(described.contains("max_side_len=2000"), "{described}");
        assert!(described.contains("never shrinks"), "{described}");
        assert!(!described.contains("ignored"), "{described}");
    }

    /// 把本模块的默认策略与**真正消费这些字段的函数**绑在一起。
    ///
    /// `src/ocr/det/detector.rs:210-224` 的 `resolve_limit_side_len_like_python` 对非 `"min"`
    /// 的 `limit_type` 丢弃 `limit_side_len`，改成按 `max_wh` 取 960/1500/2000 三档。
    /// 这条断言因此同时钉住两件事：
    ///
    /// 1. 默认的 `"max"` 策略下 `limit_side_len` 确实是死旋钮 —— 否则本模块文档、
    ///    [`TableOcrResolution::describe`] 的措辞、以及 `docs/09` §11.4 的结论都会失真；
    /// 2. 若有人把 `max` 分支改成"用配置值"，这里会先失败，提醒同步那三处。
    #[test]
    fn the_limit_side_len_is_ignored_under_the_max_policy() {
        use crate::ocr::det::detector::resolve_limit_side_len_like_python;

        for configured in [320usize, 736, 960, 1280, 2000] {
            assert_eq!(
                resolve_limit_side_len_like_python("max", configured, 800),
                960,
                "limit_type=max must ignore the configured {configured}"
            );
            assert_eq!(
                resolve_limit_side_len_like_python("max", configured, 1200),
                1500
            );
            assert_eq!(
                resolve_limit_side_len_like_python("max", configured, 1800),
                2000
            );
        }

        // `"min"` 是唯一读配置值的分支。
        assert_eq!(resolve_limit_side_len_like_python("min", 320, 1800), 320);
        assert_eq!(resolve_limit_side_len_like_python("min", 2000, 800), 2000);
    }

    /// 默认档位下 det 自己不再改尺度：前置缩放已经把长边压到 736，而 `max_wh <= 736 < 960`
    /// 让阶梯给出 960，`compute_resize_hw` 的 `max` 分支于是 `ratio = 1.0`。
    #[test]
    fn the_default_makes_the_detector_side_resize_a_no_op() {
        use crate::ocr::det::detector::resolve_limit_side_len_like_python;

        let default = TableOcrResolution::default();
        // 前置缩放之后长边至多 `max_side_len`。
        let resolved = resolve_limit_side_len_like_python(
            &default.limit_type,
            default.limit_side_len,
            default.max_side_len,
        );
        assert_eq!(resolved, 960);
        assert!(
            resolved >= default.max_side_len,
            "the bucket {resolved} must not shrink below the pre-resize cap {}",
            default.max_side_len
        );
    }

    /// §11.4.7 第 3 条登记的风险：默认档位"给 det 输入一个硬上界"这个**语义**此前没有
    /// 单独的测试 —— 它靠 `apply` 覆盖 `global.max_side_len` 实现，而覆盖本身是被
    /// `applying_overwrites_instead_of_filling_in_gaps` 测的，**上界**却没测。若有人把
    /// `apply` 改成只覆盖 `det.*`，上界会静默消失。这条把整条链路串起来断言。
    #[test]
    fn the_default_puts_a_hard_bound_on_the_detector_input_long_side() {
        use crate::config::RecImage;
        use crate::ocr::pipeline::image_ops::resize_image_within_bounds;

        let default = TableOcrResolution::default();
        let mut config = EngineConfig::default();
        default.apply(&mut config).expect("apply must succeed");

        // 一张远大于上限的图（TabRecSet 里 0.18 % 的长图会到这个量级）。
        let width = 4_129;
        let height = 700;
        let image =
            RecImage::from_bgr_u8(width, height, vec![0_u8; width * height * 3]).expect("image");

        let (bounded, _, _) = resize_image_within_bounds(
            image,
            config.global.min_side_len,
            config.global.max_side_len,
        )
        .expect("resize must succeed");

        let long_side = bounded.width().max(bounded.height());
        assert!(
            long_side <= default.max_side_len,
            "the applied max_side_len ({}) must bound the long side, got {long_side}",
            config.global.max_side_len
        );
        // 反证：若 `apply` 不覆盖 `global.max_side_len`，EngineConfig 的默认值是 2000，
        // 这张图会以 2000 的长边进入 det。
        assert!(
            long_side < EngineConfig::default().global.max_side_len,
            "the test image must be larger than the engine default, otherwise it proves nothing"
        );
    }

    /// §11.4.7 第 6 条：默认里 `max_side_len == limit_side_len == 736` 是刻意的（"表格路径的
    /// 尺度只有一个数"），但此前没有测试断言这个巧合。
    #[test]
    fn the_default_uses_one_number_for_both_side_limits() {
        let default = TableOcrResolution::default();
        assert_eq!(
            default.max_side_len, default.limit_side_len,
            "the default deliberately states the table-path scale once, as the same number in \
             both fields; if they diverge, the module docs' claim that the number is still the \
             official 736 becomes ambiguous"
        );
    }
}
