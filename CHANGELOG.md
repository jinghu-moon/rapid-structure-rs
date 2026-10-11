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
  白名单（真实发射器从不产出它们，仅保留 `colspan`/`rowspan`）；③ 引擎 span 上界由 1000 收紧到 64，与页面对齐；
  ④ 页面 `colspan` 下界由 `>1` 改为 `>=1`，与引擎的 `[1,64]` 一致，保证双重净化幂等。
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

## 已知限制

见 README 的 *Known limitations* 一节（检测器误检、抹白导致的文本重新分割、
公式后处理不是 RapidDoc 的完整等价实现、CUDA 在本机未验证等）。
