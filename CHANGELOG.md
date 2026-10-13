# 变更记录（开发期）

本 crate 处于开发期，**不承诺向后兼容**。这里只记录结构性的、会影响下游的变更。

## 改名：`rapid-ocr-rs` → `rapid-structure-rs`（未发布）

破坏性改名。旧名字只描述了三分之一的能力：这个 crate 的整页管线同时做**文本**（PP-OCRv6
检测 + 识别）、**表格结构**（SLANet-plus → HTML/Markdown）与**公式**（PP-FormulaNet_plus-M），
所以改用上游 PaddleOCR 对这条合并管线的叫法 —— **PP-Structure** 的 "Structure"。

| 面 | 旧 | 新 |
| --- | --- | --- |
| crate / lib | `rapid-ocr-rs` / `rapid_ocr_rs` | `rapid-structure-rs` / `rapid_structure_rs` |
| 二进制 | `rapidocr`（`src/bin/rapidocr.rs`） | `rapidstruct`（`src/bin/rapidstruct.rs`） |
| 库内模块 | `src/ocr/pipeline/rapid_ocr.rs` | `src/ocr/pipeline/rapid_structure.rs` |
| 环境变量前缀 | `RAPID_OCR_*`、`RAPIDOCR_SERVE_*` | `RAPID_STRUCTURE_*`、`RAPID_STRUCTURE_SERVE_*` |
| serve 令牌头 | `X-RapidOCR-Token` | `X-RapidStructure-Token` |
| serve 界面状态 | `%LOCALAPPDATA%\RapidOCR\serve-ui.json` | `%LOCALAPPDATA%\RapidStructure\serve-ui.json` |

**不改**：`RapidAI/RapidOCR` 等模型来源 URL、`THIRD_PARTY_NOTES.md` 的上游署名、
`refer/**` 与 `prototypes/**` 的第三方/图片文字内容、以及被 SHA-256 钉住的
`tests/fixtures/serve/result-real-42-regions.json`（里面的旧 URL 是**图上的文字真值**）。

## 平台收窄：Windows x64 + MSVC ABI（未发布）

破坏性平台收窄，对应 `docs/03-windows-only-optimization-tasks.md` 的阶段 0–8
（任务文档的文件名保留首轮的写法）。

### 平台

- **唯一支持的平台是 Windows x64 + MSVC ABI（`x86_64-pc-windows-msvc`）**。
  其他目标在编译期直接命中 `src/platform_gate.rs` 的 `compile_error!`。
- **明确非目标**：Windows x86（32 位，`i686-pc-windows-msvc` / `i686-pc-windows-gnu`）、
  Windows ARM64（`aarch64-pc-windows-msvc`）、Windows GNU ABI（`x86_64-pc-windows-gnu`），
  以及 Wine、WSL、Linux、macOS。
- 32 位 x86 不是“改一个 `cfg`”就能支持：预编译的 ONNX Runtime、DirectML/CUDA 的
  provider DLL 与约 566 MB 的公式识别模型都是 x64 产物，32 位地址空间对后者本身就是
  真实限制；Windows ARM64 需要重新收集并重测整套原生依赖。
- 峰值内存只保留 Windows PSAPI 口径
  （`windows:GetProcessMemoryInfo.PeakWorkingSetSize`），Linux `/proc` 实现已删除。

### 删除的公开 API

| 删除项 | 原因 |
| --- | --- |
| `RuntimeBackend` 与 `RuntimeConfig::backend` | 只有一个取值（`OnnxCpu`）的伪抽象，却要序列化并出现在 YAML 里 |
| `ProviderPreference::Cann` / `ResolvedProvider::Cann` / `cann-provider` feature | CANN 不是 Windows x64 目标 |
| `VisionBackend` 与 `RuntimeConfig::vision_backend` | 视觉后端分派只服务 OpenCV，而 OpenCV 无法在本机构建与测量 |
| `opencv-backend` feature 与 `opencv` 依赖 | 同上：没有测量依据支持保留，且让 `--all-features` 不可用 |
| `turbojpeg` 依赖与 CMake 原生构建链 | 实测比 `image` crate 的 JPEG 解码**慢 7%–33%**，端到端差值 0.036% |
| `det.runtime` / `cls.runtime` / `rec.runtime` 三段重复配置 | 合并为单一 `EngineConfig.runtime`；残留的旧键会被 `deny_unknown_fields` 拒绝 |

### 新增

- `RuntimeProfile` / `ThreadPlan` / `ThreadSource`（`runtime::profile`）：
  统一管理 provider、ORT intra/inter、Rayon 线程、arena、严格回退与公式批大小，
  并在 benchmark 报告中输出 `meta.thread_plan`；Rayon 全局池初始化失败不再是静默失败。
- `RuntimeConfig::formula_batch`（默认 16）：公式识别器按此分块，
  修掉了“17–64 个公式区域导致整页请求失败”的缺陷。
- `ort_runtime_version()`：报告实际加载的 ONNX Runtime 版本。
- `PEAK_MEMORY_SOURCE` / `peak_memory_failure_reason()`。
- `tools/check_platform_gate.ps1`、`tools/run_windows_baseline.ps1`、
  `tools/run_windows_provider_matrix.ps1`、`tools/run_thread_matrix.ps1`。

### 单变体抽象清理

- `RuntimeBackend` 删除后，`runtime/session.rs` 直接构造 ORT Session。
- 视觉入口不再接受 `backend` 参数；热路径上没有 enum 分派。
- 删除 `resolve_backend_or_pure_rust` 这条宽松回退：核心路径没有“静默回退”概念。

### 线程策略与计时账本（终审第二轮）

- **线程策略只剩一份实现**：`RuntimeConfig::effective_session_threads()`（显式值优先；
  否则按 `auto_tune_threads` 从 `runtime::profile::auto_tuned_thread_budget()` 推导；
  否则 `(None, None)` = 不配置）。`RuntimeProfile::plan()` 与 `OrtSession::open_session()`
  都调用它，因此引擎、`FormulaSession`、`formula_bench`、`formula_eval` 的线程行为
  由构造保证一致。此前 `OrtSession` 只做字段透传，默认配置下公式路径拿到的是 ORT 默认
  线程数，而引擎路径用推导值。
- `RuntimeConfig::has_explicit_thread_request()`：`intra_threads` / `inter_threads` /
  `rayon_threads` 任意一个 `Some(>0)` 即算显式请求；`ThreadSource::Explicit` 由它决定，
  因此只显式设置 `rayon_threads` 时，“已有全局池大小不同”也会报错而不是被静默采纳。
- 新增 `OrtSession::session_threads()`、`FormulaSession::session_threads()`、
  `FormulaRecognizer::session_threads()`：报告实际下发给 ONNX Runtime 的
  `(intra, inter)`（`None` = 未配置）。
- `formula_bench` / `formula_eval` 报告新增 `effective_intra_threads` /
  `effective_inter_threads`（前者在 `threads` 下，后者在 `provider` 下）。
- `LedgerConservation` 新增 `excess_ms`（= `max(0, -residual_ms)`，占比成立的上界；该字段
  终审时曾临时叫 `overlap_ms`，名字与语义均已废弃）与 `interpretation`（进入 JSON 的读法
  说明）。时间账本是**诊断**工具：残差是 `total_ms`（总窗口）与命名分量之和之间的
  **口径差异**——外层 `preprocess_ms` 窗口（`rapid_structure.rs:619-719`）与 `inner.run()` 的
  e2e 窗口（`:106-130`）**串行、不重叠**，没有任何一段墙钟被算两次——**不是** `total_ms`
  算错了，也不能作为性能验收依据。

### 计时账本的口径陈述与 `timing_split` 恒等式（复审）

- `timing_split` 新增 `identity` / `identity_residual_ms` / `input_overflow_ms` /
  `input_overflow_share` / `attributed_ms` / `attributed_share`。报告的恒等式是
  `ort_inference_ms + rust_ms + input_overflow_ms + unattributed_ms == denominator_ms`
  （与恒等式的实测残差一起报出）。此前文档写的
  `inference + rust + unattributed == total` 只在 `input_overflow_ms == 0` 时成立：`rust_ms`
  用的是被截断到外层窗口内的 `input_ms()`，而 `attributed_ms` 计入完整的输入三项
  （`input_named_within_ms + input_overflow_ms`），溢出量必须显式留在恒等式里。已有键名
  未变。
- 修正残留的因果陈述：`README.md` 与 `src/runtime/mod.rs` 不再说计时窗口“重叠”，统一为
  “分量与 `total_ms` 量的是不同（串行）范围的墙钟，因此不是严格划分”；
  `src/runtime/timing.rs` 的模块公式不再写
  `decode + resize + crop + input_other_ms == input_ms()`（溢出分支为假），改为
  `input_named_within_ms + input_other_ms == input_ms()` 加桥接式
  `decode + resize + crop == input_named_within_ms + input_overflow_ms`。

## 分位口径统一（P0-2，未发布）

破坏性口径变更，对应 `docs/12-structure-refactor-plan.md` 的 P0-2（L-1 / D-3）。

- **全 crate 唯一的分位实现**是 `evaluation::stats::percentile`（线性插值，
  `position = q * (n - 1)`，与 numpy 默认 `linear` 一致）。此前页面基准
  （`bench_warm_e2e::stats`：下中位 + 最近秩）与表格报告（`table::run::summarize`：
  最近秩）各写一套，对同一组样本会给出不同读数。
- **口径变更（数值会变，键名不变）**：`[1,2,3,4]` 的 p50 从页面基准的 `2`、表格报告的
  `3.0` 统一为 `2.5`；p90 从 `4` 统一为 `3.7`。三份报告的统计键集合逐一保持不变。
- **公式评测报告结构不变**：`Stats` 字段与序列化结构原样保留，其分位本就是线性，
  因此不新增、不删除任何键，也不加标记。
- **新增 `percentile_method` 标记**：页面基准与表格报告在**报告顶层**写
  `"percentile_method": "linear"`（单点常量 `evaluation::stats::PERCENTILE_METHOD`）。
  旧报告没有这个键，按“无标记 = 旧口径”解读。
- **跨版本不可比**：`tools/check_provider_claims.ps1` 的比较前提是“same fingerprint”，
  分位口径变化不体现在指纹里。`tests/baseline/windows-baseline/*.json` 等**已入库基线的
  数字来自旧口径**，按裁决**不重建**，因此不得把新跑出的分位数字与这些旧基线并列比较。

## 模型完整性凭证 `VerifiedModel`（P0-3 第一阶段，未发布）

对应 `docs/12-structure-refactor-plan.md` 的 P0-3（I-3 / I-4 / I-5），本阶段只做**非破坏的新增**（评审一.3、方案“先落凭证类型与 verify”）：

- 新增公开类型 `model_verify::VerifiedModel`（同时从 crate 根导出）：把“路径 + 期望摘要 +
  实际摘要”钉成一个可传递的凭证，唯一入口 `VerifiedModel::verify(path, expected)` 内部仍只调
  `verify_sha256`。`expected = None` 允许构造，但如实记录为未校验（`is_verified() == false`）。
- 新增单点文案 `missing_model_path_error(domain)`：`api.rs` 的 formula/table `validate` 与
  `rapid_structure.rs` 的 formula/table 懒加载共四个调用点，此前同一条件写了两套文案（
  “… when enabled” vs 无后缀）；现在四处经此单点函数，给出逐字节相同的错误。
- **本阶段不改任何会话/构造器签名**（`OcrSession::new` / `FormulaSession::new` /
  `TableStructureSession::open` / `FormulaRecognizer`、`FormulaDetector` 的 `from_model` 系列），
  因此无破坏性变更。把这些构造器改为收 `&VerifiedModel`、删除无摘要的公开 `from_model`，
  属于 P0-3 第二阶段（见下）。

## 会话/构造器统一收 `&VerifiedModel`（P0-3 第二阶段，破坏性变更，未发布）

承接第一阶段，落地方案 P0-3 的**结构性不变量**：“强度由凭证决定，而不是由调用路径决定”，
结构上不再存在“忘记校验”的旁路：
（注：这一句在当时的完整度上**不成立** —— 会话构造器都改了，但 `OrtSession::open_unchecked`
还是 `pub` 并随 `OrtSession` 对外可见，外部调用方可以整条绕过；该残留由复审 P1-2 关闭，见下文。）

- **`VerifiedModel::verify` 加存在性优先守卫**：无论有没有期望，缺失文件都先返回
  `FileNotFound`（不再退化为“算不出摘要”的含混错误）。这条不变量此前分散在各域构造器，
  现单点收口到凭证。
- **会话构造器改收 `&VerifiedModel`**（破坏性）：`OcrSession::new`、`FormulaSession::new`
  不再接受裸 `&Path`，也不再自己检查 `is_file`；文本三处生产构造（det/rec/cls）与
  `formula_bench` 先建凭证再传（完整性在准入/下载层已保证，会话层以 `None` 如实记录）。
- **公式构造器删掉无摘要入口**（破坏性）：`FormulaRecognizer` / `FormulaDetector` 删除
  `from_model(path, …)` 与 `from_model_with_hash(path, …, expected)`，合并为
  `from_model(&VerifiedModel, runtime)`——存在性与期望摘要都在凭证这一单点完成，
  不匹配时凭证不被构造、会话因此不被创建。
- **表格保留 registration 分层（I-5）并委派给凭证**：`TableStructureSession::open` /
  `probe` 仍经 `verify_registered_model`（“期望从哪来”的清单/登记逻辑不变），但后者现在
  把校验动作委派给 `VerifiedModel::verify`；返回值仍为实际摘要。`table::registration` 的
  14 条测试**逐条不变**，证明分层被保留。
- **`formula_bench` 的 `--expected-sha256` 现在真正生效**：此前该 CLI 字段被定义却从未
  参与加载（正是 P0-3 要修的“强度由调用路径决定”）；现在它经凭证，声明即校验。
- 三域各有一条“路径存在但摘要不符 → 拒绝且会话未创建”的用例；删掉无摘要构造器后
  `-D warnings` 仍为 0（守卫）。真实模型与合成 fixture 的端到端加载在测试与 CLI 中继续通过。

## 页面级调度重写（P2-1 + P2-2，未发布）

对应 `docs/12-structure-refactor-plan.md` 的 P2-1 / P2-2：给“页面级组合”一个类型，把
阶段顺序从 `if` 嵌套改为单一调度器。产品输出等价，非破坏性（`OcrRequest::validate` 的
`roi`/`tile` 拒绝消息逐字不变）。

- 新增公开类型 `api::PageRoute`（`runs_formula`/`runs_table`/`formula`/`table`）与
  `api::ValidatedRequest`（`plan`/`request`/`route`），二者从 crate 根导出。`plan` 一次完成
  “校验所有策略与坐标前提 → 按启用位推导页面路由”，是不可变地交给调度器的唯一产物。
- `OcrRequest::validate` 的两份逐字 `roi`/`tile` 禁令合并为单判据 `reject_off_page_premise`
  （统一模板 + 每阶段自己的 reason），消息文本保持不变。
- 引擎入口 `recognize` 改为 `ValidatedRequest::plan(request)? → recognize_page(planned)`。
  新增 `recognize_page`：**阶段顺序在整个 crate 里只出现这一次** —— 先产出页级文本通道
  （含公式），再往其上追加表格区域。
- `recognize_with_formula` → `recognize_text_with_formula(&OcrRequest, &FormulaPolicy)`；
  `recognize_with_table` → `attach_table_regions(&mut OcrOutput, &OcrRequest, &TablePolicy)`
  （不再自跑文本通道，只从定位起追加）。两处防递归改写（`= FormulaPolicy::default()` /
  `= TablePolicy::default()`）随之删除：每个阶段由调度器至多调用一次。
- 计时守恒不变：`formula_ms` / `table_detect_ms` / `table_structure_infer_ms` /
  `table_backfill_ms` / `total_ms` 的字段集合与累加关系不变；`output.validate()` 上移到
  调度器统一收口。新增 `PageRoute` 四组合矩阵与 `plan` 的 `roi`/`tile` 拒绝用例；
  serve 的 M4 公式队列与 E16.2 表格路由两块全绿。

## 输入物化入口单点化（P1-5，未发布）

对应 `docs/12-structure-refactor-plan.md` 的 P1-5：把“请求 → 像素”的转换从两处重复
收敛为单一入口。逐像素行为等价，非破坏性（不改动任何公开 API）。“只解码一次”归 P2-3，
本阶段**保持两次解码**不变。

- 新增模块私有 `MaterializedInput { rec, original_size }` 与 `RapidOcrEngine::materialize
  (&OcrRequest)`：一次把 `Pixels`/`Encoded`/`File`/`Url`/`Image` 五个变体物化为**整页**（未裁
  `roi`）的 `RecImage`。校验/限长顺序统一取 `view.validate() → ensure_decode_pixels →
  to_bgr(None)`（与 `decode_source_image`、分块原路径一致）。
- `recognize_text` 与 `recognize_tiled_request` 各自那段五变体 `match request.input` 删除，
  改为调用 `materialize`：文本路由由 `rec_image_input` 裁 `roi`，分块路由由 `crop_imm`
  裁 `roi`；`roi` 仍不在 `materialize` 里应用（页面级阶段需要未裁的整页像素）。
- `decode_source_image` 保持不动：页面级阶段的第二次解码仍由它发生，故页级路由的解码
  次数仍为 2（一次 `decode_source_image` + 一次回落文本通道的 `load_with_limit`）。
- 新增 `input::image_loader` 的测试专用解码计数（`thread_local`），并用它钉住“纯文本=1、
  页面级=2”（`materialization_preserves_the_decode_count_per_route`，随真实资产门控）。

## 几何基础函数单点化（P1-1，未发布）

对应 `docs/12-structure-refactor-plan.md` 的 P1-1（D-1，评审二.5 已缩小野心）：把真正同形的
几何例程（polygon → 轴对齐外接框、轴对齐 IoU、polygon 面积）收敛到 `src/vision/geometry.rs`。
行为等价，非破坏性：被收敛的公开函数（`axis_aligned_bbox` / `polygon_to_bbox` / `rectangle_iou`）
保留原签名，**逐位委托**到共享实现，它们的既有钉住用例即数值对拍。

- 新增 `vision::geometry`：`aabb_of` / `aabb_from_quad` / `aabb_from_flat8`（均返
  `[x_min, y_min, x_max, y_max]`）、`aabb_iou`（f64，`<=` 边界与零面积早退与原 `rectangle_iou`
  逐位一致）、`polygon_area`（f64 有符号鞋带公式，与 `det::postprocess::polygon_area_f64` 一致）。
  输出沿用仓库既有的 `[f32; 4]` 形态而非新造 `Aabb` 类型，避免把内部单点化放大成破坏性 API 变更。
- 委托点：`table::cell_content_backfill::axis_aligned_bbox` 与 `table::decode::polygon_to_bbox` →
  `aabb_from_flat8`；`cell_content_backfill::rectangle_iou` → `aabb_iou`；
  `det::postprocess::polygon_area_f64` → `polygon_area`；`rec::word_boxes::quad_to_rect_bbox` →
  `aabb_from_quad`。
- **明确不合并**（语义不同，评审二.5）：`evaluation::ocr::polygon_iou`（凸四边形裁剪）、
  `formula::detect::iou`（类别感知 NMS，`>` 边界）、`formula::route::Rect::iou`（f32）、
  `rotate_crop`/i32 轮廓的取整夹边、以及跨多 quad 聚合的 `quad_vec_to_rect_bbox`。
- 新增 `vision::geometry` 边界用例（退化 / 负坐标 / 单点 / 包含 / 共边 / 零面积无 NaN /
  有符号面积），并与独立参考实现交叉对拍。

## 有符号单位化 LUT 单点化（P1-3，未发布）

对应 `docs/12-structure-refactor-plan.md` 的 P1-3（评审二.6 已缩小野心）：rec 与 cls 各有一份
**逐字相同**的有符号单位化查表实现（`x * (2.0 / 255.0) - 1.0`，并把 `-0.0` 归一为 `0.0`），
收敛到 `src/vision/normalize.rs`。行为等价、非破坏性：两处调用点的逐像素热循环改调共享的
`normalize_to_signed_unit`，运算顺序不变（仍是先乘后减、单次比较）。

- 新增 `vision::normalize`：`signed_unit_lut() -> &'static [f32; 256]`（`OnceLock` 单例）与
  `#[inline] normalize_to_signed_unit(v: u8) -> f32`。
- 委托点：`ocr::rec::preprocess` 与 `ocr::cls::preprocess` 删除各自的局部 `signed_unit_lut` /
  `normalize_to_signed_unit` 及随之失效的 `use std::sync::OnceLock`，改 `use vision::normalize::normalize_to_signed_unit`。
- **明确不合并**（评审二.6 要求暂缓，逐像素对拍后另议）：`ocr::det::preprocess` 的 AVX2 归一化路径、
  `table::preprocess` 的 ImageNet `(raw*SCALE-mean)/std` 内联归一化，以及各域不同的 resize 尺寸策略 /
  CHW 写入顺序 / padding 语义。
- 新增 `vision::normalize` 用例：0..=255 全域 LUT 与合并前闭式逐项（含符号位）对拍、端点
  0→-1.0 / 255→+1.0、无 `-0.0`、`OnceLock` 单例稳定。

## 页文本行规范视图 `PageTextLine`（P1-4 第一批，未发布）

对应 `docs/12-structure-refactor-plan.md` 的 P1-4（D-2，评审二.7 的"先转换、后合并"）。本批只做
**第一批**：引入规范类型 + 单点转换，行为等价。旧结构 `table::detection::PageLine`（四边形）
与 `table::cell_content_backfill::TextLine`（轴对齐 bbox）**原样保留**、下游消费者不变，只是
两者的数据来源收敛到同一个 `PageTextLine`。删除其中一份属计划里的第二批迁移（需逐消费者推进、
且以字段/裁剪/浮点精度三者对拍为前提），不在本批。

- 新增 `src/table/page_line.rs`：`PageTextLine { index, text, polygon, bbox, score }`，`new()` 是
  bbox 的唯一生成点（内部调 `vision::geometry::aabb_from_flat8`，与装配点此前 `axis_aligned_bbox(&flat)`
  逐位一致），`to_page_line()` / `to_text_line()` 显式导出两个旧视图。
- 放在 `table` 基础层而非 `ocr::types`（评审二.7）；`PageTextLine::new` 只吃几何 + 文本 + 分数，
  不读 `api::OcrRegion`，避免 `table` 反向依赖输出类型（区域过滤仍留在装配点）。
- 装配点 `rapid_structure::table_source_lines` 改为先构 `Vec<PageTextLine>` 再映射出 `lines` /
  `text_lines` / `region_indices`；删除就地拼 bbox 的 `quad_to_flat8` 辅助函数。
- 新增对拍用例：规范视图导出的两视图与合并前装配手拼结果逐字段相等（含倾斜/负坐标/退化四边形）。

## 表格净化白名单规范化与转义合并（P1-6，未发布）

对应 `docs/12-structure-refactor-plan.md` 的 P1-6（D-3 / N2 裁决：唯一权威规范文件 + 生成脚本，
生成物入库）。把 Rust 引擎与页面的表格 HTML 净化收敛到同一份白名单，并以指纹测试兜底防止两者漂移；
附带四项产品可见的收紧。

- **转义合并**：删除 `table::emitter` 私有的逐字符 `escape_html`，`encode_text` 的 `Escape` 分支改调
  `output::html::escape_html`——转义从此全链单点。新增字节对照用例（委派输出与旧逐字符实现逐字节相等）。
- **权威规范 + 生成 + 指纹对拍**：新建 `assets/table_html_whitelist.json`（唯一权威）与
  `tools/generate-table-whitelist.mjs`（派生 `src/output/table_whitelist.rs` 与页面 `index.html` 的
  `generated:table-whitelist` 标记区，两份内嵌同一个对规范化串取 sha256 的 fingerprint）。`output::html`
  新增 fingerprint 测试从权威文件重算指纹，与 Rust 常量、页面生成区注释三者比对，任一不一致即失败；
  `--check` 模式供门禁校验生成物是否与权威同步。
- **消费点合并**：`output::html` 删除内联的 `ALLOWED_TABLE_TAGS` / `ALIGNMENTS`，改消费 `table_whitelist::*`；
  `sanitize_attrs` 用 `ALLOWED_TABLE_ATTRS.contains` + `MIN_SPAN..=MAX_SPAN` 判据重写。
- **产品可见的收紧（破坏性，但更安全）**：① 引擎与页面标签集统一为引擎的 28 标签集（含 `html`/`body`
  外壳，页面因此从原 11 标签扩集；未引入任何新增可执行标签/属性）；② `align`/`valign` 按安全交集移出
  白名单（仅保留 `colspan`/`rowspan`）；③ 引擎 span 上界由 1000 收紧到 64，与页面对齐；
  ④ 页面 `colspan` 下界由 `>1` 改为 `>=1`，与引擎的 `[1,64]` 一致，保证双重净化幂等。
  本批当时只给了“新规则生效”的边界测试；“不影响真实输出”的确认范围与前后对比证据由后面的
  **白名单收紧的前后对比证据（审核点 5）** 一节补上（注：① 与 ④ 对页面而言是**放宽**，不是收紧）。
- 新增用例：span 边界（1✓ / 64✓ / 65✗ / 1000✗）、`align`/`valign` 拒绝、转义字节对照、fingerprint 三份对拍。

## 评测域字节摘要单点化（P1-8，未发布）

对应 `docs/12-structure-refactor-plan.md` 的 P1-8（M-3/M-5 周边，状态：批准）：评测域里
**逐字同形**的 SHA-256 小写十六进制摘要实现收敛到 `src/evaluation/digest.rs` 的
`pub fn digest_hex(bytes: &[u8]) -> String`。行为等价：同算法、同输出，manifest/报告哈希不变。

- 合并点：`evaluation::formula::sampling` 与 `evaluation::table::fixture` 的局部 `digest_hex`（已删），
  以及 `evaluation::table::run` 的两处同形内联（`engine_fingerprint` 与 `digest_file` 的哈希段）。
- 旁证用例：已知答案向量（FIPS 180-2 示例 + 空输入）钉"小写 64 字符"；`digest_file` 与
  `digest_hex` 对同一字节给出同一摘要。sampling/fixture 侧由其既有 manifest/digest 测试继续钉行为。
- **明确不合并**（不同域，且各自的"独立性"是有意的）：`model_store` 测试模块与
  `output::json` 快照测试里的 `sha256_hex` 是构造期望值的参照实现，不与被测单点合并；
  `formula/preprocess` 的 golden 哈希比对同理。`bin/formula_eval` 参考文件摘要（审计未列入
  P1-8 的第四处生产拷贝）本批不动，已在 docs/12 §9 登记为后续候选。

## 凭证绑定实际加载内容（审核点 1，未发布）

对应外部审核结论第 1 条（高危：TOCTOU）：`VerifiedModel` 此前只存**路径 + 摘要**，
会话随后仍按路径 `open_unchecked` → 校验与打开之间文件被替换时，凭证证明的是旧内容、
ORT 加载的可能是新内容。本批把“校验对象 = 加载对象”从约定改成结构：

- `VerifiedModel::verify` 先读**全部字节**，对内存快照算摘要、比对期望，并把内容作为
  `Arc<[u8]>` 快照钉进凭证；`Debug` 手写字节不进输出（只报长度）。读盘缓存（`verify_file`）
  不再参与凭证路径：凭证本来就必须读完全量，摘要与快照出自同一次观察。
- `OrtSession::open_verified(&VerifiedModel, …)` 新增：模型一律 `commit_from_memory` 从凭证
  字节装载；`open_unchecked(path)` 保留给测试夹具/探针，内部也改为读字节后同一条装载链，
  不存在“按路径二次开卷”的第二种加载语义。
- 切换的消费方：`OcrSession::new`（det/rec/cls）、`FormulaSession::new`、
  `FormulaDetector::from_model`、`TableStructureSession::open`（探针会话与正式会话共用同一
  份凭证字节，表格路径上最长的“两次开卷”窗口随之消失）、`TableStructureModelInfo::probe`、
  `FormulaModelInfo::probe`（无期望凭证：存在性照查、内容照绑定）。
- registration 层的 `verify_registered_model`（返回摘要字符串）失去全部生产调用点，删除；
  唯一入口改为凭证形态 `verified_registered_model`（分层不变：显式期望优先、否则取登记）。
- `verify(path, None)` 的语义如实改写：它只豁免**摘要比对**，不能被描述成“模型内容已验证
  完整性”；存在性与内容绑定仍然成立（`is_verified()` 继续如实回答）。
- 内存口径（如实）：凭证构造峰值多一份模型体积的瞬态字节；会话建好后凭证即释放。
  `formula_bench`/`formula_eval` 把凭证收进构造语句作用域，566 MB 快照不随运行期驻留。
- 新增用例：凭证内容快照在替换后不变（`model_verify`）；`open_verified` 端到端——构造凭证后
  把文件换成非 ONNX 字节，从凭证打开仍成功且契约成立，按路径再开必须失败
  （`table::structure`）；`registration` 既有 14 条迁入凭证形态，断言只增不减。

## 表格会话凭证入口（审核点 2，未发布）

对应外部审核结论第 2 条（中高）：日志声称“三个域的构造器已统一改收凭证”，但
`TableStructureSession::open` 实际仍接收 `&Path` 并在内部自行校验后按路径开卷，
评测入口也是路径调用。本批把表格域统一到与文本、公式相同的凭证契约：

- `TableStructureSession::open` 签名改为接收 `&VerifiedModel`：“拿不到凭证就建不了
  会话”在类型上成立；探针会话与正式会话都从凭证的内容快照打开。
- `TableStructureConfig::expected_sha256` 字段**删除**（它是公开面 `exports.rs` 重导出的
  struct 的字段）：摘要期望属于“打开之前的凭证构造”，不属于推理配置。需要显式期望的
  调用方在凭证构造点用 `registration::verified_registered_model(path, Some(hash))` 表达。
- 生产调用点迁移：`ocr::pipeline::rapid_structure`（rebuild 时用 policy 的期望走
  registration 解析）与 `evaluation::table::run`（登记摘要在 probe 前解析，凭证显式构造）。
- 测试：新增表格级替换回归 `table_session_opens_the_bytes_the_credential_verified`（凭证
  构造后替换文件，`open` 仍成功；重新构造的凭证绑定新字节后 `open` 必失败），与
  摘要不符用例（tampered 的 `HashMismatch` 断言移到凭证构造点，并保留探针入口的
  同类拦截）成对钉住；其余全部 `open` 调用点迁入凭证形态。lib 测试 732 → 733。

## P1-7 表格场景重测（审核点 3，未发布）

对应外部审核结论第 3 条（中）：原取消测量用的是 `result-real-42-regions.json`（该页无表格、
formulas 空），其 0.496ms 不能代表标题所指的表格场景。本批补齐表格负载并重判定：

- 入库两份可复现夹具：`tests/fixtures/json-output/p17-table-real.json`（真实捕获页，74 区域、
  1 张 10 格表）与 `p17-table-synthetic.json`（以真实表 region 为模板放大：8 张 100 格表、
  每表 HTML+table_html ≈ 63 KB）。
- 量测探针 `p17_measure_json_table_views_payload_and_time`（`#[ignore]`，不依赖模型）**保留入库**，
  分列三个量：视图本体构造时间、序列化时间、负载体积；另加一条非 ignored 守卫用例钉住
  夹具可加载与派生视图键集合（lib 测试 733 → 734）。
- 数据（debug min-of-300，3 轮一致）：真实表页 `to_value`=253–279µs、`to_output_json`=707–713µs、
  派生视图占 payload 29.6%；合成大表页 `to_value`=2.134–2.140ms、`to_output_json`=2.753–2.792ms，
  items/tables 视图负载 264 KB / 508 KB（占 full 1.44 MB 的 54%）。
- **重判定：取消结论维持**，但理由改口径：在“投影重构保持输出字节不变”的约束下，可消掉的
  只有视图遍历本体（16–133µs/视图，合计 <0.4ms），而增量的大头是派生负载自身再序列化一遍，
  字节不变就无法避免。删 `items`/`tables` 重复字段属独立的破坏性协议变更，按裁决与本项分开验收。
  （**复审 P2-4 回正**：上面“增量的大头”只对合成大表页成立（剩余 492µs vs 本体 403µs），
  真实页相反（本体 335µs vs 剩余 103µs）；且旧文里“视图本体只占 2.2–5.9%”是人工推算、
  分母口径不一致。现已入库结构化记述并按声明分母重算，见下面“P1-7 判据入库为结构化记述”一节。）
- 本项零生产代码改动（`to_output_json`/`to_output_items` 未动），入库只涉及探针与夹具。
- 门禁可信性修复（审核清单外，用户批准）：`serve` 测试的记录器改为把 `TablePolicy` 与请求体在
  **同一次加锁**内成对记录，`the_table_stage_runs_from_the_model_set_when_requested` 按请求体
  取策略而非按下标。根因是记录顺序为后端进入顺序，不等于提交顺序；此前登记的
  “串行 289 恒绿”并不成立（本批首跑即在 `--test-threads=1` 下复现），该用例隔离复跑
  12/12（改前）+ 10/10（改后）均通过，证明是竞态而非逻辑回归；不改变产品行为。

## 评测报告摘要并入单点（审核点 4，未发布）

对应外部审核结论第 4 条（中）：`src/bin/formula_eval.rs` 的 `compare_with_reference` 仍自行算
SHA-256，是 P1-8 审计漏登记的第四处生产拷贝。它已如实登记为后续候选，但与总结里的
"计划内全部任务完成"不一致；本批按审核建议走第一条路径（确认同形后并入单点）：

- 逐字比对确认同形：同算法（SHA-256）、同编码（`format!("{:x}")` 小写 64 字符）、同一
  `raw.as_bytes()` 输入，因此报告字节不变；分片合并的 `same_source()` 判据不受影响。
- 改调 `evaluation::digest::digest_hex`，删去块内局部 `use sha2::{Digest, Sha256}`（本文件不再直接
  依赖 sha2）。
- 新增回归用例 `the_reference_digest_goes_through_the_single_digest_helper`：钉两个预定答案向量，
  并证明报告里的字段是**内容**摘要而不是路径摘要（同一记录换键顺序/空白写法→摘要必变、
  记录条数没有变），并断言它与库内单点逐字相等。用例不依赖模型资产；`formula_eval` 目标
  自己的用例从 14 条增长到 15 条（lib 734 不变）。

## 白名单收紧的前后对比证据（审核点 5，未发布）

对应外部审核结论第 5 条（中）：P1-6 的白名单收紧有边界测试，但“不会影响真实产品输出”的证据
较弱，“真实表格没有超过 64 列”也只是 `[推算]`。本批不改变任何产品行为，只把结论换成可复现
证据，并把确认范围明确限定为**已纳入测试的数据与模型**：

- 判据从解析骨架里抽出来：`output::html` 新增私有 `SanitizerRules`（标签集 + span 属性 + 枚举属性），
  `sanitize_table_html` 成为把判据固定为 `TABLE_RULES`（取自 `table_whitelist` 生成物）的薄封装，
  真正的解析下沉为 `sanitize_table_html_with`。生产输出字节不变。
- 测试侧 `legacy_rules()` 用**同一内核**复现收紧前的引擎判据（`align`/`valign` 放行、span `1..=1000`），
  因此前/后之差只可能来自白名单本身，不掺入第二份实现的偏差。
- 入库对比产物 `tests/fixtures/table-whitelist-before-after.json`：真实捕获表页（`p17-table-real.json`）
  与黄金参考（`table-golden/manifest.json` 的 `reference.html`）的**变更前后完整 HTML**，另存 4 条
  构造样本；守卫用例 `the_saved_before_after_record_is_current` 会重新加载语料、重算两侧并比对，
  产物陈旧即失败；重生成走 `#[ignore]` 探针 `whitelist_before_after_record_regeneration`。
- 新增用例：`whitelist_tightening_leaves_the_committed_corpus_unchanged`（仓库内全部真实/黄金/合成
  表格 HTML 逐字节 before == after）、`the_before_after_comparison_is_discriminating`（钉住零差异不是恒真）、
  `the_golden_vocabulary_cannot_emit_a_removed_attribute`（黄金词表 span token 取值 2..=20 ≤ 新上界 64，
  且词表内无 `align`/`valign` token → 被删属性模型根本发不出来）。
- **分开验收**：产品决策（收紧本身）与等价重构（判据参数化）不再混写；页面 JS 侧的改动
  （标签集 11→28、colspan 下界 `>1`→`>=1`，属放宽）不经过 Rust 内核，仍由 fingerprint 三份对拍与
  `node tools\check-page-result-contract.mjs` 验收，本组前后对比不覆盖它。
- lib 测试 734 → 738（新增 4 条非 ignored 用例），另加 1 条 `#[ignore]` 重生成探针；serve 并行 / 串行仍 289。

## 会话缓存键纳入声明摘要（复审 P1-1，未发布）

对应复审结论第一条（P1）：`ocr::pipeline::rapid_structure` 的三个懒加载会话（公式识别器、
公式检测器、表格结构会话）**只按 `FileIdentity` 判定是否重建**，而 `expected_model_sha256` /
`expected_detector_sha256` 只在重建分支里被读取。因此：第一次请求用正确摘要加载→ 第二次请求
同一路径、同一文件身份但换一个错误摘要→ 旧会话直接回答，那条摘要从未参与比对——这与
`FormulaPolicy` / `TablePolicy` 文档里“给出摘要即校验”的契约直接相背。

- 新增私有 `SessionCacheKey { identity, declared }`：声明摘要（经 `model_verify::normalize_expected_sha256`
  只折叠大小写）成为键的一部分；重建判定单点收口到 `needs_rebuild`。声明一变必重建，
  而重建分支会按**这次的声明**校验并按需返回 `HashMismatch`。
- 规范化与判定**同形**：`verify` 用 `eq_ignore_ascii_case`，所以大小写是唯一可折叠的差异；
  本处**不** trim、不去前缀 —— 那些写法在校验里本来就会失败，折叠它们等于让第二个错误声明
  因命中缓存而不参与比对（即本批要关闭的洞）。
- 表格站点的登记回落（`None` → 同目录 `manifest.json`）仍在 registration 层：键里记的是
  **调用方的声明**（缺席也是一种独立状态），而不是解析后的值，因此“声明从无到有”同样会重建。
- 回归用例：无模型的判据守卫 `session_cache_rule_tests`（4 条：声明变/带空白/文件与路径变/
  未改动文件的两次观察），以及三个站点各一条**热会话 + 改声明**的端到端用例
  （`a_changed_{detector,recognizer,table}_declaration_is_not_answered_by_a_hot_session`）。
  把判据改回“只看身份”后，5 条新用例**全部失败**（已实跑验证），因此它们钉的是本次修复本身。
- 仍是启发式（不是安全边界）：文件被替换时能否发现取决于身份里的体积/mtime + 首尾各 64 KiB
  局部摘要，**中段修改**（体积与 mtime 都写回原值）仍是已登记的盲点；本批不改变这一点，
  也不把会话缓存描述成内容完整性证明。凭证自身仍把“被校验的”与“被加载的”钉同一份字节。
- lib 测试 738 → 746：新增 8 条 = `model_verify` 的规范化同形 1 条 + 无模型判据 4 条 + 需真实模型的
  端到端 3 条。端到端用例在缺资产时提前返回（与其它模型用例同类的静默跳过），所以判据本身
  靠那 4 条无模型守卫钉住。另有 2 条 `#[ignore]` 探针；serve 串行仍 289。
- `api.rs` 两处策略文档同步写明“跨请求也成立”。本提交实跑了全套七项门禁（fmt / clippy /
  `--all-targets` / serve 串行 / serve_startup / 两个 node 检）均通过。

## 不经凭证的会话入口收回 crate 内部（复审 P1-2，破坏性变更，未发布）

对应复审结论第二条（P1）：`runtime::session::OrtSession::open_unchecked` 仍是 `pub`，并经
`exports.rs` 随 `OrtSession` 一起对外可见。它直接读路径字节建 ORT 会话，不要求 `VerifiedModel`、
也不做域契约验证：生产域会话确实全部走 `open_verified`，但库的使用者仍可整条绕过完整性边界。
开发期没有兼容包袱，因此按建议把它从公开面拿掉，而不是把结论降级成“域会话入口不可绕过”。

- `pub fn open_unchecked` → `#[cfg(test)] pub(crate) fn open_unchecked`：**可见性与存在性一起收窄**。
  非测试构建里这个函数根本不存在（`cargo check --all-targets` 会连 lib/bin 的**非测试**目标一起编，
  编过就是“生产代码里没人调用它”的编译期证据）；crate 外部也拿不到它。字节加载的汇合点
  `open_bytes` 仍为私有，避免“绕开凭证”只是换个名字。
- 公开面上的会话构造入口只剩 `OrtSession::open_verified(&VerifiedModel, …)`，`exports.rs` 就地写明这条边界。
- `config.rs` 中 `effective_session_threads` 的文档不再指向一个公开方法（改说“测试侧的夹具 opener”），
  否则那个 intra-doc 链接会指向私有项。
- 新增回潮守卫 `tests/session_open_surface_guard.rs`（4 条非 `#[ignore]` 用例，不需模型资产）：
  ① 定义必须是 `pub(crate)` 且紧邻其上的代码行只能是 cfg 测试门，出现 `pub fn open_unchecked` 即失败；
  ② `src/` 里除定义外的每一处调用都必须落在本文件第一个 `#[cfg(…test…)] mod …` 之后（两个“整文件测试”
     文件走白名单，其前提由第 ④ 条钉住；测试边界只认“门 + 模块声明”的形状，否则定义行自己的
     `#[cfg(test)]` 会被误认成分界点）；③ 公开重导出面（`exports.rs` / `lib.rs`）不得出现 `unchecked`，
     同时 `OrtSession` 必须仍可达（守卫针对的是**方法**，不是类型）；④ `test_support.rs` 与
     `serve/tests.rs` 确实靠 cfg 测试门挂进构建。
  判别力（均已实跑后精确还原）：把 `pub(crate)` 改回 `pub` → ①② 双失败；删掉 `#[cfg(test)]` 门 → ① 失败。
- 连带动了一个既有的源码扫描守卫：`runtime/session.rs` 的 `every_run_entrance_takes_an_exclusive_borrow` 把
  “第一个 `#[cfg(test)]`”当成生产/测试的分界，而新增的 cfg 门恰好在前 → 它把整段生产代码丢了，
  `cargo test --all-targets` 直接报“`.run(inputs` 必须恰好两处”失败（全套门禁抓到的，不是事后补的）。
  分界规则改为“cfg 测试门 + 紧跟模块声明”，即真正的测试模块边界。改后覆盖变强：把一个新的
  `pub fn run_*(&self)` 入口插在 `open_unchecked` **之后**，旧规则看不见、新规则拦住（已实跑后还原）。
- **结论范围**：至此“外部没有不经凭证的会话入口”就整个 crate 而言成立；不改变的是文件身份
  判定的启发式性质与中段修改盲点（见上一节与 docs/12 §4.2）。
- docs/12 附录 §10 的 `src/ocr/session.rs` 一行原文已过期（“`OcrSession::new` 不接收摘要，直接
  `open_unchecked`”），本批更正，并在§10 开头说明：未被就地标注的行不代表对当前代码仍成立。
- 测试计数：lib 仍 746 passed + 2 ignored；新增一个集成测试目标（4 条）。

## 公式队列的断言改为按请求体关联策略（复审 P2-3，未发布）

对应复审结论第三条（P2）：`src/bin/serve/tests.rs` 的 M4 用例仍用 `policies[0]` / `policies[1]`
推断“文本任务在前、公式任务在后”。记录顺序是**后端进入顺序**，多 worker 下不等于提交顺序，
所以下标断言既可能因 worker 完成顺序变化而偶发失败，也可能把策略关联到错误的请求
（后者更隐蔽：断言依旧全绿，但钉的是另一件事）。上一批修表格时登记过“公式侧是同类风险”，
本批按同一手法闭合，不再另造机制。

- `Scripted.formulas` 由 `Vec<FormulaPolicy>` 改为 `Vec<(String, FormulaPolicy)>`：`recognize` 在
  **同一次加锁**内把（请求体, 策略）成对记录，与既有的 `tables` 完全同构。
- 新增 `formula_policy(body)`：按请求体取回该请求实际收到的策略，一次都没记录时直接 panic——
  “漏跑”也算回归，而不是静默少一条断言。
- M4 用例改为 `formula_policy("text image")` / `formula_policy("formula image")`，保留 `len() == 2`
  的计数断言；失败信息带上完整 `policies` 快照，便于诊断。
- 判别力（三组变异均实跑后精确还原）：① 把记录改成 `insert(0, …)`（进入顺序倒转）→ 新断言仍
  通过（说明它不再依赖顺序）；② 同一倒转下改用下标断言 → 在 `!text_policy.enabled` 处 FAILED
  （证明旧断言确实依赖顺序，不是纸面风险）；③ 把记录的请求体换成错误值 → panic
  `no formula policy recorded for request body "text image"`（证明关联是活的）。另：M4 用例隔离
  复跑 10/10 通过。
- 全文件已无 `policies[<数字>]` 形式的断言；其余顺序敏感点（`served`）只用于说明，不按 `served[n]` 断言。
- **本批只动测试**，生产逻辑未改：没有证据表明公式队列本身错位，被修的是回归测试作为证据的可靠性。
  测试计数不变（lib 仍 746 passed + 2 ignored、serve 并行/串行仍 289）。改动经脚本化最小替换
  （4 处各断言命中次数 == 1）+ `git diff` 逐字节核验：该文件工作树是 CRLF，禁止用编辑工具批量改。

## P1-7 判据入库为结构化记述（复审 P2-4，未发布）

对应复审结论第四条（P2）：P1-7 的测量探针只 `println!` 数据，“取消投影重构”是**人工解释**出来的
结论，不是机器判据。本批把测量结果入库为结构化记述，并加一条**非 `#[ignore]`** 的守卫用例。

- 入库产物：`tests/fixtures/json-output/p17-cost-record.json` —— `method`（profile / runs / rounds /
  统计量，并显式声明 `timings_are_machine_dependent: true`）+ `judgement`（分母口径、两条前提、结论、理由）
  + `samples`（每份样本的结构计数、逐视图字节、六项时间、以及可重算的 `derived` 字段）。
- 探针 `p17_measure_json_table_views_payload_and_time`（仍 `#[ignore]`）不再只打印：它分列量测后直接
  **生成同一份记述**（需设 `RAPID_STRUCTURE_P17_WRITE_RECORD=1` 才写盘），写盘前先校时间分项相容，
  避免一轮噪声写出一份守卫拦住不了的记述。
- 守卫用例 `the_saved_p17_cost_record_is_recomputable_and_self_consistent` **不重新计时**，它钉：
  ① 样本存在且仍可加载、区域/文本/公式/表格计数与记述一致；② 字节分项在当前代码上逐项重算相等
  （输出协议漂移即红），且 `full − source = views_total + key_overhead`；③ 算术自洽（增量 =
  `to_output_json − to_value`、本体 = 四视图之和、**本体 ≤ 增量**（每条记录的算术要求，因此无条件断言）、
  占比 = 本体/声明分母）；④ 结论前提（字节冗余非破坏性不可消、未声明机器无关绝对阈值）与
  `judgement` 字段集本身。
- 判别力 9 组变异（均实跑后还原，还原后记录文件与改前逐字节一致）：改 `views_total`±1（→ `json.rs:1134`）/
  改 `increment_ns`（→ 1157）/ 改单个视图时间（→ 1162）/ 把四个视图时间一起抬过整体增量、同时保持
  “本体 = 四视图之和”自洽（→ 1167，即无条件的 `本体 ≤ 增量`）/ 夹具名指向不存在的文件（→ 879）/
  改走占比分母（→ 1077）/ 删一个判据字段（→ 1052）/ 把“非破坏性可消冗余”改成 true（→ 1190）/
  改结构计数（→ 1103）→ 守卫全部失败；还原后通过。
- **本轮量测推翻了一个旧说法**（已就地更正 docs/12 §9 与上面“审核点 3”一节）：按声明分母
  `to_output_json`计算，视图本体占全路径 **46.55%**（真实页）/ **12.96%**（大表页），
  绝对值 0.336ms / 0.403ms（debug、本机）—— 不是旧文里的 2.2–5.9% / <5%。取消结论仍然成立，
  但它现在明写建立在两条前提上：字节冗余只能靠破坏性输出协议变更消除；时间收益只覆盖
  JSON 构造这一个步骤。**未覆盖范围**：本批没有重测 JSON 构造在整页请求里的占比（仓库内无可
  引用的端到端 bench），若日后证明该步骤在端到端里可观，该结论应重新评估。
- 本项零生产代码改动（`to_output_json` / `to_output_items` 未动）；测试计数 lib 746 → 747（新增 1 条
  非 `#[ignore]` 守卫），`#[ignore]` 仍 2 条，serve 并行/串行仍 289。
- 本批提交前实跑全套七项均为 0：`cargo fmt --all -- --check`、`cargo clippy --all-targets --features serve
  -- -D warnings`、`cargo test --all-targets --features serve`（lib 747 passed / 2 ignored）、
  `cargo test --bin rapidstruct --features serve -- --test-threads=1`（serve 289）、
  `cargo test --test serve_startup … -- --test-threads=1`、两个 node 检（页面结果契约 / 白名单 `--check`）。

## 记录回正：不可绕过措辞与缓存盲点的范围（复审末段，未发布）

对应复审结论末段：“该缓存机制仍明确承认中间区域修改的残余盲点；这属于已登记的启发式限制，
**不应被表述为安全边界**”，以及 P1-2 那一节里“结构上不可绕过”的历史写法。本批**不改行为**，
只把措辞收到事实范围内：

- `src/model_verify.rs`：`FileIdentity` 的文档原写“相等即‘缓存里的摘要仍然描述这个文件’”——
  那是把启发式说成内容完整性证明的越界写法，不成立。改为“相等 = **没有观察到变化**”，并写明中段修改（同体积 +
  同 mtime + 首尾 64 KiB 保持不变）仍在盲区，要把“被校验的”与“被使用的”钉成同一份字节必须走
  `VerifiedModel` 持有的内容快照。
- `src/ocr/pipeline/rapid_structure.rs`：`SessionCacheKey` 文档新增一节“键里的文件身份是启发式，
  不是安全边界”：声明摘要进入键保证的是“给出摘要即校验”跨请求也成立，**不保证**磁盘字节与摘要一致。
- `docs/12` §5：P0-3 目标设计里“结构上不存在‘忘记校验’的旁路”就地标注为**设计目标**（该句在
  P0-3 二阶段→ `86cc76e` 之间不成立，因为 `open_unchecked` 当时仍 `pub` 且对外可见）；P1-2
  （`LazyModel`）设计里的“同一身份判据”就地标注为已被复审 P1-1 推翻，并登记 `LazyModel<T>` **至今未实施**
  （字段仍内联在 `RapidOcrEngine` 上），将来实施时键必须带声明摘要。§10 前言的“已就地标注”清单同步。
- `docs/12` §9：补齐两行列数（“门禁可信性”与“记录回正 r6”两行此前只有 6 个单元格而表头是 7 列，
  渲染时最后一列会错位）；新增本批一行。内容本身未翻：只拆了列，没改事实。
- `log.txt`（本地总结，未跟踪）：新增“第二轮复审（q1–q5）”一节；并改掉两行过期事实：
  “末次全套七项门禁在 `1fbdb6d` 上跑通 / lib 738” → 记到最新一次；“已登记未修项：`Scripted.formulas()`
  的下标断言” → 标注已由 `de44a40` 闭合。
- 本批改动会重编 lib，因此重跑全套七项均为 0（fmt / clippy `--all-targets --features serve -D warnings` /
  `--all-targets` / serve 串行 / serve_startup / 两个 node 检）；另跑 `cargo doc --no-deps --features serve`
  （退出 0）：仓库本身带 67 条 rustdoc 警告（指向私有项的链接与既有 unresolved link），逐条筛查后
  **没有一条**指向本批新增的 `FileIdentity` / `SessionCacheKey` / `VerifiedModel` 链接 —— 这是“新增链接
  可解析”，不是“仓库文档无警告”。测试计数不变（lib 747 passed + 2 ignored、serve 并行/串行 289）。
- **未改变的事实范围**：中段修改仍是已登记的启发式盲点，本批没有加新的检测手段（全文件哈希或
  内容快照比对都是独立的性能预算决策）；`log.txt` 未跟踪，不能当作仓库门禁证据。

## 表格会话缓存键纳入会话构造参数（复审 P1，未发布）

对应复审结论第一条：“第一次请求用 `fix_input_shape = true`、边长 488；第二次请求使用
`fix_input_shape = false` 或另一边长；文件和摘要不变；旧会话被复用，第二次策略实际不生效。”

- **缺陷**：[`SessionCacheKey`](src/ocr/pipeline/rapid_structure.rs) 只有文件身份 + 声明摘要，而
  [`TablePolicy`](src/api.rs) 的 `input_side_len` / `fix_input_shape` / `intra_threads` 决定的是
  **已创建的那个会话本身** —— ORT 的线程选项与 free-dimension override 都在 `open` 时一次性
  施加，之后没有第二次机会。于是“引擎实例复用 + 模型文件一个字节没变 + 只改配置”的第二次
  请求命中旧会话，那次配置根本没生效。
- **修法**：键新增 `config: SessionConfigKey`（`EngineOnly` / `Table(TableStructureConfig)`）。表格
  站点存**整个** `TableStructureConfig`，而不是手挑三个字段 —— 将来新增一只会影响会话的旋钮
  自动进键，不需要记得改这里。`table_session` 先组装 config，**同一份**既进键又交给 `open`，
  所以“键里比较的”与“会话实际用到的”不可能分叉。公式两个站点改用 `SessionCacheKey::engine_only`
  （它们的会话参数全部来自引擎唯一的运行时档案，请求里没有可变的设置）。
- **后果不是“配置稍慢生效”，而是一次形状不匹配的推理**。把键退回“不含 config”后的实测失败：
  `Got invalid dimensions for input: x … index: 2 Got: 600 Expected: 488` —— 同一张请求里的预处理
  仍按**新**边长生成张量，而被复用的会话仍钉在**旧**边长上。该事实已写进用例文档。
- 新增两条守卫：
  - 端到端 `a_changed_table_config_is_not_answered_by_a_hot_session`（真实 SLANet-plus + 真实表格页）：
    四段前后请求分别只改边长 / 形状固定策略 / intra 线程，后置条件读会话自己的
    `config()` / `input_dims()` / `session_threads()`，而不是只断言“没报错”。
  - 无模型规则用例 `a_changed_table_config_rebuilds_without_any_file_change`：同一身份 + 同一声明，
    三处配置变异各自必须重建、同配置不得重建，且表格键与公式键不得重合。
- **两类守卫的分工已实测**：把调用点的键退回 `engine_only` 后，端到端用例 FAILED（上面那条 ORT
  形状错误），而规则用例仍绿 —— 它钉的是键类型，不是调用点。因此两者都不可省。还原后全部
  通过，文件哈希与改前逐字节一致。
- 文档：`api.rs` 的 `TablePolicy` 新增“配置跨请求也生效”一节；`SessionCacheKey` 与 `table_session`
  各新增一节说明为什么构造参数必须是键的一部分。
- 本批提交前实跑全套七项均为 0（fmt `--check` / clippy `--all-targets --features serve -D warnings` /
  `--all-targets` / serve 串行 / serve_startup / 两个 node 检）；测试计数 lib **747 → 749 passed**
  （+2，`#[ignore]` 仍 2）、serve 并行与串行各 **289**（串行 278.56s），其余测试目标计数不变。
- **未改变的事实范围**：键里的文件身份仍是启发式（中段修改盲区，见“记录回正”一节）；表格的
  `expected_model_sha256 = None` 回落登记摘要这条路径**仍未进键**（复审 P1-b，下一批）。

## 表格登记摘要回落也进会话缓存键（复审 P1-b，未发布）

对应复审结论第二条：“应先解析‘有效摘要’，再用该摘要构造键和凭证，并增加‘manifest 摘要改变 +
热会话’的回归测试。”与已修的 P1-1 同构 —— 显式摘要那条路径修好了，登记回落这条没修。

- **缺陷**：`TablePolicy::expected_model_sha256 = None` 时期望摘要来自模型同目录的
  `manifest.json`（[`resolve_registered_expected`](src/table/registration.rs)），但
  `table_session` 先用**原始声明**构造键（`None` 直接进键），随后才在重建分支里
  resolve + verify。于是清单里的登记摘要被改掉时：键仍是 `declared = None`、文件身份也不变，
  旧会话继续回答，新登记值一次也不会被重新校验。
- **修法**：取文件身份**之前**先把有效摘要算出来，**同一个值**既进键又交给凭证构造。
  传给 `verified_registered_model` 的是已解析的值，所以 registration 层内部的回落分支
  不会重读清单（重读只会发生在本来就需重建的那一次）。
- **校验语义不变**：显式声明仍按原规则（非空即 `trim` 后胜出），登记摘要仍按 registration
  的小写化与合法性判定，“给出摘要即校验”没有放宽也没有变严 —— 变的只是“这句话跨请求还算不算”。
- **行为变化（如实登记）**：清单解析从“仅在重建分支”变成“每条表格请求”。因此“模型旁边有清单、
  但这个模型没被登记”（或角色不是 `table_structure`、`sha256` 为空）现在会在**任意**一次表格
  请求上报错，而不是只在会话恰好重建时报错 —— 后者等于“受清单管辖”与否取决于缓存状态。代价是
  每请求多一次 `<model_dir>/manifest.json` 的读取与 JSON 解析（本仓库那份 12 行）；本批没有
  为它做端到端计时，因此**不声明**性能影响幅度。
- 新增端到端用例 `a_changed_table_registration_is_not_answered_by_a_hot_session`（真实模型 +
  真实表格页，策略里留 `None`）：登记正确 → 通过且有真表格；登记被改掉（模型一个字节没动）→
  `HashMismatch(expected = 清单值, actual = 磁盘实测)`；登记改回来 → 又能跑（键的变化是双向的，
  一次失败不会把会话钉死）。规则用例另补 `None → Some` 方向。
- **判别力**：把键退回“原始声明”后该端到端用例在 `expect_err` 处 FAILED（即“热会话吞掉了登记
  变化”），而 `session_cache_rule_tests` 仍全绿 —— 它钉的是键类型，钉不了调用点。该分工已写进
  规则模块文档。实跑后精确还原，还原后文件哈希与改前逐字节一致。
- 文档：`api.rs` 的 `TablePolicy::expected_model_sha256` 补“进键的是解析后的有效摘要”与每请求
  解析清单的后果；`SessionCacheKey` 补一段同构论证，字段文档从“声明摘要”改为“有效期望摘要”。
- 本批提交前实跑全套七项均为 0；测试计数 lib **749 → 750 passed**（+1 端到端）、serve 并行与
  串行各 **289**，其余测试目标计数不变。
- **未改变的事实范围**：键里的文件身份仍是启发式（中段修改盲区，复审第三项另批处理）。

## 状态缓存的加载期证伪与口径回正（复审 P1/P2，未发布）

对应复审结论第三条：“局部身份缓存仍不能保证当前磁盘内容正确 …… 如果该缓存只是性能启发式，
需要在架构和 API 文档中明确‘状态不是当前内容的确定性证明’”。本轮把两件事**同时**做了：口径按启发式收窄，
并给缓存加一条**由加载驱动**的收敛口（没有改成全文件校验或强制每次重验，理由见末尾）。

- **缺陷**：`VerifiedModel::verify` 完整读盘（它是唯一能绑定内容的入口），但它**既不写也不废**
  身份键控的校验缓存。于是同体积、同 mtime、首尾 64 KiB 不变的中段修改会造成：`/api/models`
  继续报告旧摘要（甚至 `present`），而同一次加载已按新内容拒绝会话或按新内容构造会话 ——
  状态报告与实际加载结果分叉，且**没有任何收敛时刻**。
- **修法（证伪即作废）**：凭证算出完整摘要后，若与缓存条目不符则**删除**该条目，
  **匹配与不匹配两条分支都执行**（比对失败也删：那正是“磁盘不是声明的那一份”的直接证据）。
  下一次 `verify_file` 因此必然冷验证并给出当前磁盘的真实摘要。
- **为什么删而不覆盖**：这条路径上没有任何一次观察把“当前身份”与“当前摘要”一起取过来
  （`verify` 读的是内容，不取 `FileIdentity`；取也只能是读盘**之后**的另一时刻）。用任一侧的身份
  配上这次的摘要会写进一条从未被一起观察过的记录 —— 文件正在被改写的中间状态也可能被钉成
  “已验证”。删除只放弃一次加速机会（下一次冷验证重读盘），换来一条不会被误用的账。
- **口径回正（不再把启发式写成完整性保证）**：`model_verify` 模块文档新增“状态是什么、
  不是什么”；`model_set.rs` 的 `state_in` 与 `ModelFileState::Present` 写明“命中时报告的是
  上一次完整校验的摘要”；`model_plan.rs` 的 `report()` 原来写着“状态**永远**报告磁盘上的
  **当前**事实”—— 该句不成立，已按两层拆开重写（存在性与身份每次实读；摘要可能来自缓存）。
  `admit_ocr` 的“只有这一种结论需要看磁盘上的当前事实”同样改为“重新观察存在性”（旧写法把
  摘要那一半也说成了实读，越界）。
- 新增 4 条用例：中段修改后带可信摘要的加载 → 条目作废 → 下一次状态 `computed == true`
  且给出新摘要（并钉“第三次的结论是热的”，收敛不是一次性失效循环）；比对失败路径同样作废；
  `expected = None` **不作废**（无证据 —— 这条限制被钉成测试而不是留给读者推断）；
  “相符保留 / 不符删除 / 无条目不动作”三态直接测底层函数。
- **判别力**：摘掉 `verify` 里的那一次调用后，两条收敛用例 FAILED（`model_verify.rs:1288:9` 与
  `:1336:9`，断言消息分别是 "a load that falsified the cached digest must not leave a hot entry
  behind" / "a refused load falsifies the entry all the same"），另两条（限制与三态）仍绿。
  实跑后精确还原，还原后文件哈希与改前一致。
- 文档：`docs/05` §10 第 10 条改写为“三个确定性入口”（启动期 `--reverify-models`、运行期
  `POST /api/models/reverify`，以及**加载路径本身**），并写明“任何把命中时的状态写成‘已按
  磁盘当前内容核实’的措辞都是越界的”。
- 本批提交前实跑全套七项均为 0；测试计数 lib **750 → 754 passed + 2 ignored**（+4 条）、
  serve 并行与串行各 **289**（串行 277.43s），其余测试目标计数不变。
- **未改变的事实范围（必须与上一行一起读）**：
  1. 命中仍不等于当前内容 —— 一个没被加载过、也没被 reverify 过的文件，中段修改仍然不会在
     状态里出现；本批把分叉从“无限期”收窄到“至多到下一次加载”，没有把它变成实时保证；
  2. 作废只管**状态**，不管**已建好的会话**：会话缓存同样按身份判定是否重建，所以运行期要
     同时刷新“报告的状态”与“内存里的会话”，仍只有 `POST /api/models/reverify` 那一条路；
  3. `expected = None`（集合外模型、无声明摘要）不产生摘要，因此没有证伪证据；
  4. 本批没有改为全文件校验或强制重验：566 MB 模型在 8 s 轮询下重读是不可接受的代价，而
     复审给的两条路里，本批走的是“明确它是性能启发式 + 提供确定性入口 + 加载期收敛”这一条。

## 声明口径的机器化守卫 · 残余 P1 风险（第四轮复审，未发布）

本轮**不改行为**：两个表格缓存缺陷与加载期证伪已闭合，复审对剩下的“中段修改仍属启发式盲区”
给的处置是——作为**已声明的启发式限制**接受，同时要求继续禁止把 `present` 或缓存命中写成
“已核实当前磁盘内容”。这类禁令只靠人复查一轮就会漂移，所以本轮把它做成机器判据。

- 新增 `tools/check_model_state_claims.ps1`，扫描 `src/`、`docs/`、`tools/`、`tests/` 与根目录
  两份 `md`（共 220 个文本文件），两条规则：
  - **禁止式**：文本先按空行切成块，块内按 `。！？;；.` 切成句；同一句里出现 15 条越界短语
    （“完整性保证”、“实时保证”、“状态永远报告”、“已核实当前磁盘内容”、“integrity guarantee”、
    “trusted at all times” 等）而该句自己不含否定字根（不 / 无 / 未 / 勿 / 拒 / 禁 / 非 / 越界 /
    not / never / cannot / without）→ exit 1，并点名文件、行号与原句。
  - **必须式**：README、`model_verify.rs`、`model_set.rs`、`model_plan.rs`、`docs/05`、`docs/12`
    各自的口径句必须在场——只禁止越界写法而不管“悄悄删掉免责声明”，挡不住后半种回归。
  - `-SelfTest` 用 7 组合成样本自证判据仍会咬人：两处肯定式越界（其中一处刻意埋在“前几段
    全是否定词”的长文之后）、一处英文跨行折行的规范写法、以及正斜杠与反斜杠两种相对路径
    下的“口径句被删”各一组；任一不符则 exit 2。它顺带钉住“脚本按 UTF-8 读到了中文”，
    所以本脚本必须带 BOM（Windows PowerShell 5.1 读无 BOM 的 UTF-8 会把中文短语按 ANSI 误读，
    那会让判据静默失效）。
  - 把短语当**判据数据**的文件（本脚本自身、未来把字面量写进测试的文件）在文件头声明
    `claim-guard: phrases-are-data`，跳过禁止式，必须式照常跑。
- 判据是实跑纠正过三次的，不是一次写对的：
  1. 首版取“短语前 220 字符”当否定窗口，在 `src/model_verify.rs` 末尾注入一句越界注释后
     守卫**返回 0**（长中文文档里前向窗口几乎必然撞到某个“不”字），遂改为同一句内判否定；
  2. 英文段落只按 `。` 切会整段算一句，于是上一句的 `not` 能给下一句的宣称兜底，
     故补 `.` 与 `…` 作边界；
  3. “必须式”查字典用相对路径，Windows 下扫到的是 `src\model_set.rs` 而键是正斜杠，
     于是它一直在**静默失效**；归一化后第一次真正生效就抓到三处“我以为写了其实没写”——
     `model_verify.rs` 里并无“性能启发式”字样、`model_plan.rs` 的口径句被文档注释拆成两行、
     `docs/05` 写的是“只有三个入口”而不是“三个确定性入口”。必须句因此改按仓库里**实际在场**
     的写法取值，而不是按理想措辞。
- `README.md` 的 *What the digest cache does — and does not — guarantee* 一节按本轮口径补齐：
  命中时报告的是上一次完整校验留下的那份摘要、确定性入口从两个改为三个（启动期
  `--reverify-models`、运行期 `POST /api/models/reverify`、以及任何携带可信摘要的加载）、
  已建好的会话与 `expected = None` 两条仍不在覆盖范围内，并写明这条措辞边界由上述脚本
  机器执行。开头那句“每个逐文件状态问题都走摘要缓存”也已收窄：admission check 只看
  存在性，不报摘要。
- `CHANGELOG.md` 自身三处引用旧错误原句的条目补上“越界 / 不成立”标注——本轮首跑守门的
  6 条命中全部是这类“引用被废弃写法”的句子，而不是真的宣称。
- 门禁：`docs/12` §8.1 清单加入两条守卫命令（先 `-SelfTest` 再正式扫描）。本批提交前全套
  实跑，退出码均为 0；测试计数不变（lib **754 passed + 2 ignored**、serve 并行/串行各 **289**），
  因为本批零源码改动。另在真实文件上跑过一轮双向判别力：注入越界注释 → exit 1 点名
  `src/model_verify.rs:1609` 三条，删掉 `model_set.rs` 的口径句 → exit 1 报 `missing-statement`，
  还原后 exit 0 且两份文件哈希与改前一致。
- **残余风险本轮未消除，也不应当被叙述为已消除**：同体积 + 同 mtime + 首尾 64 KiB 不变的
  中段修改依旧不会实时进状态，收敛仍需要一次携带可信摘要的加载或两个 reverify 入口之一；
  已建好的会话不受证伪影响；守卫扫的是措辞与口径句在场，它管不了代码将来真的把缓存当
  完整性边界使用。验收措辞的上限就是复审给的那句：接受已声明的启发式限制，
  而不是“模型状态始终反映当前磁盘内容”。

## 已知限制

见 README 的 *Known limitations* 一节（检测器误检、抹白导致的文本重新分割、
公式后处理不是 RapidDoc 的完整等价实现、CUDA 在本机未验证等）。
