//! 表格识别 bounded context。
//!
//! 依赖方向：`table` 只依赖共享层（`runtime` / `input` / `vision` / `output` / `error` 等），
//! **不得让 `runtime` 反向依赖表格域** —— 与公式域的既有分域结论一致。
//!
//! # 与公式域的关系
//!
//! 表格域**照抄公式域的结构，但绝不共用它的实现**。两域在 metadata 上就分道扬镳：
//! 公式域的 `character` 是 JSON（内嵌 tokenizer 词表），表格域的 `character` 是
//! 换行分隔的 48 个 HTML 片段。把任一侧的解析器拿去用在另一侧，会得到一个
//! 不报错但结果全错的静默退化。
//!
//! # 模块
//!
//! - [`metadata`]：`character` 词表解析，**纯函数、零外部资产**；
//! - [`structure`]：SLANet-plus 模型 IO 契约（探针 + 契约校验）与会话；
//! - [`decode`]：结构解码器，**纯函数、零外部资产**（argmax → 片段 → 单元格槽位 → 框架树）；
//! - [`preprocess`]：结构模型输入预处理，**纯函数、零外部资产**（resize → 归一化 → 补零 → CHW）；
//! - [`emitter`]：结构树 + 单元格文本 → HTML / Markdown，并给出结构置信档；
//! - [`registration`]：模型清单登记与**加载期**完整性校验；
//! - [`cell_content_backfill`]：单元格内容回填 —— **只做几何匹配，不调识别器**（门禁 G2）；
//! - [`detection`]：表格定位 —— 从已有 det 文本框分布推断区域，**不引入新模型**（门禁 G4）；
//! - [`ocr_resolution`]：表格路径自己的 det 分辨率策略，**不继承页面级配置**（R2）。
//!
//! 生产路径有两条：整页管线 `src/ocr/pipeline/rapid_structure.rs` 的 `recognize_with_table`
//! （E14 接线，消费 [`detection`] / [`structure`] / [`preprocess`] / [`decode`] /
//! [`cell_content_backfill`] / [`emitter`]），以及离线评测 `src/evaluation/table/run.rs`
//! （`table_eval` 与 `rapidstruct evaluate --manifest` 两条命令）。

pub mod cell_content_backfill;
pub mod decode;
pub mod detection;
pub mod emitter;
pub mod metadata;
pub mod ocr_resolution;
pub mod page_line;
pub mod preprocess;
pub mod registration;
pub mod structure;
