# 11 表格识别接入整页管线与本地服务执行文档（E14–E17）

> 本文是 `docs/09-table-recognition-execution.md`（E1–E13）的**后继执行文档**，覆盖同一目标的第二阶段：
> 把已经独立跑通的表格能力链接进**引擎的整页管线**，再让**本地服务与网页**用上它。
>
> 读者：执行本阶段的人（含未来的我）。写法沿用 09：任务清单 → 验收 → 命令 → 完成记录，
> 每条结论标注证据等级，每个阶段结束时把「变更摘要 / 命令 / 关键结果 / 基线对比 / 门禁判定 / 未覆盖风险」
> 追加到本文的阶段完成记录里。
>
> **读本文前不需要重读 09**：§1 给出起点，需要的 09 细节都有文件与行号指路。

---

## 0 前提与约定

### 0.1 与 `docs/09` 的边界（这是本阶段存在的理由）

`docs/09` §14.3.8 明确记录了一个**范围边界**，而不是欠账：

- E1–E13 交付了完整的表格能力链：定位（`src/table/detection.rs`）、预处理（`src/table/preprocess.rs`）、
  结构推理会话（`src/table/structure.rs`）、解码（`src/table/decode.rs`）、内容回填
  （`src/table/cell_content_backfill.rs`）、发射（`src/table/emitter.rs`）、登记与加载期校验
  （`src/table/registration.rs`、`models/table/manifest.json`）、评测（`src/evaluation/table/`）。
- 但**整页管线没有调用它**。`src/ocr/pipeline/rapid_structure.rs:918-920` 把三个表格计时字段写死为 `0.0`，
  `src/ocr/pipeline/rapid_structure.rs:1011-1012` 把 `StageReports.table` 恒置为 `StageReport::default()`。
- 因此 **G5（整页 p50 增量 ≤ `table_structure_budget`）从来没有被实测过**，`table_structure_budget`
  至今**没有数值**（09 §0.5 写的是"待 E1 接入后的首轮实测确定"）。本文 E14 的第一件事就是给它一个数字。

一句话：E1–E13 造好了零件，E14–E17 把它装到机器上，并让用户在网页里看到结果。

> **本节描述的是 E14 开工前的状态**，保留原文以便对照。E14 收口后的现状是：整页管线已接线
> （`recognize_with_table`），`table_structure_budget` 已定值 **120 ms**（实测中位 56.491 ms），
> G5/G6/G7 已实测通过，`detection.rs` 的 `#![allow(dead_code)]` 已删除。完整记录见 §2.5。
> 下面 §0.4 的 G5 行已就地更新为定稿值。

### 0.2 证据分级（沿用 09 §0.3，不得混用）

| 标记 | 含义 | 允许出现在 |
| --- | --- | --- |
| `[实测]` | 本机跑出来的数字，命令与参数可复现 | 结论、门禁判定、基线对比 |
| `[推算]` | 由已实测的数字按明确公式推出 | 中间推理，必须写出公式 |
| `[未测]` | 尚未测量 | 风险、阻塞、待办 |

**性能绝对数字只允许来自**：`src/bin/bench_warm_e2e.rs`（页面级）、`tools/run_windows_baseline.ps1`（12 图基准）、
`src/bin/table_eval.rs`（表格域）。Python 探针数字（`Temp/probe_*.py`）**只可用于成本模型形状**，
绝不可用于性能结论 —— 依据见 `docs/07-table-recognition.md` §0 与 `docs/08`。

### 0.3 验证分层（沿用 09 §0.4）

| 层 | 范围 | 必须通过 |
| --- | --- | --- |
| **L0 契约** | 类型、序列化、`validate()` | `cargo test --lib` |
| **L1 单元** | 每个新函数的行为与后置条件 | `cargo test --all-targets` |
| **L2 回归** | 整页输出对既有能力的**零影响** | 12 图 bench + `evaluation-cpu.json` + provider smoke |
| **L3 页面** | 本地服务页面与 JSON 契约 | `cargo test --bin rapidstruct --features serve` + 手工页面自检 |

`runtime` / `session` / `contracts` / `api` / `ocr::pipeline` 的改动必须走完整四层。

### 0.4 门禁

沿用 09 的 **G1–G4**（见 09 §0.5），本阶段新增 **G5–G8**：

| 门禁 | 判据 | 类型 | 归属 |
| --- | --- | --- | --- |
| **G2**（沿用） | 回填路径 `recognizer_calls` 增量为 0 | 结构不变量（单测可断言） | E14 |
| **G3**（沿用） | 结构推理调用次数 == 表格数，且不随单元格数增长 | 结构不变量 | E14 |
| **G4**（沿用） | 表格定位不新增整页 CNN | 静态审查 + 测试 | 已由 E11 完成 |
| **G5** | 可归因表格分量之和（`table_detect_ms + table_structure_infer_ms + table_backfill_ms`，来自 `timing_ledger.totals`）中位数 ≤ `table_structure_budget` = **120 ms** | 计时（**已定值，见 §2.5.4**） | E14 ✅ |
| **G6** | 表格路由**关闭**时，整页输出与基线逐位一致 | 回归不变量 | E14 |
| **G7** | 整页路径上表格模块不额外调用识别器（`BackfillOutcome::recognizer_calls == 0`） | 结构不变量 | E14 |
| **G8** | 网页把模型产出的 HTML 渲染进 DOM 之前必须经白名单重建；注入型单元格文本不得产生可执行节点 | 安全不变量（单测可断言） | E16 |

**G6 为什么必须逐位一致**：表格路由关闭时 `TablePolicy::enabled == false`，整页管线必须走与今天
**完全相同**的代码路径。这是"新增功能没有偷偷改掉旧行为"的唯一强判据 —— 09 的 G1 用的是"落在
±10–15 % 噪声带内"，那是**性能**判据；G6 是**内容**判据，两者不能互相替代。

**G5 的判据在 E14.7 实测后被修正过一次**（原始表述是"整页 `ocr_total_ms` p50 增量 ≤
`table_structure_budget`"）。实测发现表格开销是**双峰**的（不含表格的页面增量为 0，含表格的页面
增量为整份表格成本），而 `p50` 取的中位样本恰好落在两峰的分界上，因此它是最不稳定的统计量：
4 个交替配对实测 Δp50 中位数 **+10.80 %**，已经落进本仓库用来表示"测不出变化"的 ±10–15 % 噪声带。
用一个与噪声同宽的尺子量一个比噪声还小的量，判据就不成立。改用**同一次请求内部**测出的可归因
分量之和（不受轮间漂移影响），并把 Δp50 / Δavg 作为**信息性**数字一并记录。推导见 §2.5.4。

### 0.5 阶段总表

| 阶段 | 名称 | 依赖 | 关键门禁 | 状态 |
| --- | --- | --- | --- | --- |
| **E14** | 引擎整页管线接入表格 | E1–E13 | **G5** / G6 / G7 / G2 / G3 | **✅ 已完成**（§2.5） |
| **E15** | 输出三通道（JSON / HTML / Markdown） | E14 | L0/L1/L2 | **✅ 已完成**（§3.5） |
| **E16** | 本地服务与网页（模型 UI / 模式选择 / 表格渲染） | E15 | **G8** / L3 | **✅ 已完成**（§4.8） |
| **E17** | 收尾与登记 | E16 | — | **✅ 已完成**（§5.5） |

**关键路径**：E14 → E15 → E16 → E17。E14 是唯一不可并行的一段（E15 要消费 E14 定下的类型）。

---

## 1 起点：E1–E13 已经交付了什么

### 1.1 可直接复用的能力链

| 环节 | 位置 | 入口 |
| --- | --- | --- |
| 表格定位（零模型） | `src/table/detection.rs` | `TextDistributionDetector::detect(&mut self, image, page_lines) -> Result<Vec<TableRegion>>` |
| 结构预处理 | `src/table/preprocess.rs` | `table_structure_input(&RecImage, side_len) -> Result<(Array4<f32>, TableCanvas)>` |
| 结构会话 | `src/table/structure.rs` | `TableStructureSession::open(model_path, runtime_cfg, config)` / `run_with(input, f)` |
| 结构解码 | `src/table/decode.rs` | `argmax_structure` → `decode_token_steps` → `tokens_to_structure` |
| 内容回填 | `src/table/cell_content_backfill.rs` | `backfill(cells, BackfillSource::Reuse { lines }) -> Result<BackfillOutcome>` |
| 发射 | `src/table/emitter.rs` | `emit_html(&StructureResult, &[String], TextPolicy) -> Result<RenderedTable>` |
| 登记与校验 | `src/table/registration.rs` | `verify_registered_model(model_path, expected)` |

`src/evaluation/table/run.rs:1380-1490` 是**这条链目前唯一的驱动者**，E14 的 `recognize_with_table`
应当逐行对齐它的调用顺序（`table_structure_input` → `run_with` → `argmax_structure` →
`decode_token_steps` → `TableShape`/`rescale_ratios` → `rescale_polygon` → `tokens_to_structure` →
`backfill` → `emit_html`），差异只有两处：

1. `run.rs` 的输入是**单张表格图**；整页管线的输入是**整页图**，表格区域来自 `TextDistributionDetector`，
   因此必须处理 `TableRegion::origin` 的**坐标偏移**（`src/table/detection.rs` 的 `TableRegion`
   同时给 `crop: RecImage` 与 `origin: [f32; 2]`）。
2. `run.rs` 的文本来自 `PageOcr`（它自己跑一遍页级识别）；整页管线**已经有**页级识别结果，
   必须走 `BackfillSource::Reuse`（这正是 G2/G7 的由来）。

### 1.2 已就位但恒空的槽位（E1 刻意留的）

| 槽位 | 位置 | 现状 |
| --- | --- | --- |
| `RegionKind` | `src/api.rs:466` | 只有 `Text` / `Formula`，**没有 `Table`** |
| `StageReports.table` | `src/api.rs:671` | 字段存在，恒 `StageReport::default()` |
| `OcrTimings::table_detect_ms` | `src/api.rs:696` | 恒 `0.0` |
| `OcrTimings::table_structure_infer_ms` | `src/api.rs:699` | 恒 `0.0` |
| `OcrTimings::table_backfill_ms` | `src/api.rs:702` | 恒 `0.0` |
| `TimingLedger` 的三个表格分量 | `src/runtime/timing.rs`（E1 已加） | 已计入 `attributed_ms` / `inference_ms` / `rust_ms` / `shares` / `mean`，但页面上恒 0 |

### 1.3 为什么这是"接线"而不是"新建"

第一性依据：**表格不需要新的页级前向**。

- 定位走**已有 det 文本框的分布**（E11 实测：`table.jpg` 74 框 → 1 区域，IoU 0.957；定位器自身
  `1.713 ms/call`，约占 1000 ms 整页预算的 **0.17 %**）—— 零新增 CNN（G4）。
- 单元格文本走**已有 rec 结果**的几何匹配（`BackfillSource::Reuse`）—— 零新增识别调用（G2/G7）。
- 唯一新增的前向是**每个表格一次**的结构模型（SLANet-plus，488²，E5 实测单次 25–31 ms 量级，
  约占页面预算 3 %）—— 这才是 G5 要量化的东西。

---

## 2 E14 引擎整页管线接入表格

### 2.1 任务

- [x] **E14.1** `src/api.rs`：`RegionKind` 增加 `Table` 变体（`#[serde(rename_all = "snake_case")]`
  下序列化为 `"table"`），并加 `TableOutcome`：

  ```rust
  pub struct TableOutcome {
      /// 结构树发射出的完整 HTML（`<html><body><table>…`）。
      pub html: String,
      /// 仅 `<table>` 外壳，供只想嵌一段表格的消费者使用。
      pub table_html: String,
      /// Markdown 呈现；`rowspan`/`colspan` 语义在 Markdown 里无法表达，见 E15.3。
      pub markdown: String,
      pub rows: usize,
      pub cols: usize,
      /// 结构置信档；`Low` 表示逻辑网格有空洞或被截断。
      pub confidence: StructureConfidence,
      pub degradations: Vec<StructureDegradation>,
      /// 单元格槽位（与 HTML 里的 `<td>` 同序）。
      pub cells: Vec<TableCellOutcome>,
      /// 被本表格消费的页级文本行在 `OcrOutput::regions` 中的下标。
      pub source_line_indices: Vec<usize>,
      pub model_id: String,
  }

  pub struct TableCellOutcome {
      pub row: usize,
      pub col: usize,
      pub rowspan: usize,
      pub colspan: usize,
      pub text: String,
      pub score: Option<f32>,
      pub match_kind: CellMatchKind,
      pub polygon: Option<Polygon>,
  }
  ```

  **`source_line_indices` 是刻意的**：表格单元格的文本来自页级文本区域，那些区域**仍然留在
  `regions` 里**（不删除、不改写）。删掉它们会破坏阅读顺序与 polygon 语义；保留而不标注，
  消费者就会看到两份内容。给出下标让消费者自己决定（网页默认折叠，见 E16.3）。

- [x] **E14.2** `src/api.rs`：`OcrRequest` 增加 `pub table: TablePolicy`（与 `formula: FormulaPolicy` 并列），
  `validate()` 里加与公式同形的约束（`enabled` 时 `model_path` 必填；与 `roi` / `tile` 互斥的语义
  按实际坐标语义决定，若表格支持 crop 偏移则**允许** `roi`，但必须写测试证明偏移被正确加上）。
  `TablePolicy` 字段：`enabled` / `model_path` / `model_sha256` / `input_side_len`（默认 488）/
  `fix_input_shape`（默认 `true`）/ `intra_threads`（默认 `None`）/ `min_region_score` /
  `max_regions` / `ocr_resolution`（`TableOcrResolution`，默认 736 `max`）。

  > **实际落地与本节的两处偏离**（理由见 §2.5.2）：
  > 1. `model_sha256` 落成 **`expected_model_sha256`**（照 `FormulaPolicy` 的先例）。
  > 2. **未加 `ocr_resolution`**：页面路径的回填走 `BackfillSource::Reuse`（G2/G7 要求
  >    `recognizer_calls == 0`），"裁出来自己识别"那条路在页面路径上不可达，该字段会是死字段。
  > 3. `roi` / `tile` **同公式路由一样结构化拒绝**（`"table routing does not support `roi`"` /
  >    `"… tiled preprocessing"`）。坐标恰好仍能对上（页级文本 polygon 会被 `+ offset` 映射回原图），
  >    所以这不是会崩的错，而是会**静默返回调用方没问过的结果**的错 —— 定位器读的是整页文本框分布。

- [x] **E14.3** `src/ocr/pipeline/rapid_structure.rs`：新增 `recognize_with_table`，
  与 `recognize_with_formula`（`:1035`）**同形**：先跑 `recognize_text`（得到页级 det + rec 结果），
  再在**同一张图**上定位表格、逐个跑结构模型、用**已有文本行**回填、发射。
  - 分支点与公式并列：`if request.formula.enabled { … } else if request.table.enabled { … }`。
  - **两者同时启用时的语义必须先定案再写代码**：建议顺序为"公式抹白 → 文本 → 表格"，
    即表格在**抹白后**的图上定位（否则公式像素会被当成表格内容）。这个决定要写进文档，
    并配一条同时开启两者的测试。
  - 表格区域内的文本**不抹白**（与公式相反）：表格的单元格文本正是要保留的页级识别结果。

  > **实际落地的顺序与本节相反**：派发是 `table` **在 `formula` 之前**，且定位器与结构模型
  > 看的是**原图**（`decode_source_image`），不是抹白后的图。理由见 §2.5.2 —— 结构模型吃的是
  > 表格像素，喂抹白后的图会让单元格里出现白色补丁；而"表格消费的文本行"来自文本/公式阶段
  > 的**输出**，两者都是原图坐标系，因此坐标一致。公式与表格同时启用时，落在表格内的公式区域
  > 仍以 `RegionKind::Formula` 出现、**不计入** `source_line_indices`，对应单元格为空 ——
  > 这条已知限制登记在 §2.5.6。

- [x] **E14.4** `StageReports.table` 与 `OcrTimings` 三个字段真正填充：
  `preprocess_ms` = 定位耗时（`table_detect_ms`）、`infer_ms` = 结构 ONNX 推理
  （`table_structure_infer_ms`）、`postprocess_ms` = 回填 + 发射（`table_backfill_ms`）。
  未启用时保持 `StageState::Disabled` 与 `0.0`（**这是 G6 的一部分**）。

- [x] **E14.5** `TableStructureSession` 的**生命周期**：整页管线是长驻对象，结构会话必须
  在引擎构建期打开一次并复用（**不能**每张图开一次会话：会话创建含图优化，量级远大于一次推理）。
  模型缺失时按 `TablePolicy::enabled` 如实报错，不静默降级。

  > **实际落地是"按请求取一次、按文件身份缓存"而不是"引擎构建期开一次"**：`RapidOcrEngine`
  > 新增 `table_session: Option<(FileIdentity, TableStructureSession)>`，`fn table_session(&mut self, policy)`
  > 懒加载，`FileIdentity`（路径 + 体积 + mtime）变化时重建。理由是引擎构建时 `TablePolicy` 还
  > 不存在（它是**每请求**的策略），而文件身份缓存与公式识别器用的是同一套机制。G3 要求的
  > "一张表一次结构推理"由**结构**保证：会话在区域循环**之前**取一次（见 §2.5.2）。

- [x] **E14.6** `src/runtime/timing.rs` 的三个表格分量在页面路径上不再恒 0（E1 已把它们计入
  `attributed_ms` / `inference_ms` / `rust_ms`，这里只是让它们有值）。

- [x] **E14.7** **G5 首轮实测并确定 `table_structure_budget`**：同一 12 图集、同一 `max_side_len`，
  关闭 / 开启表格路由各跑一轮，记录 `p50` / `p90` / `table_*_ms` 分布，给出预算数值与依据。
  把数值写进 09 §0.5 与本文 §0.4。

  > **实测做了 4 个交替配对而不是各跑一轮**（单轮无法把"表格开销"与"轮间漂移"分开，见 §2.5.4）。
  > 判据也因此从 `p50` 改为可归因分量之和；`table_structure_budget = 120 ms`，实测中位 56.491 ms。

### 2.2 验收

- [x] `RegionKind::Table` 与 `TableOutcome` 的序列化有测试（`"table"` 拼写、`skip_serializing_if`
  的既有约定不被破坏）。→ `the_table_wire_names_are_snake_case` / `an_absent_table_outcome_is_omitted_from_json`
- [x] `TablePolicy::validate()` 的每种拒绝各有独立测试（`enabled` 无模型路径、与 `roi`/`tile`
  的互斥、非法 `input_side_len`）。→ `the_table_policy_rejections_are_distinguishable`（6 例）+
  `table_routing_rejects_roi_and_tile` + `a_disabled_table_policy_is_valid_even_without_a_model`
- [x] **G2/G7**：整页路径上 `BackfillOutcome::recognizer_calls == 0`。用 `CountingRecognizer`
  （`src/table/cell_content_backfill.rs:616`）同形的计数器证明，**不是**靠"我们记得不要调"。
- [x] **G3**：结构会话的 `run_with` 调用次数 == 定位出的表格数，且与单元格数无关。
- [x] **G6**：`TablePolicy::enabled == false` 时，12 图 bench 的 `evaluation-cpu.json`
  `mean_cer` 与 `bench-cpu-*.json` 的 `regions_avg` **逐位不变**。
- [x] **G5**：`table_structure_budget` 有数值、有依据、有门禁测试或脚本。
- [x] 坐标正确性：表格区域的 `polygon` 与单元格 `polygon` 都在**整页像素坐标系**里，
  与页级文本区域的 `polygon` 可直接比较（用一张已知表格图，断言表格 polygon 覆盖其单元格 polygon 的并集）。

> **G2/G7 与本节字面要求的偏离（见 §2.5.2）**：没有用 `CountingRecognizer` 同形的计数器。
> 偏离的理由是**它在页面路径上无法生效**：`BackfillSource::Crop { image, recognizer }` 是唯一会
> 调识别器的变体，而页面路径只用 `BackfillSource::Reuse { lines, offset }` —— 没有 `recognizer`
> 可注入，`recognizer_calls` 由构造恒为 0。取而代之的是**三层**证明：
> ① `table_decode_and_backfill` 里的 `debug_assert_eq!(outcome.recognizer_calls, 0)`（行为面，
> 每次 debug 测试都跑）；② `the_page_path_never_asks_the_backfill_to_recognize` 的静态守卫
> （生产段里 `BackfillSource::Crop` 出现 **0** 次、`BackfillSource::Reuse` 出现 **1** 次）；
> ③ `BackfillOutcome::recognizer_calls` 在 `BackfillSource::Reuse` 下恒 0，而有单测覆盖该分支。

### 2.3 命令

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --features serve -- -D warnings
cargo test --all-targets
# 表格链的定向测试（含需要真实模型的集成测试；不设环境变量时它们静默跳过）
$env:RAPID_STRUCTURE_MODEL_ROOT = '<crate>\models\text'
$env:RAPID_STRUCTURE_TABLE_STRUCTURE_MODEL = '<crate>\models\table\slanet-plus.onnx'
cargo test --lib table_pipeline_tests -- --nocapture
# G6 内容判据：关闭表格路由的整页回归。
# ⚠️ 该脚本的 $OutDir 硬编码 tests\baseline\windows-baseline 且**没有输出目录参数**
#    → 先备份，跑完还原（E5/E12 的既有做法）。脚本自己不传 --table-model，所以它天然是"关闭"轮。
pwsh -NoProfile -File tools/run_windows_baseline.ps1
# G5：交替配对测整页预算（默认写 Temp\table-page-budget，不覆盖入库基线）
pwsh -NoProfile -File tools/run_table_page_budget.ps1 -Pairs 3
python tools/check_table_page_budget.py --dir Temp\table-page-budget
```

`tools/run_windows_provider_matrix.ps1` / `tools/check_provider_claims.ps1` 属于 E12 的 provider
口径，**E14 未触碰 provider 或线程路径**（`bench_warm_e2e` 只多了一个 `--table-model` 开关），
因此本轮不重跑；表格会话在非 CPU provider 上的行为由 E12 的 DirectML 实验单独回答（结论：不能跑）。

### 2.4 风险与阻塞（本轮实测后的状态）

- ✅ **`table_structure_budget` 无先例** —— 已定值并写明依据（§2.5.4）。原表述担心的"约束 p50
  还是 p90"在实测后被**取消**：p50 判据本身不成立（表格开销双峰，p50 落在峰界上，实测 Δp50
  +10.80 % 与本仓库的 ±10–15 % 噪声带同宽），改用同请求内测出的可归因分量之和。
- ⚠️ **公式 + 表格同时启用**的语义仍没有参考实现，本轮只**定案并登记**，未做端到端实测
  （没有"表格内含公式"的测试图资产）。见 §2.5.6 第 3 条。
- ⚠️ **整页图上定位表格**的"假合并"缺陷在整页路径上**实测复现且更常见**：`04表格与键值对.png`
  上 2 个区域里有 1 个是假合并（区域 #1 横跨两块并排结构）。E14 的端到端测试把这一现状**钉成
  断言**（修好会大声失败），但缺陷本身按 E11 的判定**不动**。
- ✅ 表格未接进页面管线是 `src/table/detection.rs` 顶部 `#![allow(dead_code)]` 存在的原因
  （09 §14.3.8）。E14 接线后该豁免**已删除**，并连带删掉两个真死代码（见 §2.5.1），clippy 仍零 warning。

### 2.5 完成记录（E14，已收口）

#### 2.5.1 变更摘要

| 层 | 文件 | 变更 |
| --- | --- | --- |
| 线契约 | `src/api.rs` | `RegionKind::Table`；`TableOutcome` / `TableCellOutcome` / `TableStructureConfidence` / `TableDegradation` / `TableCellMatch` / `TableTextPolicy` / `TablePolicy`；`OcrRegion.table` + `OcrRegion::table(…)`；`OcrRequest.table` + `validate()` 的表格分支（含 `roi`/`tile` 拒绝）；`OcrOutput::validate` 的 `Table` 臂 |
| 导出面 | `src/exports.rs` | 上列 7 个类型加入 `pub use` |
| 管线 | `src/ocr/pipeline/rapid_structure.rs` | `recognize` 的派发顺序（表格优先）；`recognize_with_table`；`table_session`（按文件身份缓存）；`decode_formula_source` → `decode_source_image`；新增 `rec_image_from_dynamic` / `table_source_lines` / `quad_to_flat8` / `TableDecodeInput` / `TableDecoded` / `table_decode_and_backfill`；`RapidOcrEngine.table_session` 字段 |
| 结构会话 | `src/table/structure.rs` | `TableStructureSession.vocabulary` + `vocabulary()`（词表从**同一次探针会话**的 `character` metadata 读出并与输出末维比对） |
| 接线清理 | `src/table/mod.rs`、`src/table/detection.rs` | 删除 `detection.rs` 顶部的模块级 `#![allow(dead_code)]`；删除两个真死代码 `TextDistributionDetector::config()` 与 `crop_axis_aligned()`；`mod.rs` 的模块清单补充"生产路径有两条" |
| 测量入口 | `src/bin/bench_warm_e2e.rs` | `--table-model` / `--table-intra-threads`；报告 `meta.table`（`enabled` / `model_path` / `input_side_len` / `intra_threads`） |
| 门禁工具 | `tools/run_table_page_budget.ps1`、`tools/check_table_page_budget.py` | G5 的采集脚本与判定器（带输出目录参数，不覆盖入库基线） |

**刻意没有做的事**：
- 没加 `TablePolicy.ocr_resolution`（理由见 2.5.2）；
- 没把 `TablePolicy` 接进 YAML 配置与 CLI（属于 E16，本轮只在 `bench_warm_e2e` 里加了测量开关）；
- 没在 `TableStructureSession::open` 时拒绝 DirectML（E12 已实测它开得出会话、推理必失败；
  本轮不做预检推理，理由见 09 §13.5.2 的"留给 E13/E14 的决定"，最终仍留到 E17 登记）。

#### 2.5.2 设计判断（每条都写清"为什么不是另一种做法"）

1. **表格分支在公式分支之前，且定位器吃原图。** 文档 §2.1 E14.3 建议"公式抹白 → 文本 → 表格"。
   反过来做的理由是：结构模型吃的是**表格像素**，喂抹白后的图会让单元格里出现白色补丁（公式
   像素被抹成白），而 SLANet 的结构预测不依赖文字内容、只依赖线条与版式 —— 抹白不会帮它，只会
   污染输入。而"表格消费的文本行"来自文本/公式阶段的**输出区域**（两者都在原图坐标系），
   所以坐标一致。把表格放在外层还有一个实际好处：两个阶段的耗时可以**如实累加**（表格在
   文本之上，而不是被文本短路）。
2. **定位器与结构模型都在后一步、而不是在 `recognize_text` 里。** 因为定位需要**页级文本框
   分布**（行高一致性、x 坐标成列），那正是 `recognize_text` 的产出。
3. **`api` 里另立 serde 类型，而不是复用 `table::emitter::StructureConfidence` /
   `StructureDegradation` / `cell_content_backfill::CellMatchKind`。** 两条理由：① `mod table;`
   在 `src/lib.rs` 是私有的，把私有模块的类型放进公开结构体字段会触发 `private_interfaces`；
   ② `api` 是**线契约**（serde、要对客户端稳定），`table` 是**实现**（可以随便重构），两者
   不该是同一个类型。代价是三处 `From`/`match` 映射，写成了穷尽 `match`（新增变体会编译失败）。
4. **不加 `TablePolicy.ocr_resolution`。** `BackfillSource::Crop { image, recognizer }` 是唯一会
   调识别器的回填路径，而页面路径只用 `BackfillSource::Reuse`（G2/G7 要求 `recognizer_calls == 0`）。
   加一个在页面路径上不可达的分辨率字段是死字段 —— 奥卡姆。
5. **`roi`/`tile` 结构化拒绝，而不是让它"看起来也能跑"。** 页级文本 polygon 会被 `+ offset`
   映射回原图（`src/ocr/pipeline/rapid_structure.rs:779-781`），所以坐标**恰好**仍能对上；但定位器读的是
   整页文本框分布，`roi` 只裁了输入 → 会**静默返回调用方没问过的结果**。这与公式路由的既有
   约束同形（`src/api.rs:319-332`）。
6. **结构会话按请求取一次，而不是按区域取一次。** 这不是性能微调，而是让 G3（"结构推理调用
   次数 == 表格数"）由**结构**保证而非靠调用点自觉：会话在区域循环**之前**取一次，循环体内只
   剩 `run_with`。同时 `table_session` 每调用一次都要按文件身份 stat，按区域取等于把一次
   文件系统探测乘上表格数。这条是**先被静态守卫测试发现**（计数为 2）才改的。
7. **`roi`/`tile` 拒绝放在 `OcrRequest::validate`，不放在 `TablePolicy::validate`。** 因为互斥是
   请求级的（`roi` 与 `tile` 是同级字段），`TablePolicy` 看不到它们。

#### 2.5.3 关键结果

**整页端到端（release、12 图、`max_side_len=2000`、`intra_threads=16`、3 轮 + 1 热身、4 个交替配对）：**

| 配对 | 可归因分量之和 | detect | infer | backfill | Δp50 % | Δavg % | Δregions | Δpeak |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| pair1 | 56.802 ms | 0.921 | 55.821 | 0.060 | +11.43 % | +6.98 % | +1.9167 | +162.3 MB |
| pair2 | 54.615 ms | 0.963 | 53.592 | 0.059 | +8.22 % | +5.34 % | +1.9167 | +161.6 MB |
| pair3 | 57.129 ms | 0.896 | 56.172 | 0.061 | +11.15 % | +6.99 % | +1.9167 | +159.7 MB |
| pair4 | 56.180 ms | 0.956 | 55.163 | 0.061 | +10.45 % | +6.70 % | +1.9167 | +161.3 MB |

- 可归因分量：n=4、min 54.615、**median 56.491**、max 57.129、极差 **2.514 ms**；
- 端到端（**信息性，不是判据**）：Δp50 % 中位 **+10.80**、Δavg % 中位 **+6.84**；
- `off` 轮三个表格分量全部为 0（脚本每次断言），`on` 轮 `regions.avg` 在 4 个配对里都是
  **34.8333 → 36.7500**（+1.9167，确定性）；
- **峰值工作集稳定上升约 161 MB**（+12.0 %）—— 这是表格会话（SLANet-plus + ORT arena）与
  整页 `RecImage` 复制的代价，登记在 2.5.6。

**单图 debug 实测（`04表格与键值对.png`，3200×2000，仅作机制核对，不得作性能证据）：**

```
TABLE_REGION[0] rows=6 cols=6 cells=31 confidence=High holes=0 consumed_lines=26
TABLE_REGION[1] rows=7 cols=5 cells=28 confidence=High holes=0 consumed_lines=27
TABLE_TIMING detect_ms=1.557 infer_ms=61.838 backfill_ms=0.548 page_total_ms=1598.469 regions=63
```

两个区域与 E11 的预判一致（顶部 5 列统计表 + 那个"假合并"）；单表结构推理 ≈31 ms，与 E7/E5 的
25–31 ms 相符。

**G6 内容判据（`tools/run_windows_baseline.ps1` 的一轮，与备份基线逐字段比较）：**

| 判据 | 基线 | 本轮 | 结论 |
| --- | --- | --- | --- |
| `evaluation-cpu.json` `mean_cer` | 0.44765135645866394 | 0.44765135645866394 | **逐位相同** |
| 12 个 per-case `cer` | — | — | 12/12 **逐位相同** |
| `bench-cpu-2000.json` `regions.avg` | 34.833333333333336 | 34.833333333333336 | **逐位相同** |
| `bench-cpu-2000.json` `regions.{min,p50,max}` | 13 / 37 / 61 | 13 / 37 / 61 | **逐位相同** |
| `bench-cpu-1280.json` `regions.avg` | 33.416666666666664 | 33.416666666666664 | **逐位相同** |
| `bench-cpu-*.json` `meta.table.enabled` | （键不存在） | `false` | 路由确实关闭 |

#### 2.5.4 G5 判据的修正与 `table_structure_budget = 120 ms` 的推导

**实测推翻原判据**：原表述是"整页 `ocr_total_ms` p50 增量 ≤ `table_structure_budget`"。4 个配对的
Δp50 是 +8.22 / +10.45 / +11.15 / +11.43 %（中位 +10.80 %），而 `off` 轮自身的 p50 极差只有
3.27 % —— 说明 Δp50 不是噪声；但它**与本仓库用来表示"测不出变化"的 ±10–15 % 噪声带同宽**
（09 §0.5 的 G1 判据）。根因是表格开销**双峰**：不含表格的页面增量为 0，含表格的页面增量为整份
表格成本，而 `p50` 取的中位样本恰好落在两峰分界上。用一个与噪声同宽的尺子量一个比噪声还小的量，
判据不成立。

**改用可归因分量之和**（`table_detect_ms + table_structure_infer_ms + table_backfill_ms`，来自
`timing_ledger.totals`）：它在**同一次请求内部**测出，因此不受轮间漂移影响；而且正好覆盖两种
真实退化模式 —— ① 每个单元格各跑一次结构推理（分量随单元格数增长）；② 每张图开一次
`TableStructureSession`（分量含图优化成本）。Δp50 / Δavg 仍然打印，但**不参与判定**。

**预算数值 = 120 ms**：实测中位 56.491 ms、极差 2.514 ms → 预算给到约 **2.1 倍**。取值不是
"感觉差不多"：下限必须容下本机整页 p50 的轮间离散度（3–9 %），上限必须让上面两种退化模式
（会让这个数变成数百毫秒）失败。与 E12 的 R5 判定不同，这次**有**超出噪声的证据支持"保留这个
功能"，所以结论是"接入并设预算"，不是"回退"。

**判定器**：`tools/check_table_page_budget.py`，默认 `--budget-ms 120`。正向实测 `G5 PASSED`
（median/budget = 0.47）；负向（`--budget-ms 40`）实测 `G5 FAILED: 56.491 ms > 40.0 ms` 且退出码
**3**。判定器还会拒绝三类不合格输入：非 release 构建、缺 `meta.table`、`off` 轮的表格分量非 0。

#### 2.5.5 验收命令与实测结果

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 退出 0 |
| `cargo clippy --all-targets --features serve -- -D warnings` | 零 warning（删掉 `#![allow(dead_code)]` 后连带清掉两个真死代码） |
| `cargo test --all-targets` | **668 + 2 + 4 + 14 passed / 0 failed**（E13 基线 614 → 净增 68） |
| `cargo test --lib table_pipeline_tests -- --nocapture` | 4 passed（含 4.87 s 的真实模型端到端） |
| `pwsh -NoProfile -File tools/run_table_page_budget.ps1 -Pairs 3` | 6 轮全部 exit 0 |
| `python tools/check_table_page_budget.py --dir Temp\table-page-budget` | **G5 PASSED**（median 56.491 ms ≤ 120 ms） |
| `python tools/check_table_page_budget.py --budget-ms 40` | G5 FAILED，退出码 3（负向路径有效） |
| `pwsh -NoProfile -File tools/run_windows_baseline.ps1` + `Temp/e14-g6-verdict.py` | **G6 PASSED**（全部内容字段逐位相同） |
| `python -m py_compile tools/check_table_page_budget.py` | 通过 |

**G2 / G3 / G7 的判定**：`mod table_pipeline_tests` 的 4 条测试（`the_page_pipeline_produces_a_table_region_with_timings`、
`the_table_polygon_covers_its_cells_in_page_coordinates`、`the_page_path_never_asks_the_backfill_to_recognize`、
`a_disabled_table_route_ignores_the_model_path_entirely`）覆盖 E14.3/E14.4/E14.6/G2/G3/G6/G7 的
可观测面与静态面。**G4** 由 E11 完成，本轮未新增任何 CNN。

**入库基线的处置**：`tools/run_windows_baseline.ps1` 会写 6 个文件到硬编码的
`tests\baseline\windows-baseline`。本轮跑完后**已从备份还原**并逐个 SHA-256 核对
（`environment.json` / `build-stats.json` / `bench-cpu-2000.json` / `bench-cpu-1280.json` /
`evaluation-cpu.json` / `summary.json` 全部 OK），本轮产物存到 `Temp/e14-baseline-after`。
理由：一次被验证为中性的改动不应静默覆盖它自己被比对的那份参考，否则"G6 通过"就是对 E14 自己
写出的参考通过（循环论证）。E5/E12 是同一做法。

#### 2.5.6 未覆盖风险

1. **假合并（E11 的已知缺陷）在整页路径上复现且未修。** `04表格与键值对.png` 上 2 个区域里
   有 1 个横跨两块并排结构。端到端测试把它**钉成断言**（`bounds.left <= 200 && bounds.right >= 2900`），
   所以修好会大声失败、必须同步文档；但本轮按 E11 的判定不动它。整页文档里的并排栏位比 E11 的
   纯表格图更常见，因此这个缺陷的**实际影响比 E11 估计的更大**，需要 E17 用 E8 的测试集量化。
2. **公式 + 表格同时启用没有端到端实测。** 语义已定案（表格在外层、定位与原图、公式区域由
   `region.kind == Formula` 过滤掉、不计入 `source_line_indices`），但没有"表格内含公式"的测试图。
   风险是：落在表格内的公式区域会**不**参与回填，对应单元格为空，而公式区域本身仍在 `regions`
   里 —— 消费者会看到"空单元格 + 一条公式"，而不是"含公式的单元格"。这不是 bug 而是**未定义的
   产品行为**，必须在 E17 用一张真实图定案。
3. **表格路由未接进 YAML / CLI / serve**（E16 的范围）。本轮只有 `OcrRequest` 层的开关与
   `bench_warm_e2e` 的测量开关。因此"用户能不能用上"要到 E16 才成立。
4. **峰值工作集 +161 MB（+12 %）没有预算。** G5 只管时间。这个增量来自表格会话与整页
   `RecImage` 复制（`decode_source_image` + `rec_image_from_dynamic` 各复制一份 3200×2000×3 ≈ 19 MB，
   其余是 ORT arena）。**没有门禁约束它**，也没有尝试复用文本阶段已经解码好的图 —— 后者是
   一个明确的后续优化点（需要把解码结果向 `recognize_text` 之外暴露）。
5. **`table_structure_budget` 只在 12 图、`max_side_len=2000`、`intra_threads=16`、CPU 上测过。**
   其它分辨率、其它 provider、其它图集上这个 120 ms 没有依据。它是**第一版**阈值。
6. **`TableOutcome.markdown` 在 E14 就填了，但 Markdown 表格表达不了 `rowspan`/`colspan`。**
   E15.3 要处理；当前字段里的内容由 `emit_markdown` 产出，其 span 损失**尚未**在任何通道上
   做过端到端核对。
7. **`table_source_lines` 只消费 `kind == Text` 且有 `polygon` + `recognition` 的区域。** 因此
   `RegionSource::Input`（整图当一张文本）与无 polygon 的区域都不参与回填。这是刻意的（没有
   几何就无从匹配），但意味着"整图识别"模式下表格会全是空单元格，而这一点没有测试也没有文档
   警告。
8. **DML + 表格模型仍不可用**（E12 实测：会话能开、推理在 `Loop.0` 的 `Cast.73` 上失败）。
   E14 未做任何缓解，也没有在 `open` 时拒绝 —— 用户拿到的是一条运行期错误，错误信息点名
   `Loop.0`/`Cast.73`。

---

## 3 E15 输出三通道（JSON / HTML / Markdown）

### 3.1 任务

- [x] **E15.1** `src/output/json.rs`：`kind` 白名单加 `"table"`（`src/output/json.rs:12-13` 的文档
  注释也要改，它现在写的是"区域类型：`text` 或 `formula`"）；表格区域输出 `table` 对象
  （与 `formula` 对象并列），并给出顶层 `tables` 数组（照 `formulas` 数组的先例）。
  > **落地与原文的偏离**：① 新增 `pub struct OcrTableItem`（`rows`/`cols`/`confidence`/`degradations`/
  > `holes`/`cells`/`table_html`），**刻意不放 `markdown`** —— 它就是该条目的 `txt`
  > （表格区域的唯一文本表示是 Markdown），再放一份会让同一份 payload 里出现两个相同字符串。
  > ② 顶层 `tables[]` 每条含 `region`/`rows`/`cols`/`confidence`/`degradations`/`holes`/`cells`/
  > `html`/`table_html`/`markdown`/`model_id`，是一条**自包含**记录（`model_id` 沿用 E14 行为，
  > 是模型路径原文，未改成逻辑名）。③ `items[i].kind` 的取值由 `"text"`/`"formula"` 扩到
  > `"table"`，判定**表格分支排在公式与文本之前**。
- [x] **E15.2** `src/output/html.rs`：表格区域渲染成**真的 `<table>`**（不是 `<pre>` 转义块）。
  必须与 E16.3 共用同一套白名单净化逻辑 —— **净化只有一份实现**，放在 `src/output/html.rs`，
  网页侧通过 `/api/ocr` 拿到的 HTML 已经是净化后的，或者网页侧调用同一个纯函数。
  > **落地**：① `pub fn sanitize_table_html(raw: &str) -> String` + `const ALLOWED_TABLE_TAGS`
  > 在 `src/output/html.rs:350,386`；② `render_report` 增加 `tables_section`
  > （`<h2>表格 (n)</h2>` + 每表一个 `<figure>`）与 `const TABLES_STYLE`（`src/output/html.rs:517`，
  > **刻意不写进共享 `<style>`**，从而"没有表格的报告逐字节不变"是结构保证而不是巧合）；
  > ③ **净化的唯一调用点**是 `src/ocr/pipeline/rapid_structure.rs::finalize_table_html`，网页侧因此不需要
  > 第二份白名单；`sanitize_table_html` 经 `src/exports.rs` 导出，E16.3 要自己净化时可直接调。
- [x] **E15.3** `src/output/markdown.rs`：表格区域渲染成 Markdown 表格。
  `rowspan`/`colspan` 在 Markdown 里**无法表达**，因此：
  - 先把 span 单元格放进逻辑网格，再按最大列数输出（`emit_markdown` 已经这么做）；
  - 跨行/跨列的格子在 Markdown 里表现为空位，**必须在表格前后各加一行显式说明**
    （例如 `<!-- 表格含跨行/跨列，Markdown 无法表达，已按逻辑网格展开 -->`），
    不允许静默丢失；
  - 单元格文本里的 `|` 与换行按 `escape_markdown_cell` 的规则处理（`|` → `\|`，`\n` → `<br>`）。
  > **落地与原文的偏离**：说明行**不在** `src/output/markdown.rs`，而在
  > `src/table/emitter.rs:152` 的 `const MARKDOWN_SPAN_NOTE` + `emit_markdown`（`:171`）。
  > 理由：**跨度损失是结构层的事实**，`src/output/markdown.rs` 只负责"把已经渲染好的块放到
  > 阅读顺序里、并抑制被消费的行"。实测证据：`04表格与键值对.png` 的两张表
  > `degradations == []`、`holes == 0`、`confidence == high`（结构完全良构）**却仍然丢跨度** ——
  > 如果说明行挂在通道层，任何"降级才提示"的实现都会漏掉这两张表。说明行由 `has_span`
  > （`rowspan > 1 || colspan > 1`）门控：**无跨度时一个字都不加**，避免给所有正常表格加噪声。
  > `src/output/markdown.rs` 侧另加三个私有函数 `consumed_by_tables` / `table_markdown_blocks` /
  > `plain_text_without_consumed`（见 E15.4）。
- [x] **E15.4** `plain_text` 与 `fullTextForCopy` 的口径：表格区域**不重复计入**正文文本。
  被表格消费的行（`TableOutcome::source_line_indices`）在纯文本输出里只出现一次。
  具体口径（按阅读顺序输出行、还是输出表格的 Markdown）必须定案并写测试。
  > **定案**：**`plain_text` 按阅读顺序输出行，表格区域自身不贡献文本**；**`to_output_markdown`
  > 相反 —— 表格区域渲染成 Markdown 表格，被消费的行被抑制**。理由：`plain_text` 是"页面的文字
  > 内容"（CER 评测、网页复制全文、搜索），Markdown 的 `|`/`---` 是标记而非文字，注进去会污染
  > 文本指标与剪贴板。因为表格区域没有 `recognition`，而 `plain_text` 只取 `recognition`，
  > 被表格消费的行**恰好出现一次**（不是"被特殊处理掉的"，而是天然如此）；Markdown 通道则必须
  > 显式抑制，否则同一段文字在 Markdown 里会出现两遍（一次作为行、一次作为单元格）。

### 3.2 验收

- [x] 三个通道对同一份 `OcrOutput` 的输出**语义一致**：表格的单元格数与文本在 JSON / HTML /
  Markdown 里都能被独立断言（不是"三个都非空"这种弱判据）。
  > **落地**：`src/output/json.rs::tests::the_three_channels_agree_on_the_table_grid_and_its_cells`
  > 用**一份** `OcrOutput`（一个被消费的文本行 + 一个 2×2 表格）同时断言三条通道，且三条通道的
  > 判据都是**独立算出来的数**：① `document["tables"][0]["cells"] == 4` 与
  > `table_item["txt"] == document["tables"][0]["markdown"]`（同一字符串）；② HTML 含
  > `2 × 2 · 4 个单元格 · 置信度 high` 且每个单元格以 `<td>{cell}</td>` 出现；③ Markdown 含
  > `| c00 | c01 |` 与 `| c10 | c11 |` 且每个单元格恰好出现 1 次。**这条测试当场查出夹具不一致**：
  > `table_outcome()` 的 `html`/`table_html` 原来只有第一行而 `cells` 与 `markdown` 有 4 个 ——
  > 三通道一致性的价值就在于它会拆掉这种"每个通道自己看着都对"的夹具。
- [x] Markdown 的 span 说明行有测试。
  > 正负两条：`src/table/emitter.rs::tests::markdown_warns_explicitly_that_spans_are_lost`
  > （有跨度 → 前后各一行说明）与 `markdown_says_nothing_extra_when_nothing_is_lost`
  > （无跨度 → 一个字都不加）；通道层再补一条
  > `src/output/markdown.rs::tests::the_span_note_survives_the_markdown_channel`
  > （说明行经 `to_output_markdown` 之后仍在）。**端到端**：真实产物的两张表都带说明行，见 §3.5.3。
- [x] `plain_text` 不重复有测试（同一段文字在 `plain_text` 里只出现一次）。
  > `src/output/markdown.rs::tests::plain_text_keeps_the_consumed_lines_and_ignores_the_table`
  > （`plain_text(Reading)` 恰好 3 行、A/B/C 各 1 次、不含 `|` 与 `---`）与
  > `a_table_region_replaces_the_lines_it_consumed`（Markdown 里 `count("A") == 1`）。
  > 三通道测试里也断言了 JSON 的顶层 `text` 中 `"被消费的行"` 恰好 1 次。
- [x] 既有 text/formula 的输出**逐字节不变**（三通道各自的快照测试）。
  > ① 通道内单元测试：`src/output/markdown.rs::tests::a_page_without_tables_renders_exactly_as_before`
  > （逐字相等 `"L1\nL2"`）、`src/output/html.rs::tests::a_report_without_tables_gains_no_table_markup_at_all`、
  > `src/output/json.rs::tests::a_page_without_tables_keeps_the_existing_item_shape`；
  > ② HTML 的强等式仍在：`static_mode_emits_no_script_at_all` 断言
  > `full.replace(REPORT_SCRIPT, "") == stat`，`full_mode_still_renders_the_exact_cli_script_block`
  > 断言 `matches("<script").count() == 1` —— 表格区块只用 `<style>`，所以"表格通道不引入脚本"
  > 是被断言掉的；③ **端到端**：真实 HTML 产物 `<script` 计 0、`class="tables"` 计 1；
  > 同一张图不给 `--table-model` 时 `class="tables"`/`<table>`/`table-polygon` 全为 0；
  > ④ **L2 回归**：12 图 bench + `evaluation-cpu.json` 的内容判据逐位相同（§3.5.4）。

### 3.3 命令

```powershell
cargo test --all-targets
# 三通道一致性：对同一张含表格的图分别导出 json/html/markdown，人工比对
target\release\rapidstruct.exe run --img-path datasets\text\04表格与键值对.png `
  --config models\configs\test-config-small.yaml --table-model models\table\slanet-plus.onnx `
  --format json
# 把 --format 依次换成 html / markdown / plain
```

> **对 §3.3 原文的两处修正**（E15 实测后写入）：
> ① 子命令名是 **`run`** 不是 `recognize`；入参是 `--img-path`（`--image` 会被 clap 拒绝）。
> ② `--format` 与 `--table-model` **原本都不存在** —— 按 §3.3 的字面命令无法验证任何东西，
> 而 `Run`/`Report` 都没有表格开关（`make_request` 里硬写 `TablePolicy::default()`）。
> E15 因此补齐了 CLI：`OutputFormat { Plain, Json, Html, Markdown }`、`TableArgs { table_model,
> table_intra_threads }`（`--table-model` 给了才 `enabled: true`），`Run` 用 `--format`（`--json`
> 保留并加 `conflicts_with = "format"`），`Report` 也接 `TableArgs`。
> **`evaluate` 刻意不接表格开关**：`--manifest` 量的是纯文本 CER 与 polygon IoU，表格会多产出一个
> 没有 `recognition` 的区域，从而改变区域数与 IoU 口径，与 `evaluation-cpu.json` 不可比（门禁 G6）。

### 3.4 风险与阻塞

- ⚠️ Markdown 的表格能力边界（无 span）是**格式本身的限制**，不是实现缺陷。文档与输出都必须
  说清楚，否则用户会以为识别错了。
- ⚠️ 表格 HTML 里可能含 OCR 文本注入（PubTabNet 真值里 `<` / `>` 各出现 4,171 次且成对，
  见 `docs/09` §14.3.2）。评测口径用 `TextPolicy::Raw`（保可比性），**面向用户的输出必须用
  `TextPolicy::Escape`**。这一条在 E15 定案，E16 依赖它。

### 3.5 完成记录（E15，已收口）

#### 3.5.1 变更摘要

| 文件 | 变更 | 理由 |
| --- | --- | --- |
| `src/api.rs` | `TableStructureConfidence::as_str`；`OcrOutput::table_count()`；`OcrOutput::tables(order)`；1 条测试 | 通道层要往**非 serde** 的结构（`OcrJsonItem::kind` 是 `&'static str`）写置信档，必须与 serde 同一真值 |
| `src/output/json.rs` | `pub struct OcrTableItem`（7 字段）；`OcrJsonItem` 加 `table: Option<OcrTableItem>`；`to_output_items` 加**表格分支**；`to_output_json` 加顶层 `tables[]`；`to_json_items` 与公式/文本分支补 `table: None`；3 条测试 | E15.1 |
| `src/output/html.rs` | `const ALLOWED_TABLE_TAGS`、`pub fn sanitize_table_html`（+ 私有 `comment_length`/`sanitize_tag`/`sanitize_attrs`）、`const TABLES_STYLE`、`render_report` 的 `tables_section` 与表格分支；7 条测试 | E15.2；净化只有一份实现 |
| `src/output/markdown.rs` | 私有 `consumed_by_tables`/`table_markdown_blocks`/`plain_text_without_consumed`；`to_output_markdown` 的表格分支与被消费行抑制；`group_has_formula` → `group_has_block`；4 条测试 | E15.3/E15.4 的通道层 |
| `src/table/emitter.rs` | `const MARKDOWN_SPAN_NOTE`；`emit_markdown` 用 `has_span` 门控说明行；2 条测试（正负各一） | 跨度损失是**结构层**事实，提示必须挂在结构层（见 3.5.2） |
| `src/ocr/pipeline/rapid_structure.rs` | `fn finalize_table_html(html, table_html, TableTextPolicy) -> (String, String)` + 一个调用点；3 条测试（不依赖模型/图片） | 净化的唯一落点；`Raw` 与 `Escape` 的分叉在这里 |
| `src/exports.rs` | 导出 `OcrTableItem` 与 `sanitize_table_html` | `src/lib.rs` 走 `pub use exports::*`，二进制与 E16.3 都要够得着 |
| `src/bin/rapidstruct.rs` | `enum OutputFormat`；`struct TableArgs`；`Run --format` / `Run,Report --table-model,--table-intra-threads`；`make_request` 改 3 参 | §3.3 的命令原本**不可执行**（见 §3.3 的修正段） |

**刻意没有做的事**：① 没有给 `evaluate` 加表格开关（会改变 CER/IoU 口径，门禁 G6）；② 没有新增 Markdown 之外的退化表示（`<br>` 拼跨度等），因为那需要 fixtures 与产品确认（`docs/07` §14）；③ 没有删 `src/output/markdown.rs` 的 `#![allow(dead_code)]`（试过，见 3.5.2 第 5 条）；④ 没有动 `src/bin/serve/`（E16 的范围）。

#### 3.5.2 设计判断

1. **`plain_text` 口径选"按阅读顺序输出行、表格不贡献文本"，不选"把 Markdown 表格注进正文"。**
   因为 `plain_text` 的消费者是 CER 评测、网页复制全文与搜索 —— `|`/`---` 是标记不是文字，注进去
   会污染文本指标与剪贴板。备选为什么不行：把表格渲染成 Markdown 塞进 `plain_text` 会让
   `evaluation-cpu.json` 的 `mean_cer` 在开启表格后变化，直接违反门禁 G6。
2. **Markdown 通道反过来抑制被消费的行。** 表格区域已经把它们渲染成单元格，再输出一遍行就是
   重复。为什么不能"两边都保留"：同一段文字在 Markdown 里出现两次，读者无法判断哪一份是表格内容。
3. **跨度说明行放在 `src/table/emitter.rs` 而不是 `src/output/markdown.rs`。** 因为跨度损失是
   **结构层**的事实，而 `degradations`/`holes` 描述的是**另一种**丢失（网格有洞、token 被截断）——
   实测里两张表都是 `degradations == []` / `holes == 0` / `confidence == high` 却仍然丢跨度。
   备选（挂在通道层、由降级档驱动）为什么不行：它会给这两张表**静默**丢掉跨度信息。
4. **净化只有一份实现，且唯一调用点在 `finalize_table_html`。** 备选（网页侧自己再净化一遍）
   为什么不行：两份白名单必然漂移，而漂移的方向永远是"其中一份忘了补新标签"。
   `Escape` 与 `Raw` 的分叉也在这里：`Raw` **刻意不净化**，因为消费者是 lxml + 官方 `metric.py`，
   改字节会破坏与 PubTabNet 真值的可比性。实测上两条路径目前**产出相同字节**（`Escape` 已把单元格
   文本的 `<` 变成 `&lt;`），所以净化是**纵深防御**而非当下行为变更 —— 这条由
   `sanitizing_is_a_no_op_on_whitelisted_emitter_output` 钉住。
5. **保留 `src/output/markdown.rs` 的 `#![allow(dead_code)]`，但补上注释说明它覆盖什么。**
   删掉这行后 `-D warnings` 报出 `to_markdown is never used`、`to_markdown_texts is never used`、
   `field center_y is never read`（第三条是前两条的连带）。它们**不是 E15 引入的死代码**，而是
   `json.rs::to_json_items` 同类的**遗留入口**。删不删是"遗留入口统一清理"的独立决定，
   不属于输出三通道这一阶段 —— 于是保留并登记，而不是在本阶段顺手删一半导致两个文件不对称。
6. **`--json` 保留为 `--format json` 的别名，而不是直接删。** 因为它是既有 CLI 契约，
   而 §3.3 要求的是"能被验证"，不是"必须只有一种写法"。`conflicts_with = "format"` 只在用户
   **显式**给出 `--format` 时触发 —— 实测 `--json` 单独用不冲突（clap 不把 `default_value_t`
   视为显式提供）。
7. **HTML 的表格样式用独立 `const TABLES_STYLE`，不并进共享 `<style>`。** 于是"没有表格的报告
   逐字节不变"是**结构保证**，而不是"反正字符串恰好一样"。这条让 §3.2 第 ④ 条在 HTML 通道上
   有可执行判据（`static_mode_emits_no_script_at_all` 的 `full.replace(REPORT_SCRIPT, "") == stat`）。

#### 3.5.3 关键结果

**三通道端到端**（`[实测]`，release，`datasets\text\04表格与键值对.png` 3200×2000，
`models\configs\test-config-small.yaml`，`--table-model models\table\slanet-plus.onnx`）：

| 通道 | 字节数 | 关键观测 |
| --- | --- | --- |
| plain | 1,051 | `regions=63 formulas=0 tables=2 total_ms=823.0`；`table[61] rows=6 cols=6 cells=31 confidence=high`、`table[62] rows=7 cols=5 cells=28 confidence=high`；被消费的行（`MODEL:PP-OCRv6`/`LATENCY:42.7ms`/`SCORE:0.931`）**仍各出现 1 次** |
| markdown | 1,845 | 两张表都渲染成网格，**每张前后各有 `MARKDOWN_SPAN_NOTE`**；`degradations == []`、`holes == 0`、`confidence == high` 却仍有说明行 —— 这正是判断 3 的实测依据 |
| json | 134,221 | 顶层键 `engine/formulas/image/items/regions/schema_version/stages/tables/text/timings`；`n_items=63`、`n_tables=2`；`tables[0] = {cells:31, cols:6, confidence:"high", degradations:[], holes:0, model_id:"models\table\slanet-plus.onnx", region:61, rows:6}` |
| html | 37,014 | `<script` **0**；`class="tables"` 1；`<table>` **2**；`<td` **59**（31+28）；`&lt;td` **0**（真表格）；`on(click|error|load)=` **0**；`style=` 1（`table-note` 的内联样式）；`<h2>表格 (2)</h2>`；表头 `原图 3200 × 2000 px · 63 个区域 · 0 个公式 · 2 个表格 · total 841.4 ms` |

**§3.2 第 ④ 条的端到端形态**（`[实测]`）：同一张图**不给** `--table-model` 时表头是
`… · 61 个区域 · 0 个公式 · total 959.9 ms`（**没有 ` · 0 个表格`**），
`class="tables"`/`<table>`/`table-polygon` **全为 0**。`items` 61（关）vs 63（开）——
表格路由多产出 2 个区域，这正是 `evaluate` 不能开表格的原因。

**工具层教训（写进这里，因为它们是"门禁看起来通过了"的反面案例）**：
- **一个路径写错的判定脚本会给出空洞的通过。** 第一版 `Temp/e15-g6-verdict.py` 把 `mean_cer`
  读成 `result.mean_cer`（真实路径是**顶层** `mean_cer`），而 `dig()` 把"键不存在"映射成 `None`，
  于是 `None == None` 恒真 —— 输出 `[OK ] … None -> None` 并报 `PASSED`。改成 `MISSING` 哨兵后
  真值才被读出来（`0.44765135645866394`）。**门禁必须区分"值恰好是 None"与"路径不存在"**，
  否则它验证的是"脚本没崩"。
- **`Move-Item <dir> <dst>` 之后再 `Copy-Item -Recurse -Force <src>\* <dir>\` 会因为目标不存在而全部失败**
  （PowerShell 报「目录名称无效」，且 `$ErrorActionPreference` 没设就只打错误不中断）。
  本轮因此一度把入库基线目录**挪空**，靠 `Temp\baseline-before-e15` 才恢复。正确顺序是先
  `New-Item -ItemType Directory`，并**每次都逐个 SHA-256 核对**（本轮 16 个文件、0 处不一致）。

#### 3.5.4 基线对比

**L2 层：12 图 bench + `evaluation-cpu.json`（`[实测]`，2 次独立运行）**

| 判据 | E14 基线 | E15 第 1 轮 | E15 第 2 轮 | 结论 |
| --- | --- | --- | --- | --- |
| `evaluation-cpu.json` `mean_cer` | 0.44765135645866394 | 0.44765135645866394 | 0.44765135645866394 | **逐位相同** |
| `evaluation-cpu.json` `exact_match_rate` | 0.0 | 0.0 | 0.0 | **逐位相同** |
| 12 个 per-case `cer` | — | 12/12 相同 | 12/12 相同 | **逐位相同** |
| `bench-cpu-2000` `regions.{avg,min,p50,max}` | 34.833333333333336 / 13 / 37 / 61 | 同 | 同 | **逐位相同** |
| `bench-cpu-1280` `regions.avg` | 33.416666666666664 | 同 | 同 | **逐位相同** |
| `bench-cpu-2000` `ocr_total_ms.p50` | 982.04541015625 | 1018.0819091796875 (**+3.67 %**) | 1004.9588012695312 (**+2.33 %**) | 噪声带内（信息性） |
| `bench-cpu-1280` `ocr_total_ms.p50` | 644.9581298828125 | 664.3182983398438 (**+3.00 %**) | 641.2742919921875 (**−0.57 %**) | 噪声带内（信息性） |
| `evaluation-cpu.json` `peak_working_set_bytes` | 1,219,506,176 | 1,218,228,224 (−0.10 %) | 1,219,162,112 (−0.03 %) | 噪声带内 |

> **内容判据 9 项全部逐位相同、两轮一致 → L2 通过。** 计时判据按 §0.3 只作信息性记录：
> 每张图都仍在整轮墙钟的 ±3–4 % 抖动内（E14 的 G6 观测到 ±3 %，E10 的冷跑观测到 ±3.5 %）。
> 入库基线已从 `Temp\baseline-before-e15` **还原并逐个 SHA-256 核对（16 个文件、0 处不一致）**；
> 本轮产物保留在 `Temp\e15-baseline-after`。理由同 E5/E12/E14：一次被证明中性的改动不应静默
> 覆盖它自己被比对的那份参考，否则"通过"就是对自己写出的参考通过（循环论证）。

**L2 层：provider smoke（`[实测]`，`Temp\e12-provider-smoke.ps1`，1 热身 + 1 轮，`max_side_len=1280`）**

| 变体 | `selected_ep` | `fallback_to_cpu` | n | `ocr_total_ms.p50` | `regions.avg` |
| --- | --- | --- | --- | --- | --- |
| `cpu` | `Cpu` | false | 12 | 656.13 | 33.416666666666664 |
| `cpu` + strict | `Cpu` | false | 12 | 605.53 | 同上 |
| `direct_ml` | `DirectMl` | false | 12 | 399.90 | 同上 |
| `direct_ml` + strict | `DirectMl` | false | 12 | 445.80 | 同上 |
| `cuda` | `Cuda` | false | 12 | 618.63 | 同上 |
| `cuda` + strict | `Cuda` | false | 12 | 656.88 | 同上 |

六变体 `exit_code` 全 0、三者都解析到**请求的** EP、无 CPU 回落、`regions.avg` 与
`bench-cpu-1280.json` 的入库值逐位相同。跑完已 `cargo build --release --bins` **重建回默认特性**
（避免 `--features cuda-provider` 的产物残留，E5 同一做法）。**注意**：该脚本读的
`meta.provider_resolution.selected_ep` 路径是错的（真实路径带 per-model 层级：
`meta.provider_resolution.{detector,recognizer,classifier}.selected_ep`），所以它自己打出的报告里
这两列是 `null`；上表是我按 `meta.provider_resolution.detector` 重新提取的。

#### 3.5.5 验收命令与实测结果

```powershell
cargo fmt --all -- --check                                    # exit 0
cargo clippy --all-targets --features serve -- -D warnings                     # 零 error / 零 warning
cargo test --all-targets                                      # 688 + 2 + 4 + 14 passed / 0 failed
python -m py_compile tools/check_table_page_budget.py         # 通过（注意 .ps1 不能用 py_compile）

# 三通道端到端（见 3.5.3 的表）
target\release\rapidstruct.exe run --img-path datasets\text\04表格与键值对.png `
  --config models\configs\test-config-small.yaml --table-model models\table\slanet-plus.onnx --format json
# --format 依次换 html / markdown / plain、并去掉 --table-model 再跑一遍

# L2 回归
Copy-Item -Recurse tests\baseline\windows-baseline Temp\baseline-before-e15
pwsh -NoProfile -File tools\run_windows_baseline.ps1
python Temp/e15-g6-verdict.py Temp\baseline-before-e15 Temp\e15-baseline-after   # L2 VERDICT: PASSED
pwsh -NoProfile -File Temp\e12-provider-smoke.ps1                               # 6/6 exit 0
cargo build --release --bins                                                    # 还原默认特性
```

**测试计数**：`cargo test --all-targets` = **688 + 2 + 4 + 14 passed / 0 failed**，E14 基线是
`668 + 2 + 4 + 14` → **净增 20 条**：`src/output/html.rs` 7（`sanitize_keeps_the_whitelisted_table_structure`、
`sanitize_removes_every_active_construct`、`sanitizing_is_idempotent`、
`sanitize_keeps_the_page_shell_that_the_metric_requires`、
`report_renders_a_table_region_as_real_table_markup`、`a_report_without_tables_gains_no_table_markup_at_all`、
`a_table_item_without_its_view_is_not_disguised_as_a_table`）、`src/output/markdown.rs` 4
（`a_table_region_replaces_the_lines_it_consumed`、
`plain_text_keeps_the_consumed_lines_and_ignores_the_table`、
`the_span_note_survives_the_markdown_channel`、`a_page_without_tables_renders_exactly_as_before`）、
`src/output/json.rs` 3（`a_table_region_reports_its_grid_without_faking_ctc_text`、
`a_page_without_tables_keeps_the_existing_item_shape`（改写强化）、
`the_three_channels_agree_on_the_table_grid_and_its_cells`）、`src/ocr/pipeline/rapid_structure.rs` 3
（`the_user_facing_path_strips_active_markup`、`the_evaluation_path_is_byte_identical`、
`sanitizing_is_a_no_op_on_whitelisted_emitter_output`）、`src/table/emitter.rs` 2
（`markdown_warns_explicitly_that_spans_are_lost`、
`markdown_says_nothing_extra_when_nothing_is_lost`）、`src/api.rs` 1
（`the_confidence_name_agrees_with_serde`）。

**门禁判定**：**L0**（`cargo test --lib` 688 passed）✅；**L1**（`--all-targets`）✅；
**L2**（12 图 bench + `evaluation-cpu.json` + provider smoke）✅ —— 内容判据逐位相同、六 provider 变体
全部解析正确；**§3.4 的两条风险处置**：Markdown 无 span 已由说明行 + `docs/07` §14 双处声明；
`TextPolicy::Escape` 在 E15 定案为面向用户输出的唯一口径，`Raw` 只留评测路径。

#### 3.5.6 未覆盖风险

1. **`TableOutcome.markdown` 与 `TableOutcome.cells` 之间没有不变量。** `markdown` 在解码期生成，
   消费者若改了 `cells`，`markdown` 会与之分叉而不报错。E15 只钉住了 `html` ↔ `table_html`
   的恒等式，而且那个 `debug_assert_eq!` **只在 debug 构建生效**（release 静默容忍）。
2. **净化只覆盖 `html`/`table_html`，不覆盖 `markdown` 与单元格 `text`。** 面向用户的
   `markdown` 里，单元格文本的 HTML 片段（如 `<sub>`）是**原样保留**的（与 `Raw` 同理，见
   `emit_markdown` 的文档）。若下游把 `markdown` 直接喂给未净化的 HTML 渲染器，这是一个注入面。
3. **净化白名单没有做过 fuzz。** 现有证据是"发射器产物 + 合成攻击样本 + 幂等性"，
   不是"任意输入"。`sanitize_table_html` 承诺"裸 `>` 截短"等边界行为只有单测，没有被对抗性输入压过。
4. **`model_id` 是模型路径原文**（`models\table\slanet-plus.onnx`），会随用户机器不同而泄漏本地路径，
   也让同一模型的两次识别在 JSON 上不可比。这是 E14 的既有行为，E15 原样保留。
5. **三通道一致性只在一份 2×2 合成夹具上被断言。** 没有在**带洞 / 带降级档 / 带跨度**的真实
   `TableOutcome` 上做跨通道断言 —— 而这三类恰恰是各通道分歧最容易出现的地方。
6. **`plain_text` 会丢掉"不在页面行里的表格文本"。** 页面路径只用 `BackfillSource::Reuse`，
   因此被消费的行仍以行的形式留在 `plain_text` 里；但若将来启用 `BackfillSource::Crop`
   （逐 cell 识别），那些**只存在于表格里**的文本不会出现在 `plain_text` 中，而 `plain_text`
   正是 CER 与复制全文的来源。
7. **网页侧（E16）尚未消费新结构。** `src/bin/serve/` 一行未改，`/api/ocr` 返回的 JSON 里
   `items[].table` 与 `tables[]` 已经就位，但页面还不渲染表格 —— 这是 E16.3 的范围，
   不能被理解为"E15 已经让用户看到表格了"。
8. **峰值内存 +161 MB（E14 的 G5 观测）在 E15 未做任何缓解，也没有预算。** 三通道都在
   同一份 `OcrOutput` 上工作，没有引入新的常驻缓冲，但也没有实测过开启表格后的 HTML 报告
   内存峰值。
9. **`cuda` / `direct_ml` 下表格输出的三个通道未被端到端核对。** provider smoke 只跑
   `bench_warm_e2e`（不开表格路由），所以"表格 HTML 在 DirectML 下是否逐位相同"仍是空白
   （与 E12 登记的 DML 项同一来源）。

---

## 4 E16 本地服务与网页

> 本章对应三项用户需求（`m08398`）：
> ① 导入模型的 UI 优化；② 识别模式优化（目前只有公式一个打勾选项）；③ 表格识别需要在识别区域
> 增加 HTML 表格渲染。
>
> 前提：**改 `src/bin/web/index.html` 必须重建 serve 二进制** —— 它由
> `src/bin/serve/run.rs:42` 的 `include_str!("../web/index.html")` 编译期嵌入。

### 4.1 E16.1 导入模型 UI 优化（需求 ①）

**现状问题**（实测，`src/bin/web/index.html`）：

- `renderBanner()`（L3565）只渲染**不齐备**的集合：`m.sets.filter((s) => !s.complete)`，
  且 `if (!bad.length) { box.hidden = true; return; }` —— **全部齐备时整个横幅隐藏**，
  用户看不到任何模型清单，也无法确认"我装的到底是哪个版本、哪些文件"。
- 齐备集合没有任何逐文件信息；不齐备集合的 `.banner-files` 只有 `.bf-state` / `.bf-name` /
  `.bf-size`，**没有 role、没有 sha256 可见入口**（sha256 只在 `title` 里）。
- `renderEngine()`（L3343）的 `queueReadyHtml()`（L3325）只产出**两行**队列就绪状态
  （`#qReady` / `#qReadyFormula`），表格没有对应行。

**任务**：

- [x] **E16.1.1** 把横幅从"只报错"改为常驻的**模型清单面板**：**总是**渲染全部模型集，
  每集显示 `id` / `family` / `version` / `complete` / 文件数 / 总字节 / 来源 host。
  不齐备集合保持现有的高亮与下载按钮；齐备集合给出「重新校验」按钮（复用
  `reverifyModels()` L3518 → `POST /api/models/reverify`）。
- [x] **E16.1.2** 逐文件行增加 `role` 与 `sha256`（前 12 位 + 可展开全文）；
  `state` 三态（就绪 / 缺失 / 损坏）用不同的图标与颜色，**不只用文字**。
- [x] **E16.1.3** 显示本机解析出的**模型计划**：哪个集合的哪个文件被哪条管线使用
  （表格模型的 role 是 `table_structure`，E13 已加）。数据源与 `/api/status` / `/api/models`
  同源，**不允许网页自己拼路径**。
- [x] **E16.1.4** 下载中的逐文件进度（当前 `pollDownloadJob()` L3792 只给集合级）。
  若服务端不提供逐文件粒度，就在 UI 上如实显示"集合级进度"，**不要伪造**。
- [x] **E16.1.5** `queueReadyHtml()` 增加第三行表格队列就绪状态，与公式行同形。
- [x] **E16.1.6** 不引入任何新的下载来源：下载仍只走 `POST /api/models/download`，
  来源 host 仍由服务端的 `ALLOWED_DOWNLOAD_HOSTS` 约束（门禁 G8 的一部分）。

### 4.2 E16.2 识别模式优化（需求 ②）

**现状问题**（实测）：

- 页面上**只有一个打勾选项**：`#fxRow`（L1847）里的 `#fxToggle`，文案「公式识别路由 `formula`」。
- 提交路径只有一条表达：`submitImage()`（L4006）里 `const queue = state.formula ? "formula" : "text";`
  （L4016）→ `POST /api/ocr?queue=<text|formula>`（L4019）。注释明确写「**只有** `?queue=` 一个表达，
  没有第二个开关」。
- 因此模式的取值空间只有两态：`text`（纯文本）与 `formula`（文本 + 公式，因为
  `recognize_with_formula` 在 `src/ocr/pipeline/rapid_structure.rs:1076` 会先跑
  `recognize_text`，再追加公式区域）。**没有"文本 + 表格"这一态**。

**任务**：

- [x] **E16.2.1** 把单勾选改成**模式选择器**（`#modeRow`，与 `#fxRow` 同区）。至少四态：
  | 模式 | 表达 | 说明 |
  | --- | --- | --- |
  | 文本 | `queue=text` | 今天的行为 |
  | 文本 + 公式 | `queue=formula` | 今天的行为 |
  | 文本 + 表格 | `queue=text&table=1` | **E14 定案后可用** |
  | 文本 + 公式 + 表格 | `queue=formula&table=1` | **E14 定案后可用** |
  - 请求表达以 E14 的契约为准；**若 E14 尚未落地，表格相关的两个选项必须显式标注「未接入」
    并禁用**，绝不允许发送一个服务端会静默忽略的参数（那会让用户以为开了表格）。
- [x] **E16.2.2** 每个可选路由按**就绪度**独立禁用，复用 `modelsReadyFor(route)`（L2223）
  与 `formulaBlockedText(st)`（L2254）的模式，新增 `tableBlockedText(st)`；不可开启时
  在 `#modeCap` 里点名缺哪个文件并指向清单面板的下载按钮（照 `renderFormulaToggle()` L3930 的文案结构）。
- [x] **E16.2.3** 三处联动必须同时改，不能只改 UI：
  - `#ocrBtn` 的 click 门禁（L3970，现在只看 `modelsReadyFor("text")`）；
  - `state.formula` → `state.mode`（含 `state.table`），并在 `toast` / `zhQueue` 的文案里体现；
  - `xhrSend` 的 URL 构造（L4019）。
- [x] **E16.2.4** 契约变更必须登记：`docs/05-local-web-demo-implementation.md` 的 §2.2 / §4.2
  现在写的是「队列类别就是管线选择」，`?table=` 是**新的契约面**，必须在 doc 05 登记
  （新增参数、取值、默认值、与 `queue` 的关系），并同步 `src/bin/serve/tests.rs` 的契约断言。
- [x] **E16.2.5** 流动日志与诊断面板显示实际发送的模式（用户要能一眼看出"到底发了什么"）。

### 4.3 E16.3 表格 HTML 渲染（需求 ③）

**现状问题**：`renderRegions()`（L4580）只有两路 `kind` 分支
（`tag-fx`/`tag-txt` → `公式`/`文本`，见 `textCell`），`#fxRenderBar` 的显示条件
`const hasFormula = state.result.regions.some((r) => r.kind === "formula");` 只认公式。

**任务**：

- [x] **E16.3.1** `renderRegions()` 增加 `kind === "table"` 分支，渲染表格区域。
- [x] **E16.3.2** **净化是硬前置（门禁 G8）**：`TableOutcome.html` 是**模型产物**，
  直接 `innerHTML` 就是注入面。必须实现 `sanitizeTableHtml(html)`：
  - 用 `DOMParser` 解析成文档，再**白名单重建**（只允许
    `table` / `thead` / `tbody` / `tr` / `td` / `th` / `b` / `i` / `sub` / `sup` / `br`
    与 `colspan` / `rowspan` 两个属性）；
  - 其余节点**按文本处理**（保留文字、丢弃标签），属性一律丢弃；
  - **不得**用正则替换做净化（正则不能正确解析 HTML，这是已知的注入绕过面）。
  - 若 E15 已在服务端净化，网页仍应做一次 —— 前端不能假设服务端版本。
- [x] **E16.3.3** 表格区域的信息条：网格尺寸（`rows × cols`）、结构置信档
  （`high` / `low`）、降级原因（`grid_holes` / `truncated`）、单元格数、空洞数、
  匹配到的行数。`low` 必须**可见地**标注（照 E13 的"低置信必须显式标记"原则）。
- [x] **E16.3.4** 被表格消费的页级文本行（`source_line_indices`）：默认**折叠**，
  标注「已归入表格 N」，展开可见。这样用户既不会看到两份内容，也不会丢失原始行。
- [x] **E16.3.5** `#fxRenderBar` 的显示条件从"有公式"扩展为"有公式**或**表格"；
  表格不需要 KaTeX/MathJax，因此该工具条对表格只显示"原始 HTML"一态。
- [x] **E16.3.6** 选中高亮：表格区域沿用 `polygon` 叠加层（`drawOverlays()` L2978），
  单元格不做单独叠加（避免几百个多边形拖慢 canvas）。
- [x] **E16.3.7** 复制全文（`fullTextForCopy()` L4674）：表格区域按阅读顺序插入，
  内容用 `TableOutcome.markdown`（纯文本友好），且**不重复**插入被消费的文本行。
- [x] **E16.3.8** 演示模式（`DEMO`）需要一份含表格的 `demoResult()` 样例，
  否则演示模式下这条分支永远不会被执行（也就永远没有人工验证）。

### 4.4 E16.4 页面契约测试（改 UI 必须同时过）

- [x] `src/bin/serve/tests.rs` 中所有 `matches!(kind, "text" | "formula")` 形式的白名单断言
  必须同步加 `"table"`（漏改会让新功能在测试里"不存在"）。
- [x] 新增断言：页面里存在 `#modeRow` / `#tablePanel`（或实际采用的 id），
  且表格渲染路径调用了净化函数。
- [x] 新增断言：`?table=` 的取值校验（合法值通过、非法值 400），与 `?queue=` 同形。

### 4.5 验收

- [x] **G8**：注入型单元格文本（`<script>alert(1)</script>`、`<img onerror=…>`、
  `javascript:` 链接）经 `sanitizeTableHtml` 后**不产生可执行节点**，有单测。
- [x] L3：`cargo test --bin rapidstruct --features serve` 全绿；`GET /` 返回的页面里能找到新元素。
- [~] ~~手工页面自检：三态模式切换、模型清单常驻、含表格图片的识别结果里表格以真表格渲染、低置信表格有可见标注。~~
  —— **部分完成**：模式选择器（4 项）、模型清单常驻、队列就绪行、表格卡片渲染、低置信可见标注
  都取到了实测文本（§4.8.3(b)），但**不是用鼠标点出来的**，而是无头浏览器 + 真实页面脚本 + 伪造结果。
  真实提交一张含表格的图片这条链路**没有手工走过**（见 §4.8.6 第 1 条）。
- [~] ~~关闭表格路由时，页面行为与今天**逐字一致**（模式选择器回到"文本 / 文本 + 公式"两态）。~~
  —— **本条按原字面既不成立也不应成立**：模式选择器本身就是 E16.2 要求的改动，关闭表格路由时页面
  **不会**与今天逐字相同。实际验收口径 = **可用模式集合一致**：表格两项 `aria-disabled`（不隐藏，
  理由见 E16.2.2）、`#ocrBtn` 的第二层门禁拒绝不可用模式、`?table=1` 被服务端 400 拒绝
  （`the_table_switch_is_refused_when_table_routing_is_off`）。未做 DOM 逐字比对。

### 4.6 命令

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --features serve -- -D warnings
cargo test --bin rapidstruct --features serve
cargo build --release --bins
# 起服务后手工自检（浏览器打开 http://127.0.0.1:<port>/）
target\release\rapidstruct.exe serve --config models\configs\test-config-small.yaml
```

### 4.7 风险与阻塞

- ⚠️ **需求 ③ 依赖 E14**：没有 `RegionKind::Table` 就没有可渲染的数据。E16.3 的代码可以先写
  （对未知 `kind` 是安全的），但**验收必须等 E14**。
- ⚠️ 前端净化是**最后一道**防线，不是第一道。服务端必须先保证 `TextPolicy::Escape` 用在了
  面向用户的输出上（E15.4）。
- ⚠️ `src/bin/web/index.html` 是 253,135 B / 6195 行的单文件（E16 之前是 211,933 B / 5430 行），没有构建步骤。改动必须
  **手工可读**（不要引入打包器）；这也意味着新逻辑要写在该文件的既有分区里，不要另起体系。

### 4.8 完成记录（E16，已收口）

#### 4.8.1 变更摘要

| 层 | 文件 | 变更 |
| --- | --- | --- |
| 服务端载荷（E16.1 的数据源） | `src/bin/serve/server.rs` | `set_status_json` 增 `family` / `version`（清单未声明时写 `null`，不编占位字符串）；`pipelines_json` 的每条管线增 `plan[]`：`{name, role, pipeline, state, declared_sha256, has_declared_digest}`，**刻意不给绝对路径**（与 `/api/models` 其余部分同一脱敏口径） |
| 页面：模式选择器（E16.2） | `src/bin/web/index.html` | `#fxRow` / `#fxToggle` / `#fxCap` → `#modeRow`（`#modeOpts` 四态 + `#modeCap`）；`state.formula` 布尔 → `state.mode` 字符串；`QUEUE_ROLES.table = ["table_structure"]`；`modelsReadyFor` 支持表格路由；新增 `MODES` / `modeById` / `modeAvailability` / `renderModeRow` / `defaultRouteReason` / `routeBlockedText` / `tableBlockedText`；`submitImage` 构造 `?queue=…` + `&table=1`；`beginJob` 记 `state.job.mode` |
| 页面：表格渲染（E16.3） | 同上 | 新增 `sanitizeTableHtml`（`DOMParser` 白名单重建）、`parseTable`、`degradationText`、`tableRegionHtml`、`regionKindLabel`；`normalizeResult` 放行 `kind === "table"`；`renderRegions` 加表格卡片 + `consumedBy` 折叠被消费行；`fullTextForCopy` 用 `table.markdown` 且不重复被消费行；`#regionList` 补 click 委托；`updateFxBarUI` 支持"只有表格" |
| 页面：模型清单（E16.1） | 同上 | `renderBanner` 从"只报错、全齐时隐藏"改成**常驻清单面板**；新增 `fileStateCell` / `planRowsHtml` / `PIPE_ZH` / `FILE_STATE_ZH`；`queueReadyHtml` 加第三行 `#qReadyTable`；`renderVerify` 加表格管线一行；`[data-reverify]` 复用 `reverifyModels()` |
| 契约测试 | `src/bin/serve/tests.rs` | 新增 3 个用例（模式选择器与表格渲染器的页面断言、模型清单面板的页面断言、**G8 浏览器净化用例**）与 4 个辅助函数；`assert_result_satisfies_page_contract` 的 kind 白名单加 `table`，并断言表格区域的 `recognition` 必须为 `null` |
| 契约文档 | `docs/05-local-web-demo-implementation.md` | §4.2 端点表改写 + 新增 §4.2.2「表格阶段的请求协议（E16 定案）」 |
| 刻意**没有**做 | — | 未改 `Cargo.toml`、未引入打包器或新依赖、未新增下载来源（仍只走 `POST /api/models/download`，host 仍由 `ALLOWED_DOWNLOAD_HOSTS` 约束）、未给表格增加第二个表达（模式选择器只有一个真相：`?queue=` + `?table=`） |

#### 4.8.2 设计判断（每条都写清"为什么不是另一种做法"）

1. **模式选择器用 `role="radiogroup"` 而不是两个复选框**：四态是**互斥**的（`queue` 只能二选一），两个布尔量虽然也能表达四种组合，但同时表达了三种无意义的组合（例如"既开公式又声称队列是 text"），并且会让"当前到底是什么模式"在代码里没有单一表示。
2. **选项由 JS 渲染进 `#modeOpts`，不在 HTML 里写死**：每个模式的可用性随 `/api/models` 变化。写死会立刻产生两处真相（HTML 里的禁用态与 `modelsReadyFor` 的判定），而两处真相必然漂移。
3. **`state.formula`（布尔）→ `state.mode`（字符串）**：表格是**正交的第二维**，布尔装不下"文本 + 表格"。改名同时迫使全部 6 处 `renderFormulaToggle()` 调用点显式迁移，不会留下"某个路径还在读旧字段"的暗坑。
4. **净化用 `DOMParser` + 白名单重建，而不是正则或黑名单**：HTML 的容错解析规则无法用正则正确表达（`<svg/onload>`、属性大小写、实体编码、`<template>` 改变内容模型），而黑名单必然漏项。净化器**不拼接原 HTML 的任何一个字节** —— 输出全部由自己按节点重建，这是"不可绕过的白名单"与"容易漏的黑名单"之间的分界。docs/11 §4.3 也明确禁止正则方案。
5. **非白名单元素降级成文本，而不是整块丢弃**：服务端可能返回 `<td><span>42</span></td>`。整块丢弃会静默吃掉单元格内容（用户看到空表却以为是模型没识别出来）；丢弃标签、保留文字是"内容不变、表现力受限"，且与 `esc()` 的语义一致。
6. **`colspan` / `rowspan` 只接受 `Number.isInteger(n) && 1 < n <= 64`，越界直接丢弃而不 clamp**：服务端可能给出 `0` / `abc` / `1e9`。clamp 会把畸形的表**显示成正常表**，掩盖上游缺陷；丢弃则让畸形可见（浏览器对 `colspan="0"` 的处理本来也不是"0 列"，保留它等于让浏览器替我们做第二个决定）。`1` 视为"没写"，因为它在语义上等价。
7. **G8 用无头浏览器跑页面里的真实 `sanitizeTableHtml`，而不是在 Rust 侧写一个等价单测**：净化器消费的是 DOM API，`DOMParser` 的行为本身就是判据的一部分（实测证明：`rescue` 掉 `<td>` 这类细节只有浏览器给的答案算数 —— 裸 `<td>` 会被解析器丢掉，所以 11 个 payload 全部包进了真实 `<table><tr>`）。Node v24 没有 DOM（`typeof DOMParser === "undefined"`），Rust 侧只能**复制一份实现**，那测的就不是页面里那段代码。
8. **表格区域沿用 `polygon` 叠加层，单元格不做单独叠加**：一份真实表格有 30+ 个单元格，逐个画多边形会拖慢 canvas；而表格卡片本身已经把结构完整呈现（每格文字都在），叠加层对表格的增量信息只有"位置"。
9. **`#regionList` 的 click 委托里，点进 `.rt-wrap` 时直接 return**：用户在表格里选文字（复制某个单元格）不应把区域选中态切掉。这条同时修掉一个**既有缺陷**：此前 `#regionList` 只有 `dblclick` 处理器，`.region-item` 的 `data-i` 从未被读取 —— "点击列表项在图像上定位高亮"（doc 05 §9.2 与空态文案都承诺过）**实际不工作**。
10. **服务端载荷加 `family`/`version`/`plan[]` 而不是让网页自己拼**：网页不可能知道"哪个文件被哪条管线使用"（这由 `ModelPlan` 决定，且 `--formula-detector` / `--table-model` 可以指向集合之外的文件）。让网页拼路径就等于把模型解析逻辑复制一份到前端，且必然泄漏开发机绝对路径。

#### 4.8.3 关键结果

**(a) G8：浏览器里的真实净化器（1 passed，1.01 s）**

`cargo test --bin rapidstruct --features serve the_table_sanitizer -- --nocapture`。用例把页面主脚本原样取出、在脚本**开头**（紧跟 `"use strict";`）暴露 `sanitizeTableHtml`，再用无头浏览器执行 11 个 payload，结果以十六进制回传（避开 DOM 序列化对 `<`/`&` 的实体转义）。实测输出：

| # | 输入 | 输出（要点） |
| --- | --- | --- |
| 0 | `<td><script>alert(1)</script></td>` | `<td>alert(1)</td>` —— 标签消失、其文字降级为惰性文本 |
| 1 | `<td><img src=x onerror="alert(1)"></td>` | `<td></td>` —— 整块消失 |
| 2 | `<td><a href="javascript:alert(1)">click</a></td>` | `<td>click</td>` —— 标签与 `href` 一起丢弃 |
| 3 | `<td onclick="alert(1)">tap</td>` | `<td>tap</td>` |
| 4 | `<iframe src="javascript:alert(1)"></iframe>` | `<td></td>` |
| 5 | `<td><svg/onload=alert(1)></td>` | `<td></td>` |
| 6 | `<td style="background:url(javascript:alert(1))">styled</td>` | `<td>styled</td>` |
| 7 | `<td colspan="2" rowspan="3">spanned</td>` | **原样保留**（合法跨度不能被杀） |
| 8 | `<td colspan="0">…</td><td colspan="abc">…</td><td colspan="1e9">…</td>` | 三个 `colspan` **全部丢弃**，文字保留 |
| 9 | `<td><b>bold</b><sub>2</sub><sup>+</sup><br>next</td>` | 原样保留（白名单标签） |
| 10 | `<td>&lt;script&gt;alert(1)&lt;/script&gt;</td>` | 原样保留（已转义文本**不被二次解码**） |

每个 payload 的负向断言（大小写不敏感）覆盖 `<script` / `<img` / `<iframe` / `<svg` / `<a ` / `onerror` / `onclick` / `onload` / `javascript:` / `style=`，并断言输出不是 `__MISSING__`（证明净化函数真的可达，用例不是空洞的）。

**(b) 无头页面自检（DEMO 模式）**

- `#modeRow` 存在且未 `hidden`；`#modeOpts` 渲染出 **4 个真实选项**：`文本` / `文本 + 公式` / `文本 + 表格` / `文本 + 公式 + 表格`；`aria-checked="true"` 恰好 1 个。
- `#modeCap` 实测 `文本：请求 queue=text。公式与表格区域各自独立处理，正文文本 OCR 不受影响。`
- `#banner` **未 hidden**（常驻生效），实测 `模型清单 · 2 个集合` + 来源/下载状态摘要，每集合一个 `<details class="mset">`（`ms-state ok/bad` + `ms-id` + `ms-meta` 含 family/version/文件数/字节/来源 host），逐文件 `.bf-row` + 兄弟 `<details class="bf-sha-row"><summary>sha256 8f3ac21d90e……</summary>`，`.plan-wrap` 计划表存在。
- `#qReadyTable` 实测 `表格队列 · 不可用（不影响普通 OCR） 缺少 表格结构；表格路由默认关闭，普通文本 OCR 照常可用。`
- **表格卡片**：用真实 `normalizeResult` + `renderRegions()` 驱动一份含 `kind === "table"` 的结果（载荷形状取自 `scripted_table_outcome`），实测
  `items=4 | tables=1 | realTables=1 | tds=3 | spanKeep=1 | dropped=0 | folds=1 | badge=低置信（网格空洞或截断） | barHidden=false | regions=4`
  —— 其中 `realTables=1`（`.rt-wrap > table`）证明渲染成**真表格**；`spanKeep=1` 证明 `colspan="2"` 活了下来；`dropped=0` 证明注入的 `<script>` 在真实渲染路径里也没变成节点；`folds=1` 证明被消费的文本行被折叠；`items=4` 证明每个区域恰好产出一个 `.region-item`（`selectRegion` 依赖位置下标，这条是它的前提）。

**(c) 服务端契约**

`the_table_*` 共 6 条（4 条在 `serve::tests`、2 条在 `serve::server::tests`），覆盖：路由关闭时 `?table=1` 400 且 `disabled_reason` 点名 `--table-model`；取值只接受 `1`/`0`/缺省且非法值**不触达后端**（断言调用计数为 0）；四种模式提交后 `TablePolicy.enabled` 与 `expected_model_sha256` 正确；启动后模型被移走时 `?table=1` 409（`detail.scope == "table"`）而文本 OCR 照常；表格区域经 `/api/jobs/{id}/result` 往返，且线上载荷里 `table.html` **仍含** `<script>`（服务端不净化、净化是输出层与网页侧的职责）。

#### 4.8.4 基线对比

| 项 | 前 | 后 | 判定 |
| --- | --- | --- | --- |
| `cargo test --bin rapidstruct --features serve` | 284 passed | **287 passed / 0 failed** | 净增 3（E16 起点 284） |
| `cargo fmt --all -- --check` | 0 | 0 | — |
| `cargo clippy --all-targets --features serve -- -D warnings` | 0 | 0 | 修掉一条 `clippy::manual_is_multiple_of` |
| 表格路由关闭时的 `/api/models` | `table.routing == false` | 同 | 未回归；页面把两个表格模式标为 `aria-disabled`（**不隐藏**，理由见 §4.7 与 E16.2.2） |
| 表格路由关闭时的页面行为 | — | 模式选择器仍渲染四态，但表格两项禁用并给出理由 | §4.5 的"逐字一致"按"可用模式集合一致"验收，不是 DOM 逐字 |
| 12 图 `/api/ocr` 基线 | — | **本次未跑** | E16 只改了 serve 二进制与网页，引擎/管线/输出层一行未动；代价是"页面改动对既有基线零影响"只在结构上成立，没有实测 |

**§4.6 的命令修正**：原文写 `cargo test --lib bin::serve`，这条**恒为 0 个测试** —— `serve` 不是 default feature，`--lib` 只构建 lib target，`bin::serve` 的测试根本不会被编译。正确命令是 `cargo test --bin rapidstruct --features serve`。已在 §4.6 改正。

#### 4.8.5 验收命令与实测结果

```powershell
cargo test --bin rapidstruct --features serve                    # 287 passed / 0 failed
cargo test --bin rapidstruct --features serve the_table_sanitizer -- --nocapture   # 门禁 G8，1 passed / 1.01s
cargo fmt --all -- --check                                    # exit 0
cargo clippy --all-targets --features serve -- -D warnings    # exit 0
cargo build --release --bins                                  # exit 0
```

§4.5 四条的判定：

- **G8** ✅ 由浏览器用例覆盖（11 个 payload 逐条实测，见 §4.8.3(a)）。
- **L3** ✅ `cargo test --bin rapidstruct --features serve` = 287 passed；`GET /` 的页面里能找到新元素（由 `the_page_carries_the_mode_selector_and_the_table_renderer` 与 `the_page_carries_the_model_manifest_panel` 断言）。
- **手工页面自检** ⚠️ **部分完成**：模式选择器、模型清单、队列就绪行、表格卡片渲染、低置信可见标注都取到了实测文本（§4.8.3(b)），但**不是用鼠标点出来的** —— 是在无头浏览器里用真实页面脚本 + 伪造结果驱动的。真实提交一张含表格的图片这条链路见 §4.8.6 第 1 条。 —— **手工验收由用户负责**（用户已声明「本地网页测试由我负责」）：本条按**交接项**处理，不再计为本阶段的未覆盖风险；§4.8.6 第 1 条的技术事实（真实提交链路未在浏览器里点过）在交接完成前仍然成立。
- **关闭表格路由时行为一致** ⚠️ 结构性成立（模式两项 `aria-disabled` + `#ocrBtn` 门禁的第二层检查），未做 DOM 逐字比对。

**本机网页自测的交接命令**（按用户声明，手工验收由用户执行）：

```powershell
.\serve.ps1                    # 交互菜单：选档位 → 确认 → 启动（E19；非交互环境下退回快档 CLI，不等待按键）
.\serve.ps1 -Menu              # 强制进菜单
.\serve.ps1 -ModelsOnly        # 只盘点模型并给出下载地址
# 浏览器打开 http://127.0.0.1:8760
# （端口常量 `DEFAULT_PORT = 8760`；监听地址在 `src/bin/serve/security.rs` 里硬编码 127.0.0.1）
# 该脚本自己 = 建 serve 二进制 + 启动前盘点模型（缺则打印下载地址/-Download 自动下载并校验）+ 起服务。
# 等价的手写命令（脚本打印的就是它）：
#   cargo build --release --bins --features serve
#   .\target\release\rapidstruct.exe serve --config models\configs\test-config-small.yaml `
#     --table-model models\table\slanet-plus.onnx
```

> **E18 更新**：上文原先只给手写命令。现在推荐 `.\serve.ps1`（仓库根目录），它把"模型不自带"这件事
> 处理掉：`-DryRun` 只打印、`-ModelsOnly` 只检查/下载、`-Download` 预先同意自动下载、
> `-ServeConfig models\configs\serve-local-full.json` 切到全功能档（medium + 公式，18–27 s/页）。
> 机制与优先级见 `docs\05-local-web-demo-implementation.md` §3.1。

> **E19 更新**：`.\serve.ps1` 无参数调用时在可交互终端里进**方向键菜单**（启动 / 检查下载模型 / 诊断 /
> 高级选项；档位记住上次成功启动的那一个），给出任何原有操作参数则仍是 E18 的非交互语义。
> 菜单与 UI 状态、测试缝、以及本阶段修掉的四个启动器缺陷（单候选下载只请求 URL 首字符、
> 异常留下孤儿 `serve`、5.1 下 profile 读不出来、缺 UTF-8 BOM）见同文档 §3.2。

要看的四件事：① 模式选择器四个选项，`文本 + 表格` 与 `文本 + 公式 + 表格` 在模型齐备时可选；② 模型清单面板**常驻**，表格结构模型那一行显示为「命令行指定」（它来自 `--table-model`，不在模型集清单里）；③ 提交一张含表格的图（例如 `datasets\text\04表格与键值对.png`）后，表格以**真表格**渲染、低置信有**可见文字**徽标、被归入表格的页面行默认折叠；④ 去掉 `--table-model` 重启后，表格两项变灰并在 `#modeCap` 里点名缺 `表格结构`，而普通文本 OCR 照常可用。

跑完把二进制重建回默认特性（`cargo build --release --bins`），避免 `--features serve` 的产物留在 `target\release`（E5 起沿用同一做法）。

#### 4.8.6 未覆盖风险

1. **真实提交链路没在浏览器里点过**：`enterDemo()` 只设模型状态、不设 `state.image`，所以"选一张含表格的图 → 提交 `/api/ocr?table=1` → 结果渲染"这条端到端链路**没有手工走过**。表格卡片的渲染是用 `normalizeResult(伪造载荷)` 驱动的，载荷形状取自 `scripted_table_outcome`。→ 剩余风险集中在"服务端真实载荷与伪造载荷的差异"，目前只有 `assert_result_satisfies_page_contract` 的字段级断言覆盖。
2. **服务端净化与前端净化的组合未被端到端验证**：服务端（E15.4）对 `TextPolicy::Escape` 生效，但 `table.html` 字段本身**是模型原始产物、未在线上净化**（已由契约测试断言"仍含 `<script>`"）。网页侧净化已单独验证，两者串起来只看过一次。
3. **净化器白名单未 fuzz；`<template>` / `<math>` / `<foreignObject>` 一类会改变内容模型的元素没有专门用例**（它们都会走"降级成文本"分支，但没有断言）。
4. **`table.cells[].text` 与 `table.markdown` 没有过净化器**：前者进 `textCell` 用的是 `esc()`，后者进剪贴板；两条路径都没有注入用例。
5. **`planRowsHtml` 的"来源集合"按 `name + role` 找第一个匹配**：同一文件名出现在两个集合里时会指错集合（当前清单没有重名，但契约上允许）。
6. **`fileStateCell` / `ms-state` 依赖 `index.html` 里已存在的 symbol id**（`check` / `alert` / `cloud-off`）：新图标名拼错会静默显示空白，没有测试。
7. **`--table-model` 的启动期存在性检查（`ServeStartError::TableModel`）没有单测**（`--formula-detector` 的同类检查也没有，两者都是 `run()` 内联的 4 行）。
8. **提交前不重新拉取 `/api/models`**：若用户选好模式后模型被移走，页面要等下一次 `renderModeRow()` 才回落；服务端此时会以 409 拒绝，所以不会静默出错，但用户看到的仍是"提交中"。
9. **逐文件下载进度未实现**：服务端只提供集合级（`files_done` / `current_file`），页面如实显示"集合级进度"，**没有伪造**（满足 E16.1.4 的"不伪造"要求，但"逐文件"这条本身未做）。
10. **`?table=` 的非法值用例只在服务端层面**：页面侧的 URL 构造不会产生非法值（`MODES` 是常量表），所以"页面发出非法值"这种情况没有测试（也没有必要）。

---

### 4.9 后续修正（E22：表格模式被误置灰，用户实测 m12799）

**现象**（用户原话）：「文本+表格置灰，鼠标放在按钮上面，显示禁止点击的图标，点击不了」——服务端日志与 `/api/models` 都表明表格路由是**可用**的。

**根因（第一性事实）**：页面有**两个数据源**在回答"表格模型在不在"，而它们对"模型可以不在任何集合里"这件事的判断不一致。

| 层 | 数据源 | 结论 |
| --- | --- | --- |
| 就绪判定（`modelsReadyFor`） | `QUEUE_ROLES.table = ["table_structure"]` → 去 `sets[].files[]` 里按 role 找文件 | **找不到 → 永久置灰**（还给出错误理由"缺少 表格结构"） |
| 服务端 | `/api/models.table` = `routing: true, complete: true, in_plan: true, structure.state: "present"` | 可用 |
| 展示层（`planRowsHtml`，E16.1.3 就写对了） | `pipelines.table.plan[]`，找不到归属集合就显示"命令行指定" | 可用（命令行指定） |

`--table-model` 指定的结构模型**按设计可以完全不在模型集合里**：`models/serve/manifest.json` 声明的五个 role 是 detector / recognizer / dictionary / formula_recognizer / formula_detector，**没有** `table_structure`（结构模型有它自己的清单 `models/table/manifest.json`）。活服务实测 `sets` 只有 1 个集合、5 个文件、无 `table_structure` → 角色扫描必然失败。**同一个"集合之外的文件"在展示层是"命令行指定"、在判定层却是"模型缺失"** —— 这是缺陷的全部。

**修法**（`src/bin/web/index.html`，只动前端）：

1. `modelsReadyFor()` 为 `queue === "table"` 增加**提前返回**的分支，权威改用服务端的 `table` 块：`routing === false` → 显示服务端 `disabled_reason`；否则收集 `in_plan === false` / `missing` / `corrupt` / `blocked`，`!complete || 有坏项` → 不可用（并点名**文件名**，不再报"缺少 表格结构"）；否则 `ready: true`。该分支**必须排在共用的角色扫描之前**（顺序本身是不变量，有断言钉住）。
2. `QUEUE_ROLES` **删掉 `table` 条目**并写明理由：留着它就等于把"表格模型必须在某个集合里"重新写回去。
3. `normalizeModels()` 的 `route()` 补 `corrupt` / `blocked` 两个清单（与线上 JSON 同形，文案要区分"缺失/损坏/被启动参数挡住"）。
4. 顺带修一处**恒不生效的字段名**：`modelsReadyFor` 原来读 `route.disabled_reason`，而 `normalizeModels` 早已把它改名成 `reason` → 服务的 `disabled_reason` 永远读不到，只剩兜底文案。现读 `route.reason`（实测：服务端说 `"table routing is off: pass --table-model"` 时页面如实显示该句）。
5. 演示数据（`demoInitialModels()`）补 `table` 路由块与 `pipelines.*.plan`（含 `planRow` 助手）：离线预览原本既没有表格路由块也没有计划表，按新判定会把两个含表格的模式置灰，而且 E16.1.3 的"模型计划"面板在演示模式下一直是空的。演示里的结构模型**刻意没有归属集合**，正是为了复现这条路径。

**验证**（两处，都能失败才算数）：

- 新增 `tools/check_web_model_gating.js`：把页面里的 `modelsReadyFor` / `modelFiles` / `normalizeModels` / `demoInitialModels` **原样抽出**（大括号配平，不复制逻辑），喂真实 `/api/models` JSON 跑 13 条断言；同时把**修前的同名函数**作为反向对照跑同一份数据 —— 对照必须报"置灰 + 缺少 表格结构"，否则这个检查是空洞的。实测：对活服务数据 `FAILURES = 0`；`node --check` 页面主脚本块 0 错误。
  ```
  node tools\check_web_model_gating.js --models Temp\e22-models-8761.json --page Temp\e22-served-page.html
  # live: sets=1 file_roles=[detector,recognizer,dictionary,formula_recognizer,formula_detector] table.routing=true complete=true
  # [OK] A1 表格就绪（新判定）        [OK] A2 表格置灰（旧判定，证明本检查能失败）
  # [OK] B1 routing=false → 显示服务端理由   [OK] C1 缺文件 → 点名文件
  # FAILURES = 0
  ```
- Rust 侧新增绊线测试 `serve::tests::the_page_gates_the_table_mode_on_the_server_route_block`：断言表格分支存在、**排在角色扫描之前**、`QUEUE_ROLES` 里不再有 `table: ["table_structure"]`、演示数据里有 `planRow(` 与 `required_roles: ["table_structure"]`。它钉的是"这段代码还在、顺序还对"，**不是**行为 —— 行为由上面那个 Node 检查负责（页面的 JS 在 Rust 测试里跑不起来）。

**门禁**：`cargo fmt --all -- --check` 0；`cargo clippy --all-targets --features serve -- -D warnings` 0；`cargo test --bin rapidstruct --features serve` → **288 passed / 0 failed**；`cargo build --release --bin rapidstruct --features serve` 成功（新二进制 42,443,264 B，页面由 `src/bin/serve/run.rs:42` 的 `include_str!` 编译期嵌入，**改页面必须重新编译 + 重启服务**）。

**一处编译期障碍（如实登记）**：用户当时正在跑的 `rapidstruct serve`（PID 36820）**锁住了 `target\release\rapidstruct.exe`**，`cargo build` 报 `拒绝访问。 (os error 5)` / 构建 101。处理办法：**把运行中的 exe 改名**（`Move-Item ... rapidstruct.pre-e22.exe`，运行中的映像仍持有自己的句柄、改名不影响它），路径即被释放，构建成功后删掉那份陈旧副本。这是"服务占着二进制"的通用解法，比先杀进程温和。

**未覆盖**：① 修复没有在**真实浏览器**里点过（只验证了"服务端返回的页面含新代码" + "同一份数据下判定函数给出 ready"）；② 前端净化、结果渲染那条链路与本修正无关，沿用 E16 的验证结论；③ 用户当前那个服务进程在验证期间退出（8760 已释放），所以"重启后按钮可点"这一步由用户的下一次启动落地。

---

### 4.10 后续修正（E23：折行单元格把一张表切碎，用户实测 m13018）

**现象**（用户原话）：「为什么表格的识别效果这么差？上传的图片，表格识别了不到一半的内容」—— 一张 12 行 × 2 列的中文生产记录表（`Temp/e23-real-table.png`，982×694）。

**复现（`/run` 的真实产物，修复前 `Temp/e23-real-small.json`）**

| | 表格区域 | 行 × 列 | 单元格 | 覆盖的视觉行 |
| --- | --- | --- | --- | --- |
| 修前 | 2 个 | 3×2 + 3×2 | 6 + 6 | 6 / 12（标题、表头、8:00、12:00、13:00、14:00 六行退回普通文本） |
| 修后 | **1 个** | **12 × 2** | **24**（holes 0、confidence high、source_line_indices 27 条） | **12 / 12** |

两个 3×2 片段**本身是完全正确的**（满格、holes=0、confidence=high、每格都有文本）→ 结构模型、解码、回填都没问题，缺陷 100 % 在**定位器**：它把一张表切成了两片，另外六个视觉行根本没进表格区域。

**根因（第一性事实）**：定位器把**一个 OCR 文本框当成一个表格行**，但一个文本框是**一行视觉文本**，不是一个单元格。折行单元格的第二行文本落在自己的 y 上 → `group_rows`（按 `center_y` 聚类，容差 = 0.5 × 行高中位数 = **12.50 px**）把它聚成独立的"行"：27 条页面文本框聚成 **19 个行**（真值 12 行），其中锚行（框数 ≥ `min_columns` = 2）只有 **8** 个（rows 1、5、6、7、9、16、17、18），细行 **11** 个（rows 0、2、3、4、8、10–15）。随后"行区间"延展要求相邻两行 `shared_column_count ≥ 2`，而一条细行最多只能匹配 1 个 → **每一处折行都打断一次区间**。这就是"表格被切碎"的全部机制；用 `group_rows` + `detect_regions` 的 Python 端口逐值复现，端口给出的两个区间（rows 5..7 → y155-302、rows 16..18 → y518-670）加上 2 % 边距后与产物的 y136-321 / y500-688 **完全一致**。

**修法**（`src/table/detection.rs`，奥卡姆：**没有新增任何参数、没有新增函数**）：

1. 细行**不再是打断者，而是填充**：区间延展遇到非锚行就 `end += 1; continue;`（它仍计入 `bounds` 与 `members`），列匹配只在**锚行**之间做。
2. 检出判据从"区间长度 ≥ `min_rows`"改为"**锚行数** ≥ `min_rows`"。
3. `row_height_consistency` 只喂锚行：折行细行更矮，算进来会把"更像表格"罚成"更不像表格"（方向错了）。`DetectedRegion.rows` 因此是真实行数的**下界**（锚行数），权威网格由结构模型给出。
4. 表题（表格框线**之上**的跨列标题）留在区域之外、作为普通文本区域输出：它与表头之间没有列结构，几何上与"表格上方的标题"不可区分。这条要如实登记，不能靠 `region_margin_ratio` 蒙。

**验证（三条，全部可失败）**

- 新增 `a_wrapped_cell_does_not_split_the_region`：2 列 3 行、第 2 行右列折成两行且与左列不同 y（7 个框）→ 断言 1 个区域、`columns == 2`、`rows == 3`、`members.len() == 7`、`score > 0.95`（实测 `0.9564`；低于 1 只因为折行行的并集高 2 px，列对齐率仍是 1.0）。
- 新增 `a_production_log_whose_cells_wrap_is_one_region`：**硬编码上面那张图的 27 个真实文本框坐标**（当前失效输入；由 `/run` 产物导出，四舍五入到 0.1 px）→ 断言恰好 1 个区域、`columns == 2`、`rows == 8`、`bounds` 四边 ≈ 47 / 670 / 4 / 956（容差 2.0）、`members.len() == 26`（除表题外一个都不能丢）、`!members.contains(&0)`。
- 既有夹具回归：`datasets/text/04表格与键值对.png`（61 个文本框 → 16 个行）产出 `rows=5 cols=5 (169.35,328.53)-(2433.87,641.03)` 与 `rows=6 cols=5 (150,943.91)-(3030.65,1314.10)` —— 表题仍被排除，**已知假合并仍在**（`a_multi_block_page_still_merges_side_by_side_blocks` 照旧断言它存在）。

**门禁**：`cargo fmt --all` 0；`cargo clippy --all-targets --features serve -- -D warnings` 0；`cargo test --lib table::` → **244 passed / 0 failed**；`cargo test --all-targets` → **690 + 2 + 4 + 14 passed / 0 failed**（E22 的 lib 是 688，本轮净增 2）。真实图复跑：`regions=28 {text 27, table 1}`、`table[27] rows=12 cols=2 cells=24`、`holes=0`、`confidence=high`、`source_line_indices` 27 条、包围盒 `(0,27)-(976,690)`、`total_ms 484.3`。

**本轮的第二个发现（E25 已按下面的方向修好，见 §4.12）**：**折行单元格的续行会被回填匹配到上一行**。实测 `r6 c1` 里塞进了三行（含本属于 `r7` 的 `大圈正常出料，由于大超粗超发飞料…`），`r7 c1` 里塞进了两行（含本属于 `r8` 的 `14:30内径精磨维修完成…`），`r8 c1` 只剩一行。几何数据：续行 `y383-407` 与 `r6c1` / `r7c1` 的 IoU 是 **0.1989 / 0.1884**（差 1 %），下一行的首行 `y443-468` 与 `r7c1` / `r8c1` 是 **0.2154 / 0.2072** —— 决定胜负的是**结构模型预测的单元格框整体比真实行低 8–15 px**（模型行带 r6 `333-398`、r7 `392-460`、r8 `452-518`，真实行约 `318-380`、`380-445`、`440-500`）。**"保序约束"救不了它**：E23 试过"在行号非降约束下最大化总重叠"的动态规划，实测**正确的指派总 IoU 反而更低**（`1.3479` 对 `1.3666`），因为按 y 排序后两种指派的表行号本来就都是非降的 —— 约束根本不生效；重新编译后复跑真实图，产物与修前**逐字段相同**，于是**完整回退**（`src/table/cell_content_backfill.rs` 只留一条"已知局限"文档注释）。真正的方向是**从文本行序列本身推行使区间**：行边界是相邻行中心距里最大的那几个间隙（实测 32 / 31.5 px，而行内间隙只有 12.5–15.5 px），而不是信任漂移的模型框。

**全量 TEDS 重跑（本轮已做，结论是"没有变化"，而原因必须写清）**：命令 `target\release\table_eval.exe --content page-reuse --ocr-config models/configs/test-config-small.yaml --ocr-cache Temp/e13-cache-baseline.json --intra-threads 14 --predictions Temp/e23-pred-full.json --report Temp/e23-report-full.json`（7,104 样本，20 条失败，与入库基线同一份失败明细）+ `python tools\table_teds.py --true datasets\table\tabrecset\derived\pubtabnet_val_gt_html.json --pred Temp\e23-pred-full.json` → **`mean TEDS = 0.5834044577250798`，与入库基线逐位相同**；预测文件 3,744,372 B、`sha256 = df6bd2c0e17bdba5f17bfa74f3d9146c30f30a3ec98f2feae53f38c727d840d2`，与 `tests/baseline/windows-baseline/table-teds-full-cpu.json` 的 `determinism.prediction_sha256[0]`、以及 `Temp\e13-*-pred.json` **逐字节相同**。

**为什么指标不动（这条决定了怎么补测试）**：**离线评测根本不调用定位器**。`src/evaluation/table/` 里没有任何 `table::detection` 的引用（`Select-String` 零命中）；评测把**整张图**喂给结构模型（`src/evaluation/table/run.rs:1437-1459`，`CellSource::Reuse` 的 `offset` 是 `[0.0, 0.0]`，注释写明"`result.cells` 的坐标是整张图"），`--content page-reuse` 只决定**单元格文本**从哪来。定位器的唯一生产消费者是 `src/ocr/pipeline/rapid_structure.rs:1391`（整页管线）。TabRecSet val 是**已经裁好一张表一张图**的图片，所以它对这台定位器的行为**完全不敏感**。两个后果：

1. 入库基线 `table-teds-full-cpu.json`（`mean 0.583404`）**没有失效、也不需要重跑** —— 本轮改动对它是恒等变换（逐位可证）；
2. **反过来说：这份指标对定位器零覆盖。** 要度量"表格被切碎/漏检"必须换一份**整页**（一页多块内容、含表外正文）的数据集或自建 L1–L4 分层集（`docs/07` §3 / `docs/10` §2），裁剪表数据集量不出来。这是本轮发现的方法学缺口，登记在未覆盖项里。

**一条工具层事实**：E9 那个 OCR 缓存 `Temp/ocr-cache-val.json`（`{version, entries}`）已被现在的引擎**拒绝** —— `Error: Decode("the OCR cache Temp/ocr-cache-val.json is not valid JSON: missing field `engine_fingerprint` at line 1 column 10542465")`；能用的是 E13 之后带指纹的缓存（`Temp/e13-cache-baseline.json`，`{version, engine_fingerprint: "d9a39222…", entries: 7104}`）。热跑因此仍约 4 分钟（`cache_hits 7104 / cache_misses 0`，`regions_seen 0` —— 热跑不给页级 OCR 计时证据，这条与 E10 一致）。

**未覆盖**：① 只在 `test-config-small.yaml`（page_reuse）与这张图 + 既有夹具上验证过，**整页级数据集上没有指标**（上面的方法学缺口）；② 真实浏览器里未点过（表格渲染链路与本次改动无关）；③ 表题落在区域之外是**已知遗留**，会让 `rows` 比真值少 1；④ 折行续行的行归属（上面的第二个发现）未修。

---

### 4.11 后续修正（E24：表格改为惰性渲染按钮，用户提出 m13439）

**用户原话**：「我建议对于表格，需要增加一个渲染按钮，就像公式那样。点击这个按钮，就渲染识别的表格」。

**改动前的形态（E16.3）**：表格卡片**无条件**把模型 HTML 渲染进列表 —— `tableRegionHtml()` 里一行 `<div class="rt-wrap">${sanitizeTableHtml(t.html)}</div>`。而公式是"**默认显示原文**、选择引擎后才渲染"（`.fx-slot` + `data-latex`）。两者语义不一致：表格没有"看见模型产物"的入口，一张大表还会在结果一出来就把列表撑长。

**改动（`src/bin/web/index.html` 四处 + 一段 CSS）**

| 位置 | 改法 |
| --- | --- |
| CSS | 新增 `.rt-actions` / `.rt-btn` / `.rt-hint` / `.rt-raw`（含 `.rt-wrap[hidden]`、`.rt-raw[hidden]` 的 `display:none`） |
| `tableRegionHtml()` | `.rt-wrap` 默认 `hidden` 且**不预置内容**；新增 `.rt-actions`（`<button data-table-render="1" data-rendered="0" aria-expanded="false" data-table-html="${esc(html)}">渲染表格</button>` + 提示）与 `<pre class="rt-raw"><code></code></pre>` |
| `toggleTableRender(btn)`（新函数） | 点击 → `wrap.innerHTML = sanitizeTableHtml(btn.getAttribute("data-table-html") || "")`、显示 wrap、隐藏原文行、文案"收起表格"、`aria-expanded="true"`；再点 → **`wrap.innerHTML = ""`** + 全部复位 |
| `renderRegions()` | innerHTML 装配之后加一遍回填：`wrap.querySelectorAll(".region-table [data-table-html]")` 把 `getAttribute("data-table-html")` 写进同卡片的 `.rt-raw > code` |
| `regionList` 的 click 委托 | 在 `.region-item` / `.rt-wrap` 判据**之前**插 `const tbtn = e.target.closest("[data-table-render]"); if (tbtn) { toggleTableRender(tbtn); return; }` |
| `updateFxBarUI()` | 只有表格时工具条 hint 由"表格始终以净化后的 HTML 渲染"改为"点每个表格区域的「渲染表格」按钮就地渲染（先过白名单净化）" |

**四条设计判断**

1. **与公式同构**：默认可见的是**原文**（模型的真实产物），渲染是用户的选择。这既满足了"像公式那样"，也让"模型给出的 HTML 长什么样"仍然可核对。
2. **收起时清空 `innerHTML`，而不是只加 `hidden`**：所谓"不常驻"必须是真的 —— 一张 300 格的表展开后留在 DOM 里，滚动与复制都会变慢。
3. **原文与渲染输入是同一份**：模型 HTML 只存在 `data-table-html` 一个属性里，原文行由 `getAttribute()` 写回（`getAttribute` 拿回的是**未转义**原文）。两份会分叉；同源的代价是原文行依赖一次 JS 回填，收益是"看到的必是即将渲染的"。这条能成立的前提是 `esc()` 对属性的往返无损 —— 也就是 `.fx-slot` 的 `data-latex` 用了很久的同一个办法。
4. **净化仍是唯一入口**：惰性渲染只改了**时机**，没改**通路** —— 渲染路径上唯一的赋值语句是 `wrap.innerHTML = sanitizeTableHtml(...)`（门禁 G8 的断言从 `class="rt-wrap">${sanitizeTableHtml(t.html)}"` 改成这一条）。

**测试（结构断言 + 真浏览器）**

- `serve::tests::the_page_carries_the_mode_selector_and_the_table_renderer`：旧断言（精确匹配那行 `rt-wrap` 模板）替换为 `wrap.innerHTML = sanitizeTableHtml(`，新增 4 条 —— 按钮三态标记与两个文案、`.rt-wrap` 默认 `hidden` + `.rt-raw` 存在、`data-table-html` 的写入与读取两处、委托里 `closest("[data-table-render]")` 必须早于区域选中。既有的三条"模型 HTML 绝不直接进 innerHTML"否定断言与"净化不得用正则"否定断言**未动**。
- 新用例 `serve::tests::the_table_render_toggle_is_lazy_and_stays_sanitized`（headless Edge，驱动**页面自己的** `tableRegionHtml` + `toggleTableRender`，经 `globalThis.__RROCR_TABLE_HTML__`/`__RROCR_TOGGLE__` 两个暴露缝）：两个 payload（一个含合法 `colspan`、一个含 `<script>`/`<img onerror>`）各自断言 —— `esc()` + `getAttribute` **逐字节还原**模型 HTML（含引号）、初始折叠且原文可见且按钮文案"渲染表格"、点击后 wrap 显示、`innerHTML` 含真 `<table>`、合法 `colspan` 存活、`<script`/`<img`/`onerror`/`onload` 一个都不剩、再点后 wrap 隐藏**且被清空**、表格卡片不会长成折叠行（无 `.ri-fold`）。
- 新 helper `run_headless_dom(browser, html, tag) -> Result<(String, String), String>` 收拢了"写 harness → 浏览器落盘 → 轮询 stdout → 收进程"这段（G8 的原启动块**保留不动**，避免重构已验证的门禁用例）；`Err` = 这台机器没跑成 = 跳过。
- 为什么不是"在真页面上点一下"：`regionList` 的委托处理器在页面尾部才注册（空白文档里 `renderVerify()` 会先执行并抛），所以用例直接调函数，而"按钮必须被委托拦在选中区域之前"由上面的结构断言把守 —— 两条腿合起来才等于"点得动"。

**门禁**：`cargo fmt --all -- --check` 0；`cargo clippy --all-targets --features serve -- -D warnings` 0；`cargo test --all-targets` → **690 + 2 + 4 + 14 passed / 0 failed**；`cargo test --bin rapidstruct --features serve` → **289 passed / 0 failed**（E16 的 288 + 本用例 1）；`node tools/extract-page-scripts.mjs` 抽出的两个脚本体 `node --check` **都 OK**。

**二进制与重启**：页面由 `src/bin/serve/run.rs` 的 `include_str!("../web/index.html")` **编译期**嵌入，所以改动必须重建 —— 用 E22 的办法绕过正在运行的服务（`Move-Item` 改名后 `cargo build --release --bin rapidstruct --features serve`，12.18 s 成功），新二进制里 `data-table-render` 2 处、`toggleTableRender` 3 处（用 Python 字节扫描确认；**`findstr` 在 42 MB 的 exe 上三个串都返回 1，不可用**）。**用户当时正在运行的服务仍是旧二进制，必须重启页面服务才能看到按钮。**

**未覆盖**：① 真页面上的**点击**没跑过（见上的两条腿）；② 渲染后的大表滚动/复制体验未实测；③ 演示模式（DEMO）里的按钮只由同一套代码路径覆盖，未肉眼验；④ 按钮状态不持久化（跨结果、跨刷新都复位），这是有意的（状态属于一次结果，不属于引擎偏好 —— 与公式的 `localStorage` 引擎选择不同类）。

---

### 4.12 后续修正（E25：两行内容进同一个单元格 —— 行边界改由文本行推，用户实测）

**现象（用户原话）**：「问题：内容 A B 都成功识别为表格，但是表格渲染将 A B 渲染到一个单元格内」。用户那张 982×694 的生产记录表（12 行 × 2 列）复跑后**表格区域与网格完全正确**（`regions=28 {text 27, table 1}`、`rows=12 cols=2 holes=0 confidence=high`），错的只有**哪个单元格拿到哪几行文字**：

| 单元格 | 修前 | 修后 |
| --- | --- | --- |
| `r6c1`（12:00~13:00） | 12:00 的两句 **+ 13:00 的第一句**（`…内径精 磨冷却液小，打维修 大圈正常出料，由于大超粗超偶发飞料，出料速度较慢，小圈出料到内径精磨工`） | 只剩 12:00 的两句 |
| `r7c1`（13:00~14:00） | 只剩「序」+ 14:00 的第一句 | 13:00 完整（`大圈正常出料，…内径精磨工 序`） |
| `r8c1`（14:00~15:00） | 只剩 14:00 的第二句 | 14:00 完整 |

**根因（第一性事实，实测不是推断）**：`src/table/cell_content_backfill.rs` 对**每个文本行独立地**在**所有**单元格里取最优 IoU —— 而结构模型预测的单元格框**整体比真实行低 8–15 px**。用户那张表的页级行中心间隙是**双峰**的：行内（折行续行）`12.5–15.5 px`，行间 `31.5–59 px`；模型框 r6c1 `y332.7–397.5`、r7c1 `y392.5–459.7` 都把边界压在 13:00 第一行（`y383–407`）身上，于是它对 r6c1 的 IoU 是 **0.195**、对 r7c1 是 **0.189** —— 判给上一行，之后每行串一格。这正是 E23 §4.10"第二个发现"登记的那条缺陷。

**修法（根因，不是调参）**：**行数用结构模型的、行边界用文本行的**。在 `match_cells_with_min_iou` 里新增 `fn assign_rows_from_text(lines, kept, table_top, table_bottom, rows) -> Option<Vec<Option<usize>>>`：

1. 只取**纵向与表格范围相交**的行（`bbox[3] >= table_top && bbox[1] <= table_bottom`）—— 页面上下方的正文不能给表格定行边界；
2. **合格间隙**：按中心 y 排序后，"上一行的下沿 ≤ 下一行的上沿"（两条行**纵向不相交**）的相邻两行之间才有资格当行边界，位置取两端中点。同一视觉行上的左右两个单元格互相重叠，永远不产生候选；
3. 在合格间隙里取**最大的 `rows-1` 个**作边界；`rows = max(row_start + rowspan)` 从网格里数出来（含跨行单元格）；合格间隙不足 `rows-1` 个 → `None`；
4. **只在"选中的最小间隙 > 其余所有间隙的最大值"时才认这条划分**（间隙交错说明文本布局不呈行状）→ 否则 `None`，退回逐格匹配；
5. 按边界给每个行的**中心**定行号（严格大于边界 → 落到下一行，与参考实现"平局取第一个候选"同口径），再在该行内按 `(1-IoU, distance)` 取最优**列**；跨行单元格在它覆盖的**每个**行里都是候选。

**没有新增任何参数**：判据比较的是"两个间隙集合的上下确界"与"两行是否纵向不相交"，不是"中位数的 1.5 倍"这类阈值。

**两条退路（"只改归属、绝不丢文字"）**：① 合格间隙不足 / 判据不成立 → 整张表退回旧的逐格匹配；② 某一行在它被指派的行里**够不着任何框** → **该行**退回全集。第 ② 条是实测逼出来的：第一版只按行带过滤时，`datasets\text\04表格与键值对.png` 的 region62 比修前**少了一个字符**（`Temp\e25-chars.py` 实测 dropped `{'9': 1}`）。`BackfillOutcome` 因此新增 `pub text_bands_applied: bool`，让"这条修法有没有在这张图上生效"可被断言。

**为什么不用更复杂的设计（全量 TEDS 实测，7,104 样本，命令模板见下）**

| 版本 | 算法 | mean TEDS | 相对入库基线 `0.5834044577250798` |
| --- | --- | --- | --- |
| v1 | 最大的 `rows-1` 个间隙（**不**过滤"合格"） | `0.5776642455721679` | −0.00574（better 821 / worse 1110 / same 5173） |
| **v2（本条）** | **最大的 `rows-1` 个*合格*间隙 + 判据** | `0.5835628168807342`（第一次）/ `0.5836405337617874`（最终代码复跑） | +0.00016 / +0.00024 |
| v3 | 视觉行分组 + "行号非降"保序 DP | `0.543954301224465` | −0.03940 |
| v4 | 模型行边界吸附到最近的合格间隙 | `0.5722868332776865` | −0.01112 |

**这一列数字怎么读（确定性已证）**：上表 v2 的两个数字来自**两个不同的 v2 变体** —— 候选边界都是"合格的 `rows-1` 个间隙"，但前者按**相邻行中心距**排序、后者（= 最终代码）按**两行之间的纵向净空**排序。为排除"是不是运行噪声"，另做了探针：**同一个二进制连跑两次**，预测摘要**逐字节相同**（`sha256` 前缀 `ee528ea737e909fd`，3,744,387 B，与最终那次相同）；因此本流水线在全量集上是**确定的**（与 E13/E23 的 `df6bd2c0…` 三次逐字节相同一致），上表所有差异都来自代码差异，不是噪声。最终代码 = `0.5836405337617874`（基线 `0.5834044577250798`，**+0.00024**），v1/v3/v4 的距离（`−0.0057` / `−0.0394` / `−0.0111`）远超这一点差量。

两个把 v2 比下去的具体反例（都进过代码、都被量测否掉）：**v1** 会把 6 列表头 `A B C D E F` 的 1–2 px y 抖动当成行边界（`139659444_0_all-line.jpg`：`0.875 → 0.250`），所以加了"合格"过滤；**v3/v4** 能修好"单元格内三条堆叠行"的样本（`14_0c01373ca6999ceae4af96ed037efaa9_segR__tR__yf_0_all-line.jpg`：修前 `0.860`、v2 `0.461`、v3 `0.769`），代价是全量集大掉 —— 这是**净收益**的判断，不是"更聪明的算法更好"的直觉。命令：`target\release\table_eval.exe --content page-reuse --ocr-config models/configs/test-config-small.yaml --ocr-cache Temp/e13-cache-baseline.json --intra-threads 14 --predictions <out> --report <out>`，再 `python tools\table_teds.py --true datasets\table\tabrecset\derived\pubtabnet_val_gt_html.json --pred <out> --out <out>`。

**验证（六条，全部可失败）**

- 新增 `a_wrapped_continuation_stays_in_its_own_row`：`real_wrapped_rows()` **硬编码生产图 12:00 / 13:00 两行的真实坐标**（4 个模型框 + 6 条行，行号压成 0/1）→ 断言 `text_bands_applied`、四个单元格的文本、`kinds = [Single, Multiple, Single, Multiple]`、`matched_lines_total == 6`。**这条用例确实会失败**：临时把行归属分支置空后重跑，报 `left: "申博康第二批吃饭，内径 磨冷却液小，打维修 大圈正常出料，内径精磨"` —— 正是用户看到的症状；随后还原。
- 新增 `the_text_gap_replaces_the_models_drifting_boundary`：直接驱动 `assign_rows_from_text`，断言同一组真实坐标下 6 条行是 `[0,0,0,1,1,1]`（模型自己的边界 `395` 被文本间隙 `379` 取代）。
- 新增 `rows_come_from_the_grid_and_boundaries_from_the_text`（3 行 2 列，行内间隙 3–11 px vs 行间 18/24 px）。
- 新增 `a_spanning_cell_collects_lines_from_both_of_its_rows`（`rowspan=2` 的单元格在两个行里都能收行）。
- 新增 `a_line_its_own_row_cannot_reach_is_still_matched_not_dropped`（行内够不着 → 退回全集，`matched_lines_total == 3`）。
- 新增 `a_header_row_of_side_by_side_cells_is_not_split_by_its_own_noise`（`A..F` 表头：一个合格间隙都没有 → `text_bands_applied == false`，6 格全留第 1 行、第 2 行全空）；`a_cell_with_several_stacked_lines_keeps_all_of_them`（堆叠表头的**良性**版本：4 处合格间隙里最大的两个正好是两条真实行边界）。
- **四张图的字符多重集逐字符比对（`Temp\e25-verify.py`）**：real 12×2 `non_empty 23→23 / 474→474`、region61 `26→26 / 187→187`、region62 `22→22 / 194→194`、`table.jpg` `10→10 / 151→151` —— **没有丢字，也没有造字**，且 region61、region62、`table.jpg` 的单元格内容**逐格与修前相同**（`table.jpg` 的表头仍留在第 1 行：`r0c0=Methods r0c1=R r0c2=P …`）。

**门禁**：`cargo fmt --all -- --check` 0；`cargo clippy --all-targets --features serve -- -D warnings` 0；`cargo test --lib table::` → **251 passed / 0 failed**（E24 的 244 + 本轮的 6 条 + 中间版删除/替换后的净增）；`cargo test --all-targets` → **695 + 2 + 4 + 14 passed / 0 failed**。

**已知影响（如实登记）**：`04表格与键值对.png` 的 **region62 是 E23 就登记的"假合并"区域**（两张表被并成一个 7×5 网格，相邻模型行互相重叠 30–38.7 px，网格本身不可信）。它的文本分布在本轮改变了（例如修前 `r1c0 = 设备编号 Device 模型版本 Model`，修后 `r1c0 = 模型版本 Model`、`r0c0` 收下块首的几行），但字符多重集不变。该区域本来就是缺陷产物，等定位器"不合并两张表"之后才会有确定的期望输出。

**未覆盖**：① 同一个单元格里多行的**拼接方式**仍是空格连接（`join_line_texts`），折行处没有换行 —— 修后 `r6c1` 是 `…内径精 磨冷却液小，打维修`（原文在「精」与「磨」之间断行），这是既有行为，本轮没改；② 行归属在"模型行数 < 真实行数"时会把一个折行拆到两个模型行（`text_bands_applied` 可见，但没有更好的信息可用）；③ **"单元格内三条堆叠行、且堆叠间隙比行边界还大"的形态仍会错**（实测 `14_0c01373ca6999ceae4af96ed037efaa9_segR__tR__yf_0_all-line.jpg` 修前 `0.860`、本条 `0.461`）—— v3/v4 都能修好它但全量 TEDS 反而更差，所以本条**明知**带着它；④ 只在 `test-config-small.yaml` 的四张图上抽查过。

**入库基线不重建（用户裁定：删掉这条待办）**：`tests/baseline/windows-baseline/table-teds-full-cpu.json` 的 `mean = 0.5834044577250798`、预测 `sha256 = df6bd2c0…`（3,744,372 B）测于 E25 之前，E25 之后是 `0.5836405337617874` / 3,744,387 B（差 `+0.00024`，远小于本轮记录在案的样本级波动：`14_0c0137…` 单样本差 `0.40`）。按 E10 协议（§4.3）重建要 **1 次冷跑 + 2 次热跑**，收益只是把这 0.00024 写进入库文件，因此**决定不重建**。两条使用约定：① 引用这两个基线数字时必须连着"测于 E25 之前"一起读；② 将来若需要一次新的精度主张，先按 §4.3 重建再引（热跑确定性本轮已证：同一二进制连跑两次预测摘要逐字节相同 `ee528ea737e909fd`）。

---

## 5 E17 收尾与登记

- [x] **E17.1** `THIRD_PARTY_NOTES.md`：若表格模型进入默认分发，更新再分发结论；
  **未确认许可前继续写 `<待确认>`**（`docs/09` §14.3.1 已登记权重许可未确认）。
  —— **✅ 已完成**：表格模型**不在默认分发集合**（现行默认表 `assets/default_models.yaml` 无 `table` 段；**E18 更正**：当时引用的 `models/configs/manifest.json` 是 legacy 死文件，已删除），所以权重 `<待确认>` 的结论保持；fixtures 清单与 `.gitignore` 例外说明已补齐（§5.5.3）。
- [x] **E17.2** `cargo package --list --allow-dirty --offline` 复核：包内不得出现
  `models/` / `datasets/` / `refer/` / `Temp/` / `benchmarks/` / `prototypes/` / `tests/baseline/`，
  且不得有开发机绝对路径（判据与工具层教训见 09 §14.3.5）。
  —— **✅ 已完成**：修前 **271** 条（**缺** `input_bgr_resized.bin`）→ 修后 **272** 条；forbidden 目录 0、开发机绝对路径 0（§5.5.3 / §5.5.5）。
- [x] **E17.3** 文档同步：`docs/07-table-recognition.md` 的 §7（定位/输出）、§10（分档）、
  §13（落地状态）、§14（未确认项）；`docs/09` §0.5 的 `table_structure_budget`；
  `docs/10-table-evaluation-protocol.md` 若评测口径受影响。
  —— **✅ 已完成**：`docs/07` §7（新增 §7.7）/ §10 / §13 / §14 已同步；`docs/09` 的 `table_structure_budget` 在 E14 收口时已写入（本轮只做确认）；`docs/10` 判定"未受影响"并写明理由（§5.5.1）。
- [x] **E17.4** 登记本阶段**未做**的事与**新发现**的缺陷（照 09 的做法：如实登记，不掩盖）。
  —— **✅ 已完成**：E16 的 5 条 + E17 的 5 条登记项写入 `docs/07` §14；E17 的完整 5 条在 §5.5.6。
- [x] **E17.5** `docs/11` §1 总表状态列全部改为 ✅ 并指向各自的完成记录。
  —— **✅ 已完成**：**§0.5 阶段总表**四行全部 ✅（原文写"§1 总表"；本文件的阶段总表实际在 §0.5，§1 是"起点：E1–E13 已经交付了什么"）。


### 5.5 完成记录（E17，已收口）

#### 5.5.1 变更摘要

| 文件 | 变更 | 理由 |
| --- | --- | --- |
| `.gitignore` | 增加 `!tests/fixtures/table-golden/input_bgr_resized.bin` 例外（3 行，含 2 行注释） | 该文件被上一行的 `*.bin` 静默吃掉，包清单里根本没有它 |
| `THIRD_PARTY_NOTES.md` | fixtures 清单补表格域两项；"exceptions are limited to …" 段改写为"恰好两条"并说明缘由；新增 `table.jpg` 的来源与 Apache-2.0 归属段（含兜底） | 表格式 fixture 之前**没有任何来源记录**；`input_bgr_resized.bin` 的存在依赖于一条没人知道的 ignore 例外 |
| `docs/07-table-recognition.md` | 新增 **§7.7 网页侧消费（E16 定案）**；§10 轻量档新增「E16 落地（网页侧如何暴露分档）」；§13 第 12 行的包清单数字 268 → 272；§14 关闭 E15 的"网页侧尚未消费"条目，新增 **E16** 与 **E17** 两组登记项 | `docs/11` §5 的 E17.3 文档同步清单 |
| `docs/11-table-page-pipeline-execution.md` | §5 五条勾选；新增本节 §5.5；§0.5 阶段总表 E17 行 → ✅ | E17.5 |
| `docs/09-table-recognition-execution.md` | **未改** | §0.5 的 G5 行在 E14 收口时**已经**含 `table_structure_budget = 120 ms`，E17.3 要求的这一项已满足（本轮只做确认） |
| `docs/10-table-evaluation-protocol.md` | **未改** | E16/E17 没有触碰评测口径、评分驱动或基线文件；§5.2.1 的两份基线文件与 §5.3 的两份驱动脚本都没变 |

**刻意没有做的事**：① 没有把 `src/table` 或表格式 fixtures 加进 git（登记为 release 前的必做项，不在收尾阶段代做）；② 没有改 `TableOutcome.model_id` 的路径泄漏；③ 没有在 `TableStructureSession::open` 里拒绝 DirectML；④ 没有为 `input_bgr_resized.bin` 补一条"文件必须存在"的测试。

#### 5.5.2 设计判断

1. **`.gitignore` 用"一条精确例外"而不是"把 `*.bin` 规则收紧"**：`*.bin` 对仓库其余部分是**对的**（编译产物、本地模型缓存都是 `.bin`），而 `tests/fixtures/**` 下的 `.bin` 是手工烘焙的**真值**。收紧成 `!tests/fixtures/**/*.bin` 会让未来任何 fixture 二进制都意外入库；精确例外只对一个文件生效，且它的存在有 `THIRD_PARTY_NOTES.md` 的解释。
2. **包清单数字写进文档时带上"改动前后"**（268 → 272）：只写 272 会让人以为这个数字是稳定的；带上前后才能让人在下次复核时看出"这个数字被改过、原因是什么"。
3. **`docs/07` 的 §7.7 单独立节而不是塞进 §7.6**：§7.6 讲的是三个**输出通道**（同一份结构的三种序列化），§7.7 讲的是**第四个消费者**（网页），关注点（净化、跨度白名单、折叠被消费行）与序列化无关。混在一起会让"净化是输出层职责还是网页职责"重新变含糊。
4. **`docs/07` §10 的注记放在"轻量档"而不是"精度档"**：网页禁用表格两项的判据正是轻量档（无 SLANet-plus）。放在这条档位下面，"没有模型就不给开关"与"有模型但结果不可信就标记 low"才是同一段话里的两件事。
5. **`docs/09` 与 `docs/10` 一个字不改并写明理由**：E17.3 的原文是条件句（"若评测口径受影响"），条件句需要一个**明确的不改判定**，否则下一次复核会重新问一遍。
6. **E17 的登记项写进 `docs/07` §14 而不只写在本文件**：§14 是未确认项的**唯一汇总处**（`docs/09` §14.3 的登记项当初也回填到了那里）。只写在本文件，下一个读 `docs/07` 的人会以为表格域已经没有未决项。

#### 5.5.3 关键结果

- **`cargo package --list --allow-dirty --offline`**：修前 **271** 条、**缺** `tests/fixtures/table-golden/input_bgr_resized.bin`；修后 **272** 条、该文件出现 **1** 次。两次均 exit 0、forbidden 目录 **0**、开发机绝对路径 **0**（`models/` / `datasets/` / `refer/` / `Temp/` / `benchmarks/` / `prototypes/` / `tests/baseline/` 一条都不出现）。
- **`git check-ignore -v tests/fixtures/table-golden/input_bgr_resized.bin`**：修前命中 `.gitignore:31:*.bin`；修后不再被忽略。
- **表格式 fixture 规模**：`tests/fixtures/table-golden` + `tests/fixtures/table-onnx` = **18 文件 / 799,909 B**（`input_bgr_resized.bin` 563,640 B 为最大一项，`manifest.json` 114,334 B 次之）。
- **表格模型不在默认分发集合**：判定依据是**现行**的默认表 `assets/default_models.yaml`——顶层只有 `onnxruntime` 与 `formula` 两段，**没有 `table` 段**，而库在模型目录没有 `manifest.json` 时回退到的正是它。**更正（E18）**：E17 当时引用的 `models/configs/manifest.json` 是本地未跟踪的 **legacy 四字段死文件**（`{detector, recognizer, dictionary, classifier}`、`id = pp-ocrv6-medium-multilingual`、**没有任何代码读它**，`src/model_source.rs:322-328` 明确拒绝该形状并给出迁移提示），**已在 E18 按用户决定删除**。所以 `THIRD_PARTY_NOTES.md` 的 SLANet-plus 段结论（`Not bundled` + 权重许可 `<待确认>`）**仍然不需要修改**——依据换成默认表之后结论不变，但引用它的那处证据不能再用那个文件。这是 E17.1 的条件句"若表格模型进入默认分发"的判定依据。
- **`refer/RapidTable-main/LICENSE` = Apache-2.0**（11,337 B，本地核实文件头）→ `table.jpg` 的归属有依据，不再是"来路不明的测试图"。

#### 5.5.4 基线对比

E17 **不产生性能或精度数字**（收尾阶段），可比对的只有打包面：

| 项 | E13 收口时（`docs/07` §13 第 12 行） | E17 复核 | 判定 |
| --- | --- | --- | --- |
| `cargo package --list` 条目数 | 268 | **272** | 差量**全部来自本阶段新增的未跟踪文件**（`src/table/*`、表格式 fixtures、`docs/07`–`docs/11`、表格式 tools），不是"包变脏" |
| 包内 forbidden 目录 | 0 | **0** | 不变 |
| 包内开发机绝对路径 | 0 | **0** | 不变 |
| `evaluation.` / 性能基线 | — | **未重新生成** | E17 不改代码路径，`tests/baseline/windows-baseline/` 下的基线文件本轮**一个都没动** |

#### 5.5.5 验收命令与实测结果

```powershell
cargo package --list --allow-dirty --offline        # exit 0；272 条；forbidden 0；绝对路径 0
git check-ignore -v tests/fixtures/table-golden/input_bgr_resized.bin   # 修后无输出 = 不再被忽略
cargo fmt --all -- --check                         # exit 0
cargo clippy --all-targets --features serve -- -D warnings   # exit 0
cargo test --all-targets                           # 688 + 2 + 4 + 14 passed / 0 failed
cargo test --bin rapidstruct --features serve          # 287 passed / 0 failed（4 次运行中 3 次；见 §5.5.6 第 6 条）
```

| 条目 | 判定 | 依据 |
| --- | --- | --- |
| E17.1 | ✅ | fixtures 清单与例外说明已更新；`table.jpg` 来源/许可已记录；权重 `<待确认>` 按"不在默认分发集合"保持 |
| E17.2 | ✅ | 包内零 forbidden 目录、零开发机绝对路径（判据同 `docs/09` §14.3.5） |
| E17.3 | ✅ | `docs/07` §7/§10/§13/§14 已同步；`docs/09` 的 `table_structure_budget` 已确认在 E14 写入；`docs/10` 判定"未受影响"并写明理由 |
| E17.4 | ✅ | E16 的 5 条 + E17 的 5 条登记项写入 `docs/07` §14（E16 的完整 10 条在本文 §4.8.6） |
| E17.5 | ✅ | §0.5 阶段总表四行状态**全部 ✅** |

#### 5.5.6 未覆盖风险

1. **`input_bgr_resized.bin` 的"必须存在"没有被测试断言**：它再次被某条 ignore 规则吃掉时，逐位比对只会**安静地 skip** —— 唯一的发现手段是人工跑 `cargo package --list`。
2. **本阶段的全部产物仍未提交 git**（`git ls-files src/table` = 0、`git ls-files tests/fixtures/table-golden` = 0）：`--allow-dirty` 让包清单检查通过，但**干净 clone 既拿不到代码也拿不到 fixture**。这是 release 前的阻塞项，须在本阶段之外完成。
3. **`THIRD_PARTY_NOTES.md` 只补齐了表格域**：其余 fixture 的来源/许可没有重新清点（`table.jpg` 是唯一新增记录的第三方测试资产）。
4. **`?table=` 契约面的更早文档未复核**：`docs/04-*` / `docs/06-*` 里"队列类别就是管线选择"的字面表述仍在，只在 `docs/05` §4.2.2 与 `docs/07` §6.1/§7.7 登记了新参数。
5. **E16 的页面手工自检已交接给用户**（用户声明「本地网页测试由我负责」，见 §4.5 第 3 条）：本轮做到"无头浏览器 + 真实页面脚本 + 伪造结果"，**用鼠标点一遍**由用户完成；交接完成前 §4.8.6 第 1 条的技术事实仍然成立。
6. **`cargo test --bin rapidstruct --features serve` 观测到一次未复现的抖动**：同一配置下，一次运行报 `285 passed; 2 failed`（紧跟 `cargo test --all-targets` 之后跑），随后 **3 次连续运行都是 `287 passed; 0 failed`**。**失败的两条用例名没有留下**（第一次只抓了 `test result:` 行），所以**根因未知** —— 可能是端口 / 无头浏览器等共享资源竞争，也可能是本轮 E17 改动的无关偶发。判定口径：§5.5.5 的 287 是 4 次运行中 3 次的稳定值；**下次动 serve 之前建议先连跑两遍确认基线**，否则无法区分"我改坏了"与"它本来就偶发"。

---

## 6 阶段完成记录模板

每个阶段结束时，在对应 §x.5 / §x.8 追加以下小节（照 `docs/09` §14.3 的七节结构）：

```markdown
### X.5 完成记录（EXX，已收口）

#### X.5.1 变更摘要
| 文件 | 变更 | 理由 |
| --- | --- | --- |

#### X.5.2 设计判断
（每条判断写"选了什么 / 为什么 / 备选为什么不行"。）

#### X.5.3 关键结果
（实测数字 + 命令 + 复现方式。数字必须标注是 `[实测]` 还是 `[推算]`。）

#### X.5.4 基线对比
（与哪个基线、差多少、是否落在门禁内。）

#### X.5.5 验收命令与实测结果
（命令块 + 测试计数 + 门禁逐条判定。）

#### X.5.6 未覆盖风险
（如实列出，不许写"无"。）
```

---

## 附录 A 本阶段涉及的既有事实索引

| 事实 | 位置 |
| --- | --- |
| 公式路由走"抹白 → 文本 → 追加公式区域" | `src/ocr/pipeline/rapid_structure.rs:1035-1130` |
| 表格三个计时字段恒 0 | `src/ocr/pipeline/rapid_structure.rs:918-920` |
| `StageReports.table` 恒默认 | `src/ocr/pipeline/rapid_structure.rs:1011-1012` |
| `OcrRequest` 的字段面 | `src/api.rs:300-311` |
| `RegionKind` 只有两态 | `src/api.rs:466-470` |
| `FormulaPolicy` 的文档（E14 的 `TablePolicy` 应对齐它的严谨度） | `src/api.rs:484-514` |
| `StageReports` / `OcrTimings` 的表格槽位 | `src/api.rs:655-705` |
| 表格链当前唯一的驱动者 | `src/evaluation/table/run.rs:1380-1490` |
| 回填的复用/裁剪两条路径与计数器 | `src/table/cell_content_backfill.rs:178-230`、`:497-540` |
| serve 的队列类别 = 管线选择 | `src/bin/serve/server.rs:2382-2441` |
| serve 的路由表 | `src/bin/serve/http.rs:505-545`、`:901-908` |
| 网页的三处待改位置 | `src/bin/web/index.html` L1847（模式）、L3565（模型横幅）、L4580（结果渲染） |
| 网页由 `include_str!` 嵌入 | `src/bin/serve/run.rs:42` |
| G5 至今无数值 | `docs/09-table-recognition-execution.md` §0.5 |
| 定位器的实测精度与已知缺陷 | `docs/09` §12.4.2 / §12.4.3 / §12.4.7 |
| 降级档的首次量化（350/7084 低置信） | `docs/09` §14.3.4 |
| `cargo package` 判据与工具层教训 | `docs/09` §14.3.5 |
