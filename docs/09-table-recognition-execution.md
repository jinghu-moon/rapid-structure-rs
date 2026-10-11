# 表格识别（SLANet-plus）接入 rapid-structure-rs：分阶段执行文档

> 文档状态：待实施执行计划（序号 09）
>
> 上游依据：[docs/07-table-recognition.md](07-table-recognition.md)（设计 v2：性能与精度平衡）、[docs/08-table-recognition-perf-review.md](08-table-recognition-perf-review.md)（性能专项推导记录，结论地位已被 07 取代，仅作推导留档）。
>
> 目标：按可验证的顺序把 SLANet-plus 表格结构识别接入 `rapid-structure-rs` —— **先用公开数据建立精度基线与性能基线，再打开"用精度换性能"的旋钮**。
>
> 适用范围：项目仍处于开发期，尚未正式发布。允许破坏性改动，不保留旧接口、不承担向后兼容成本。
>
> **本文档只回答"按什么顺序做、每步做什么、判据是什么"，不重复 v2 的推导过程。**每条结论均可回溯到 07 的章节号；两处刻意的顺序偏离在 §0.6 说明。

---

## 0. 执行规则

### 0.1 开发期决策（不可变）

- 不考虑历史版本兼容性；允许删除、重命名、重构现有公共 API 与配置字段。
- 不增加仅用于兼容旧调用方的 adapter、wrapper、legacy branch 或重复实现。
- 不为了让测试通过而弱化断言、跳过测试、修改错误真值或降低指标。
- 任何完成声明都必须附带实际命令、结果和未覆盖风险（模板见 §15）。

### 0.2 必须保持的现有行为

表格功能可以破坏不合理的内部抽象，但不得无意破坏：

- 普通 OCR 的检测 / 方向分类 / CTC 识别；文件、内存、URL 输入限制；
- JSON / Markdown / HTML / 可视化输出；阅读顺序与多栏行为；
- 公式识别链路（独立识别、页面 route、tokenizer、batch/EOS、指标）；
- provider 选择与 CPU/DirectML/CUDA 编译路径；模型 SHA 校验入口；
- **表格开关关闭时，`ocr_total_ms` 与全部输出必须与基线可比**（判据见 §0.5 的 G1）。

### 0.3 证据分级与性能数字的唯一来源

| 标记 | 含义 | 可用范围 |
| --- | --- | --- |
| **[实测]** | 来自 Rust harness、模型探针或数据核验 | 可作决策依据 |
| **[推算]** | 由实测的成本模型外推 | **只可信其形状（比例）**，绝对值必须重测 |
| **[未测]** | 尚未量化 | 是空缺，不得当作已定结论 |

- **性能绝对数字只允许来自 Rust harness**：`src/bin/bench_warm_e2e.rs` 与 `tools/run_windows_baseline.ps1`。
- 本机 Python `onnxruntime` 1.20.1（`C:\A_Softwares\Python\python.exe`，Python 3.12.8）测得的绝对延迟比 Rust 静态链接的 ORT 1.28 慢 **rec ≈ 30× / det ≈ 20×**，且无法用线程数解释。→ **Python 数字禁止用于任何性能结论**，只能描述成本模型的形状（07 §0.3）。
- Python 仅允许用于三件事：契约 fixture 生成（`tools/build_*_onnx_fixtures.py`）、离线数据集转换（detokenize 适配层）、参考实现对照（`tools/formula_detect_reference.py` 同类手法）。

### 0.4 日常与重型验证分层

| 变更类型 | 必须执行 | 不要求默认执行 |
| --- | --- | --- |
| 文档、模块移动 | `cargo fmt --all -- --check`、`cargo test --all-targets`、`cargo clippy --all-targets -- -D warnings` | 12 图 bench、TEDS |
| `runtime` / `session` / `contracts` | 上述命令 + 12 图 bench + provider smoke | TEDS |
| 表格预处理 / 解码 / 回填 | 上述命令 + L1 契约单测 + `datasets\text\04表格与键值对.png` 回归 | TabRecSet val 全量 7,548 |
| 评测 / 指标实现变更 | 上述命令 + TEDS 抽样对拍（Python 参考实现 vs Rust） | 全量评测 |
| 阶段收尾 | L1–L4 全跑 + 净室复跑基线 | 公式全量集 |

### 0.5 门禁（来自 07 §1.3，每个阶段至少落到其中一条）

| 门禁 | 断言 | 类型 |
| --- | --- | --- |
| **G1** | 表格开关关闭时，既有 `ocr_total_ms` p50 落在基线 ±10–15 % 噪声带内 | 计时 |
| **G2** | 回填路径的 rec 调用次数增量为 **0** | **结构不变量（单测可断言）** |
| **G3** | 结构推理调用次数 **= 表格数**，且不随单元格数增长 | **结构不变量（单测可断言）** |
| **G4** | 表格定位不新增整页 CNN（模型数量不增加） | **结构不变量（审查）** |
| **G5** | 可归因表格分量之和（`table_detect_ms + table_structure_infer_ms + table_backfill_ms`，来自 `timing_ledger.totals`）**中位数** ≤ `table_structure_budget` = **120 ms** | 计时（**已定值**，见下） |

**基线锚点**（`tests/baseline/windows-baseline/bench-cpu.json`，[实测]，release / 12 页 × 3 轮 = 36 样本 / max_side_len 2000 / intra_threads 16）：`ocr_total_ms` p50 = **990.11 ms**、p90 = **1136.14 ms**；平均 **34.8** 区域/页；`detector_infer_ms` **570.57**（57.9 %）、`recognizer_infer_ms` **273.53**（27.7 %）。

**G5 的判据在 E14.7 实测后被修正过一次，数值在 E14.7 定稿**（原文是"整页 p50 增量 ≤ 预算（数值待 E1 接入后的首轮实测确定）"）。修正的理由：表格开销**双峰**——不含表格的页面增量为 0，含表格的页面增量为整份表格成本——而 `p50` 取的中位样本恰好落在两峰分界上，实测 Δp50 中位 **+10.80 %** 与本仓库用来表示"测不出变化"的 **±10–15 % 噪声带同宽**（即 G1 的判据）。用一个与噪声同宽的尺子量一个比噪声还小的量，判据不成立。改用**同一次请求内部**测出的可归因分量之和后，它不受轮间漂移影响，且正好覆盖两种真实退化模式：① 每个单元格各跑一次结构推理（分量随单元格数增长）；② 每张图开一次 `TableStructureSession`（分量含图优化成本）。完整推导见 `docs/11-table-page-pipeline-execution.md` §2.5.4（`temp` 侧工具 `tools/run_table_page_budget.ps1` + `tools/check_table_page_budget.py`）。

### 0.6 与 07 §13 的映射，以及两处刻意的偏离

| 07 §13 步 | 本文档阶段 | 说明 |
| --- | --- | --- |
| 1 | E1 | R9 接入 `TimingLedger` |
| 2 | E2 | R3 多输出契约 |
| 3 | **E3 → E4** | 拆成"先写解码器纯函数层，再实测坐标系" |
| 4 | E5 | R4 固定 H/W |
| 5 | E6 → E7 | 拆成"先做 detokenize 适配层，再跑 baseline" |
| 6 | E8 | 评测协议定稿 |
| 7 | E9 | R1 回填重构 |
| 8 | E10 | R2 表格路径 det 分辨率 |
| 9 | E11 | R8 表格定位 |
| 10 | E12 | R5 线程 + R7 DirectML per-EP |
| 11 | E12 | 同上 |
| — | **E13（新增）** | 输出、降级与登记收尾 |

**偏离一：新增 E3（结构解码器纯函数层）。** 07 §13 第 3 步直接要求"用 golden 钉死 `rescale_cell_bboxes` 的坐标系"，但坐标系只有在解码器存在之后才有实测对象。→ 必须先把解码器拆成**不依赖 ONNX** 的纯函数层，坐标系的 golden 才能独立成立。

**偏离二：新增 E13（输出与登记收尾）。** 07 §13 的 11 步只覆盖会改性能的步骤，未包含 Markdown span 规则、结构降级档、manifest 登记位置这些 07 §14 里仍然开放的项目。这些是功能正确性所必需的，不能留给"以后再说"。

---

## 1. 阶段总表

| 阶段 | 名称 | 核心产出 | 依赖 | 完成门槛 | 07 对应 | 状态 |
| --- | --- | --- | --- | --- | --- | --- |
| E1 | `TimingLedger` 接入表格分量 | 3 个命名分量可观测 | 无 | G1 + 守恒不破 | R9 | **✅ 已完成**（§2.5） |
| E2 | `OrtSession` 多输出契约 | 2 输出可同时取回 + fixture | E1 | 契约单测绿 | R3 | **✅ 已完成**（§3.4） |
| E3 | 结构解码器纯函数层 | `src/table/decode.rs` | E2 | 合成输入单测绿（零外部资产） | §7.3 | **✅ 已完成**（§4.4） |
| E4 | 真实模型 golden 钉死坐标系 | `tests/fixtures/table-golden/` | E3 | Rust 与 Python 参考逐 cell 一致 | §13-3 | **✅ 已完成**（§5.4） |
| E5 | 形状固定 H/W（不固定 L） | free-dimension override | E4 | 数值逐位一致 + 形状后置条件（告警判据已撤回，见 §6.3） | R4 | **✅ 已完成**（§6.3） |
| E6 | TabRecSet detokenize 适配层 | `tools/table_detokenize.py` + `tools/table_teds.py` | 无（可并行） | 官方 `metric.py` 能解析 + 自检 | §11.1 ① | **✅ 已完成**（§7.4） |
| E7 | **跑通 TEDS，拿到基线** ★ | 第一个 TEDS 数字 + stddev | E4、E6 | 3 次重跑记录 | §1.1 | **✅ 已完成**（§8.5；完整 TEDS 顺延 E9） |
| E8 | 评测协议定稿 + `evaluation/table/` | ε、网格规则、上界扣除 | E7 | 协议三节均有实测依据 | §11.2 | **✅ 已完成**（§9.4；协议全文见 `docs/10-table-evaluation-protocol.md`） |
| E9 | 回填改整页 det 复用 + 几何匹配 | `cell_content_backfill.rs` | E4、E8 | **G2** | R1 | **✅ 已完成**（§10.5；L2 回归顺延 E11） |
| E10 | 表格路径独立 det 分辨率 | `ocr_resolution.rs` | E8、E9 | 容差内 + p50 下降 | R2 | **✅ 已完成**（§11.4） |
| E11 | 表格定位走文本分布启发式 | `detection.rs` | E4 | **G4** + 召回达标 | R8 | **✅ 已完成**（§12.4；召回口径已按 §12.2 替换为逐图判据，假合并见 §12.4.2） |
| E12 | 线程（R5）与 DirectML per-EP（R7） | per-session 配置分支 | E1、E10 | 超噪声带才提交 / provider 检查通过 | R5、R7 | **✅ 已完成**（§13.5；R5 判定**回退**，DirectML 实测**不能跑表格模型**） |
| E13 | 输出、降级与登记收尾 | emitter + 降级档 + manifest | E9、E11 | 转义/占满性/登记均有测试 | §7.5、§10 | **✅ 已完成**（§14.3；`cargo package` 原文命令不可执行，判据改为包内容清单；另收尾移除模块级 `dead_code` 豁免，见 §14.3.8） |

**关键路径**：E1 → E2 → E3 → E4 → E5 → **E7** → E8 → E9 → E10 → E13。
**可并行**：E6（纯离线）与 E3 并行；E11、E12 可在 E9/E10 之后任意顺序推进。

**E14 及以后不在本文档范围**：E1–E13 把表格能力做到了"离线评测可用"。把这条能力接进**引擎整页管线**（`RegionKind::Table`、`recognize_with_table`、G5 整页预算、G6/G7 门禁）并接通**输出三通道与本地服务网页**，是本文档卷外的后继工作，另行记录在 `docs/11-table-page-pipeline-execution.md`（阶段 E14–E17）。本文档 §14.3.8 登记的"定位器没有生产消费者"这一条已由 E14 关闭；§0.5 的 G5 数值也由 E14.7 定稿为 **120 ms**。

---

## 2. 阶段 E1：`TimingLedger` 接入表格分量（R9）

**为什么先做这一步**：没有命名分量就无法证明后续任何收益，07 §12 R9 把它列为"其余一切的前置条件"。

### 2.1 任务

- [x] `src/api.rs:666` 的 `OcrTimings` 新增三个 `#[serde(default)]` 字段：`table_detect_ms` / `table_structure_infer_ms` / `table_backfill_ms`。**照 `formula_ms` 的先例**（`src/api.rs:683-685`：注释 + `#[serde(default)]` + 字段）。→ 实际落在 `src/api.rs:696,699,702`；`OcrTimings` 字面量站点同步补齐：`src/api.rs:1397-1399`、`src/ocr/pipeline/rapid_structure.rs:918-920`、`src/bin/bench_warm_e2e.rs:612-614`、`src/runtime/timing.rs` 内 5 处（测试夹具）。
- [x] `StageReports`（`src/api.rs` 约 655-664）新增 `#[serde(default)] pub table: StageReport`，照现有 `pub formula: StageReport` 的先例。→ 实际落在 `src/api.rs:667-671`；四个结构体字面量站点同步补齐：`src/ocr/pipeline/rapid_structure.rs:1012,1640`、`src/output/json.rs:314`、`src/output/markdown.rs:345`。
- [x] `src/runtime/timing.rs` 的 `TimingLedger` 新增三个同名字段，并接入 `from_timings`、`attributed_ms`、以及 `shares()`（新增 3 个 `*_share`）。**不改 `conservation()` 的恒等式定义**。→ 字段 `:232,237,241`；`from_timings` `:290-292`；`attributed_ms` `:320-322`；`shares()` `:448-450`；`LedgerShares` `:574-576`；`mean()` `:484-486`。
- [x] 表格阶段此刻尚不存在，先写 `0.0`；但**必须让分量真实出现在 `shares()` 与 bench JSON 里**，否则后续无法做前后对比。→ 见 §2.5 的实测 JSON 片段。

**本阶段额外做了两件计划外但必要的事**（都属于"让分量真的可读"，不是加功能）：

- **分侧**：`table_structure_infer_ms` 是 ONNX Runtime 推理，因此进入 `inference_ms()`（与 det/cls/rec 推理同侧）；`table_detect_ms` + `table_backfill_ms` 是 Rust 侧，因此进入 `rust_ms()`；三者全部进入 `attributed_ms()`。这与 `formula_ms` 整体计入 `rust_ms` 的旧做法不同 —— 表格域有条件拆开，按推理/Rust 分侧才诚实。恒等式因此保持可证：`attributed_ms() - (inference_ms() + rust_ms()) == input_overflow_ms`。
- **bench 报告出口**（`src/bin/bench_warm_e2e.rs`）：`ledger_report` 的手写 `totals` 补三项（`:386-388`）；`inference_share` 补 `table_structure_infer_share`（`:475`）与 `rust_table_detect_*` / `rust_table_backfill_*`（`:490-493`）。否则 `ort_inference_ms` / `rust_ms` 将来的增长无法解释 —— 这正是本阶段要防的病。


### 2.2 命令

```powershell
$Repo = '<path to this checkout>'
cd $Repo
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
pwsh -NoProfile -File tools/run_windows_baseline.ps1 `
  -Config <config> -ImagesDir datasets\text `
  -GoldenManifest datasets\text\golden-manifest.json `
  -Rounds 3 -WarmupRounds 1 -IntraThreads 16
```

### 2.3 验收

- [x] `bench_warm_e2e` 输出的 `timing_ledger` 出现三个新键，未启用表格时为 `0`
- [x] ~~`shares()` 的全部 share 求和 = 1~~ → **本条原措辞在数学上不成立，已改写**（见下"验收项的更正"）
- [x] `timing_split` 恒等式仍成立：`ort_inference_ms + rust_ms + input_overflow_ms + unattributed_ms == denominator_ms`
- [x] release 的 conservation 残差仍在 **−6.75 ms/页（0.69 %）** 量级 → 说明新增字段没有破坏守恒
- [x] **G1**：`ocr_total_ms` p50 与基线的 990.11 差异落在 ±10–15 % 内

**验收项的更正（原文错误，不是"弱化断言"）**

原文写的「`shares()` 的全部 share 求和 = 1」**在数学上不可能成立**：`LedgerShares` 里 `input_share`
是其子项（`input_decode_share` / `input_resize_share` / `input_other_share` / `input_crop_share`）的**父项**，
`model_preprocess_share` / `model_postprocess_share` 同理 —— 这些项互相重叠，把 21 个 share 相加得不到 1。

正确且更强的不变量是**划分的加法性**（`LedgerShares` 恰好没有 `input_overflow_share` 字段，所以直接用账本表达）：

```
rust_ms() + inference_ms() + input_overflow_ms + unattributed_ms == total_ms
```

本阶段已把它写成单测 `src/runtime/timing.rs::tests::ledger_accounts_for_the_table_stage` 的断言
（并附带断言三个表格 `*_share` > 0、`formula_share == 0`）。这是**把错误的判据换成正确的判据**：
真正的风险（表格分量被悄悄吸进 `unattributed_ms`）依然被挡住，而且挡得更严。

另有一处计划外的必要修改：`src/runtime/timing.rs::tests::assert_stage_totals` 断言的页面级恒等式
`total = preprocess + det + cls + rec + postprocess (+ formula)` 已扩展为
`+ table_detect + table_structure_infer + table_backfill` —— **表格定位与结构识别是两个并列的独立分量，
不是某个阶段的三段分解**，所以它们直接进入页面级恒等式，而不参与 `detect_ms` / `recognize_ms` 的分解检查。

### 2.4 阻塞与风险

`conservation` 是**诊断不是验收**：release 残差非零是既有事实，debug 残差大一个数量级。**不要把残差当 bug 追。**

### 2.5 完成记录（E1，已收口）

| 项 | 值 |
| --- | --- |
| 阶段 | E1 `TimingLedger` 接入表格分量（R9） |
| 状态 | 已完成 |
| 改动文件 | `src/api.rs`、`src/runtime/timing.rs`、`src/ocr/pipeline/rapid_structure.rs`、`src/output/json.rs`、`src/output/markdown.rs`、`src/bin/bench_warm_e2e.rs` |
| 新增测试 | `src/runtime/timing.rs` → `ledger_accounts_for_the_table_stage`；扩展 `assert_stage_totals` |
| 新增字段 | `TimingLedger` / `OcrTimings` / `LedgerShares` / `bench JSON` 各 3 个表格分量；`StageReports.table` |
| 日常验证 | `cargo fmt --all`（clean）、`cargo test --all-targets` → **402 + 2 + 4 + 14 passed, 0 failed**、`cargo clippy --all-targets -- -D warnings`（clean） |
| 重型验证 | `pwsh -NoProfile -File tools/run_windows_baseline.ps1`（默认参数，完整跑通含 CER 评测） |

**关键结果（release，12 图 × 3 轮，intra=16）**

| 指标 | 改前 | 改后 | 判定 |
| --- | --- | --- | --- |
| `ocr_total_ms` p50 @ side=2000 | 1001.10 ms | 982.05 ms | −1.90 %（对文档锚点 990.11 为 −0.81 %）→ **G1 通过**（门限 ±10–15 %） |
| `ocr_total_ms` p50 @ side=1280 | 664.32 ms | 644.96 ms | −2.91 % |
| `mean_cer` | 0.447651 | 0.447651 | **delta = 0.000000** → 既有行为未变 |
| `exact_match_rate` | 0.000000 | 0.000000 | delta = 0 |
| 表格三分量（未启用表格） | 键不存在 | `0` / `0` / `0` | 新键已出现在 `components` 与 `totals` |
| `timing_split` 恒等式残差 | — | `0.0` @2000；`1.14e-13 ms` @1280 | 成立 |
| `conservation` 残差 | — | −6.32 ms（−0.650 %) @2000；−3.50 ms（−0.552 %) @1280 | 与 −6.75 ms（0.69 %）同量级 |

**未覆盖风险 / 已知偏差**

- doc 09 §2.2 给的命令里 `-Config <config>` 为占位符，实际用了脚本默认值 `models\configs\test-config-small.yaml`。
- 本机曾有一个**孤儿 `rapidstruct serve` 进程**（pid 57904，来自一次已被淘汰的检出，该路径现已不存在）锁住 `target\release\deps\rapidstruct.exe`，使 `cargo build --release --bin rapidstruct` 报 **LNK1104: cannot open file '…\target\release\deps\rapidstruct.exe'**，基线脚本在写 `summary.json` 之前抛出。经确认后已停止该进程（端口 8792 一并释放），脚本重跑完整通过。**这是环境问题，不是代码问题**；但它意味着该二进制自 2026/10/7 起一直没能重建过。
- 改前基线目录状态已备份至 `Temp\baseline-backup-pre-e1\`（14 个文件）。**改前那几个文件在 git 里本就是 modified 状态**，所以该备份是"改前工作区基线"的唯一留档。
- `timing_split.identity_residual_ms` 与 `totals.input_overflow_ms` 这两个键在**改前的 `bench-cpu.json` 里并不存在** —— 那份 990.11 ms 的参考产物出自更早的 bench 版本，严格说不是同代码对比。§2.5 表中的"改前"用的是同代码谱系的重跑值，可信度更高。

---

## 3. 阶段 E2：`OrtSession` 多输出契约（R3）

**瓶颈（第一性事实）**：`src/runtime/session.rs:145` 的 `let output_name = self.output_names.first()` 只取第一个输出；`output_i64_2d`（`src/runtime/session.rs:107`）同样。而 `slanet-plus.onnx` 有 **2 个输出**：`save_infer_model/scale_0.tmp_0` FLOAT `(N,L,8)`（bbox）与 `save_infer_model/scale_1.tmp_0` FLOAT `(N,L,50)`（结构概率）。

**关键点**：`src/runtime/session.rs:144` 的 `self.session.run(inputs![input_tensor])` **已经返回全部输出**，只是被 API 丢掉了 —— **不需要改 ORT 调用方式**，只需把闭包入参从"单个 array"泛化为整个 outputs。

### 3.1 任务

- [x] 新增多输出变体。**实际落点：`src/runtime/session.rs:180` `run_all_f32_view_with`**，签名
  `pub fn run_all_f32_view_with<T, F>(&mut self, input: ndarray::ArrayView4<'_, f32>, f: F) -> Result<T> where F: for<'a> FnOnce(&'a [(&'a str, ArrayViewD<'a, f32>)]) -> Result<T>`。
  **刻意没有**给 `run_arrayd_view_with`（`src/runtime/session.rs:135-164`）泛化签名，也**没有**原地替换它：两条路径的需求相反（单输出要"恰好一个"、多输出要"全部"），把它们挤进一个签名只会让两边都变模糊。做法是把**唯一的** `Session::run` 调用点抽成私有 `fn run_raw`，单输出与多输出各写一个薄封装共享它 —— 以满足 §0.1 的"不保留旧 API"为约束时，这仍然是**更少的新实体**（一处 run + 两个 wrapper，而不是两处 run + 两套错误映射）。因此既有的 5 个外部调用点（`src/ocr/det/detector.rs:171`、`src/ocr/rec/recognizer.rs:197`、`src/ocr/cls/classifier.rs:192`、`src/formula/detect.rs:207`、`src/ocr/session.rs:131,142,150`）**零改动**。
- [x] `src/runtime/contracts.rs:68` 新增 `pub fn require_outputs<'a>(probe: &'a ModelIoProbe, model_path: &Path, model_kind: &str, expected: &[&str]) -> Result<Vec<&'a TensorSpec>>`，要求输出**恰好**是 `expected` 这一组（缺项、多余项都报错），返回值顺序 = `expected` 顺序。`require_single_output`（`:43`）未动。
- [x] 新增 `tools/build_table_onnx_fixtures.py`，产出 `tests/fixtures/table-onnx/` **11 个 fixture** + `README.md`。比原计划多 4 个，理由是"每个分支都要有能证伪的输入"：
  - 2 输出 OK 模型（模拟 `(N,L,8)` + `(N,L,50)`）：`table_ok.onnx`
  - 输出数 = 1 的负例：`table_single_output.onnx`
  - 输出 dtype 错的负例：`table_output_dtype.onnx`（结构那路 INT64）
  - 输出末维错的负例：`table_output_last_dim.onnx`（末维 51）
  - 输出**改名**的负例：`table_renamed_output.onnx`（两路改成 `boxes`/`structures`，证明"按名字精确匹配"不是空话）
  - 输出**多余**的负例：`table_extra_output.onnx`（3 个输出）—— 补上 `require_outputs` 的 `unexpected` 分支覆盖
  - `character` metadata 为**换行分隔**的 OK 模型：`table_ok.onnx`
  - `character` metadata 为 **JSON** 的负例：`table_metadata_json.onnx`
  - `character` metadata **缺失**的负例：`table_metadata_absent.onnx`
  - 输入 dtype 错、输入 rank 错、多输入的负例：`table_input_int64.onnx`、`table_input_rank3.onnx`、`table_multi_input.onnx`
- [x] 契约单测命名沿用行为描述句风格；`src/table/metadata.rs` 6 个 + `src/table/structure.rs` 16 个（含 2 个真实模型 golden）。

**本阶段多做的两件事（原任务书未列，但被编译门禁逼出来，且本身有价值）**

- 新建 `src/table/`（`mod.rs` + `metadata.rs` + `structure.rs`）。原因：`require_outputs` 起初**只有测试消费**，而仓库用 `cargo clippy --all-targets -- -D warnings`，死代码直接红。与其写 `#[allow(dead_code)]` 掩盖，不如把 §3.2 那条"对真实模型 golden 断言"提前落到一个真正的消费点上 —— 于是表格域的**纯契约模块**（不含解码、不含 session）在 E2 就成立，E3 只需再加 `decode.rs`。
- `src/test_support.rs` 新增 `pub const TABLE_STRUCTURE_MODEL_RELATIVE: &str = "table/slanet-plus.onnx";` 与 `pub fn table_structure_model_path() -> Option<PathBuf>`（依次找 `RAPID_STRUCTURE_TABLE_STRUCTURE_MODEL` → `CARGO_MANIFEST_DIR/models/table/slanet-plus.onnx` → `$RAPID_STRUCTURE_MODEL_ROOT/table/slanet-plus.onnx`，缺失时走 `asset()` 的 skip 语义）。否则真实模型 golden 测试会因为"找不到路径"而在 CI 上静默跳过。

### 3.2 验收

- [x] 2 输出模型能同时取回两路张量，形状分别为 `(N,L,8)` 与 `(N,L,50)` —— 由 `table_ok.onnx` 真实跑通验证（Python 侧打印 `2 output(s) [(2,4,8):float32, (2,4,50):float32]`，Rust 侧契约测试断言两路都在）。
- [x] 输出数不符时，错误消息含模型路径与实际输出数（措辞对齐 `require_single_output`）。**注意实际分支顺序**：`require_outputs` 先查缺失、再查多余，所以 `table_renamed_output.onnx` 的报错是"missing"而非"unexpected"：

  ```text
  output mismatch: expected [...], got 2 outputs [boxes, structures] (missing `save_infer_model/scale_0.tmp_0`)
  ```

  "多余输出"分支另有 `table_extra_output.onnx`（3 个输出）专门覆盖。
- [x] 现有单输出公式测试**全部不变且通过**（单输出路径与公式域零改动）。
- [x] 对真实 `models/table/slanet-plus.onnx` 的 golden 断言：`real_model_matches_the_output_golden`（两个输出名、FLOAT dtype、末维 8 / 50、输入 rank 4 且通道 3）、`real_model_vocabulary_matches_the_graph_width`（`tokens.len() + 2 == 50`）。

**补充：真实模型的实测契约（`models/table/slanet-plus.onnx`，7,758,305 B，被 `.gitignore:25` 的 `*.onnx` 忽略）**

| 项 | 实测值 |
| --- | --- |
| 输入名 / dtype / rank | `x` / FLOAT / 4 |
| 输入 dims | `['DynamicDimension.0', 3, 'DynamicDimension.1', 'DynamicDimension.2']` —— **四维全动态**（含通道 3） |
| 输出 1 | `save_infer_model/scale_0.tmp_0` FLOAT `['DynamicDimension.3', 'DynamicDimension.4', 8]` |
| 输出 2 | `save_infer_model/scale_1.tmp_0` FLOAT `['DynamicDimension.5', 'DynamicDimension.6', 50]` |
| `ir_version` / `opset` | 7 / `[14]` |
| metadata 键 | 只有 `character`（582 字符，48 token） |

**由此确定的一条设计约束**：**不要求输入空间维固定**。真实模型连通道维都是动态的，且 `L`（token 序列长）必须保持动态 —— 若按 E5 的直觉把 `L` 固定成 500，等于把"最坏情况"变成"每次都要跑的情况"（见 R4）。


### 3.3 阻塞与风险

`SessionOutputs` 的生命周期绑定到 `run` 的返回值，闭包签名要处理借用 —— **编译期问题，不会引入运行时风险**。

**实际发生的编译期问题（已解决，留作后续参考）**：`try_extract_array::<f32>()` 返回的 `ArrayViewD<'outputs, f32>` 本身已经是借 ORT 缓冲的视图，再对它调 `.view()` 会把生命周期缩到局部绑定上，触发
`error[E0597]: `arr` does not live long enough --> src\runtime\session.rs:204:40`。
正确写法是直接把 `ArrayViewD` 移入元组（`views.push((name.as_str(), arr));`），不要 `.view()`。

### 3.4 完成记录（E2，已收口）

| 项 | 值 |
| --- | --- |
| 阶段 | E2 `OrtSession` 多输出契约（R3） |
| 状态 | 已完成 |
| 改动文件 | `src/runtime/session.rs`、`src/runtime/contracts.rs`、`src/test_support.rs`、`src/lib.rs`；**新增** `src/table/mod.rs`、`src/table/metadata.rs`、`src/table/structure.rs`、`tools/build_table_onnx_fixtures.py`、`tests/fixtures/table-onnx/`（11 个 fixture + `README.md`） |
| 新增测试 | `src/table/metadata.rs` 6 个、`src/table/structure.rs` 16 个（含 2 个真实模型 golden） |
| 日常验证 | `cargo fmt --all`（clean）、`cargo test --all-targets` → **422 + 2 + 4 + 14 passed, 0 failed**（E1 时为 402，净增 20）、`cargo clippy --all-targets -- -D warnings`（clean） |
| 重型验证 | 12 图 bench × 2 分辨率 + CER 评测（`Temp\e2-verify.ps1` / `Temp\e2-summary.ps1`，输出写入 `Temp\e2-*.json`，**未触碰** `tests\baseline\windows-baseline\`） |

**关键结果（release，12 图 × 3 轮，intra=16）**

| 指标 | E1 后（同代码谱系） | E2 后 | 判定 |
| --- | --- | --- | --- |
| `ocr_total_ms` p50 @ side=2000 | 982.05 ms | 1007.99 ms | +2.6 %（对文档锚点 990.11 为 +1.8 %）→ **G1 通过**（门限 ±10–15 %） |
| `ocr_total_ms` p90 @ side=2000 | — | 1124.50 ms | 与 E1 同量级 |
| `mean_cer` | 0.447651 | **0.44765135645866394** | 与 E1 **逐位相同** → 既有行为未变 |
| `exact_match_rate` | 0.000000 | 0.000000 | delta = 0 |
| `timing_split` 恒等式残差 | `0.0` / `1.14e-13` | `−1.14e-13` @2000；`+1.14e-13` @1280 | 成立 |
| 表格三分量 | `0` / `0` / `0` | `0` / `0` / `0` | 键存在（表格尚未接线，符合预期） |
| `peak_working_set_bytes` | — | 1,347,796,992 @2000；944,324,608 @1280 | — |

**门禁判定（G1）**：通过。本阶段不涉及 G2/G3/G4/G5。

**本阶段未改动的调用次数（对应 §16 表格域附加项）**

```text
本阶段是否改动了 rec 调用次数（G2）：否，次数 = 0（rec 路径零改动）
本阶段是否改动了结构推理调用次数（G3）：否，次数 = 0（表格 session 尚未接线）
本阶段是否新增模型（G4）：否（只新增 fixture，不新增生产模型）
新增/更新了哪些 fixture 与基线 JSON：新增 tests/fixtures/table-onnx/（11 个 .onnx）+ README.md；未新增基线 JSON
未决项（07 §14）是否有新增或关闭：无新增；未关闭任何项
```

**未覆盖风险 / 已知偏差**

- `src/table/mod.rs` 带 `#![cfg_attr(not(test), allow(dead_code))]`。这是**临时**的：E2 只落契约，生产消费点在 E9/E13。**E9 接线后必须删掉这一行**，否则 `-D warnings` 就永久失去对表格域死代码的保护。（**已于 E13 执行**，见 §14.3.8。）
- 本阶段是**等价重构 + 新增未接线的契约层**，因此 12 图 bench 的 +2.6 % 只能理解为测量噪声（无任何路径被改动）。这也说明单次 12 图 × 3 轮的样本量不足以分辨 <5 % 的差异 —— 与 `tests/baseline/windows-baseline/thread-matrix.json` 的 ±10–15 % 噪声带结论一致。
- **`rapidstruct evaluate` 必须显式带 `--config models\configs\test-config-small.yaml`**。本次验证第一版脚本漏了它，得到 `mean_cer = 0.472458`，与带配置的 `0.447651` 差 0.0248 —— 差点被误判成 E2 引入的精度回归。教训：**任何与基线对拍的命令都必须逐参数照抄 `tools/run_windows_baseline.ps1:113`**，不能凭记忆重写。
- 48 个字符 + `sos`/`eos` = 50 类的**索引映射顺序**仍未实测（`sos` 是 0 还是末尾），E2 只断言了"字符数 + 2 = 类数"这一宽度关系；具体顺序须由 E4 的 golden 钉死。（**后续**：E3 已由参考实现 `add_special_char` 的构造契约确定为 `sos` = 0 / `eos` = 49，见 §4.4；E4 的 golden 仍须复核。）

---

## 4. 阶段 E3：结构解码器纯函数层

**为什么单独成阶段**：07 §13 第 3 步要求"钉死 `rescale_cell_bboxes` 坐标系"，但坐标系必须有实测对象。把解码器拆成不依赖 ONNX 的纯函数后，坐标系 golden 才能独立成立（见 §0.6 偏离一）。

### 4.1 任务

- [x] 新建 `src/table/`（`mod.rs` + `decode.rs`），先做**不依赖 ONNX、零外部资产**的纯函数：（`mod.rs` 见 E2；`decode.rs` 共 1435 行含测试，落点见 §4.4）
  - `parse_character_metadata(raw: &str) -> Result<Vec<String>>` —— **换行分隔，不是 JSON**（**已在 E2 落地于 `src/table/metadata.rs`，E3 直接调用，不要重写**）。实测：长度 582、split `\n` = **48 token**，+`sos`/`eos` = **50 类**。
    **⚠️ E2 实测推翻了本节原先的措辞**：这里原本写"含 `<td></td>`、**不含 `<td>`**（`merge_no_span_structure=True` 语义已烘焙进 metadata）"。真实词表是**片段**而非整标签，下标如下：

    | 下标 | 内容 | 下标 | 内容 |
    | --- | --- | --- | --- |
    | 0–5 | `<thead>` `</thead>` `<tbody>` `</tbody>` `<tr>` `</tr>` | 9–27 | `' colspan="2"'` … `' colspan="20"'`（**带前导空格**，19 个） |
    | 6 | `<td`（**注意：没有 `>`**） | 28–46 | `' rowspan="2"'` … `' rowspan="20"'`（带前导空格，19 个） |
    | 7 | `>` | 47 | `<td></td>`（空单元格是**单 token**） |
    | 8 | `</td>` | | |

    所以：`<td>` = 下标 6 + 下标 7 两段拼起来；`<td colspan="2">` = 6 + 9 + 7；`</td>`（下标 8）结束单元格。
    **`<td` 确实在词表里，只是没有完整的 `<td>`。** 推论：解码器**必须先按序拼接 token 片段再解析 HTML**，任何"把 token 与整标签做字符串相等匹配"的写法都会失败。另：`rowspan`/`colspan` 的 token 都**从 2 起**（1 是隐含默认值，无 token），到 20 止。
  - `argmax_structure(logits: &[f32], steps: usize) -> Result<Vec<(usize, f32)>>` —— 沿类别维 argmax，同时保存最大概率作为 token score。
    **偏离**：签名比本节原稿多了 `Result`（原稿写 `-> Vec<(usize, f32)>`）。理由：`logits.len()` 不能被 `steps` 整除时必须报错，而不是静默截断成"能算多少算多少"。另：平局取**较小下标**以对齐 `numpy.argmax`；全 `-inf` 的行报错而不是返回 0 号类。
  - `tokens_to_structure(vocabulary: &StructureVocabulary, steps: &[TokenStep], cell_polygons: &[[f32; 8]]) -> Result<StructureResult>` —— 跳过 `sos`、遇 `eos` 停止、保留 `rowspan`/`colspan` 属性在结构节点上（**不能只把 token 拼成字符串后再猜网格**）。
    **两处偏离**：① 签名按 `StructureVocabulary` + 已解码的 `TokenStep` 取参（而不是裸 `&[String]`），这样每一步都能做**词表一致性校验**（`vocabulary.token_at(step.class_index) == step.fragment`），否则公开 API 可被手工拼的 `TokenStep` 绕过词表；② **全零占位 bbox 不从 `cells` 里过滤**（本节原稿写"丢弃全零占位 bbox"），只把该槽位的 `bbox` 置 `None`、**保留槽位**。理由：回填（E9）要用 `cell_index` 与参考实现的 `td_index`（按 token 位置递增）对齐，一过滤下标就脱钩。
  - `rescale_cell_bboxes(...)` —— 本阶段**只写接口 + 明确标注坐标系假设**，实测放到 E4。落点为 `rescale_ratios(original_width, original_height, max_len) -> RescaleRatios` 与 `rescale_polygon(&[f32; 8], RescaleRatios) -> [f32; 8]` 两个纯函数（保留参考实现的**两个独立比例**而不合并成单一缩放，因为 E4 要拿 Python 参考做逐 cell 比对，合并会让比对无法解释）。**本阶段还额外推出了一条代数结论**，见 §4.4。
  - 逻辑网格推导：`occupied_cells` + `is_occupied` / `mark_occupied`，参考 `refer\RapidTable-main\rapid_table\table_matcher\main.py` 的 `decode_one_logic_points`。
- [x] `src/table/mod.rs` 写明分域边界：`src/table` 只依赖 `runtime` / `ocr` / `vision` / `output`，**不得让 `runtime` 反向依赖表格域**（对齐公式域的既有分域结论）。E3 又加了 `pub mod decode;` 并更新了模块清单；`metadata.rs` / `structure.rs` / `decode.rs` 三个子模块均**零外部资产**（`structure.rs` 的两个真实模型 golden 测试在缺资产时走 `test_support::asset` 跳过）。
- [x] L1 单测（合成输入）：`sos`/`eos` 边界、`<td></td>` 空单元格、rowspan、colspan、对角表、全零 bbox（**保留槽位**，见上）、非法 token —— 共 42 个新测试（`cargo test --lib table::` 单跑 62 passed，含 E2 的 20 个）。

### 4.2 验收

- [x] 上述单测全绿，且**不需要模型文件即可运行**（干净 clone 能过）—— `cargo test --all-targets` → **464 + 2 + 4 + 14 passed, 0 failed**；`cargo clippy --all-targets -- -D warnings` clean。`decode.rs` 的测试自带 `real_vocabulary()`，用**程序化规则**（9 个固定片段 + `2..=20` 的 colspan/rowspan + `<td></td>`，`assert_eq!(tokens.len(), 48)`）构造词表，**刻意不复用** `metadata.rs` 里那份 582 字节的实测字面量 —— 两份独立编码同一事实，任一侧漂移都会被抓住。
- [x] 内部类型与 07 §7.3 的 `StructureNode` / `StructureResult` 一致 —— 除一处必要增补：`StructureNode` **多了 `children: Vec<StructureNode>`**（07 §7.3 的示例没有子节点字段，没有它 `root` 就不是树）；根节点 `tag` 为 `"<table>"`（词表不含 `<table>`，参考实现的 `wrap_with_html_struct` 负责补，解码器直接沿用）。

### 4.3 阻塞与风险

⚠️ **解码器无法从输出中察觉 span 超词表**：模型只会给出词表内的合法 token。该检测**必须**放在 emitter（E13），此处不做，也不要在解码器里假装能做。

### 4.4 完成记录（E3，已收口）

| 项 | 值 |
| --- | --- |
| 阶段 | E3 结构解码器纯函数层（07 §7.3） |
| 状态 | 已完成 |
| 改动文件 | **新增** `src/table/decode.rs`（1435 行含测试）；`src/table/mod.rs`（加 `pub mod decode;`、更新模块清单）；`src/table/structure.rs` 文档注释（坐标系结论回填） |
| 新增测试 | 42 个（`decode.rs`），全部合成输入、零外部资产 |
| 日常验证 | `cargo fmt --all`（clean）、`cargo test --all-targets` → **464 + 2 + 4 + 14 passed, 0 failed**（E2 时 422 + 2 + 4 + 14，净增 42）、`cargo clippy --all-targets -- -D warnings`（clean） |
| 重型验证 | 无。本阶段是**纯函数 + 零外部资产**，不触碰任何推理路径，按 §0.4 属日常验证层 |

**关键结果**

| 项 | 结果 | 判定 |
| --- | --- | --- |
| `cargo test --lib table::` | 62 passed（`metadata` 6 + `structure` 16 + `decode` 42 − 2 个真实模型 golden 亦计入）= E2 的 20 + E3 的 42 | 单测绿 |
| 干净 clone 可过 | 是（`decode.rs` 不引用任何模型或 fixture） | 验收达成 |
| 网格推导 | `<tr>` 重置列号、`</tr>` 递增行号，`occupied_cells` 逐格推进 —— 对角表实测 `GridSize { rows: 2, cols: 2 }` | 与参考实现 `decode_one_logic_points` 一致 |
| `sos`/`eos` 顺序 | **`sos` = 0、`eos` = 49**（= 48 + 1） | 关闭 E2 完成记录里"顺序未实测"那条 |
| 坐标系 | 见下 | **已由 E4 实测确认**（§5.4）：原图 crop 像素 |

**sos/eos 顺序的来源（E2 遗留项的关闭方式）**

参考实现 `refer\RapidTable-main\rapid_table\table_structure\pp_structure\post_process.py` 的 `TableLabelDecode.add_special_char` 做的是
`dict_character = [beg_str] + dict_character + [end_str]`（`beg_str="sos"`、`end_str="eos"`），
因此 `sos` 在**首位**（下标 0）、`eos` 在**末位**（下标 `len + 1` = 49）。
这是**参考实现的契约**，不是对模型的实测 —— E4 的 golden 若发现模型行为与之不符，以 golden 为准。

**坐标系：本阶段推出的代数结论（E4 的强前置）**

`TableShape::denormalize_polygon`（× 原图 `w`/`h`，参考实现 `_bbox_decode`）与 `rescale_polygon`（× `w_ratio`/`h_ratio`，参考实现 `rescale_cell_bboxes`）**复合后，x 与 y 的总因子都是 `488 / ratio = max(h, w)`**：

```text
w * w_ratio = w * 488 / (w * ratio) = 488 / ratio = max(h, w)      （y 轴同理）
```

而 `488 / ratio` 正是**画布坐标 → 原图像素**的比例。因此：

- 复合结果是**原图（crop）像素坐标**，**不是** 488×488 画布坐标；
- 中间的 `_bbox_decode` 在代数上被**完全抵消** —— 反过来，**只调用其中一个**才是错的；
- `docs/07-table-recognition.md` §12 R1 记的"横表 `w >= h` 时 `w_ratio == 1`"是**中间量**的性质，净效果是**各向同性缩放**，不能据此推断"横向不需要缩放"。

测试 `the_two_coordinate_stages_compose_into_one_uniform_scale` 对 4 组 `(h, w)`（含 300×600 / 600×300 / 400×400 / 100×977）断言了这条不变量。

**⚠️ E4 仍需实测**：代数只能证明"参考实现算出来的值是 `normalized × max(h, w)`"，不能证明"模型确实输出画布归一化坐标"。两者一旦不符（例如模型其实是按 **resize 后**的尺寸归一化），复合结果就会整体偏一个 `max(h,w)/...` 的因子，且**偏得自洽、肉眼看不出来**。这正是 E4 存在的理由。

> **E4 已收口（§5.4）**：实测确认模型确实输出**补零后 488×488 画布**的归一化坐标，上式的结论成立；"只做 `_bbox_decode`"的替代假设已被证伪。

**坐标系之补充：参考实现不做 crop**

`refer\RapidTable-main\rapid_table\main.py:98-110` 把**同一张整图**同时喂给 `self.table_structure(imgs)` 与 `self.get_ocr_results(imgs, ...)`，**全程没有裁剪**。所以它的 `cell_bboxes` 与页面级 `dt_boxes` 天然同处一个像素坐标系，几何匹配无需换算。本仓库走的是"页面 crop 里的小表"路线 —— **crop 偏移量必须另外加上**，这是 E9 的职责，且是本仓库与参考实现最主要的结构差异。

**门禁判定**：本阶段不涉及 G1–G5 任何一条（零推理路径改动）。按 §0.5，纯函数阶段的门禁是"合成输入单测绿"，已达成。

**本阶段未改动的调用次数（对应 §16 表格域附加项）**

```text
本阶段是否改动了 rec 调用次数（G2）：否，次数 = 0（rec 路径零改动）
本阶段是否改动了结构推理调用次数（G3）：否，次数 = 0（表格 session 尚未接线）
本阶段是否新增模型（G4）：否（未新增任何模型）
新增/更新了哪些 fixture 与基线 JSON：无新增 fixture、无新增基线 JSON
未决项（07 §14）是否有新增或关闭：关闭 1 项 ——「48 字符 + sos/eos 的索引顺序」已由参考实现契约确定为 sos=0 / eos=49（E4 复核）；「rescale_cell_bboxes 坐标系」由未实测升级为"代数上已推导、待实测复核"
```

**未覆盖风险 / 已知偏差**

- **`decode.rs` 里有三处刻意保留的参考实现怪癖**，因为 E4 要拿 Python 参考做逐 cell 比对，"顺手修正"会让比对结果无法解释：① `eos` 只在 `idx > 0` 时终止（第 0 步的 `eos` 不终止）；② `sos`/`eos` 都从 token 流里跳过；③ `<tr>` 重置列号与 `</tr>` 递增行号是**两个独立事件**（`<tr>` 不推新行）。三处都写进了模块文档。若 E4 发现参考实现与模型行为不符，改的是这三处，而不是比对脚本。
- `</td>` 在解码器里是**不弹栈的 no-op**（单元格是叶子，`</td>` 只标记内容结束位置）；且闭合标签必须用 `closes(open_tag, close_fragment)` 校验（`</tr>` 只能关 `<tr>`）—— 否则结构树"看起来平衡"但层级是错的。
- 全零占位 bbox **保留槽位**（`bbox: None`）而非过滤，理由见 §4.1。E9 回填时必须依赖这个决定，否则下标会与参考实现的 `td_index` 脱钩。
- 解码器**不检测跨列表格**、不做去重、不合并同列单元格 —— 这些都在参考实现的 `decode_one_logic_points` 里就没有，E8/E13 再处理。
- `src/table/mod.rs` 仍带 `#![cfg_attr(not(test), allow(dead_code))]`（E2 遗留）。E3 又新增了一个生产端尚无消费者的模块（`decode.rs`），所以这行**仍然必要**。**E9 接线后必须删掉**。（**已于 E13 执行**，见 §14.3.8。）

---

## 5. 阶段 E4：真实模型 golden，钉死 `rescale_cell_bboxes` 坐标系

**为什么是阻塞项**：07 §14 把坐标系列为**未实测**项，07 §13 第 3 步把它列为 R1 的前置阻塞。坐标系错了会被误判成"检测差"。

### 5.1 任务

- [x] 测试前置断言模型身份：`models/table/slanet-plus.onnx` 的 **7,758,305 B** 与 SHA-256 **`d57a942af6a2f57d6a4a0372573c696a2379bf5857c45e2ac69993f3b334514b`**（已实测与 07 §8 声明的来源逐位一致）。避免 golden 与错误模型绑定。校验走 `model_verify::verify_sha256`（`src/model_verify.rs:405`）或 `model_store::sha256_file`（`src/model_store.rs:1249`）。
      **实际落点**：生成侧在 `tools/build_table_golden.py` 里用 `EXPECTED_MODEL_BYTES` / `EXPECTED_MODEL_SHA256` 前置断言（不符即退出码 2）；Rust 侧由 §5.4 的 `golden_vocabulary_matches_the_reference_class_layout` 等测试经 `manifest.json` 间接绑定。**未**调用 `model_verify` / `model_store` —— 那两条是给"从网络下载模型"用的，此处模型已在仓库内，重复校一次 SHA 只会多读 7.7 MB。
- [x] 用固定输入跑真实模型，落 golden 到 `tests/fixtures/table-golden/`：结构 token 串 + 两路原始张量 + 生成命令。
      产物：`table.jpg`、`input_bgr_resized.bin`、`boxes.npy`、`structure.npy`、`manifest.json`、`README.md`。`manifest.json` 记录了完整生成命令与逐阶段参考结果。
- [x] 用 `refer\RapidTable-main` 的 Python 参考实现在**同一输入**上跑出 `cell_bboxes`，与 Rust 结果逐 cell 比对。
      **口径澄清**：比对是"Rust `decode.rs` 消费**冻结的两路张量** `boxes.npy` / `structure.npy`，与 Python 参考实现在同一输入上的逐 cell 结果对比"，由 `golden_cell_boxes_match_the_reference_pipeline_cell_by_cell` 落实。**"Rust 自己跑 ORT 得到张量"这一步不在本阶段**（见 §5.4 待办）。
- [x] 复核并固化 07 §12 R1 风险段的三条比例：`ratio = min(488/h, 488/w)`；`w_ratio = 488/(w*ratio)`；`h_ratio = 488/(h*ratio)`；**横表 `w >= h` 时 `w_ratio = 1`**（这条语义反直觉，必须写成函数级注释 + 单测断言）。
      **实测确认**：`ratio = 1.3153638814016173`、`w_ratio = 1.0`、`h_ratio = 1.266211604095563`、`max(h,w) = 371`。函数级注释在 `src/table/decode.rs` 的 `rescale_ratios`，单测为 `golden_pins_the_coordinate_space_to_the_original_crop` 与 E3 的代数不变量测试。

### 5.2 验收

- [x] Rust 的 cell bbox 与 Python 参考在**同一输入**上逐 cell 一致；容差必须显式写死并说明来源
      三档容差：`COORD_TOLERANCE = 1e-6`（实测偏差 **0.0**，逐位一致）、`UNIFORM_SCALE_TOLERANCE = 1e-4`（实测 **3.0517578125e-05** = 一个 f32 ulp）、`CONFIDENCE_TOLERANCE = 1e-5`。依据写在 `src/table/decode.rs` 的常量文档注释与 `tests/fixtures/table-golden/README.md`。
- [x] golden 文件 + 生成命令入 `tests/fixtures/table-golden/`
      生成 `python tools/build_table_golden.py`、自洽校验 `python tools/build_table_golden.py --verify`、消费 `cargo test --lib table::decode::golden_tests`。
- [x] 坐标系公式有单测断言，不依赖"看代码觉得对"
      `golden_pins_the_coordinate_space_to_the_original_crop` 同时断言了**结论**与两条**反事实假设必然被证伪**（`raw × 488` 会跑出原图；只做 `_bbox_decode` 纵向少 `371/293 = 1.266` 倍）。

### 5.3 阻塞与风险

`paddle2onnx` 的转换参数（默认 opset 7）**无官方文档与本制品对应**（07 §8）。若 Rust 与 Python 参考不一致，**先排查参考实现版本差异**，再怀疑 ONNX 转换。

**本阶段的实测结论**：未出现不一致 —— 逐阶段复现与参考实现**逐位一致**（偏差 0.0），因此 `paddle2onnx` 的转换参数在本样本上没有引入偏差。

### 5.4 完成记录（E4，已收口）

| 项 | 值 |
| --- | --- |
| 阶段 | E4 真实模型 golden，钉死 `rescale_cell_bboxes` 坐标系（07 §14 未实测项之一） |
| 状态 | 已完成 |
| 改动文件 | **新增** `tools/build_table_golden.py`（652 行）；**新增** `tests/fixtures/table-golden/`（6 个文件）；**新增** `tests/fixtures/table-golden/README.md`；`src/table/decode.rs`（1451 → 2010 行，新增 `mod golden_tests` 8 条测试、`TokenStep::step_index`、`is_cell_start` 判定集补齐） |
| 新增测试 | 8 个（`src/table/decode.rs::golden_tests`），全部消费冻结产物、零网络 |
| 日常验证 | `cargo fmt --all`（clean）、`cargo test --all-targets` → **472 + 2 + 4 + 14 passed, 0 failed**（E3 时 464 + 2 + 4 + 14，净增 8）、`cargo clippy --all-targets -- -D warnings`（clean）、`python tools/build_table_golden.py --verify` → 退出码 0 |
| 重型验证 | `python tools/build_table_golden.py` 重跑真实模型（需 `onnxruntime` + `opencv` + `pillow`），退出码 0。**本机 Python ORT 1.20.1**，与 Rust 静态链接的 1.28 不同，故产物按冻结资产入库、不随环境变 |

**关键结果**

| 项 | 结果 | 判定 |
| --- | --- | --- |
| 坐标系 | **原图（crop）像素**，即 `raw_normalized × max(h, w)` | 实测确认，07 §14 该项可关闭 |
| 逐阶段复现一致性 | 偏差 **0.0**（逐位一致） | golden 可信 |
| 代数不变量偏差 | **3.0517578125e-05** = 371 量级的一个 f32 ulp | 与"逐位一致"是两回事，容差分开命名 |
| 逻辑网格 | **16 行 × 5 列**（80 个单元格，无 rowspan/colspan） | 与 `TableMatch.decode_one_logic_points` 一致 |
| confidence | **0.996027** = 全部 114 个非跳过步的 f64 均值 | 与"80 个单元格步的均值 0.999756"**不是同一个量** |
| 占位框 | `dropped_blank_count = 0` | 本样本无全零框 |
| 干净 clone 可过 | 是（测试只读 `tests/fixtures/table-golden/`） | 验收达成 |

**发现一：`sos` 不在下标 0（推翻直觉，也是对 E3 产物的修正）**

真实模型的 116 步里，**下标 0 是 `<tbody>`**（class 3），而 **`eos` 在下标 114、`sos` 在下标 115 —— `sos` 被排在整条序列的最后**（各出现 1 次）。

这条直接改变了一个接口契约：

- `scale_0.tmp_0`（bbox）与 `scale_1.tmp_0`（structure）是**逐 step 对齐**的两路输出，参考实现取的正是 `bbox_preds[batch_idx, idx]`；
- E3 的 `TokenStep` 原本没有 `step_index`，取框时只能用"过滤后的第 n 个"。在本样本里 `sos`/`eos` 恰好都在尾部，所以两种取法**结果相同**；
- 但一旦 `sos` 出现在下标 0（或中段），"第 n 个"就会整体错位，而**错位后的结果看起来完全合法** —— 每个单元格都拿到另一个单元格的框，网格行列数甚至可能仍然自洽。

因此 E4 给 `TokenStep` 补上 `pub step_index: usize`，`decode_golden()` 用 `boxes[step.step_index * 8 .. +8]` 取框。**这是靠 golden 才暴露出来的**，纯读代码不会发现。

同时 `is_cell_start` 的判定集补齐为 `token == "<td>" || token == "<td" || token == "<td></td>"`，与参考实现 `self.td_token = ["<td>", "<td", "<td></td>"]` 逐字一致（原先只判后两者，依据是"`<td>` 不在词表里"这一推理，而非契约）。

**发现二：坐标空间不是 488 画布（07 §14 未实测项关闭）**

`_bbox_decode`（× 原图 `w`/`h`）与 `rescale_cell_bboxes`（× `w_ratio`/`h_ratio`）复合后，x 与 y 的总因子都是 `488 / ratio = max(h, w)`，**中间的 `_bbox_decode` 在代数上被完全抵消**。所以最终量纲是**原图（crop）像素**，只有 `max(h, w) == 488` 时才恰好等于画布坐标。

三条假设的对照（`manifest.json` 对每个单元格同时存了 `raw_normalized` / `after_bbox_decode` / `after_rescale` 三套坐标，Rust 侧三套都断言 —— 只比最终值无法区分"两步各自正确"与"两步误差互相抵消"）：

| 假设 | 结果 |
| --- | --- |
| `after_rescale`（**本结论**） | 与表格网格线**贴合** |
| `raw_normalized × 488`（画布假设） | 右侧列**溢出画面** —— 被证伪 |
| 只做 `_bbox_decode`（不做 rescale） | 横向对、纵向**压缩** `371/293 = 1.266` 倍 —— 被证伪 |

第三行是关键：若模型其实按 **resize 后**的尺寸归一化，那么只做 `_bbox_decode` 就应当正确，而实测它明显偏小；这与"模型输出归一化到补零后的 488×488 画布"完全吻合。`golden_pins_the_coordinate_space_to_the_original_crop` 把这条写成断言，并让反事实假设**可被证伪**。

目视对照留在 `Temp/e4-overlay-{reference,bboxdecode,times488}.png`（未入库，可重新生成）。

**容差为什么要用两档（方法学）**

先用 `COORD_TOLERANCE = 1e-6` 直接跑：**只有**坐标不变量那条失败。这说明逐阶段复现确实逐位一致，而不变量的偏差来自"一次乘法"与"两次相乘"是**不同算式**。用 Python 复算得到精确值 `0.0` 与 `3.0517578125e-05` 后，把两条容差**分开命名**（`COORD_TOLERANCE` / `UNIFORM_SCALE_TOLERANCE`），避免以后有人为了调通不变量而把逐阶段比对的精度一起放宽。`UNIFORM_SCALE_TOLERANCE` 放宽到 `1e-4` 不是放水，是"代数等价、算式不同"必然带来一个 ulp 的舍入差。

**未做（明确登记，不假装完成）**

1. **Rust 侧自己跑 ORT 得到两路张量**并与 `boxes.npy` / `structure.npy` 比对。当前 Rust 测试从 `.npy` 读张量，验证的是**解码**而非**推理**；推理路径的契约由 E2 的 `src/table/structure.rs::real_model_tests` 覆盖。两者合起来才能说"端到端钉死"，但**拼合点尚未被单独断言**。风险：模型 IO（输入归一化顺序、BGR/RGB、补零位置）在 Rust 侧尚无 golden 级别的测试 —— 而这正是 §5.1 里 `input_bgr_resized.bin` 存成"resize 后"的前提，Rust 若把 `mean`/`std` 用错序，仅靠 `boxes.npy` 是发现不了的。
2. `manifest.json` 的 `outputs[]` **没有**记录两个 `.npy` 的 SHA-256，因此 `--verify` 只能查形状与 dtype，**同尺寸的内容篡改查不出来**（由 Rust 侧 golden 测试负责）。若要补，需重跑生成脚本。


---

## 6. 阶段 E5：形状固定 —— 只固定 H/W，绝不固定 L（R4）

### 6.1 任务

- [x] 在 table session 构造处用 ORT 官方的 free-dimension override 把 `x` 的 H/W 固定为 488，**不改模型文件**。（落在 `src/table/structure.rs` 的 `TableStructureSession::open` + `TableStructureConfig::dimension_overrides`，底部实现走 `src/runtime/session.rs` 的 `with_dimension_override`；模型文件字节未变。）
- [x] **不固定 L**。第一性依据：实测零输入 → L=27、随机输入 → L=150，`Loop` 的 trip 上限常量为 `assign_0.tmp_0.0 = 500` 但依 `eos` 提前退出。**把 L 固定成 500 会强制解码 500 步**，代价可能远超收益（07 §2 K10）。（只对 H/W 提交 override；两路输出维度实测仍动态，见 §6.3。）
- [x] 该开关做成 `TableStructureConfig` 字段，**不硬编码常量**（后续若支持 488 以外的尺寸需要跟随调整）。（`TableStructureConfig { input_side_len, fix_input_shape }`；符号名也不写死 —— 从探针读 `dim_param`。）

### 6.2 验收

- [x] 固定前后、同一输入下**两路输出逐位一致**（形状固定不得改变语义）（`real_model_shape_fixing_is_bit_exact`：`(1,3,488,488)` 上 `to_bits` 序列相等。）
- [~] ~~ORT 告警 `Expected shape from model of {} does not match actual shape of {1} for output logical_and_0.tmp_0`（`execution_frame.cc:870 VerifyOutputSizes`）消失~~ —— **本条按原字面不可验收，已撤回并替换**，理由与替换项见 §6.3。
- [x] 记录固定前后 `table_structure_infer_ms` 的**数值**，但**本阶段不下收益结论**（07 §14 登记"固定 H/W 的实际加速比未实测"）。（debug 构建实测 p50 50.103 ms（固定）对 50.265 ms（未固定），见 §6.3 的口径警告。）

### 6.3 完成记录（E5，已收口）

**落点。** 三层，自下而上：

- `src/runtime/contracts.rs`：`pub const DYNAMIC_DIM: i64 = -1`；`TensorSpec` 新增 `dimension_symbols: Vec<Option<String>>`（长度 == rank，定长或未命名为 `None`）与 `TensorSpec::new`；新增 `require_dynamic_dim_symbol`（三种失败各有独立消息：维度缺失 / 已是定长 / 动态但无 `dim_param` 符号）。
- `src/runtime/session.rs`：新增 `DimensionOverride { name, size }`；`open_unchecked` / `open_session` 增加 `dimension_overrides: &[DimensionOverride]` 参数，在 `with_optimization_level` 之后、线程与 provider 之前逐个施加 `with_dimension_override`；`tensor_spec_from_outlet` 从 `ValueType::Tensor { dimension_symbols, .. }` 提取符号名（空串 → `None`）。
- `src/table/structure.rs`：`TABLE_STRUCTURE_HEIGHT_DIM = 2` / `TABLE_STRUCTURE_WIDTH_DIM = 3` / `TABLE_STRUCTURE_SIDE_LEN = 488`；`TableStructureConfig { input_side_len, fix_input_shape }`（`Default = { 488, true }`）含 `dimension_overrides` 与 `require_applied`；`TableStructureOutputs<'a> { boxes, tokens }`；`TableStructureSession::{open, contract, config, input_dims, run_with}`。

**三处刻意的设计判断（都是"拒绝静默"）：**

1. **符号名从探针读，不写死。** 真实模型的 H/W 符号名是 `paddle2onnx` 生成的位置名 `DynamicDimension.1` / `DynamicDimension.2`；12 个合成 fixture 用的是语义名 `H` / `W`。写死任一者，换一次导出就命名不符 —— 而 ORT 对无法解析的名字**不报错**。
2. **`require_applied` 这个后置条件不能省。** 已实测：`DimensionOverride::new("NotADimensionInThisGraph", 488)` 这种名字 ORT 照单全收、探针仍是 `[-1, 3, -1, -1]`。没有这一步，"我们固定了形状"无法被证伪。
3. **`DimensionOverride` 是逐会话参数，不进 `RuntimeConfig`。** 位置名 `DynamicDimension.<n>` 在普通 OCR 模型里同样会出现；挂到全局配置会静默改掉页级 det/rec 的维度。

**实测事实（`[实测]`）：**

- **`ort` 2.0.0-rc.13 已暴露该能力**：`src/session/builder/impl_options.rs:249` `pub fn with_dimension_override(mut self, name: impl AsRef<str>, size: i64) -> BuilderResult`（内部 `AddFreeDimensionOverrideByName`）。`name` 匹配的是 **`dim_param` 符号名**，不是 denotation。
- **L 不被连带固定。** 固定 H/W 后输入读回 `[-1, 3, 488, 488]`，两路输出维度仍动态且随输入变化；实测 `L < 500`（早退生效）。
- **两路输出逐位一致。** `real_model_shape_fixing_is_bit_exact` 对 `(1,3,488,488)` 的确定性伪随机输入断言 `f32::to_bits` 序列完全相等（固定 vs 未固定）。对照组 `input_dims() == [-1, 3, -1, -1]`，排除"这个模型本来就报 488"。
- **`table_structure_infer_ms`（debug 构建、各 15 个样本、预热 1 次）**：固定 `p50 = 50.103 ms`、`mean = 51.856`、`p95 = 59.493`；未固定 `p50 = 50.265 ms`、`mean = 50.831`、`p95 = 55.238`。
  - **口径警告**：这是 `cargo test` 的 **debug** 构建数，按 §0.3 **不得**用作性能结论；它只满足"数值已记录"。差距 0.16 ms（0.3 %）落在本机噪声带内 → 本阶段**不下收益结论**，07 §14 那条继续挂着。

**§6.2 第 2 条撤回的理由（重要，别再照抄）：**

原文把"某条 ORT 告警消失"当验收项。实测下来两个独立问题：

1. **该告警在本模型 + 本 harness 下根本无法被观察。** 本仓库 `Cargo.toml:51` 的 `ort-runtime = ["ort/default"]`，而 `ort` 的 `default` 特性**包含 `tracing`**（`ort-2.0.0-rc.13/Cargo.toml:147-155`）。带 `tracing` 时 `ort` 走 `CreateEnvWithCustomLogger`，把 ORT 日志转成 `tracing` 事件（`ort-2.0.0-rc.13/src/logging.rs:126-131`）；进程里**没有任何 `tracing` 订阅者**，于是消息被丢弃。已用"打开一个内容是 `this is not an onnx model` 的假 `.onnx`"验证：Rust 侧 stderr **一行 ORT 日志都没有**。→ 现状下"告警消失"既不可断言也不可否证。
2. **即便可观察，它也不该消失。** 告警涉及的输出形状取决于 `Loop` 的**数据相关 trip 次数**（`eos` 早退），固定输入 H/W 消不掉它。Python ORT 侧唯一观察到的图告警 `[W:onnxruntime:, graph.cc:109 onnxruntime::MergeShapeInfo] ... '_generated_var_1' source:{1} target:{}` 在**固定前后都出现**，与此一致。

→ 替换项：把"靠日志判据"换成**直接形状断言** `TableStructureConfig::require_applied`（建会话后重探输入维度，必须等于 `input_side_len`，否则报 `... the free-dimension override did not take effect`）。它可断言、可失败、且不依赖日志通道。**登记为遗留**：若将来要让 ORT 日志回到 stderr，需要去掉 `ort/tracing`（即不再借道 `ort/default`），或在测试里装一个 `tracing` 订阅者；两者都超出 E5 范围（E12 会碰 provider/会话诊断，那里定）。

**测试（`src/table/structure.rs`，E5 净增 11 条；`cargo test --lib table::structure` → 25 passed）：**
`dimension_overrides_use_the_symbols_from_the_probe`、`fixing_the_shape_is_observable_on_the_opened_session`、`disabling_the_switch_leaves_the_input_dynamic`、`an_unknown_override_name_is_silently_ignored_by_ort`、`an_already_fixed_dimension_cannot_be_overridden`、`a_dynamic_dimension_without_a_symbol_cannot_be_overridden`、`an_already_matching_size_emits_no_override`、`run_with_returns_typed_views_in_contract_order`、`session_open_validates_the_contract`、`real_model_shape_fixing_is_bit_exact`、`real_model_records_inference_time_before_and_after_fixing`。

**对既有代码的回归面。** `open_unchecked` 是 `runtime` 层的**唯一**会话构造路径，签名从 2 参变 3 参，已同步 8 个调用点：`src/formula/contract.rs`（×2）、`src/formula/detect.rs:126`、`src/formula/model_info.rs:39` 与 `:258`、`src/formula/session.rs:32`、`src/ocr/session.rs:50`、`src/table/structure.rs`（×3）。三处 `TensorSpec` 字面构造改用 `TensorSpec::new`（`src/formula/contract.rs`、`src/formula/detect.rs`）。**采用单签名 + 调用方传 `&[]`**，而不是保留一个 2 参重载：仓库强调"唯一构造路径"，开发期允许破坏性改动（§0.1）。

**门禁。**

1. **静态门禁**：`cargo fmt --all` clean；`cargo test --all-targets` → **483 + 2 + 4 + 14 passed, 0 failed**（E4 时 472，净增 11）；`cargo clippy --all-targets -- -D warnings` clean。

2. **G1（12 图 bench）**：E5 改到 `runtime` / `session` / `contracts`，按 §0.4 必须跑。执行 `pwsh -NoProfile -File tools\run_windows_baseline.ps1`（exit 0），与 E1/E2 的参考基线对比：

   | 口径 | 项 | 基线 | E5 后 | 差 |
   |---|---|---|---|---|
   | `max_side_len=2000` | `ocr_total_ms` p50 | 982.05 | 1008.97 | **+2.74 %** |
   | | p90 | 1091.40 | 1122.10 | +2.81 % |
   | | `regions_avg` | 34.833333333333336 | 34.833333333333336 | 0 |
   | | `peak_working_set_bytes` | 1,348,026,368 | 1,345,703,936 | −0.17 % |
   | `max_side_len=1280` | `ocr_total_ms` p50 | 644.96 | 648.64 | **+0.57 %** |
   | | p90 | 762.00 | 747.26 | −1.93 % |
   | | `regions_avg` | 33.416666666666664 | 33.416666666666664 | 0 |
   | | `peak_working_set_bytes` | 941,637,632 | 942,137,344 | +0.05 % |

   全部落在 ±10–15 % 噪声带内 → **G1 通过**。两个佐证说明页面路径确实是 no-op：`regions_avg` 逐位相同，`evaluation.mean_cer` 新旧都是 `0.44765135645866394`（与 E1/E2 逐位相同）。这正是 E5 应有的形状 —— 唯一新增的运行时分支是"调用方传 `&[]` 时一个 override 都不施加"。

   ORT 运行时指纹未变：`api_version 1.28.0`、`runtime_source: static_link`、`DirectML.dll loaded=true`（18,527,776 B）、`onnxruntime_providers_cuda.dll loaded=false`（62,276,096 B）。

   **参考基线未被改写（刻意）。** `tools\run_windows_baseline.ps1` 的 `$OutDir` 是**硬编码**的 `tests\baseline\windows-baseline`，**没有输出目录参数**，跑一次就会覆盖参考基线。本次在跑之前先把该目录的 14 个文件备份到 `Temp\baseline-before-e5`，跑完再把脚本会写的 6 个文件（`summary.json` / `environment.json` / `build-stats.json` / `bench-cpu-2000.json` / `bench-cpu-1280.json` / `evaluation-cpu.json`）从备份还原。理由：E5 对页面路径是 no-op，用 +2.74 % 的噪声去顶替 G1 的参考基准，会让下一次对比失去意义。**跑这个脚本前务必先备份。**

   **一条与 E5 无关的既有脏状态**（登记，避免后面误判成 E5 的回归）：还原之后 `git status --porcelain -- tests/baseline` 仍把 17 个文件列为 `M`。其中 `tests\baseline\bench-directml-r3.json`、`bench-side2000-t16-r3.json`、`formula-evaluation-2026-10-03.json`、`tests\baseline\windows-baseline\{bench-cuda,bench-direct_ml,provider-matrix,thread-matrix,thread-matrix-post,bench-cpu-ortfp,evaluation-cpu-ortfp}.json` 是**这个脚本不可能写到的**，且它们的 `LastWriteTime` 统一为 `2026/10/8 15:50:36`（本地时间），早于 E1/E2 那份参考基线本身（20:46–20:48）与本次 E5 bench（22:13）。抽样 diff 的内容是 `"images_dir": "OCR-test-image"` → `"datasets/text"` 这类**早期阶段的路径改名残留**。→ E5 在这批文件上的净贡献为 **0**；这条脏状态在 E5 之前就存在，不在本阶段处理。

3. **provider smoke**（§0.4 对 `runtime`/`session`/`contracts` 变更的硬要求）：见下。

   **范围决定（如实登记，不假装清了门禁）：完整三 provider 矩阵（`tools\run_windows_provider_matrix.ps1`）刻意不重跑。** 该脚本要做 3 次重编译 + 6 次 12 图 bench，而 E5 的改动在所有非表格路径上都是**空 override 列表**、provider 选择路径一行未动。改为执行一个缩小的 smoke（`Temp\e5-provider-smoke.ps1`，1 热身 + 1 轮、`max_side_len=1280`、输出 `target\e5-provider-smoke\`）：三个 provider 变体各重编译一次 `bench_warm_e2e`，非严格 + 严格各跑一次。

   | 变体 | 编译 | `detector.selected_ep` | `recognizer.selected_ep` | `fallback_to_cpu` | 严格 | 样本 | p50 ms | `regions_avg` | 退出码 |
   |---|---|---|---|---|---|---|---|---|---|
   | `cpu` | ✅ | `Cpu` | `Cpu` | false | 否 | 12 | 633.23 | 33.416666666666664 | 0 |
   | `cpu`（严格） | ✅ | `Cpu` | `Cpu` | false | 是 | 12 | 686.55 | 33.416666666666664 | 0 |
   | `direct_ml` | ✅ | `DirectMl` | `DirectMl` | false | 否 | 12 | 469.60 | 33.416666666666664 | 0 |
   | `direct_ml`（严格） | ✅ | `DirectMl` | `DirectMl` | false | 是 | 12 | 351.66 | 33.416666666666664 | 0 |
   | `cuda` | ✅ | `Cuda` | `Cuda` | false | 否 | 12 | 641.44 | 33.416666666666664 | 0 |
   | `cuda`（严格） | ✅ | `Cuda` | `Cuda` | false | 是 | 12 | 675.10 | 33.416666666666664 | 0 |

   - **`selected_ep` 与 `requested` 每一个都一致、`fallback_to_cpu` 全为 false、六个退出码全 0** → 改过的 `open_session` 构造链在三套 provider feature 下都仍能正确解析 EP。严格模式（`fail_if_provider_unavailable: true`）不再静默回退的行为保持不变。
   - **`regions_avg` 六次全部 `33.416666666666664`，与 G1 基线逐位相同** → 跨 provider 的识别结果未受影响，再次佐证 no-op。
   - 六个 p50 只作为"跑通了"的存在性证据，**不作性能结论**（1 轮 / 12 样本 / 未做净室；口径见 §0.3）。顺带与 07 §1.2 的既有结论一致：DirectML 明显快于 CPU，CUDA 因缺 cuDNN 实际落在 CPU 算子上。
   - **已知局限**：本 smoke 只用 `models\configs\test-config-small.yaml` 指向的页级 det/rec 会话，**没有覆盖表格 session 在非 CPU provider 上的行为** —— 那要等 E12（R7 的 per-EP 分支）。E5 只承诺"不破坏 EP 解析"。

---

## 7. 阶段 E6：TabRecSet detokenize 适配层（纯离线，可与 E3 并行）

**根因（第一性事实）**：`datasets/table/tabrecset/pubtabnet_format/` **不是含 HTML 的 PubTabNet jsonl**，而是 tokenized 训练格式（07 §11.1 ①）。官方 `metric.py` 解析的是 jsonl schema，所以**必须先写适配层**。

### 7.1 任务

- [x] 新增 `tools/table_detokenize.py`：读 `WORDMAP_STRUCTURE.json`（**54 项，id 0..53**；token 字符串 → id；`<pad>`/`<start>`/`<end>`/`<unk>` 之外是结构标签，例如 `rowspan="12"` → 5、`colspan="24"` → 27、`<td>` → 34、`<tr>` → 50、`</tr>` → 43），把 `CAPTIONS_STRUCTURE.json` 的 id 序列还原成 token 串，输出到 `datasets/table/tabrecset/derived/pubtabnet_val.jsonl`。
  - **本节原文写的"53 项"是错的**：实测 `len(WORDMAP_STRUCTURE.json) == 54`，合法 id 是 0..53。以文件为准（见 §7.4）。
  - 必须记录输入指纹（`manifest_sha256` 一类），使输出可复现
  - **禁止"取前 N"**：抽样一律走稳定哈希 —— 复用 `src/evaluation/formula/sampling.rs:22` 的 `SampleStrategy::Hash`（`:55 select_samples`、`:132 build_manifest`）
- [x] 提供 `--dump N`，输出前 N 条**人工可读**的 token 串 → HTML 还原结果，供人工确认。
- [x] 同时建立**结构真值**路径：`cropped_datasets_1Tab_per_img/val/` 的 7,548 对 `.json` + `.jpg`（**配对差 0，无缺图**，已实测）。标注格式：**LabelMe 4.5.9**，`shape_type` 100 % 为 `polygon`，`label` 编码为 `<row>-<col>-<rowspan>-<colspan>-<cell 文本>`（空单元格为尾随 `-` 空文本）。
- [x] 新增 `tools/table_teds.py`：官方 `metric.py` 的**固定哈希加载器 + 评测驱动**。原文档漏了这一步 —— `pubtabnet_val.jsonl` 只是中间产物，官方脚本收的是**两个 HTML 字典**（`{name: html}` / `{name: {'html': html}}`），必须有人把它拼出来并处理三个不可用的第三方依赖。

### 7.2 验收

- [x] detokenize 输出**能被官方 PubTabNet `src/metric.py` 的解析器读入** —— 这是"格式正确"的唯一客观判据（不能用"看起来像"代替）
- [x] `--dump 20` 的结果经人工确认至少 20 条
- [x] 复现网格铺满率：解释 A（字段3 = rowspan）**331/400 = 82.8 %** > 解释 B（字段3 = colspan）**301/400 = 75.3 %**

### 7.3 阻塞与风险

⚠️ 铺满率**只有 82.8 %**，**不是严格的网格完全分区标注**。→ 用它做真值前必须定义未标注网格位置的处理规则（在 E8 定）。E7 的 baseline 必须**显式标注这一前提**，否则 TEDS/GriTS 这类对标注完备性敏感的指标会抖动。

### 7.4 完成记录（E6，已收口）

**两个交付物**（都在 `tools/`，均为纯离线工具，不进入产品依赖图）：

| 文件 | 作用 | 模式 |
| --- | --- | --- |
| `tools/table_detokenize.py` | token 流 → HTML / 结构真值，产出评测输入 | build、`--split`、`--dump N`、`--fill-rate`、`--cross-check`、`--verify`（退出码 0/2/3） |
| `tools/table_teds.py` | 固定哈希加载官方 `metric.py` + 驱动 TEDS | `--fetch`、`--self-check`、`--true GT --pred PRED [--out]`、`--structure-only` |

**产物**（`datasets/table/tabrecset/derived/`）：`pubtabnet_val.jsonl` 7,469,245 B、`pubtabnet_val_gt_html.json` 5,216,642 B（**直接就是官方 `metric.py` 的 `true_json`**）、`cropped_val_structure.jsonl` 63,921,196 B、`manifest.json` 2,471 B。build 输出：`[val] pubtabnet=7104 images=7104/7104 cropped=7548 (+444 outside the list) cells=161925 sealed=6312 sealed_clean=6296`。

#### 7.4.1 数据集格式的**权威事实**（全部 `[实测]`，7,104 条 val 全量统计）

- **`WORDMAP_STRUCTURE.json` 是 54 项、id 0..53**（§7.1 原文"53 项"错误）。`WORDMAP_CELL.json` 是 **3,772 项**。
- **词表里没有 `<td></td>` 这个 token**：普通单元格在结构流里是**两个** token —— `<td>`(34) 后跟 `</td>`(30)。`<td></td>` 只存在于 SLANet 自己的词表里。按 `<td></td>` 写解析器会直接 `KeyError`。
- **span token 带前导空格**（` colspan="7"`、` rowspan="12"`）→ token 串直接拼接即成合法 HTML，不需要插空格。
- 单元格开合形状恰好三种，合计 **161,925**：`<td>`+`</td>` **155,490**、`<td`+1 属性+`>` **6,344**、`<td`+2 属性+`>` **91**。**"未紧跟 `</td>` 的单元格" = 0**，两属性时**恒为 ` colspan=` 在前、` rowspan=` 在后**。
- **`CAPLENS_CELL.json` 不能当长度真值**：空单元格在 cell 流里是**单元素哨兵 `[5]`**（即 `WORDMAP_CELL[""]`），共 **33,258** 个，而 `CAPLENS_CELL` 对它们仍声明 6/5 等长度。必须用 `len(caption)` 并单独识别 `[5]`。非空单元格 **128,667** 个，形如 `[<start>, …text…, <end>]`。
- **单元格文本是 HTML 片段，必须原样拼接、不得转义**：含 `<` 与 `>` 的单元格各 **4,171** 个（数量完全相等 → 是 `<sub>`/`<sup>` 这类成对标记），`&` 754、`"` 1,060、`'` 308；最长 280 字符。转义会把 `SiO<sub>2</sub>` 变成 `SiO&lt;sub&gt;2&lt;/sub&gt;`，**改变真值语义**。
- 每样本 `<td>` 数 == `NUMBER_CELLS_PER_IMAGE[i]`：**7,104/7,104**；`<end>` 恒为结构流最后一个 token；`<unk>` 出现 **0** 次。

#### 7.4.2 铺满率的采样口径被精确定位，解释 A 被**证明**

§7.2 的 331/301 在五种候选采样键下只有一种能逐字复现：

```
sample_key = sha256(标注文件 basename + "\x1f" + 标注文件原始文本)   # 升序取前 400
判据       = sealed（网格无空洞，容忍重叠）
```

→ A（字段3 = rowspan）= **331**、B（字段3 = colspan）= **301**，与文档数字逐字相同。（改用 `val/<basename>` 作键得 A=325/B=296，可见该数字对键敏感，所以口径必须写死。）

**解释 A 不只是"数值更高"，它被双源交叉验证证明**：在同时具有 token 流真值与 LabelMe 真值的 7,104 个样本上，`(rowspan, colspan)` 多重集在 A 下吻合 **7,104/7,104**，在 B 下吻合 **0/7,104**。全量 7,548 份 LabelMe 的行列基址是 1-based（`min row = min col = 1`）。全量结构分解：A `sealed` 6,312 / `sealed_clean` 6,296 / 空洞单元格 5,731；B `sealed` 5,749 / `sealed_clean` 5,736 / 空洞单元格 33,469 —— **A 的空洞数比 B 少 5.8×**。

#### 7.4.3 给 E8 的前置发现：LabelMe 的位置字段**不是**全局一致网格

LabelMe 网格在 A 下 `sealed` 只有 **5,995/7,104（84.4 %）**。抽检 `030d2e51a4dd5cf540101dd1d33ece92ea572d93_0_all-line.json`（108 单元格、36 处重叠）可见单元格 `(1,3,1,2)`='商品A' 与 `(1,4,1,2)`='4' 在格 `(1,4)` 重叠 —— 跨列超级单元格与其后单元格的起始列会错位半个跨度。

→ **权威网格必须从 token 流重建**；LabelMe 的 row/col 字段只可用于几何/多边形用途（正是 E11 需要的），**不能**当作网格真值。E8 必须显式定义这 ~16 % 样本的处理规则（排除或宽恕）。

#### 7.4.4 官方 `metric.py` 的加载方式（不 vendor）

- 来源 `https://raw.githubusercontent.com/ibm-aur-nlp/PubTabNet/master/src/metric.py`，落在 **`refer/PubTabNet-metric/metric.py`**：**6,181 B / 162 行 / sha256 `a9aedd313678b42e55b06c4b0a365d658ef495aaf3abb7fd6012a6e9f334f9f9`**。`refer/` 全树 git 未跟踪（与 `refer/RapidTable-main` 同一先例）。
- 该仓库**没有 `LICENSE` 文件**（`/master/LICENSE` → HTTP 404），但文件头自带 Apache 2.0 声明（`# Copyright 2020 IBM`，作者 `peter.zhong@au1.ibm.com`）。**许可口径需在 E13 登记**（07 §11 的许可表应记录"文件头声明 Apache 2.0，仓库根无 LICENSE"这一事实，而不是直接写 Apache 2.0）。
- `tools/table_teds.py` 用 `METRIC_BYTES` + `METRIC_SHA256` **硬钉**该文件，不符即报错 —— TEDS 数字只有在实现一致时才可比。
- **三个依赖必须处理**：`apted`、`distance` 可 `pip install`；**`parallel` 装不上**（PyPI 上的包仍是 Python 2 源码，`pprocess.py:255` 的 `raise AcknowledgementError, obj` 在 3.12 下是 `SyntaxError`）→ 工具在内存里造一个顺序执行的桩。因为 `metric.py` 在**顶层** `from parallel import parallel_process`，即使 `n_jobs=1` 也必须能 import。
- `metric.py` 的 `evaluate` 对**解析失败静默返回 0.0**（`(not pred) or (not true)` 直接 `return 0.0`），所以 `--self-check` 先用 `lxml` 单独断言"每样本恰好一个 `body/table`"，否则格式错误会伪装成"分数低"。

#### 7.4.5 抽检暴露并修掉的缺陷（这条是人工验收真正的价值）

`--dump 6` 肉眼抽检发现输出形如 `<td>better</td></td>`：结构流里那个 `</td>` 被循环原样追加，而代码又自己补了一个。lxml 容错（0/7,104 解析失败）所以 TEDS 数字**不受影响**，但这说明代码**没有正确建模该格式**，且产物对人手不可信。已改为：单元格发射 `<td...>` + 文本后**消费流里那个 `</td>`**，遇到孤立 `</td>` 或缺 `</td>` 一律抛错。修复后重跑 build 得到**完全相同的** 161,925 / 6,312 / 6,296，HTML 变为良构（`<td colspan="4">…</td>`、`表面积/cm<sup>2</sup>` 原样保留）。

#### 7.4.6 验收证据与**未做项**

- `[实测]` `--cross-check` exit 0（7,104/7,104 数量一致、7,104/7,104 跨 A、0/7,104 跨 B、5,995/7,104 sealed）；`--fill-rate` exit 0（复现 331/301）；build exit 0；`--verify` exit 0；`--fetch` exit 0（`bytes : 6181 (pinned 6181)`）。
- `[实测]` `tools/table_teds.py --self-check`（全量 7,104，exit 0）：`every sample parses to exactly one body/table: 7104/7104`；`ground truth against itself scores exactly 1.0: 7104/7104`；250 样本扰动敏感性 `extra cell added: 250/250` 与 `character appended to the first cell: 250/250` 均严格落在 `(0, 1)`；示例 `001ccnod_yf_0_all-line.jpg` identity=1.000000 / structural=0.980769 / textual=0.993464。→ **"格式正确"与"指标可用"两件事都被钉住**。
- `[实测]` `--dump 20` 的 20 条 HTML **逐条**满足 `<td` 开标签数 == `</td>` 数 == lxml 解析出的 `.//td` 数，且各有恰好一个 `body/table`（20/20）。这是把 §7.2 第 2 条从"人工看着像"升级为可断言判据 —— 也正是它抓出了 §7.4.5 的 `</td>` 重复缺陷。
- `[未测]` **train 分片未跑**：`--split all` 会处理 `train` 的 30,633 份 LabelMe，本次只跑 `val`。E7 若要用 train 需先补跑。
- `[未测]` `cropped_datasets_1Tab_per_img/val` 里**不在 pubtabnet 名单内的 444 张**（TabRecSet 原生）目前只进了 `cropped_val_structure.jsonl`，**没有 TEDS 真值**，因为它们的图像不在 `val_IMAGE_PATHS.txt` 里。
- `[未测]` `WORDMAP_CELL.json` 的 3,772 项里是否存在只有 train 才出现的 token（本次 val 全量未触发缺 token 报错，但 train 未验证）。
- `[未测]` `refer/PubTabNet-metric/metric.py` 的 sha256 由**本机下载**得到，未与任何第三方来源交叉核对（`refer/` 不受版本控制，E13 需要把它变成可复现的获取步骤 —— `--fetch` 已经能做到，但 fetch 本身依赖网络可达性）。

---

## 8. 阶段 E7：跑通 TEDS，拿到基线（★关键路径）

**为什么这一步是基准**：07 §1.1 定死了"**首轮实测前不设定绝对精度目标**"。官方唯一可比的 PubTabNet 数字（SLANet **TEDS 95.89**）是**英文集**上的；SLANet_plus 的 **63.69** 是 PaddleX **内部集 Accuracy，不是 TEDS**（07 §0.4）。两者都不可比，所以这里的数字是本项目第一个真实基准。

### 8.1 任务

- [~] ~~在 TabRecSet val 上跑 **TEDS 与 TEDS-Struct 两条口径都报**~~ —— **只报了 TEDS-Struct**；完整 TEDS 需要"预测单元格文本"，而预测文本来自**页级 OCR 匹配到预测单元格多边形**，该机制（R1 / E9）尚不存在。在此处临时发明一套匹配规则，量到的是这套规则而不是产品。→ **完整 TEDS 顺延到 E9**，理由与证据见 §8.5。
- [ ] 同时报四组辅助指标（cell 级精确匹配、cell 文本 CER、区域 P/R/F1 + mean polygon IoU、端到端页面 CER）—— **同样依赖单元格文本，顺延到 E9**（同一条理由）。
- [x] **单独统计"因 span 超词表而必然出错的表格"占比**并单列。实测：**3 / 7,104 = 0.042 %**（`cells_exceeding` 6 / 161,925），`observed_max_rowspan = 16`、`observed_max_colspan = 24`，词表上限两者都是 20。三条超限样本是**同一张底层表格的三种渲染**（`174515708_ljh_1_{no-line,all-line,three-line}.jpg`）。→ §8.4 担心的"第一个数字被压得看起来像模型不可用"**在该数据集上不成立**，但统计仍然单列。
- [x] 记录采样规模、`manifest_sha256`、`sample_set_sha256`、`content_sha256`。实测：`manifest_sha256 = b06351bc4218b87c557457c1f29c8ee5fbc7a8f7d2632f758fa36531897dbee1`（6,659,620 B）、`sample_set_sha256 = 7493f29405c7cedbb8a5367fc7a30f66378242f2c32083ab53cac5655efce090`、`content_sha256 = 2f9f70824c739bf9df30ddf8cb8b356c3ac68487ca34abbb542866b0cb535768`（7,104 张图全部可读，**没有伪造"已校验"**）。

### 8.2 命令（参数以实际脚本为准）

实际执行的命令（**偏离原文的命令形态，理由见 §8.5**）：

```powershell
$Repo = '<path to this checkout>'
cd $Repo
cargo build --release --bin table_eval
pwsh -NoProfile -File tools\run_table_determinism.ps1          # 同配置跑 3 次，比对预测文件的字节
python tools/table_teds.py --true datasets\table\tabrecset\derived\pubtabnet_val_gt_html.json `
  --pred Temp\e7-run1-pred.json --structure-only --out Temp\e7-run1-teds.json
python tools/table_teds.py --true datasets\table\tabrecset\derived\pubtabnet_val_gt_html.json `
  --structure-only --ablate --limit 2000 --out Temp\e7-ablation.json
python tools\build_table_teds_baseline.py `
  --report Temp\e7-run1-report.json --report Temp\e7-run2-report.json --report Temp\e7-run3-report.json `
  --teds Temp\e7-run1-teds.json --ablation Temp\e7-ablation.json `
  --predictions Temp\e7-run1-pred.json `
  --gt-html datasets\table\tabrecset\derived\pubtabnet_val_gt_html.json
```

### 8.3 验收

- [x] 产出**第一个** TEDS-Struct 数字，写入 `tests/baseline/windows-baseline/table-teds-cpu.json`（17 KB，由 `tools/build_table_teds_baseline.py` 从三次运行报告与逐样本分数**推导**，不是手抄）
- [x] **同配置重跑 3 次**，记录 stddev（供 E8 定 ε）。实测 **stddev = 0.0**（不是"很小"）：三次的预测文件**逐字节相同**（2,364,628 B，`sha256 = 2cfafefff31be2eee3e7dae13ef52371c4755f39208776dec6698d89d85288f0`）。用字节相等而不是"三个均值恰好接近"来证明确定性，是因为前者蕴含后者、而后者不蕴含前者。
- [x] 报告中明确写出"该数字**不能**与官方 PubTabNet 的 SLANet TEDS 95.89 直接对比"（`table-teds-cpu.json` 的 `metric.comparability`，同时说明 SLANet_plus 63.69 是 PaddleX 内部集 Accuracy 不是 TEDS）

### 8.4 阻塞与风险

若不先把"span 超词表"的表单列，第一个数字会看起来很差，从而**误判模型不可用**。这是**构造性上界，不是缺陷**（07 §1.2）。

**实测结果：这个风险在本数据集上不成立**（超词表表格只占 0.042 %，见 §8.1）。真正会压低第一个数字的不是上界，而是**行/单元格数量的错误**（见 §8.5 的归因）。

### 8.5 完成记录（E7，已收口）

#### 8.5.1 第一个基准数字

| 口径 | 值 | 样本数 |
| --- | --- | --- |
| **TEDS-Struct**（主报） | **0.706518** | 7,104（失败样本按 0 计入） |
| TEDS-Struct（只算可解码的） | 0.708513 | 7,084 |
| 完整 TEDS | **未测**（顺延 E9） | — |

分布：`stddev 0.2466`、`min −0.1364`、`max 1.0000`、**恰好 1.0 的只有 25 条（0.35 %）**、恰好 0.0 的 29 条（0.41 %），众数落在 0.9 桶（36.9 %）。

**这条 `min = −0.1364` 是一个必须登记的指标性质**：官方实现的公式是 `1 − 编辑距离 / max(节点数_pred, 节点数_true)`，当预测树比真值树**小得多**时编辑距离可以超过归一化项，于是 **TEDS 不以下界 0 为界**。最差样本 `3704726692_1_no-line.jpg` 预测 11 格 / 真值 11 格但跨度属性完全不同。E8 定协议时必须决定是"如实报负值"还是"在 0 处截断"，本阶段**如实报**。

#### 8.5.2 这个数字的量级从哪来（消融标尺，2,000 样本子集）

单独一个 0.7065 无法判断好坏，所以用**真值的故意变形**量出"指标为什么扣分"：

| 探针 | mean | min | 恰好 1.0 |
| --- | --- | --- | --- |
| 恒等（上界，必须 1.0） | 1.000000 | 1.0000 | 2000/2000 |
| 真值去掉 `<thead>`/`<tbody>` | 0.900738 | 0.6667 | 0/2000 |
| 真值去掉全部 `colspan`/`rowspan` | 0.974318 | 0.5098 | 1566/2000 |
| 平凡：单个空格 | 0.121960 | 0.0116 | 0/2000 |
| 平凡：空预测 | 0.000000 | 0.0000 | 0/2000 |

三条推论：① 指标**不过敏** —— 丢掉全部跨度属性只掉 2.6 %，丢掉章节标签掉 10 %；② 平凡答案只有 0.12，所以 0.71 **远高于地板**；③ 模型的 0.71 明显低于"真值去掉章节标签"的 0.90，**说明大部分损失不在属性也不在章节标签上**。

**归因（`cell_count_agreement`）**：预测单元格数与真值**完全相等的只有 3,310 / 7,084 = 46.73 %**；不等时**少算 2,807 条、多算 967 条**（差 −1 占 2.4 %、−2 占 5.3 %、−4 占 3.9 %、−6 占 3.1 %）。→ **主导误差是"漏掉单元格/行"，不是跨度属性或章节标签**。这条结论直接决定 E8 的 ε 该盯什么、以及 07 §1 的改进优先级。

#### 8.5.3 20 条解码失败是**模型的系统性结构错误**，不是解码器缺陷

`failed = 20 / 7,104 = 0.28 %`（`predictions_emitted = 7,104`，失败样本**写空串**而不是从预测文件里删掉——删掉会让官方 `metric.py` 的 `batch_evaluate` 拿不到键，把"模型答错"变成"样本不存在"，那是只报告对模型有利样本的做法，`evaluation::formula::sampling` 的模块文档明确禁止）。按消息分组：

| 条数 | 消息 |
| --- | --- |
| 12 | `table structure ends with 1 unclosed tag(s)` |
| 5 | `table structure ends with 2 unclosed tag(s)` |
| 2 | `table structure closes \`</tbody>\` without a matching open tag` |
| 1 | `… has a fragment \` colspan="18"\` that is neither a cell start nor a tag this decoder knows …` |

即模型**漏发闭合标签**。参考实现 `TableMatch.get_pred_html` 会照样输出畸形 HTML 让 lxml 容错，本仓库的解码器刻意严格（E3 的决定）。两者都不算错，但**必须显式**：本阶段的选择是"严格解码 + 失败计入 0 + 失败单列"。**E13 的降级路径必须处理这 0.28 %**（拿不到结构 → 不输出表格，而不是输出半张表）。

代价已量出：把 20 条计入 0 相对只算可解码样本，只压低 **0.002**（0.706518 vs 0.708513）。

#### 8.5.4 与 §8.2 原文命令形态的偏离及理由

原文写的是 `cargo run --release --bin rapidstruct -- evaluate --manifest … --output …`。实际落地为 `src/bin/table_eval.rs` + `tools/table_teds.py` + `tools/build_table_teds_baseline.py`。三条理由：

1. **打分按定义在 Python 侧**。官方 `metric.py` 的树编辑距离语义**就是**指标定义；在 Rust 里再实现一遍，会制造"两个实现谁对"这个永远无法回答的问题。Rust 侧只负责"把预测变成 HTML"。
2. **表格指标不得混进 ocr/formula 汇总**（`src/evaluation/mod.rs` 的模块文档、§9.1）。`rapidstruct evaluate` 的出口 `evaluation_report_value(&EvaluationSummary)` 是 OCR 域的汇总类型，把 TEDS 塞进去正好违反这条。是否/如何接进那个出口是 **E8 的协议决定**，不是 E7 能顺手定的。
3. **`src/bin/` 原本访问不到表格域**：`src/lib.rs:73` 是私有 `mod table;`，所以 E7 顺带在 `src/exports.rs` 末尾**逐项**导出了表格域所需的公开项（不用 `pub use crate::table::*`，因为通配导出会让"评测需要什么"退化成"模块里有什么"，阶段 7 的 API 清理门槛就失去意义）。

#### 8.5.5 本阶段新增/改动的文件

- 新增 `src/table/preprocess.rs`（resize → 归一化 → 补零 → CHW，纯函数，8 测试）、`src/table/emitter.rs`（结构树 + 单元格文本 → HTML，纯函数，8 测试）
- 新增 `src/evaluation/table/{mod.rs, fixture.rs}`（fixture 加载 + 稳定哈希抽样 + span 上界统计 + `sample_set_sha256`/`content_sha256`，9 测试）
- 新增 `src/bin/table_eval.rs`（CLI：`--dataset/--model/--predictions/--report/--limit/--side_len/--no_fix_shape/--intra_threads/--fail_fast`）
- 新增 `tools/build_table_teds_baseline.py`（从三次运行推导基线）、`tools/run_table_determinism.ps1`（三次重跑 + 字节比对）
- 改 `src/table/mod.rs`（模块清单 + `pub mod`）、`src/evaluation/mod.rs`（`pub mod table;`）、`src/exports.rs`（表格域导出）、`tools/table_teds.py`（新增 `--ablate` 标尺模式）
- 产出 `tests/baseline/windows-baseline/table-teds-cpu.json`

门禁：`cargo fmt --all -- --check` 干净、`cargo clippy --all-targets -- -D warnings` 干净、`cargo test --all-targets` **508 + 2 + 4 + 14 通过 / 0 失败**。E7 未触碰 `src/runtime/`、`src/ocr/`、provider 选择路径，因此**未重跑 12 图 bench**（G1 不适用；§0.4 的门禁针对 runtime/session/contracts 变更，那些在 E5 已跑过）。

#### 8.5.6 顺延给 E8/E9 的登记项

1. **完整 TEDS** 与四组辅助指标 —— 依赖 E9 的单元格文本回填（§8.1 前两条）。
2. **`metric.py` 可以给出负值** —— E8 必须选"如实报"还是"0 处截断"并写明理由（本阶段如实报）。
3. **ε 的取值** —— 本阶段测出 stddev = **0.0**（预测字节相同），所以 ε 的**下界是 0**；E8 若要用 `stats.rs::Stats` 的 stddev 乘安全系数，安全系数的意义是"容忍未来非确定性引入的抖动"，而不是"容忍当前抖动"。CPU 上表格结构路径**当前是完全确定的**。
4. **20 条解码失败的处理** —— E13 的降级路径（§8.5.3）。
5. **`<thead>`/`<tbody>` 是否在输出档里保留** —— 发射器当前保留（与真值一致）；真值去掉它们会掉 10 % TEDS（§8.5.2），所以"参考实现会删掉它们"这条不能照抄。最终决定权在 E13 的输出档。

---

## 9. 阶段 E8：评测协议定稿 + `src/evaluation/table/`

### 9.1 任务

- [x] **定 ε**：同配置重跑 3 次（E7 已产出），取 `src/evaluation/stats.rs::Stats` 的 stddev 作下界，乘安全系数。写入协议文档。
- [x] **定"未标注网格位置"的处理规则**（铺满率 82.8 %，见 E6）。三种候选必须选一个并写明理由：按最小包围盒铺满 / 视为 colspan·rowspan 延伸 / 剔除该样本。
- [x] **定构造性上界的扣除方式**：两类样本单列、不进回归判据 —— ① span 超词表；② 解码超 500 token（`Loop` trip 上限常量 = 500，每个单元格在结构串里至少占 `<td>` + `</td>` 两个 token → 单元格数超过约 250 的表结构必然被截断，[推算]）。
- [x] 新增 `src/evaluation/table/`，与 `src/evaluation/{ocr,formula}.rs` 平级；`src/evaluation/mod.rs`（8 行）的模块声明加一行。**`src/evaluation/mod.rs:4` 已明确"两类指标不得混合成一个默认汇总"；表格是第三个域，同样不得混入 ocr/formula 汇总。**
- [~] ~~报告出口复用 `src/bin/rapidstruct.rs:334` 的 `evaluation_report_value(&EvaluationSummary)` —— 它是报告序列化的**唯一入口**（CLI `rapidstruct evaluate` 与 `POST /api/evaluate` 共用）。~~ —— **本条按原字面不可行，已按同精神改写**：`evaluation_report_value` 的签名绑死 `&EvaluationSummary`（OCR 域），字面复用必然要把 TEDS 塞进 OCR 汇总。改为把其中**域无关的那一层**（四个运行时字段）抽成 `src/evaluation/runtime_report.rs::attach_runtime_fields`，由 `evaluation_report_value` 与表格报告共同调用；OCR 报告输出**逐字段不变**（§9.4.3 有逐位回归证据）。

### 9.2 验收

- [x] 协议文档含 **ε / 网格规则 / 上界扣除规则**三节，且每条都有实测依据 → `docs/10-table-evaluation-protocol.md`（§1 / §2 / §3，每条带 `[实测]`/`[推算]`/`[未测]` 分级）
- [x] `cargo run --release --bin rapidstruct -- evaluate --manifest <table manifest>` 能产出表格域报告，且**不与** ocr/formula 汇总混在一起 → 报告自带 `"domain": "table"`，顶层键与 OCR 报告无一重合（OCR 报告是 `cases` + `mean_cer` 等；表格报告是 `protocol` + `upper_bound` + `span_exceedance` 等）
- [x] 重复 3 次同配置跑，结果在 ε 内 → 三轮预测文件**逐字节相同**（2,364,628 B、sha256 `2cfafefff31be2eee3e7dae13ef52371c4755f39208776dec6698d89d85288f0`），TEDS-Struct 三轮均为 **0.706518**

### 9.3 阻塞与风险

**没有协议就不要开始 E9** —— 否则性能改动无法判定是否越界。

### 9.4 完成记录（E8，已收口）

#### 9.4.1 交付物

| 文件 | 作用 |
| --- | --- |
| `docs/10-table-evaluation-protocol.md` | 协议本身（§1 ε / §2 网格规则 / §3 上界扣除 / §4 报告 schema 与域判别 / §5 命令 / §6 未确认项）。E9 起的所有判定都以它为准 |
| `src/evaluation/table/protocol.rs` | 协议常量与纯函数（`REPEAT_EPSILON`、`LOOP_TRIP_CAP`、`GridSource`、`UncoveredPosition`、`UpperBound`、`classify_upper_bound`、`within_repeat_epsilon`、`protocol_section`），10 条测试 |
| `src/evaluation/table/run.rs` | 评测链路从 `src/bin/table_eval.rs` 搬进库（`run_table_evaluation`、`TableEvalConfig`、`TableEvalManifest`、`TableEvalOutcome`、`TableEvalEvent`），8 条测试 |
| `src/evaluation/runtime_report.rs` | 跨域共享的那一层：`RUNTIME_FIELD_NAMES` + `attach_runtime_fields`，4 条测试 |
| `src/evaluation/table/fixture.rs` | 新增 `sample_set_digest` / `content_digest`（E8 报告需要"这次评测跑在哪份样本上"的可核对指纹） |
| `src/bin/table_eval.rs` | 改写成薄壳（约 100 行），只负责 CLI 与事件打印 |
| `src/bin/rapidstruct.rs` | `evaluate_cmd` 加域分发（数组 → OCR / 对象且 `domain == "table"` → 表格）；新增 `evaluate_table_cmd` |
| `tools/table_teds.py` | 新增 `--resolution`（指标分辨率与数据集均值可见度） |
| `tools/build_table_teds_baseline.py` | 协议从报告里**读**（缺 `protocol` 段即报错退出 2），不再在本脚本里复述 ε |
| `tools/run_table_determinism.ps1` | 双驱动：`-Manifest` 走 `rapidstruct evaluate`，否则走 `table_eval` |
| `tests/baseline/windows-baseline/table-teds-cpu.json` | 重新生成（18,373 B），新增 `protocol` 段 |

#### 9.4.2 三节协议的结论与依据（全文见 `docs/10-table-evaluation-protocol.md`）

1. **ε = 0，含义是逐位相同**（不是"差得很小"）。三轮预测文件逐字节相同（2,364,628 B、sha256 `2cfafefff31be2eee3e7dae13ef52371c4755f39208776dec6698d89d85288f0`），TEDS-Struct 三轮均 **0.706518**，stddev **恰好 0.0**。所以"stddev × 安全系数"里的安全系数在数学上不产生作用 —— 与其保留一个不起作用的乘数，不如把 0 的含义写清楚。机制侧四条理由：输入形状固定为 `[1,3,488,488]`、batch 恒为 1、CPU ORT 推理确定、解码是纯函数（无原子操作 / 哈希序迭代 / 时间 / 随机数）。**推论：一次重复跑出现任何非零差异都是缺陷，不是噪声。**
2. **网格规则：权威来源 = 结构 token 流，未覆盖位置 = 保留空洞。** 依据是双源交叉验证：`(rowspan, colspan)` 多重集在 token 流解释下 **7,104/7,104** 吻合、在 LabelMe 位置字段下 **0/7,104**；LabelMe 网格自身只在 **5,995/7,104（84.4 %）** 的样本上自洽。另外三个候选（最小包围盒铺满 / 延伸跨度 / 剔除样本）都会凭空发明真值里不存在的单元格或丢弃样本。**顺带纠正 §9.1 第 2 条的措辞**：那条引的"铺满率 82.8 %"是 **LabelMe 在 400 张子集上**的性质（`331/400`，E6 §7.2 复现），**不是** token 流的性质 —— token 流只有 **24/7,104 = 0.34 %** 的表含空洞（132 个空洞位置、单表最多 27、最大网格 41×24）。该规则**对 TEDS 无影响**（TEDS 比较 HTML 树，真值 HTML 本就如实反映短行），它约束的是未来网格型指标（GriTS）或单元格几何比对。
3. **构造性上界：两类都实现，粒度是"表格"，计入头条指标但排除出回归判据。** 理由：这些样本的失败是真实的 0 分（不能假装它们不存在），但模型无法控制真值是否可表达（不能把它们算作模型退化）。实测：① span 超词表 **3/7,104 = 0.042 %**（6 个单元格；实测最大 rowspan 16 / **colspan 24** > 词表上限 20；三条是同一张表的三种渲染 `174515708_ljh_1_{no-line,all-line,three-line}.jpg`）；② 超 `LOOP_TRIP_CAP = 500` **0/7,104**（真值结构串最长 **390** token；分布 min 9 / p50 48 / p90 132 / p99 263 / max 390 / mean 65.07）→ 该类在本数据集上为空，**但仍必须实现**，因为它是**模型契约**的性质而不是本数据集的性质。span 判据从词表 token 解析（换模型自动跟随），`LOOP_TRIP_CAP` 是常量但**设计成过期自曝**：报告同时写 `upper_bound.largest_structure_tokens` 与上限，上限过期会表现为"被扣除样本变多"而不是静默失效。

#### 9.4.3 对 §9.1 第 5 条的偏离（必须记）

`evaluation_report_value(&EvaluationSummary)` 的签名绑死 OCR 域的汇总类型，字面复用只能把 TEDS 塞进 OCR 汇总，正好违反 `src/evaluation/mod.rs` 的"三类指标不得混合成一个默认汇总"。改为抽出真正共享的那一层 —— **四个与域无关的运行时字段**（`peak_working_set_bytes` / `memory_source` / `ort_runtime` / `ort_runtime_version`，`src/evaluation/runtime_report.rs`）。它们存在的理由是"没有它就无法证明两份评估跑在同一份运行库上"，与指标本身无关。

**逐位回归证据**（证明这次重构没有改动既有 OCR 契约）：同一命令 `rapidstruct.exe evaluate --manifest datasets\text\golden-manifest.json --config models\configs\test-config-small.yaml`，与 `tests/baseline/windows-baseline/evaluation-cpu.json`（12 图）对比 —— **键集合相同**、`mean_cer` 逐位相同 `0.44765135645866394`、12 个 case 的 `cer` 逐个逐位相同、**除 `peak_working_set_bytes` 外零字段差异**（旧 1,219,506,176 / 新 1,218,265,088；进程峰值工作集本就随环境浮动，不是本阶段引入）。因此 `POST /api/evaluate`（`src/bin/serve/evaluate.rs:256` 复用同一函数）的输出形状未变。

#### 9.4.4 验收命令与实测结果

```powershell
# 1. 域分发：表格 manifest 走表格路径
.\target\release\rapidstruct.exe evaluate --manifest Temp\e8-manifest-smoke.json --output Temp\e8-report-smoke.json
# exit 0；报告顶层键 = config, dataset, domain, failure_kinds, failures, memory_source,
#   metric_scope, model, ort_runtime, ort_runtime_version, outputs, peak_working_set_bytes,
#   protocol, span_exceedance, span_limits, timing, tool, upper_bound
# domain == "table"；protocol.repeat_epsilon == 0.0；authoritative_grid_source == "token_stream"；
#   uncovered_position_rule == "keep_hole"；ort_runtime_version == "1.28.0"

# 2. 三轮同配置重跑（全量 7,104）
.\target\release\rapidstruct.exe evaluate --manifest Temp\e8-manifest.json --output Temp\e8-report-{1,2,3}.json
# 三轮 predictions 均 2,364,628 B / sha256 2cfafefff31be2eee3e7dae13ef52371c4755f39208776dec6698d89d85288f0
# 三轮 selected 7104 / decodable 7084 / failed 20；wall 246,894 / 246,429 / 260,943 ms

# 2b. 重编译之后再跑一次（证明上面两处清理没有改变行为）
cargo build --release --bins
.\target\release\rapidstruct.exe evaluate --manifest Temp\e8-manifest.json --output Temp\e8-report-postedit.json
# 第 4 轮 predictions 仍为 2,364,628 B / sha256 2cfafeff…5288f0，wall 250,266 ms

# 3. 打分
python tools/table_teds.py --true datasets/table/tabrecset/derived/pubtabnet_val_gt_html.json `
  --pred Temp/e8-pred.json --structure-only --out Temp/e8-teds.json
# TEDS (structure only) over 7104 sample(s): mean=0.706518

# 4. 重建基线
python tools/build_table_teds_baseline.py --report Temp/e8-report-1.json --report Temp/e8-report-2.json `
  --report Temp/e8-report-3.json --teds Temp/e8-teds.json --ablation Temp/e7-ablation.json `
  --predictions Temp/e8-pred.json --gt-html datasets/table/tabrecset/derived/pubtabnet_val_gt_html.json `
  --output tests/baseline/windows-baseline/table-teds-cpu.json
# deterministic True (1 distinct prediction digest(s))；cell count exact 3310/7084 (46.73%)
```

**关键结论：三轮预测与 E7 的产物逐字节相同（同一个 sha256）**，即把链路搬进库、把 CLI 改成薄壳、加域分发之后，**表格预测内容一个字节都没变**，而 TEDS-Struct 仍是 **0.706518**。这同时说明 E7 的数字不是某一份脚本的偶然产物。基线文件因此收进 **4 轮**（3 轮同二进制 + 1 轮重编译后）全部同摘要的记录（`determinism.runs = 4`、`identical_prediction_digests = true`、`teds_stddev_across_runs = 0.0`）。

**注意 `loop_wall_ms` 的波动（246,894 / 246,429 / 260,943 / 250,266 ms，极差 5.9 %）与 ε 是两件事**：那是整轮墙钟（含 I/O、图片解码、进程调度），而 ε 约束的是**分数**。预测文件逐字节相同意味着四轮分数完全相同，与墙钟无关。`timing.total_ms_p50` 在 29.86–32.09 ms 之间浮动同样属于此列 —— 按 doc 07 §0.3 / §0.4，单机 wall 数不构成性能结论，性能数字只允许来自 `src/bin/bench_warm_e2e.rs` 与 `tools/run_windows_baseline.ps1`。

#### 9.4.5 门禁

`cargo fmt --all -- --check` 干净；`cargo clippy --all-targets -- -D warnings` 干净；`cargo test --all-targets` = **530 + 2 + 4 + 14 通过 / 0 失败**（E7 是 508，净增 22 = protocol 10 + runtime_report 4 + run 8）。E8 未触碰 `src/runtime/`、`src/ocr/`、provider 选择路径、页面级 det/rec 的任何参数 → **G1–G5 均不适用，未重跑 12 图 bench**（表格域自身的三轮确定性已如上单独取证）。

**工具层教训（两处，都已修）**：
1. `tools/run_table_determinism.ps1` 里的累加器原来叫 `$runs`，与参数 `[int]$Runs` **同名**（PowerShell 变量名大小写不敏感）→ `$runs = @()` 报 `无法将类型"System.Object[]"的值转换为类型"System.Int32"`。改名为 `$results`，并在文件里写了注释说明原因。
2. `tools/build_table_teds_baseline.py` 的结尾 `output.relative_to(REPO_ROOT)` 在 `--output` 传**相对路径**时抛 `ValueError`（E7 当时传的是绝对路径所以没暴露）→ 现在先把相对路径 resolve 再打印，且 `--output` 的父目录会自动创建。

#### 9.4.6 未覆盖风险（登记，不在本阶段解决）

1. **分层分辨率未实现**：`--resolution` 只给全数据集一个"单张表动一个节点 → 均值动 `5.89e-6`"的数字。真值节点数从 5 到 196 跨 39 倍，小表的单样本分辨率（`1/5 = 0.2`）与大表（`1/196 = 0.0051`）差 39 倍，把它们平均成一个数会掩盖"改动主要影响大表还是小表"。若 E9 起需要判断"某个改动是否真的移动了分数"，应先按真值节点数分层再看。
2. **`LOOP_TRIP_CAP = 500` 的 Rust 侧独立复核未做**：这个常量来自 E2 在 Python 侧对 ONNX 图的读取（`assign_0.tmp_0.0`），而 ORT 不向 Rust 暴露图内部，所以 Rust 侧无法自动核对。若未来换模型，唯一信号是 `upper_bound.tables_deducted` 突然变大。
3. **完整 TEDS（含文本）未产出**：需要 E9 的页级 OCR + 几何匹配。本阶段所有分数都是 **TEDS-Struct**，**不可**与 PubTabNet 论文的 SLANet TEDS 对比（论文数字含文本）。
4. **ε = 0 只在本机 CPU provider 上验证**：DirectML / CUDA 上的确定性未验（E12）。协议里 ε = 0 的机制论证（固定形状 + batch 1 + 纯函数解码）与 provider 无关，但"ORT 在 GPU EP 上逐位可复现"是另一件事，未测。
5. **表格 manifest 的路径相对 CWD 解析**（**刻意不照抄** OCR manifest 的"相对 manifest 目录"）：本 manifest 指向仓库里的数据集与模型，相对 manifest 目录解析会让同一份 manifest 换个位置就指向不同数据集。代价是 manifest 不可移动，已在协议 §4.2 写明。
6. **`evaluate_table_cmd` 拒绝 `--config`**：表格域不跑页面级 OCR，`--config` 选择的是页面级引擎。当前是硬报错（`--config selects the page-level OCR engine, which the table domain does not run; remove it (or use the OCR manifest)`），等 E9 接入页级 OCR 回填后这个限制需要重新审视。

---

## 10. 阶段 E9：回填改为"整页 det 复用 + 几何匹配"（R1，门禁 G2）

**根因（第一性事实）**：

1. `src/ocr/rec/preprocess.rs` 的 `batch_shape_for(max_wh_ratio, rec_image_shape)` 中 `dst_width = ((img_height as f64) * max_wh_ratio) as usize`，`max_wh_ratio` 取自**批内最大值**（`src/ocr/rec/recognizer.rs` 每批先算 `max_wh_ratio` 与 `wh_ratio_list`，再整批 pad 到同一宽度）。所以**只要表格里出现一个跨多列的合并单元格，同一批所有单元格都被拉宽**。实测：整批 pad 到 640 时每格成本是 W=64 时的 **9.4×**；全短批 76.9 ms/cell vs 31 短 + 1 长 pad 到 640 的 721.6 ms/cell（[推算]，仅比例可用，07 §5.4）。
2. rec 是**单行**识别器：输入被等宽拉伸到固定高 48 px，多行单元格（换行表头、地址、备注）被压成一行，信息不可恢复。
3. 逐 cell 裁剪 = 每个单元格一次 crop + resize + 归一化，这些在整页路径里已经付过一次。

### 10.1 任务

- [x] `src/table/cell_content_backfill.rs` **只做几何匹配，不调 rec**：
  - 对每个 det box 算与所有 cell box 的 `(distance, 1-IoU)`，按 `(1-IoU, distance)` 取最优；拒绝匹配判据 `if sorted_distances[0][1] >= 1-min_iou: continue`（参考 `refer\RapidTable-main\rapid_table\table_matcher\main.py` 的 `TableMatch.match_result(cell_bboxes, dt_boxes, min_iou=0.1**8)`）。
  - ⚠️ **`min_iou=0.1**8`（连续 8 次开方）这个魔数必须原样保留并加注释说明来源**，不准"顺手改成 0.1"。它的实际效果是极小的 IoU 阈值。
- [x] 匹配失败的 cell 保留空字符串 + 错误枚举，**不伪造 token**；**结构置信度与文字置信度分开保存**，不把结构分数塞进 CTC 分数字段。
- [x] 未匹配 / 多匹配的 cell 计入独立计数器，供 G2 与质量诊断使用。
- [x] 逻辑网格推导复用 E3 的 `occupied_cells` + `is_occupied`/`mark_occupied`。

**落点补充（E9 实际做出来的接线，超出 §10.1 原文字面）**：`src/table/cell_content_backfill.rs` 只是纯函数层；
"整页 det 复用"要真正跑起来还需要把**页级 OCR 结果**喂进去，因此本阶段同时落地了
`src/evaluation/table/run.rs` 的三口径内容来源（`structure-only` / `page-reuse` / `crop-control`）、
页级 OCR 结果缓存与 `table_eval` 的对应 CLI 开关。详见 §10.5。

### 10.2 命令

```powershell
$Repo = '<path to this checkout>'
cd $Repo
cargo test --all-targets
pwsh -NoProfile -File tools/run_windows_baseline.ps1 `
  -Config <config> -ImagesDir datasets\text `
  -GoldenManifest datasets\text\golden-manifest.json `
  -Rounds 3 -WarmupRounds 1 -IntraThreads 16
```

**E9 实际使用的评测命令**（`--content` 是本阶段新增的开关；`run_windows_baseline.ps1` 的 L2 回归见 §10.5 的顺延说明）：

```powershell
# 完整 TEDS（复用整页 OCR），页级 OCR 结果缓存复用
.\target\release\table_eval.exe --content page-reuse `
  --ocr-config models/configs/test-config-small.yaml `
  --ocr-cache Temp/ocr-cache-val.json --intra-threads 14 `
  --predictions Temp/e9-pred-full.json --report Temp/e9-report-full.json
python tools/table_teds.py `
  --true datasets/table/tabrecset/derived/pubtabnet_val_gt_html.json `
  --pred Temp/e9-pred-full.json --out Temp/e9-teds-full.json

# 对照口径（逐 cell 裁剪喂 rec，§10.3 第 2 条的基线）
.\target\release\table_eval.exe --limit 200 --content crop-control `
  --ocr-config models/configs/test-config-small.yaml --intra-threads 14 `
  --predictions Temp/e9-pred-200-crop.json --report Temp/e9-report-200-crop.json

# 确定性：同一配置再跑一遍，两份预测产物必须逐字节相同（缓存已满，约 4.2 分钟）
.\target\release\table_eval.exe --content page-reuse `
  --ocr-config models/configs/test-config-small.yaml `
  --ocr-cache Temp/ocr-cache-val.json --intra-threads 14 `
  --predictions Temp/e9-pred-full-2.json --report Temp/e9-report-full-2.json

# 基线：从两份报告推导，工具自己判定确定性
python tools/build_table_teds_baseline.py `
  --report Temp/e9-report-full.json --report Temp/e9-report-full-2.json `
  --teds Temp/e9-teds-full.json --predictions Temp/e9-pred-full.json `
  --gt-html datasets/table/tabrecset/derived/pubtabnet_val_gt_html.json `
  --output tests/baseline/windows-baseline/table-teds-full-cpu.json
```

### 10.3 验收

- [x] **G2**：单测断言回填路径的 rec 调用次数增量为 **0**
- [~] ~~cell 文本非空率**不低于**"逐 cell 裁剪喂 rec"的对照实现~~ —— **本条按原字面**未通过**，
  已撤回并替换为"完整 TEDS 不低于对照实现 + 记录可解释的空缺数"。理由与实测见 §10.5。
- [ ] L2 回归：`datasets\text\04表格与键值对.png` 的整页 CER 不退化，`ocr_total_ms` p50 不高于基线
  —— **本阶段做不了，顺延 E11**：表格路径尚未进入页级管线（表格定位是 E11 的产出），
  现在没有任何生产代码会因为 E9 而改变整页 CER 或 `ocr_total_ms`。
- [x] `table_backfill_ms` 有实测值

### 10.4 阻塞与风险

坐标系错误会被误判成"检测差" → **E4 的 golden 必须先绿**。

### 10.5 完成记录（E9）

#### 10.5.1 交付物

| 文件 | 内容 |
| --- | --- |
| `src/table/cell_content_backfill.rs`（新建，1,048 行） | 纯函数层：几何匹配 + 文本拼接 + 两条文本来源；26 条测试 |
| `src/table/mod.rs` | 注册 `pub mod cell_content_backfill;` 与模块清单条目 |
| `src/exports.rs` | 导出 16 项（`BackfillOutcome` / `BackfillSource` / `CellMatchKind` / `CellRecognizer` / `TABLE_MATCH_MIN_IOU` / `TextConfidence` / `TextLine` / `axis_aligned_bbox` / `backfill` / `bbox_to_quad` / `join_line_texts` / `match_cells` / `match_cells_with_min_iou` / `match_cells_with_offset` / `rectangle_distance` / `rectangle_iou`） |
| `src/evaluation/table/run.rs` | 三口径内容来源 `TableContentSource`、页级 OCR 复用 `PageOcr`、结果缓存 `OcrCache`、对照识别器 `CropRecognizer`、回填统计 `BackfillStats` / `summarize_backfill` |
| `src/evaluation/table/mod.rs` | 重导出 `TableContentSource` |
| `src/bin/table_eval.rs` | `--content` / `--ocr-config` / `--ocr-cache` |
| `tools/table_teds.py` | `--restrict-to-predictions`（子集跑时缺键是"没跑"而不是"答错"） |
| `tools/build_table_teds_baseline.py` | 三处泛化：口径不再硬编码、单运行时确定性报 `null` 而不是空洞的 `true`、`stage` 随口径切换 |
| `tests/baseline/windows-baseline/table-teds-full-cpu.json`（新建，18,367 B） | **全量 7,104 完整 TEDS 基线**（`0.570710`，2 轮逐字节相同），结构口径基线的姊妹件 —— **⚠️ 该文件已在 E10 按新默认分辨率重新生成（18,955 B，`0.583404`），本节数字是 E9 当时的值，见 §11.4.6** |

#### 10.5.2 §10 三条根因各自的落点

| 根因 | 落点 |
| --- | --- |
| ① 批内 `max_wh_ratio` 污染（一个合并单元格拉宽整批） | 生产路径**不再调用识别器**，改几何匹配；对照路径**刻意逐格调用**（batch 1）以忠实复现这条朴素做法 —— 攒批会人为放大根因①，那样对照就不公平了 |
| ② rec 是单行识别器，多行单元格被压扁 | 复用整页 det：多行单元格的每一行是**独立的 det box**，`join_line_texts` 按行拼接（不是把多行图压成一行） |
| ③ 逐 cell 裁剪重复付 crop/resize/归一化 | 回填只做几何运算：实测 `backfill_ms` p50 **0.0083 ms/样本**（对照口径 p50 **133.1 ms/样本**，差 4 个数量级） |

#### 10.5.3 三处有意偏离参考实现

1. **占位（全零）bbox 保留槽位**，只把 `bbox` 置 `None`。参考实现 `normalize_bboxes` 里的 `filter_blank_bbox` 会**删除**占位框，于是其后所有 `cell_bboxes[j]` 与 `get_pred_html` 的 `td_index` 脱钩 —— 那是个潜伏 bug（中间位置出现占位框时，后面的文本会串格）。E3 的决定正好修掉了它，E9 沿用。
2. **不做 `<b>` 增删**。参考实现在 `"<b>" in content` 成立时就 `content[3:]`、在 `"</b>" in content` 成立时就 `content[:-4]`，判据是"串里任意位置出现"而不是"以它开头/结尾"。E6 实测真值里有 **4,171** 个单元格含 `<`（成对的真实 HTML 标签），这条会静默损坏真实文本。
3. **文字置信度 = 匹配行分数的算术平均**是**本模块的选择**：参考实现把 `rec_res[i][1]` 丢掉，从不汇报文字置信度。结构置信度（`StructureResult::confidence`）与文字置信度（`TextConfidence`）分开保存，不互相污染。

#### 10.5.4 门禁 G2

三层证据，从强到弱：

1. **类型层（最强）**：`match_cells` 的签名里没有任何识别器 —— "回填时调 rec"在这个 API 上写不出来。`Crop` 分支的存在被关在 `BackfillSource::Crop` 这个显式变体里，调用方必须主动选它。
2. **单测**：`the_reuse_path_calls_the_recognizer_zero_times`（`CountingRecognizer.calls == 0`）、`the_crop_control_calls_the_recognizer_once_per_cell_with_a_bbox`（反向对照，证明计数器真的会动）。
3. **真实链路**：200 样本 `page-reuse` 跑出的报告里 `backfill.recognizer_calls = 0`，而 `cells = 4266`、`non_empty_cells = 3091` —— 不是"因为没跑所以是 0"。

对照口径的 `recognizer_calls = 4266` 是**设计意图**。报告里的 `recognizer_calls_note` 随 `content` 切换判据方向：`structure-only` / `page-reuse` 下写"非零即 G2 失败"，`crop-control` 下写"零调用不变式在此不适用"。

#### 10.5.5 §10.3 第 2 条的实测结论与撤回

**原判据**：cell 文本非空率**不低于**"逐 cell 裁剪喂 rec"的对照实现。

**实测（同一 200 样本子集，`sample_set_sha256 = 5cc64d5463ebaabb0af0d6311b0f4db27b6530219413079e4eab6ac26a2d6ab5`）**：

| 量 | 复用整页 OCR（`page-reuse`） | 逐 cell 裁剪（`crop-control`） |
| --- | --- | --- |
| 完整 TEDS | **0.573928** | 0.529408 |
| 逐样本胜负（200 样本） | **胜 139 / 负 51 / 平 10** | — |
| `non_empty_rate` | 0.724566 | **0.842944** |
| `unmatched_cells` | 1175 | 0 |
| `text_confidence` | **0.964217** | 0.717093 |
| `backfill_ms` p50 | **0.0083** | 133.1122 |
| `recognize_ms` p50 / count | —（0 次调用） | 8.8509 / 4266 |

**判据原字面未通过**（0.7246 < 0.8429）。但把它当质量代理是错的，两条独立证据：

1. **真值本来就空**：这 200 样本的真值共 **4,779** 个单元格，其中 **1,024** 个（21.43 %）文本为空。复用路径的 1,175 个未匹配里，**只有 +151（占模型单元格 3.54 %）超出"真值本来就空"这个解释**。也就是说约 **87 %** 的空缺是**正确留空**，不是漏检。
2. **多填出来的文本是低置信度的幻觉**：对照口径比复用多填 505 个单元格（3,596 − 3,091），代价是文字置信度从 0.964 掉到 0.717，且完整 TEDS **下降 0.0445**，在 139/200 的样本上更差。若那 505 个多填的文本是真值里真有的文字，TEDS 应该上升而不是下降。

**因此本条撤回并替换为**：

- **（新 2a，主判据）完整 TEDS 不低于对照实现** —— 这是"复用没有让结果变差"的直接检验，实测 **+0.0445**（且逐样本胜 139/200，不是被少数样本拉偏）。
- **（新 2b，诊断判据）记录 `unmatched_cells` 与真值空单元格数之差**，即"**可解释空缺之外**的残留"。本子集该残留 = **151**，占模型单元格 3.54 %。它才是真正值得跟踪的"可能漏检"信号。
  - ⚠️ 这个 151 是**上界**而不是精确值：模型预测的单元格数（4,266）与真值（4,779）不等，`cell_count_exact` 只有 46.73 %（E8 §9.4），所以"多出来的未匹配"里有一部分是**结构本身对不齐**而不是漏检。要精确归因需要按 E6 的权威网格逐格对齐，本阶段未做，登记为未覆盖项。

> 保留原判据的**动机**是对的（复用路线不该丢文本），但它的**代理量**选错了：非空率无法区分"填对了"与"在空白裁剪上编了字"。这也解释了为什么单看这一个数字会得出相反结论。

**全量 7,104 样本上的同一诊断**（`Temp/e9-probe-empty-cells-full.py`，读 `datasets/table/tabrecset/derived/pubtabnet_val_gt_html.json` 用 `lxml` 数 `//td` 中 `text_content().strip()` 为空的格）：

| 量 | 值 |
| --- | --- |
| 真值单元格总数 | 161,925 |
| 真值空单元格 | 33,369（**20.61 %**） |
| 模型单元格数（`backfill.cells`） | 143,456 |
| 复用路径非空 | 104,123（72.58 %） |
| 复用路径未匹配（`unmatched_cells`） | 39,333（27.42 %） |
| **`unmatched − 真值空`** | **+5,964（占模型单元格 4.16 %）** |
| 未匹配中被"真值本来就空"解释掉的比例 | **84.84 %** |

全量结论与 200 样本子集一致：**约 85 % 的空缺是正确留空**，"可能漏检"的残留是 5,964（4.16 %），比子集的 3.54 % 略高但同一量级。该残留同样是**上界**（模型与真值单元格数不等，混了结构错位）。

#### 10.5.6 验收命令与结果

```powershell
cargo fmt --all -- --check                      # 退出 0
cargo clippy --all-targets -- -D warnings       # 零 warning
cargo test --all-targets                        # 562 + 2 + 4 + 14 passed, 0 failed（E8 为 530，净增 32）
cargo test --lib table::cell_content_backfill   # 26 passed
cargo test --lib evaluation::table              # 33 passed
cargo build --release --bins                    # 成功
```

E9 净增的 32 条测试：`cell_content_backfill` 的 26 条 + `evaluation::table::run` 的 6 条
（`structure_only_is_the_default_content_source`、`the_manifest_can_select_the_page_reuse_content_source`、
`the_manifest_can_select_the_crop_control_baseline`、`an_unknown_content_value_is_refused_rather_than_guessed`、
`the_page_ocr_label_names_the_config_that_produced_the_text`、
`a_backfill_summary_over_no_samples_reports_counts_not_ratios`）。

#### 10.5.7 全量 7,104 完整 TEDS 基线

全量命令（`loop_wall_ms` 4,196,553.64 ms ≈ **69.94 分钟**，冷跑；随后两次带缓存重跑各约 4.2 分钟）：

```
.\target\release\table_eval.exe --content page-reuse ^
  --ocr-config models/configs/test-config-small.yaml ^
  --ocr-cache Temp/ocr-cache-val.json --intra-threads 14 ^
  --predictions Temp/e9-pred-full.json --report Temp/e9-report-full.json

python tools\table_teds.py --true datasets\table\tabrecset\derived\pubtabnet_val_gt_html.json ^
  --pred Temp\e9-pred-full.json --out Temp\e9-teds-full.json

python tools\build_table_teds_baseline.py ^
  --report Temp\e9-report-full.json --report Temp\e9-report-full-2.json ^
  --teds Temp\e9-teds-full.json --predictions Temp\e9-pred-full.json ^
  --gt-html datasets\table\tabrecset\derived\pubtabnet_val_gt_html.json ^
  --output tests\baseline\windows-baseline\table-teds-full-cpu.json
```

| 量 | 值 |
| --- | --- |
| **完整 TEDS** | **0.570710** |
| decodable-only | 0.572321 / 7,084 |
| 同一全量集 TEDS-Struct（E8 §9.4） | 0.706518 → 完整口径低 **0.135808** |
| stddev / min / max | 0.260805 / −0.285714 / 0.978632 |
| `exact 1.0` / `exact 0.0` / 负值 | 0 / 22 / 25 |
| `backfill.cells` / `non_empty` / `unmatched` | 143,456 / 104,123（72.58 %）/ 39,333 |
| `backfill.recognizer_calls` | **0**（G2 在全量真实链路上成立） |
| `backfill.text_confidence` | 0.963315 |
| `backfill.backfill_ms` p50 / mean / p90 / max | **0.0077 / 0.012247 / 0.0246 / 0.2433** |
| `page_ocr.ocr_ms` p50 / mean / p90 / max | 549.159 / 570.666 / 851.914 / 1557.793 |
| `page_ocr.cache_hits` / `cache_misses` / `regions_seen`（冷跑） | 200 / 6,904 / 140,373 —— 两次基线运行都是 7,104 / 0 |
| `timing` p50：preprocess / infer / decode / total（冷跑） | 1.0014 / 29.1462 / 0.0318 / 31.2325 ms |
| 失败样本 | **20**（12 × 1 个未闭合标签、5 × 2 个未闭合标签、2 × 无匹配开标签的 `</tbody>`、1 × 游离的 ` colspan="18"` 片段） |
| 预测产物 | 3,741,061 B，`sha256 100d327f206fce0cdddddbe552a2ecd0344dc31e878008f7983ea95d4823c6a8` |

**基线文件里的确定性是实测的**：`determinism.runs = 2`、`identical_prediction_digests = true`、`teds_stddev_across_runs = 0.0`，两份预测产物摘要都是
`100d327f206fce0cdddddbe552a2ecd0344dc31e878008f7983ea95d4823c6a8`（各 3,741,061 B）。两次运行的 `loop_wall_ms` 是 **250,200.64** 与 **249,197.03 ms**（≈ 4.2 分钟，极差 0.4 %）——**这两次都是页级 OCR 缓存全命中的运行**（`cache_hits 7104 / cache_misses 0`），所以 `total_ms_mean` 只有 34.6 ms/样本；**冷跑（无缓存）的 `loop_wall_ms` 是 4,196,553.64 ms（69.94 分钟）**，即缓存把重跑成本压低了 **16.8×**。这就是 §10.5.8 第 3 条强调"完整 TEDS 绑定在配置上、换配置必须重跑"的代价量级。

工具在只有一次运行时**拒绝**声称逐字节相同：`identical_prediction_digests` 与 `runs_agree_within_epsilon` 都写 `null`，因为一次运行的"逐字节相同"是空洞真命题（单元素集合必然只有一个元素）。这是 `tools/build_table_teds_baseline.py` 在本阶段被泛化出来的三条之一。

**顺带修掉的一处口径缺陷**：`PageOcr` 原本把报告里的 `page_ocr.engine` 写死成常量 `"page OCR: engine ready"`，**读基线的人因此无法知道单元格文本是哪份配置产出的**。已改为记路径（`"page OCR: models/configs/test-config-small.yaml"`）或显式声明用的是内置默认配置，并加了单测 `the_page_ocr_label_names_the_config_that_produced_the_text` 钉死两种形态。这不是装饰 —— 完整 TEDS 的绝对值绑定在产出文本的引擎配置上。

#### 10.5.8 未覆盖风险

1. **L2 回归顺延 E11**（§10.3 第 3 条）。表格路径还没进入页级管线，现在没有任何生产代码会改变整页 CER 或 `ocr_total_ms`；`table_backfill_ms` 的 ledger 字段（`src/runtime/timing.rs`）同理未接线。**这不是"做不了"而是"还不存在要回归的对象"**，必须在 E11 接线时补做。
2. **151 的归因未做**（见 10.5.5 的 2b）。模型与真值的单元格数不等时，"未匹配"混了"漏检"与"结构错位"两种原因。
3. **页级 OCR 用的是 small 配置**（`models/configs/test-config-small.yaml`，与 `tests/baseline/windows-baseline/evaluation-cpu.json` 同款）。medium 配置在 200 样本上跑到 34.9 分钟仍未完成（约 10.5 s/样本，CPU 时间 20,489 s），换 small 后为 0.56 s/样本。**完整 TEDS 的绝对值因此绑定在 small 配置上**，换配置必须重跑。基线文件里的 `page_ocr.engine` 会记下产出文本的配置路径；缓存全命中时冷跑 69.94 分钟可压到约 4.2 分钟（16.8×）。
4. **`page-reuse` 的 offset 目前硬编码 `[0.0, 0.0]`**，因为 `lines` 与 `result.cells` 都在整张图的坐标系里。E11 若改成喂**裁剪后**的小图，这个 offset 必须换成 crop 原点 —— 它已经是参数，但**没有任何测试会因此失败**，只有注释在提醒。这是一个真实的回归风险点。
5. **对照口径只跑了 200 样本**（约 2 分钟），没有跑全量：它的用途是给出"朴素做法有多差"的量级，不是产出基线。
6. **`<b>` 增删的偏离会让本仓库的完整 TEDS 与参考实现不可逐位对齐**。参考实现会砍掉内容里的 `<b>`/`</b>`，本仓库保留。两者在同一批数据上会有系统性小差异，**不能把差异当成 bug**。

---

## 11. 阶段 E10：表格路径独立 det 分辨率（R2）

**根因（第一性事实）**：det 是整页稠密卷积网络，计算量正比于输入像素数。**决定像素数的不是 `max_side_len` 一个数，而是三处代码**：`global.max_side_len` 的前置缩放（只缩不放）、`det.limit_side_len` + `det.limit_type` 的 `compute_resize_hw`（`"min"` 只放不缩）、以及 `src/ocr/det/detector.rs:210-224` 的 `resolve_limit_side_len_like_python`（**非 `"min"` 时丢弃配置值**，改用 `max_wh` 派生的 960/1500/2000 阶梯）。本阶段原稿只提了 `max_side_len`，是**不完整的**，见 §11.4.2。

### 11.1 任务

- [x] 为表格路径引入独立的 det 限制，**不继承页面级 `max_side_len`**：新增 `src/table/ocr_resolution.rs` 的 `TableOcrResolution`，把 `max_side_len` / `limit_side_len` / `limit_type` 三者显式化并在 `apply` 里**无条件覆盖**引擎配置。
- [x] 按 §0.1 做破坏性配置改法 —— **不加兼容旧字段的分支**。默认值直接改掉（`736/"min"/2000` → `736/"max"/736`），没有保留旧行为的开关；`TableEvalManifest` 新增的 `ocr_resolution` 是**整结构体**字段而不是三个标量。

### 11.2 验收（口径直接复用仓库既有容差文件 `benchmarks/image-bench/api-comparison-small-medium.json`）

- [~] ~~mean CER ≤ **0.03**~~ —— **本条在 TabRecSet 上不可测**：该数据集没有文本框/文本真值，只有结构 token 与单元格多边形，CER 无从计算。已登记，见 §11.4.7 第 1 条。
- [~] ~~polygon IoU ≥ **0.85**~~ —— **同上不可测**（无文本框真值）。替代判据是完整 TEDS + `unmatched_cells` + `non_empty_rate`，见 §11.4.4。
- [x] 区域数差 ≤ **5 %** —— 实测：默认档位 `+3.56 %`（140,373 → 145,377），**通过**；`max/cap1280` `+3.78 %` 通过；`min320` `+6.06 %` **不通过**。这正是默认值取 `736/"max"/736` 的决定性依据之一。
- [x] `table_detect_ms` 与整页 p50 有实测对比 —— **表格路径没有独立的 det**：`content_source = page-reuse` 跑的是整页引擎（det + cls + rec），所以可比量是 `page_ocr.ocr_ms`。实测 p50 **108.907–112.736 ms**（基线 549.159 ms，**4.87–5.04×**），见 §11.4.4。

### 11.3 阻塞与风险

⚠️ **若 CER 上升超过容差，就放弃这一项**（`AGENTS.md` 的决策优先级：正确性 → 根因 → 性能）。表格里小字号内容漏检是真实风险，**这是一个必须用数据决定的旋钮，不是无条件优化**（07 §2 K1）。

→ **实测没有上升，反而上升的是精度**：全量 7,104 上完整 TEDS 从 **0.570710 升到 0.583404**（+0.012694），同时 `page_ocr.ocr_ms` p50 从 549.159 降到 **108.907–112.736**（**4.87–5.04×**）。所以这一项**不放弃，且采纳**。风险的实际形态与预期相反：真正的风险不是"降尺度掉精度"，而是**`"min"` 策略的放大没有上界**（见 §11.4.7 第 3 条）。

### 11.4 完成记录（E10）

#### 11.4.1 交付物

| 文件 | 变更 |
| --- | --- |
| `src/table/ocr_resolution.rs` | **新建**。`TableOcrResolution { limit_side_len, limit_type, max_side_len }` + `apply(&mut EngineConfig)` + `describe()`，10 条测试 |
| `src/table/mod.rs` | 模块清单加 `[ocr_resolution]` 一行 + `pub mod ocr_resolution;` |
| `src/exports.rs` | `pub use crate::table::ocr_resolution::TableOcrResolution;`（`src/lib.rs:73` 是私有 `mod table;`，二进制只能走 crate 根重导出） |
| `src/evaluation/table/run.rs` | `TableEvalConfig.ocr_resolution`；`TableEvalManifest.ocr_resolution: Option<TableOcrResolution>`；`PageOcr::open` 加参并先 `apply` 再算指纹；**缓存加引擎指纹**（见 11.4.5）；报告加顶层 `ocr_resolution`（含 `live_knob`），抽出 `ocr_resolution_report()` 以便单测 |
| `src/bin/table_eval.rs` | `--ocr-limit-side-len` / `--ocr-limit-type` / `--ocr-max-side-len`，默认值取自 `TableOcrResolution::default()`（不写第二遍字面量） |
| `src/ocr/det/detector.rs` | `resolve_limit_side_len_like_python` 由私有改 `pub(crate)`，让 `ocr_resolution` 的测试能把它和默认策略绑在一起断言 |
| `tools/build_table_teds_baseline.py` | 全量口径下补记 `ocr_resolution`；新增 `--stage` 覆盖字段；修掉 `determinism.method` 里硬编码的 "three runs"（改用实际份数插值） |
| `tests/baseline/windows-baseline/table-teds-full-cpu.json` | **重新生成**（默认值已变，见 11.4.4） |

#### 11.4.2 原稿的根因不完整：三处代码，不是一个旋钮

`src/ocr/det/detector.rs:210-224`：

```rust
pub(crate) fn resolve_limit_side_len_like_python(
    limit_type: &str, configured_limit_side_len: usize, max_wh: usize,
) -> usize {
    if limit_type == "min" { configured_limit_side_len }
    else if max_wh < 960 { 960 } else if max_wh < 1500 { 1500 } else { 2000 }
}
```

调用点 `src/ocr/det/detector.rs:150-154` 传的 `max_wh` 是**前置缩放之后**的图。**结论：`limit_type = "max"` 时 `limit_side_len` 是死旋钮**，写什么都不影响结果（这是复刻 PaddleOCR 的行为，由 `src/ocr/det/detector.rs:236-240` 的 `limit_side_len_for_non_min_matches_python_buckets` 钉死）。这条事实由本轮实测**独立证实**：200 样本子集上 `736/max`、`960/max`、`1280/max` 三档的预测文件**字节完全相同**（`sha256 2301B76B6404F19A3A6D38033161AECE7627D712260816FF0E2FF0277D4A7008`），因为三档的前置 `max_side_len` 都是 2000、阶梯只看 `max_wh`，于是跑的是**同一个策略**。

这件事被写成代码里的不变量，而不是只写在注释里：`src/table/ocr_resolution.rs` 的 `the_limit_side_len_is_ignored_under_the_max_policy` 直接调用该函数、对五个不同的配置值断言结果恒为阶梯值；`the_default_makes_the_detector_side_resize_a_no_op` 断言默认档位下 det 自己的 resize 是恒等变换；`the_report_names_the_live_knob_of_the_resolution_policy` 断言报告里 `live_knob` 随 `limit_type` 切换（**只断言默认分支是不够的** —— 那样把 `live_knob` 写死成常量也能通过）。

`TabRecSet` 的尺度事实（val 全量 7,104 张实测）：长边中位 **448** px、短边中位 **150** px；**短边 < 736 有 7,055 张（99.3 %）**，长边 > 2000 只有 13 张（0.18 %）。所以官方表格脚本的 `--det_limit_type=min` 隐含前提（"输入是整页、短边本来就在 736 附近"）在这份数据上**不成立**：一张 448×150 的裁剪表会被放大到约 2208×736（**约 24 倍面积**）再喂 det。

#### 11.4.3 200 样本六档扫描

固定 200 样本子集（`sample_set_sha256 = 5cc64d5463ebaabb0af0d6311b0f4db27b6530219413079e4eab6ac26a2d6ab5`，与 E9 同一子集），`--content page-reuse --ocr-config models/configs/test-config-small.yaml --intra-threads 14`，每档独立缓存文件：

| 档位（`limit_side_len` / `limit_type` / `max_side_len`） | **生效策略** | 完整 TEDS | ΔTEDS | `ocr_ms` p50 | 加速比 | regions | Δregions | `non_empty` | `unmatched` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **736 / min / 2000**（E9 基线） | 短边放大到 736 | 0.573928 | — | 504.7 | 1.00× | 4,261 | — | 0.7246 | 1,175 |
| 480 / min / 2000 | 短边放大到 480 | 0.587666 | +0.013738 | 290.4 | 1.74× | 4,404 | +3.36 % | 0.7475 | 1,077 |
| 320 / min / 2000 | 短边放大到 320 | **0.593718** | +0.019791 | 190.4 | 2.65× | 4,441 | +4.22 % | 0.7534 | 1,052 |
| 736 / max / 2000 | 永不放大（阶梯 ≥960） | 0.592182 | +0.018254 | 117.3 | 4.30× | 4,363 | +2.39 % | 0.7508 | 1,063 |
| 960 / max / 2000 | 同上（`limit_side_len` 被忽略） | 0.592182 | +0.018254 | 116.3 | 4.34× | 4,363 | +2.39 % | 0.7508 | 1,063 |
| 1280 / max / 2000 | 同上 | 0.592182 | +0.018254 | 118.4 | 4.26× | 4,363 | +2.39 % | 0.7508 | 1,063 |

**基线档位逐位复现 E9**（TEDS `0.5739277027404647`、`non_empty_rate 0.7245663384903891`、`unmatched_cells 1175`、`recognizer_calls 0`、`regions_seen 4261`）→ 分辨率覆盖这次重构是**行为保持**的。

#### 11.4.4 全量 7,104 四点对照与默认值判定

三点是新跑的（`Temp/e10-full-runs.ps1`，各档独立缓存，`--intra-threads 14`），第四点是 E9 已公布的全量基线（不重跑）：

| 档位 | **生效策略** | 完整 TEDS | ΔTEDS | `page_ocr.ocr_ms` p50 | 加速比 | `regions_seen` | Δregions | `non_empty` | `unmatched` | `loop_wall_ms` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 736 / min / 2000（E9 基线） | 短边放大到 736 | 0.570710 | — | 549.159 | 1.00× | 140,373 | — | 0.725818 | 39,333 | 4,196,553.6（冷跑） |
| **736 / max / 736** | **永不放大，长边 ≤ 736** | **0.583404** | **+0.012694** | **108.907** | **5.04×** | 145,377 | **+3.56 %** | 0.741342 | 37,106 | 1,272,123.8 |
| 1280 / max / 1280 | 永不放大，长边 ≤ 1280 | 0.583932 | +0.013222 | 111.376 | 4.93× | 145,684 | +3.78 % | 0.742123 | 36,994 | 1,315,393.6 |
| 320 / min / 2000 | 短边放大到 320 | **0.590587** | **+0.019877** | 200.599 | 2.74× | 148,883 | **+6.06 % ✗** | 0.752572 | 35,495 | 1,838,418.4 |

**`page_ocr.ocr_ms` 的分位数的冷跑抖动必须交代**：上表 `736/max/736` 的 108.907 来自 `Temp/e10-report-full-max736.json`；而**生成入库基线的那一次冷跑**（`Temp/e10-baseline-cold-report.json`）量到 **112.736**（`mean` 150.033、`p90` 299.082）。两次冷跑同为 `736/max/736`、同一配置、预测字节相同，p50 极差 **3.5 %** —— 这是整轮墙钟级别的抖动，不是配置差异。所以对 E9 基线（549.159）的加速比应当报成 **4.87–5.04×**，而不是单点 5.04×。**入库基线文件里记的是 112.736**（它是被引用、被复现的那份产物）。

**判定：默认值取 `limit_side_len = 736 / limit_type = "max" / max_side_len = 736`。** 依据按强度排序：

1. **§11.2 唯一可测的门槛（区域数差 ≤ 5 %）淘汰了精度最高的那一档。** `min320` 的 TEDS 最高（0.590587）但区域数 **+6.06 %**，超容差；`736/max/736` 是 **+3.56 %**，通过。
2. **在通过的两档之间，TEDS 差异（0.583404 vs 0.583932，0.09 % 相对）低于任何有意义的分辨率，而代价是实打实的**：`ocr_ms` p50 108.9 vs 111.4、`loop_wall_ms` 1,272 s vs 1,315 s。奥卡姆剃刀：取更紧、更快、精度无差别的那个。
3. **降尺度几乎是免费的。** 长边上限从 736 放到 1280（影响长边 > 736 的那 11 % 图）只换来 +0.000528 TEDS。这是"det 输入超过 736 对本数据集无增益"的直接证据，也是"把上限压到 736"这个决定的第一性依据。
4. **`"max"` 给 det 输入一个硬上界（长边 ≤ 736，≤ 0.54 Mpx），`"min"` 给不了。** `"min"` 只约束短边下界，一张 1280×100 的图会被放大到 4096×320 —— 生产路径上这是不可预测的延迟。这条独立于门槛。
5. 相对基线，默认档位在**两个轴上都更好**：TEDS +0.012694、`ocr_ms` p50 **4.87–5.04×**。**这不是取舍，是严格改进。**

**为什么 `"max"` 反而更准**：`"min"` 把小裁剪表放大到训练分布之外（448×150 → 2208×736，24 倍面积），det 在这类输入上给出的文本框既多又碎（`regions_seen` 反而更少、`unmatched_cells` 反而更多）；`"max"` 让输入保持原尺度。`min320` 比 `max736` 准 0.0072，说明**最极小的那些表**（短边 p10 = 60 px）需要一点放大 —— 但它的收益被区域数超容差这一条挡在门外，且它没有输入上界。

#### 11.4.5 顺带修掉的静默缺陷：页级 OCR 缓存没有引擎溯源

E9 的 `OcrCache` 只以样本相对路径为键（`src/evaluation/table/run.rs`），**没有引擎指纹**。后果：换 `--ocr-config` 或换分辨率之后，缓存会被**静默复用**，产出的单元格文本来自另一份引擎，而报告里的 `page_ocr.engine` 只记录**这一次**的配置 —— 读者无从察觉。这违反仓库"不得静默"的一贯口径，也是本阶段所有分辨率实验的前置阻塞项（否则对照实验拿到的全是脏文本）。

修法：`OcrCache` 加 `engine_fingerprint: String`，`VERSION` 由 1 升到 2，`load(path, expected_fingerprint)` 指纹不符时报 `RapidOcrError::Config` 并**拒绝复用**。指纹取 `serde_json::to_string(&EngineConfig)` 的 sha256 —— 页级文本是"图像字节 + 引擎配置"的函数，所以取整份规范化配置：不取配置路径（同路径内容可变）、不取模型路径（漏掉分辨率与 provider）。`runtime` 段也在指纹里（provider 改变数值累加顺序），代价是换线程会让缓存失效一次。

**这条守卫在本轮真实触发过一次**：一次重启的作业复用了上一档留下的同名缓存，`table_eval.exe` 1.3 秒退出并报 `"the OCR cache ... was produced by a different engine than the one requested"`，而脚本的 `Select-String` 过滤把 stderr 吞掉了、只看到下游 Python 报"预测文件不存在"。**那不是崩溃，是守卫在按设计工作。** 脚本随后改为每档先删缓存、并把全量日志落盘后再判 `$LASTEXITCODE`。

`describe()` 也改成按 `limit_type` 分支措辞：`"max"` 下明确写出 `limit_side_len` 不参与计算，报告里同样加了 `live_knob` 字段 —— 否则读基线的人会以为改那个数会有影响。

#### 11.4.6 验收命令与结果

```powershell
cargo fmt --all -- --check                      # 退出 0
cargo clippy --all-targets -- -D warnings       # 零 warning
cargo test --all-targets                        # 579 + 2 + 4 + 14 passed, 0 failed（E9 为 562，净增 17）
cargo test --lib table::ocr_resolution          # 10 passed
cargo test --lib evaluation::table              # 40 passed
cargo build --release --bins                    # 成功（20.11 s）
python -m py_compile tools\build_table_teds_baseline.py   # 通过
```

净增 17 = `ocr_resolution` 10 + 缓存指纹 5 + `live_knob` 2。`ocr_resolution` 的 10 条里有 **2 条是补齐 §11.4.7 第 3、6 条登记的风险**（上界语义、两个字段相等的巧合），见 §11.4.7。过程中 `cargo clippy` 抓出一条真实的 `needless_borrow`（`src/evaluation/table/run.rs:591` 把已是引用的 `config` 又取了一次 `&`），已修；修完 `cargo clippy` 归零。**这条修改被证明对产物零影响**：用最终源码重建 release 二进制后再跑一轮（`Temp/e10-recheck.ps1`），预测文件摘要仍是 `1ab736e7…`，报告与修改前逐字段对照只剩时间戳、路径分隔符、`peak_working_set_bytes` 与各类 `*_ms` 分位数的抖动，**没有一个指标字段或结构字段变化**。

全量默认档位两轮（`Temp/e10-baseline-runs.ps1`，共享 E10 那份指纹匹配的页级 OCR 缓存）：

```
.\target\release\table_eval.exe --content page-reuse ^
  --ocr-config models/configs/test-config-small.yaml --intra-threads 14 ^
  --ocr-cache Temp/e10-cache-full-max736.json ^
  --predictions Temp/e10-final-pred-1.json --report Temp/e10-final-report-1.json
```

（`--ocr-limit-side-len` / `--ocr-limit-type` / `--ocr-max-side-len` 全部省略 —— 走的就是 `TableOcrResolution::default()`。）

**两轮预测逐字节相同，且与早先的 `Temp/e10-pred-full-max736.json` 也相同**：三份都是 3,744,386 B / `sha256 1ab736e73dfcf6f981f1df01954b03ae658283e4740b2d593d1d86d46ad1b07c`。`loop_wall_ms` = 255,056.89 / 248,816.94（热缓存，极差 2.4 %，属整轮墙钟）。打分 `TEDS = 0.583404`，与冷跑那一轮**逐位相同**。

**基线 `tests/baseline/windows-baseline/table-teds-full-cpu.json` 已按新默认重新生成**（18,955 B，`determinism.runs = 3`）。因为热缓存报告里 `page_ocr.ocr_ms` 只有 `{count: 0}`、`regions_seen` 为 0，主报告改用一轮**冷缓存**全量跑（`Temp/e10-baseline-cold.ps1` → `Temp/e10-baseline-cold-report.json`，`cache_hits 0 / misses 7104`），另两轮热跑作为确定性证据；`--teds` 复用 `Temp/e10-final-teds.json`（预测字节相同，TEDS 必然相同）。基线新增 `ocr_resolution` 段：`{limit_side_len: 736, limit_type: "max", max_side_len: 736, applies_to: "page_reuse", live_knob: "max_side_len (det ignores limit_side_len under limit_type=max)"}`；`determinism` 段：`runs = 3`、`identical_prediction_digests = true`、`teds_stddev_across_runs = 0.0`、`prediction_sha256` 三份同为 `1ab736e7…`、`timing_across_runs.loop_wall_ms = [1,315,957.35（冷）, 255,056.89, 247,231.56]`（**冷热混排是刻意的**：它顺带记录了缓存把重跑成本压低的幅度）。

顺带修掉 `tools/build_table_teds_baseline.py` 一处**事实不一致**：`determinism.method` 原先硬编码"three runs"，而 `runs` 字段是实际份数（本次为 2），现在 `method` 用实际份数插值。

#### 11.4.7 未覆盖风险

1. **§11.2 的两条代理量在 TabRecSet 上不可测。** 该数据集只有结构 token 与单元格多边形，**没有文本框与文本真值**，所以 `mean CER ≤ 0.03` 与 `polygon IoU ≥ 0.85` 无从计算。本阶段改用**完整 TEDS**（直接测最终产物）+ `unmatched_cells` + `non_empty_rate` + `regions_seen` 作替代判据。**这不是把门槛放宽，是换成更直接的指标** —— 但读者必须知道原稿那两条在本数据集上是空的。
2. **`table_detect_ms` 在表格路径上不存在。** `content_source = page-reuse` 跑的是整页引擎（det + cls + rec），没有 det 单独计时。可比量是 `page_ocr.ocr_ms`。**E11 若改成复用页级 det 结果，这条会再次变化**，届时要重新定义。
3. ~~**`"min"` 策略的放大没有上界**，而默认档位（`"max"`）是有的。上界这件事**没有单独的测试**……~~ → **本轮已补测**：`the_default_puts_a_hard_bound_on_the_detector_input_long_side` 把整条链路串起来断言 —— `apply` 覆盖 `global.max_side_len` 之后，一张 4129×700 的图经 `resize_image_within_bounds` 的长边必须 ≤ 736，**并反证**该图大于 `EngineConfig` 的默认 2000（否则这条测试什么也证明不了）。若有人把 `apply` 改成只覆盖 `det.*`，这条测试会失败。剩下的风险只是"`"min"` 侧仍然没有上界"这一条**设计事实**（不是缺陷，是 `min` 的语义）。
4. **`limit_side_len` 在默认档位下是死字段，只是被文档与测试标注了，没有被类型系统禁止。** 更彻底的形态是把它改成"策略枚举"（`NeverEnlarge { max_side_len }` / `MinShortSide { limit_side_len, max_side_len }`），让死字段**无法被表达**。本轮没做，理由是三个字段与引擎配置的键一一对应、且六个档位的对照实验需要它才能通过 CLI 复现。**这是一个已知的、有意留下的设计债。**
5. **没有测长边上限低于 736 的档位**（如 512、320）。已测的证据只说明"736 与 1280 在 TEDS 上无差别"，**不能推出"再往下压也无损"**。中位长边是 448，压到 512 很可能开始损精度，但这只是推测，未实测。
6. ~~**`max_side_len` 与 `limit_side_len` 在默认里取同一个值（736）**……没有测试断言这个巧合。~~ → **本轮已补测**：`the_default_uses_one_number_for_both_side_limits` 断言两者相等，并在断言消息里写明"若它们分叉，模块文档里'数字仍沿用官方的 736'这句话会变得含糊"。这两条补测使 `cargo test --lib table::ocr_resolution` 从 8 条增至 **10 条**。
7. **`min320` 被门槛淘汰，但它是本阶段精度最高的档位。** 若将来判定"区域数差 ≤ 5 %"这条稳定性代理量**不适用于表格路径**（对表格路径而言，检出更多文本框是收益而非漂移），那么默认值应当重新评估为 `320/min/2000`（TEDS +0.019877，2.74×）。**这条判定没有被做，本轮按原稿字面执行。**

---

## 12. 阶段 E11：表格定位走文本分布启发式（R8，门禁 G4）

### 12.1 任务

- [x] `src/table/detection.rs` 从**已有 det 的文本框分布**推断表格区域（表格区域内文本框呈规整行列、行高一致、x 坐标成列）。这是**零额外模型成本**的做法。
- [x] 接口形态对齐 07 §7.1 的**意图**，但签名必须纠正：`TableDetector::detect(&mut self, image: &RecImage, page_lines: &[PageLine]) -> Result<Vec<TableRegion>>`，`TableRegion { polygon, score, crop, origin }`。**`LineResult` 不带任何几何字段**（`src/ocr/types.rs:22-27` 只有 `text` / `score` / `word_info`），所以 07 §7.1 原稿的 `&[LineResult]` 不可实现；带几何的是 `src/ocr/det/detector.rs:70` 的 `DetOutput::boxes: Vec<Quad>`，与 `src/ocr/pipeline/types.rs` 的 `ExecutionOutput { boxes, txts, scores, .. }` 里的 `txts`/`scores` **同序并行**。表格域因此自定义 `PageLine { polygon: Quad, text, score }`，**不引入 `pub(crate)` 的 `ExecutionOutput`**。
- [x] 专用模型（`PP-LCNet_x1_0_table_cls` 6.6 MB / `RT-DETR-L_*_table_cell_det` 124 MB）**降级为"实证不达标时的备选"，不预先引入** —— 本轮拿到了**不引入的实测依据**（见 §12.4.3）。

### 12.2 验收

- [x] **G4**：模型数量不增加。写成**两层断言**：① 行为面 `the_detector_needs_no_model_and_no_external_asset`（构造 + `detect` 全程不碰模型与外部资产）；② 静态面 `the_module_constructs_no_inference_session`（扫 `detection.rs` 自身源码，禁 `OrtSession` / `RapidOcrEngine` / `TableStructureSession` / `.onnx` / `with_dimensionOverride` / ORT 路径前缀）。
- [~] ~~区域召回 / 精确率在 `datasets\text\04表格与键值对.png` + `refer\RapidTable-main\tests\test_files\table.jpg`、`table_without_txt.jpg` 上达标~~ —— **本条按原字面不可验收，已替换为逐图判据**。这三张图**没有任何区域级真值**（`datasets/text/golden-manifest.json` 里该页的 `boxes` 是 `[]`），"达标"没有可比的基准；改成每张图各自的客观判据（几何 IoU / 结构不变量 / 已知缺陷显式断言），见 §12.4.2。
- [x] 整页 `ocr_total_ms` 无增量：定位器是纯函数且**尚未接进页面管线**，所以增量**按构造为 0**；已实测定位器自身在 61 个文本框上 **1.713 ms/call**（10 次调用、预热一次），相对约 1000 ms 的整页预算约 **0.17 %**。接线后仍需按 G5 复测。

### 12.3 阻塞与风险

无线框表格、跨页表格、嵌套表格会退化 → **必须用 E8 的表格测试集量化，而不是靠直觉**。

**实测结论：退化确实发生了，而且机制已经量到。** 三张图覆盖了三种情形，判据刻意不同，因为"从文本框分布推断"这一原理在三者上的可用性本来就不同：

| 图 | 尺度 | det 文本框 | 检出 | 判定 |
| --- | --- | --- | --- | --- |
| `table.jpg` | 371×293 | 74 | 1 个区域 `[9,0,364,293]`，score 0.9426 | **强项**，IoU 0.957 |
| `table_without_txt.jpg` | 896×500 | **1** | **0 个区域** | **原理性不适用**（无文本即无输入） |
| `04表格与键值对.png` | 3200×2000 | 61 | 2 个区域 | 顶部统计表正确；**左下键值对与右下表格被合并**（已知假合并） |

**假合并的确切机制**（本阶段最重要的发现）：det 框紧贴**文字**而不是单元格，所以真表格内部的列间空隙可以很大 —— 该页顶部统计表内部最大到 **404.8 px**；而左下键值对与右下表格之间的 gutter 只有 **159.7 px**。**没有任何"间距阈值"能把二者分开**：阈值 > 404.8 就什么都切不开，阈值 < 159.7 就会把真表格切碎。→ 这不是可调参数问题，是启发式的原理性退化，正是本条预告的形态。**这正是 §12.1 保留专用模型作备选的实测依据。**

---

### 12.4 完成记录（E11，已收口）

#### 12.4.1 交付物

| 交付物 | 说明 |
| --- | --- |
| `src/table/detection.rs` | 新增（约 1,370 行含测试）。纯几何定位器：`Bounds` / `LineRow` / `TableDetectorConfig` / `DetectedRegion` / `TableRegion` / `TableDetector` trait / `TextDistributionDetector`，以及 `group_rows` / `shared_column_count` / `detect_regions` / `clamped_rect` / `crop_axis_aligned` / `crop_rect` |
| `src/table/mod.rs` | 模块清单加 `[detection]` 一行 + `pub mod detection;` |
| `src/test_support.rs` | 新增 `pub fn table_detection_fixture(name: &str) -> Option<PathBuf>`（`RAPID_STRUCTURE_TABLE_DETECTION_IMAGES/<name>` → `<manifest>/refer/RapidTable-main/tests/test_files/<name>` → `<manifest>/datasets/text/<name>`；缺失时 `asset(...)` 返回 `None`，**不 panic、不回落绝对路径**） |
| 25 条测试 | `table::detection`：20 条纯几何 + 5 条真实页面（`probe_real_pages` 是人工核对入口，不是验收断言） |

#### 12.4.2 逐图判据与实测

**`table.jpg`（371×293，74 个文本框）—— 强项，用几何 IoU 判。**

- 检出 **恰好 1 个区域** `[9,0,364,293]`，score **0.9426**，与整图 `[0,0,371,293]` 的 **IoU = 0.957**。
- 裁剪图 **355×293**，`origin = [9,0]`，与多边形包围盒逐位一致（**逐像素来自原图，不是重新渲染**）。
- 结构：`rows=16 cols=5`，`bands=5`，`median_band_width=43.0`，四个带间距全部 **14–16 px**。
- 断言：`lines >= 50`、恰好 1 区域、IoU ≥ 0.9、score ≥ 0.9、裁剪尺寸与 origin 与多边形一致。

**`table_without_txt.jpg`（896×500）—— 原理性不适用，期望 0 区域。**

- det 只找到 **1 个**文本框（整页没有任何文字），检出 **0 个区域**。
- 断言：`lines <= 2` **且** 区域为空。前者是**关键**：它把"没有文本"与"det 失败"区分开，否则这条测试会因为错误的原因通过。
- 这不是缺陷而是边界 —— 定位器的唯一输入是文本框分布，没有文本就没有输入。该情形必须走专用模型（07 §7.1 的备选路线），而不是靠调参。

**`04表格与键值对.png`（3200×2000，61 个文本框）—— 已知假合并，用结构判据 + 显式断言缺陷。**

- 检出 **恰好 2 个区域**：
  - `#0 = [124,283,2480,687]`，score **0.8100**，`rows=5 cols=5`，`bands=8`，`median_band_width=162.9`，带间距依次 `404.8 / 158.1 / 17.7 / 143.5 / 21.0 / 217.7 / 14.5`。
  - `#1 = [92,886,3089,1372]`，score **0.7732**，`rows=6 cols=5`，`bands=7`，`median_band_width=179.0`，带间距依次 `751.6 / 159.7 / 311.3 / 185.5 / 19.4 / 201.6`。
- **顶部统计表的真实外框由页面自己的网格线实测**（不依赖 det）：在 `y ∈ [328,648]` 的横带里，灰度 < 230 且覆盖该带 80 % 以上高度的列恰好 6 条 —— `160/161, 794/795, 1196/1197, 1714/1715, 2232/2233, 3038/3039` —— 即 **5 列**，横向网格线落在 `y ∈ [328,647]`，故真实框 = `[160,328,3039,647]`。
  - `IoU(检出 #0, 真实框) = 0.655`。检出右界（2480）比真实右边框（3039）少 **559 px**，因为最右列内容（"置信度 > 0.90"等）左对齐且很短，**det 框不延伸到表格右边框** —— 与假合并同一个根因。
- 断言：恰好 2 区域；`#0` 的 `(rows, cols) == (5, 5)`、`bottom < 1000`（页面上半部）、IoU ≥ 0.5；`#1` 的 `left <= 200 && right >= 2900`（**跨越两块并排结构**）。
- **`#1` 的断言刻意断言缺陷仍然存在**：它一旦失败，说明定位器学会了分开并排块，那就必须同时更新本文档、`docs/07` §7.1 与 §14，而不是把断言改松。

#### 12.4.3 为什么不引入专用模型（实测依据，而非直觉）

三条独立的证据：

1. **间距不可分**（§12.3）：真表格内部空隙 404.8 px > 并排块 gutter 159.7 px，任何阈值都必然错一边。
2. **无文本表格原理上不适用**：`table_without_txt.jpg` 上定位器有 0 个输入框，调参无济于事。
3. **`LineResult` 不带几何**：要在生产路径上拿到文本框，本来就需要把 `DetOutput::boxes` 透出来（E11 已用 `PageLine` 解决），引入第三个模型并不能省掉这一步。

因此专用模型保持为**备选**，等待真实业务页面的失败率数据再决定（届时用 E8 的表格测试集量化）。

#### 12.4.4 算法与判别式

**唯一的判别式**是"相邻两行之间共享列数 `>= min_columns`（默认 2）"。竖排一列（段落 / 目录 / 清单）每对相邻行只有 1 个匹配 → 被排除。其余全部是稳健性处理：

- `group_rows`：按 `center_y` 排序后贪心聚类，容差 = `row_center_tolerance × median(全页文本框高度)`；**行中心用运行均值递推**（避免同行成员变多后中心漂移）；行内按 `left` 升序。
- `shared_column_count`：**双向一对一**贪心匹配（对 `a` 的每个成员挑一个未被占用、覆盖率最高且 `>= column_overlap_ratio` 的 `b` 成员）。**一对多不算匹配** —— 否则一个跨列的合并单元格会把两列误算成一列。
- `detect_regions`：从列数 `>= min_columns` 的行开始向右生长，相邻行需 `shared_column_count >= min_columns` 且 `gap <= max_row_gap × median(行高)`；行数 `>= min_rows` 才成区域；区域按行区间互不重叠。
- **分数 = 列对齐率 × 行高一致性**（列对齐率 = `Σ实际匹配 / Σ min(上行框数, 下行框数)`；行高一致性 = `1/(1+行高变异系数)`，落在 `(0,1]`）。**分数不参与是否检出的判断**：把行高一致当硬门槛会让"某行是合并单元格"直接漏检，所以它只作排序/置信度用。
- `crop_axis_aligned` **不用** `src/ocr/pipeline/image_ops.rs:72` 的 `crop_text_regions`：后者是透视裁剪并在 `h/w >= 1.5` 时 `rotate_90`（识别器的行归一化），**旋转会把行变成列**。`clamped_rect` 先把浮点盒夹进图像再 `floor`/`ceil`，夹完退化（零宽/零高）报 `InvalidImage`。

#### 12.4.5 验收命令与门禁

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
# 人工核对（打印三张图的全部检出）
cargo test --lib table::detection::real_page_tests -- --nocapture
# 追加列带与带间空隙诊断（假合并的量化依据）
$env:RAPID_STRUCTURE_TABLE_DETECTION_DEBUG="1"; cargo test --lib table::detection::real_page_tests -- --nocapture
```

实测结果：`cargo fmt --all -- --check` 干净；`cargo clippy --all-targets -- -D warnings` 零 warning；`cargo test --all-targets` → **604 + 2 + 4 + 14 passed / 0 failed**（E10 为 579，**净增 25** = 20 条纯几何 + 5 条真实页面）；`cargo test --lib table::detection` → 25 passed。

**门禁 G4 判定：通过。** 两层断言（行为面 + 静态面）均绿，`src/table/detection.rs` 不构造任何推理会话、不读任何模型或外部资产。

#### 12.4.6 工具层教训

1. **G4 静态面断言的针必须够精确**：`["ort", "::"].concat()` 会被 `crate::test_support::…` 命中（`test_supp` + 前缀），导致断言恒假。已改为**检查前一个字符不是标识符字符**（`mentions_ort_path`）。同理，测试自己构造针时必须**由碎片拼出**，否则 `include_str!` 会把测试里的字面量读进来。
2. **地面真值要实测，不要目测**：我最初按渲染图目测顶部统计表为 `[109,253,2383,513]`，与检出区域的 IoU 只有 0.459 —— 差得刚好落在阈值附近，很容易滑向"把阈值调低"。**正确做法是从页面自己的网格线量真值**（→ `[160,328,3039,647]`，IoU 0.655）。目测在 3200×2000 的页面上最大偏差达 134 px。
3. **反向对照断言要能区分错误实现**：`table_without_txt.jpg` 那条如果只断言"0 区域"，一个**永远返回 0 区域**的实现也会通过；必须同时断言 `lines <= 2`，把"没有输入"与"det 失败"分开。
4. 本轮触发的三条 clippy lint（新工具链）：`neg_cmp_op_on_partial_ord`（`!(mean > 0.0)` → `!mean.is_finite() || mean <= 0.0`）、`manual_is_multiple_of`（`len() % 2 == 0` → `len().is_multiple_of(2)`）、`field_reassign_with_default`（测试里逐字段改写 `Default` → 结构体字面量 + `..Default::default()`）。

#### 12.4.7 未覆盖风险

1. **定位器尚未接进页面管线**。阶段表 E1–E13 的 §14 只有六项任务（发射器 / 占满性 / Markdown / 降级档 / manifest 登记 / 第三方声明），**没有任何一项是页面管线集成**，所以这是 E1–E13 范围外的后继工作，不是本阶段的欠账。后果是"整页 `ocr_total_ms` 无增量"**按构造为 0**，而不是端到端实测；接线后必须按 G5 复测。
   - **E13 的实测修正（原文曾把这件事写成"E13 的职责"）**：本阶段验证了这一点 —— 移除 `src/table/mod.rs` 的模块级 `allow(dead_code)` 后，`cargo clippy --all-targets -- -D warnings` 报出 **21 条错误，全部来自 `detection.rs`**，其余 8 个模块（`decode` / `emitter` / `structure` / `preprocess` / `cell_content_backfill` / `ocr_resolution` / `registration` / `metadata`）都已被 `src/evaluation/table/run.rs` 在生产构建里消费。即：**唯一没有生产消费者的模块就是定位器**，这正是"接线未做"的机器可验证证据。处置见 §14.3.8。
2. **假合并未修**（§12.3 已量化机制）。生产路径上多块并排页面会把两块并成一块，回填时单元格与文本框的几何匹配会跨块错配。
3. **`table_without_txt.jpg` 类的无文本表格不适用**，必须走专用模型 —— 而专用模型本轮**未引入**，所以这类页面目前是"检不到"，不是"降级处理"。
4. **三张图不是统计样本**：`table.jpg` 与 `table_without_txt.jpg` 来自 `refer/RapidTable-main/tests/test_files/`，只有 3 张图、共 136 个文本框。区域召回/精确率的真实分布必须用 E8 的表格测试集（`datasets/table/tabrecset`）量化，本轮只证明了机制。
5. **`TableDetectorConfig` 的 6 个默认值只在 3 张图上验过**，没有做过参数敏感性分析。`min_columns = 2` 是唯一有第一性依据的（见 §12.4.4），其余（`row_center_tolerance = 0.5` / `column_overlap_ratio = 0.5` / `max_row_gap = 1.5` / `region_margin_ratio = 0.02`）是工程默认值，**没有实测支撑**。
6. **跨页表格未测**：定位器只在单张图上工作，跨页表格需要调用方拼接，本轮没有接口也没有测试。
7. **`#1` 的结构断言（`rows=6 cols=5`）刻意不写死**：只断言了横向跨度。合并后的区域行列数是"两块内容的并集"的产物，会随任一块的版式变化，写死它等于把偶然当契约。

> **E23 后续修正（本节的语义已被改写，完整记录见 `docs/11-table-page-pipeline-execution.md` §4.10）**：本节记录的判据把**一个 OCR 文本框当成一个表格行**，而一个文本框是**一行视觉文本**。折行单元格的第二行落在自己的 y 上 → 被判成独立的行 → 每一处折行都打断行区间。用户的一张 12 行生产记录表因此只检出 2 个 3 行片段（另 6 行退回普通文本区域）。**现行语义**：细行不打断区间、只当**填充**；列匹配只在**锚行**（`column_count() >= min_columns`）之间做；检出判据是锚行数 ≥ `min_rows`；`row_height_consistency` 只喂锚行；`DetectedRegion.rows` 因此是真实行数的**下界**（权威网格来自结构模型）。**没有新增任何配置项。** §12.4.7 的第 2 条（假合并）与第 6 条的断言值在本轮改动后**原样成立**（夹具页产出仍是 `rows=5 cols=5` 与 `rows=6 cols=5`、包围盒与 §12.4.2 记录的一致）。同时**本节之后的所有 TEDS 数字（含 E8–E10 的入库基线 `0.5834044577250798`）对定位器不敏感**：E23 改动后重跑全量 7,104 张，`mean TEDS` 与预测文件 `sha256` 都与入库基线**逐位相同** —— 因为离线评测不走定位器（它把整张图喂给结构模型，val 集是"一张表一张图"的裁剪集），所以这份基线**不必重跑、也没有被本轮改动影响**；反过来说，**现有评测协议量不出定位器的缺陷**，要度量它必须换整页数据集（详见 `docs/11` §4.10）。

---

## 13. 阶段 E12：线程（R5）与 DirectML per-EP（R7）

### 13.1 任务

- [x] **R5**：`TableStructureConfig` 声明 `intra_threads: Option<usize>`，在构造 `OrtSession`（`src/runtime/session.rs:243` 的 `open_session`）时覆盖。**不改** `src/runtime/profile.rs:84` 的 `pub(crate) fn auto_tuned_thread_budget()`。
  - 第一性依据：ORT 官方默认 `session.intra_op.allow_spinning = "1"`（**空闲线程自旋而非休眠**），仓库已有 det + rec（+ 公式）多个 session，表格再拿 16 个 intra 线程会产生自旋争抢；且 SLANet 只有 830 节点（77 Conv / 73 HardSwish），batch=1 时并行度不足。
  - **机制已交付，默认值未改**：字段与覆盖路径都在（`src/table/structure.rs` 的 `TableStructureConfig::intra_threads` 与 `effective_runtime_config`），但 §13.3 的判据判定**没有任何线程设置被证明优于自动推导值** → 默认保持 `None`（继承）。见 §13.5.3。
- [x] **R7**：在 `open_session` / `resolve_execution_providers`（`src/runtime/provider.rs:147`）里按 `selected_ep` 分支：
  - **DirectML** → `DisableMemPattern` + `ORT_SEQUENTIAL`（`execution_mode`）+ 固定 H/W（依赖 E5）+ 拒绝并行 `Run`。**这是 session 创建的必要条件，不是优化**：DML **不支持 memory pattern 优化，也不支持并行执行**，设置不当会直接报错；此外 DML 不支持对同一 session 的多线程 `Run`，要求 **opset ≤ 20**，并偏好静态形状。
  - **CUDA** → 官方默认 `arena_extend_strategy = kNextPowerOfTwo`、`cudnn_conv_algo_search = EXHAUSTIVE`。
  - **CPU** → **保持现状**：`src/runtime/session.rs:248` 的 `GraphOptimizationLevel::Level3` 与 `:260` 的 `enable_cpu_mem_arena` 已实测最优（ALL 43.9 ms vs DISABLE_ALL 79.2 ms；arena 开 41.8 vs 关 49.3 ms），**不要动**（07 §5.3）。
  - **落地形态与原文的差异**：策略落在 `src/runtime/session.rs` 新增的 `SessionBuildPolicy` + `SessionBuildOption` 上，`for_provider(selected_ep)` 只对 `DirectMl` 返回偏离项；`resolve_execution_providers` 被**提前到 builder 创建之后**（EP 特有选项必须在 `commit_from_file` 之前施加）。DirectML 的 opset 约束由新增的 `require_direct_ml_opset` 检查（`DIRECT_ML_MAX_OPSET = 20`）。"拒绝并行 `Run`"**不是新加的运行时检查**，而是既成事实的静态回归守卫：所有 run 入口都取 `&mut self`，新增测试 `every_run_entrance_takes_an_exclusive_borrow` 把这条钉死。见 §13.5.1。

### 13.2 命令

```powershell
$Repo = '<path to this checkout>'
cd $Repo
# R5：表格域（不是页级）。run_thread_matrix.ps1 走 bench_warm_e2e，而表格尚未接进页面管线
# （src/ocr/pipeline/rapid_structure.rs 把 table_structure_infer_ms 写死 0.0）→ 它原理上观测不到 R5。
pwsh -NoProfile -File Temp/e12-r5-sweep.ps1          # 6 次重复，交错 default / 4 / 8
python Temp/e12-r5-verdict.py                        # 读 6 份报告并判定是否超出噪声带
# R7 页级回归（open_session 重排影响每一个会话，所以必须跑）
pwsh -NoProfile -File tools/run_windows_baseline.ps1
python Temp/e12-page-regression.py
# R7 provider smoke + 声明核查
pwsh -NoProfile -File Temp/e12-provider-smoke.ps1
python Temp/e12-provider-smoke-report.py
pwsh -NoProfile -File tools/check_provider_claims.ps1 -Reports "tests/baseline/windows-baseline/bench-cpu.json,tests/baseline/windows-baseline/bench-direct_ml.json,tests/baseline/windows-baseline/bench-cuda.json"
```

> ⚠️ `tools/run_windows_baseline.ps1` 的 `$OutDir` **硬编码**为 `tests\baseline\windows-baseline`，**没有输出目录参数** → 跑之前必须先备份，跑完还原（§13.5.4）。
>
> `tools/run_windows_provider_matrix.ps1` 与 `check_provider_claims.ps1` 的完整用法仍见 07 §5.3 与工具自身的 `-Help`；本阶段**刻意不重跑完整三 provider 矩阵**（与 E5 同口径），理由见 §13.5.5。

### 13.3 验收

- [x] **R5：p50 必须超出 ±10–15 % 噪声带才允许提交**；落在带内就**回退改动**（07 §2 K4 的判定不是"再调一调"）
  - **判定：回退**。6 次重复下最大偏差 −6.89 %，既小于 10 % 带底，也小于 `default` 自身的 9.30 % 离散度 → 默认保持继承。§13.5.3 附完整数字与"±10–15 % 对本机表格模型偏乐观"的方法学修正。
- [x] **R7**：`check_provider_claims.ps1` 通过；`selected_ep` **不作为**加速证据
  - 对已入库的 `bench-cpu.json` / `bench-direct_ml.json` / `bench-cuda.json` 判定与本阶段前**逐字相同**（`direct_ml` PASS 2.00×、`cuda` FAIL —— 先于 E12 存在且已登记的本机性质，见 §13.5.5）；对 1 轮 smoke 报告该工具**正确拒绝**比较（exit 2，分辨率不同）。本阶段**没有提出任何加速声明**，所以这条的实质是"工具仍能识别无依据的声明"。
- [x] 多输出情况下拒绝并行 `Run` 有测试覆盖
  - `every_run_entrance_takes_an_exclusive_borrow`（静态：生产段里恰好两处 `Run`，两处都在取 `&mut Session` 的辅助函数内）+ `serialized_sharing_of_one_session_produces_the_same_outputs`（行为：`Arc<Mutex<TableStructureSession>>` 两线程与顺序跑的两路输出逐位相同）。

### 13.4 阻塞与风险

> **本节原文的两条前提已被本阶段实测推翻一条、修正一条，下面保留原文并紧跟实测结论。**

- ~~⚠️ **本机无 DirectML DLL**（Python 环境只有 `AzureExecutionProvider` + `CPUExecutionProvider`）→ **DML 行为无法在本机验证**~~ —— **前提错误**：这是**工具链错配**，不是硬件缺失。仓库的 Rust 构建静态链接自己的 ONNX Runtime 1.28 并加载 `DirectML.dll`（E5 已实测 `selected_ep == direct_ml`、`fallback_to_cpu == false`）。因此 DML 路径在本机**可以**通过 Rust 二进制实测。
- ⚠️ SLANet 含 `Loop` 控制流算子，**DML 对控制流子图的支持未验证** → **已实测：不支持**。会话能开、推理必失败（失败点在运行期 `Loop.0` 体内的 `Cast.73`，DML 报 `E_INVALIDARG`/`0x80070057`）。opset 约束不是障碍（SLANet-plus 是 14 ≤ 20）。→ **不得写进对外承诺**，且"能开会话"不能作为可用性证据。见 §13.5.2。
- ⚠️ `selected_ep` **只表示交给 ORT 的 EP 链头部**：本机 CUDA 因缺 cuDNN 而节点全落 CPU，API 察觉不到（实测 p50：CPU 1039.5 / DirectML 508.7 / CUDA 1034.2）。→ 任何加速结论必须来自实测 P50/P90。**这条仍然成立**，已在 `ProviderResolution::selected_ep` 与新增访问器 `TableStructureSession::provider_resolution` 的文档里写明。
- ⚠️ 新增：**`infer_ms.p50` 的重复间离散度本身就大**（同配置 6 次重复 spread 9.3 %，`intra=4` 达 36 %），远超 n=500 的抽样误差（约 2 %）。→ 表格域任何"小幅提升"的判定必须先用足够重复数建立该配置自身的离散度，不能直接套 ±10–15 %。见 §13.5.3。

### 13.5 完成记录（E12，已收口）

#### 13.5.1 变更摘要

| 文件 | 变更 | 依据 |
| --- | --- | --- |
| `src/config.rs` | 新增 `RuntimeConfig::with_pinned_intra_threads(threads) -> Self`（一次写 `intra_threads`/`inter_threads = Some(1)`/`auto_tune_threads = false` 三个字段；`threads == 0` 原样返回） | 只写 `intra_threads` 不够：`inter_threads` 仍会被自动推导填 1、`auto_tune_threads` 仍为 true，语义是"运行时觉得合适"而非"调用方要求这个数字" |
| `src/table/structure.rs` | `TableStructureConfig` 加 `intra_threads: Option<usize>`（`Default = None`）+ `effective_runtime_config(&RuntimeConfig) -> RuntimeConfig`；`TableStructureSession` 加 `session_threads: (Option<usize>, Option<usize>)` 与 `session_threads()`；`open` 第一步就解析线程配置，**探针会话与正式会话共用** | §13.1 R5 |
| `src/table/structure.rs` | 新增 `provider_resolution() -> ProviderResolution` 与 `onnx_opset() -> Option<u32>`（委托内部 `OrtSession`） | 让 per-EP 分支可被观察；E13 判定 DML 处理方式需要它们 |
| `src/evaluation/table/run.rs` | **删除** `fn runtime_config(config: &TableEvalConfig)`（第二份"钉线程"实现）；`table_config` 构造提前到探测之前并带上 `intra_threads`；`open` 处改用 `session.session_threads()` + `debug_assert_eq!` | 消除重复实现，且让"生效线程数"只有一个来源 |
| `src/runtime/session.rs` | 新增 `SessionBuildPolicy { memory_pattern, parallel_execution, must_be_explicit }` + `SessionBuildOption` + `ORT_DEFAULTS` + `for_provider(selected_ep)` + `required_options()` + `apply(builder)`；`resolve_execution_providers` **提前**到 builder 创建之后、`commit_from_file` 之前；新增 `DIRECT_ML_MAX_OPSET = 20` 与 `require_direct_ml_opset`；`OpenedSession` 从四元组改成 struct 并新增 `onnx_opset` 字段与 `OrtSession::onnx_opset()` | §13.1 R7；EP 特有选项必须在 commit 前施加 |
| `src/table/structure.rs` | 新增 `#[cfg(feature = "directml-provider")] mod direct_ml_experiment`（测试 `the_real_table_model_runs_on_direct_ml_or_says_why_not`） | 回答 E5/E11 登记的开放问题 |

**没有做的事（刻意）**：没有改 `src/runtime/profile.rs` 的 `auto_tuned_thread_budget()`；没有改 `TableStructureConfig::intra_threads` 的默认值（§13.5.3 判定回退）；没有在 `TableStructureSession::open` 里拒绝 DirectML（§13.5.2 末段）。

#### 13.5.2 关键结果一：DirectML **不能**跑表格模型（实测）

命令与逐字输出：

```powershell
cargo test --features directml-provider --lib table::structure::direct_ml_experiment -- --nocapture
```

```
DIRECT_ML_VERDICT=session_opened
DIRECT_ML_SELECTED_EP=DirectMl
DIRECT_ML_FALLBACK=false
DIRECT_ML_OPSET=Some(14)
DIRECT_ML_VERDICT=inference_failed
DIRECT_ML_ERROR=decoding failed: ONNX Runtime error: Non-zero status code returned while running Loop node.
  Name:'Loop.0' Status Message: Non-zero status code returned while running Cast node. Name:'Cast.73'
  Status Message: ...DmlExecutionProvider\src\MLOperatorAuthorImpl.cpp(2870)... Exception(1) ... 80070057
```

三条结论：

1. **失败在运行期，不在创建期**。会话开得成（`session_opened`、`selected_ep == DirectMl`、`fallback_to_cpu == false`），第一次推理才失败，失败点是 `Loop.0` 体内的 `Cast.73`，DML 报 `E_INVALIDARG`（`0x80070057`），源头 `MLOperatorAuthorImpl.cpp(2870)`。→ **任何"能开会话就报告 DirectML 可用"的检查都会假阳性。**
2. **opset 约束不是障碍**。SLANet-plus 的 opset 是 **14 ≤ 20**，`require_direct_ml_opset` 正常放行。真正的障碍是 DML 对 `Loop` 体内 `Cast` 算子的支持。→ `DIRECT_ML_MAX_OPSET` 保留价值在于"把模糊的 ORT 失败变成能定位的错误"，不在于拦住这个模型。
3. **§13.4 原文的前提是工具链错配**。那条阻塞项的依据是 **Python** 的 `onnxruntime` 只列出 `AzureExecutionProvider` + `CPUExecutionProvider`；但仓库的 Rust 构建静态链接自己的 ORT 1.28 并加载 `DirectML.dll`（E5 已实测 `selected_ep == direct_ml`）。两者不是同一个 ORT。→ 这条阻塞项已按实测重写。

**控制组**：不加 `--features directml-provider` 时该测试模块 **0 个测试被编译**（`0 passed; 614 filtered out`）→ feature 门有效。

**这条记录是可伪证的，不只是打印**。测试本身不断言"成功"（在别人的机器上把"不支持"断言成失败没有意义），但它钉住了三条会随现实变化而失败的性质：① `selected_ep` 必须是 `DirectMl`；② `fallback_used` 必须为 false（否则时间与"控制流能否执行"两个答案描述的都是 CPU —— 由 `fail_if_provider_unavailable: true` 保证，一旦 provider 被静默换掉这两条会失败）；③ 失败时必须同时满足**错误来自 `DmlExecutionProvider` 自身**（否则这次测量测的是环境而不是图）**且错误点名 `Loop`**（否则文档记录的根因过时了，必须同步修改文档而不是让断言静默通过）。

**留给 E13 的决定（本阶段刻意不做）**：是否在 `open` 时就拒绝 DirectML，或做一次预检推理把运行期失败转成清晰的 `Config` 错误。不做的两个理由：① 预检要跑一次推理（`Loop` 的 trip 上限常量是 500，最坏 500 步）；② 硬编码"DML 不支持表格模型"会在 ORT 修好后**误拒**。当前行为是运行期失败，错误信息已点名 `Loop.0`/`Cast.73`，足以定位。

#### 13.5.3 关键结果二：R5 **回退**（没有任何线程设置被证明更优）

命令：`pwsh -NoProfile -File Temp/e12-r5-sweep.ps1`（6 次重复，交错顺序 default/4/8），每轮 `target\release\table_eval.exe --limit 500 --content structure-only --intra-threads N`。报告里的 `config.intra_threads` 是**生效值**，所以 `default` 显示 **14**（本机 `auto_tuned_thread_budget()`）。

| 设置 | 6 次重复的 `infer_ms.p50`（ms） | 均值 | 自身 spread | 相对 default |
| --- | --- | --- | --- | --- |
| `default`（生效 14） | 27.742 / 26.437 / 27.279 / 28.896 / 28.516 / 28.861 | 27.955 | **9.30 %** | — |
| `intra=4` | 27.811 / 26.397 / 30.970 / 25.825 / 29.675 / 22.695 | 27.229 | **36.46 %** | −2.60 % |
| `intra=8` | 27.306 / 25.308 / 25.989 / 28.089 / 24.221 / 25.254 | 26.028 | 15.97 % | −6.89 % |

**判定：回退。** 最大偏差 −6.89 % 既小于 §13.3 的 10 % 带底，也小于 `default` 自身的 9.30 % 离散度 → 按"落在带内就回退改动"保持 `intra_threads` 默认 `None`（继承自动推导的 14）。**回退的是"改默认值"这件事，§13.1 要求的机制（声明字段 + 可覆盖）完整保留。**

**方法学修正（本阶段最重要的教训）**：3 次重复时看到的是 "`intra=4` 慢 21.76 %、`intra=8` spread 45.64 %" —— **两者都是噪声**。同一配置 6 次重复的离散度就有 9.3 %（`intra=4` 达 36 %），远超 n=500 时 p50 的抽样误差（约 2 %）→ 主导方差是**重复间系统漂移**，不是抽样误差。**§13.3 抄来的 ±10–15 % 噪声带对本机表格模型偏乐观**；表格域任何"小幅提升"的判定必须先建立该配置自身的离散度。判据工具 `Temp/e12-r5-verdict.py` 在 `|delta| < max(10 %, default 自身 spread)` 时判 INSIDE。

#### 13.5.4 门禁判定

| 门禁 | 判据 | 实测 | 判定 |
| --- | --- | --- | --- |
| **G1**（页级无增量） | `ocr_total_ms` p50 落在基线 ±10–15 % 内 | 2000：982.05 → 1018.68（**+3.73 %**）；1280：644.96 → 650.44（**+0.85 %**） | **PASS** |
| G1（指标不变） | `mean_cer` 与 per-case CER 逐位相同 | `0.44765135645866394` 逐位相同，**12 个 per-case CER 全部逐位相同**；`regions_avg` 两边相同（34.833333333333336 / 33.416666666666664） | **PASS** |
| G1（内存） | 峰值工作集不劣化 | 2000：−0.09 %；1280：+0.21 % | **PASS** |
| §0.4 provider smoke | 三 provider × 非严格/严格 全部可解析 | 六变体全 `exit_code 0`、`selected_ep` == 请求链、`fallback_to_cpu` 全 false | **PASS** |
| §13.3 R5 | 超出噪声带才提交 | 最大偏差 −6.89 % < 10 % 带底 | **回退**（判定通过） |
| §13.3 R7 | `check_provider_claims.ps1` 通过 | 已入库三报告判定与本阶段前逐字相同（`direct_ml` PASS 2.00×、`cuda` FAIL）；1 轮 smoke 报告被**正确拒绝**比较（exit 2） | **PASS** |
| §0.4 代码门禁 | fmt / test / clippy | `cargo fmt --all -- --check` 退出 0；`cargo test --all-targets` **614 + 2 + 4 + 14 passed / 0 failed**（E11 为 604，**净增 10** = 8 条 session + 2 条 config）；`cargo clippy --all-targets -- -D warnings` 零 warning | **PASS** |

**为什么页级回归是必需的**：`open_session` 的重排与 per-EP 策略的施加影响进程里**每一个**会话（det / rec / 公式 / 表格），不只表格。G1 的 `mean_cer` 逐位相同把"CPU 路径逐条调用不变"从**构造保证**升级成了**测量结果**。

**基线处置**：`tools/run_windows_baseline.ps1` 的 `$OutDir` 硬编码、无输出目录参数 → 跑前备份到 `Temp/baseline-before-e12`，跑完把该脚本会写的 6 个文件（`bench-cpu-2000.json` / `bench-cpu-1280.json` / `evaluation-cpu.json` / `environment.json` / `build-stats.json` / `summary.json`）从备份还原并逐个哈希核对，新跑结果存 `Temp/e12-baseline-after`。理由：一次被验证为中性的改动不应静默覆盖它自己被比对的那份参考（与 E5 同做法）。`table-teds-cpu.json`（18,543 B）与 `table-teds-full-cpu.json`（18,955 B）未被该脚本写，保持原样。

#### 13.5.5 未覆盖风险

1. **DirectML 路径没有任何加速结论**。本阶段只证明了"表格模型在 DML 上跑不起来"。页面级 det/rec 在 DML 上的行为由 E5 的 provider smoke 覆盖（`direct_ml` p50 469.60/351.66），但那不是表格域。
2. **完整三 provider 矩阵刻意未重跑**（与 E5 同口径）。理由：本阶段改动在所有非表格路径上是**空策略**（`for_provider(Cpu|Cuda)` 返回 `ORT_DEFAULTS`，`required_options()` 为空 → 一条 setter 都不调用），且 provider 选择逻辑一行未动。已入库的 `bench-direct_ml.json` / `bench-cuda.json` 是本阶段之前的状态。
3. **`bench-cuda.json` 的 FAIL 是先于本阶段存在的已登记性质**（CUDA EP 对此模型不比 CPU 快，p50 1042.33 vs 990.11），不是 E12 造成的。工具自身的 `NOTE` 已写明"不要靠放宽规则来'修'它"。
4. **R5 只在 structure-only、`--limit 500`、本机 14 线程预算下测过**。页级 OCR 与表格共存的线程争抢（§13.1 的第一性依据）**没有被实测**，因为表格尚未接进页面管线（`src/ocr/pipeline/rapid_structure.rs` 把 `table_structure_infer_ms` 写死 0.0）。→ 表格真正接线后（E13 或之后）必须重测这一点，**当前"继承 14"的结论只对"表格单独跑"成立**。
5. **`SessionBuildPolicy` 的 `must_be_explicit` 字段是为 DirectML 的正确性要求加的**，而 DirectML 现在已知跑不通 → 该分支在**本机**没有端到端可用路径。它由 `direct_ml_disables_memory_pattern_and_parallel_execution` 单测覆盖（断言恰好返回两项），但没有真实 DML 会话验证过"这两项被下发"这一事实本身。
6. **CUDA 分支未加任何 CUDA 专属选项**。§13.1 说 CUDA 用官方默认（`kNextPowerOfTwo` / `EXHAUSTIVE`）→ 既然就是默认值，`for_provider(Cuda)` 返回 `ORT_DEFAULTS` 且不下发。但**本机 CUDA 缺 cuDNN、节点全落 CPU**，所以"CUDA 分支正确"在本机不可验证。
7. **`infer_ms` 只覆盖表格结构模型推理**，不含 preprocess / decode / emitter / 回填。R5 的结论只对这一段成立。

---

## 14. 阶段 E13：输出、降级与登记收尾

### 14.1 任务

- [x] `src/table/emitter.rs`（文档原写 `html-emit.rs`，实现落为 `emitter.rs`）：属性**只允许**来自结构解码器验证过的正整数 `rowspan`/`colspan`；非法或超网格上限返回值错误；文本转义；与现有 `src/output/html.rs` 统一 API（具体 API 见 07 §7.5 的 `<待确认>` 项，本阶段定案）。
- [x] **网格占满性校验**：emitter 是**唯一能察觉 span 超词表**的位置（解码器察觉不到，见 E3 §4.3）。逻辑网格出现空洞时 → 输出 `structure_confidence=low`，**不静默产出看似正常的表格**。
- [x] Markdown：先将 span 单元格放入逻辑网格，再按最大列数输出；**不能从已拼好的 HTML 做正则替换**。rowspan/colspan 的最终呈现规则需 fixtures + 产品确认（07 §14 登记项）。
  > **该登记项已由 E15.3 关闭**（`docs/11-table-page-pipeline-execution.md` §3.5.3）：规则定为"span 语义仍在 Markdown 里丢失，但 `has_span` 时表格前后各加一行 HTML 注释显式说明"，`<br>` 类替代表示仍未采纳。E14 及以后的阶段不在本文档范围内，此处只留指路。
- [x] **结构降级档**（07 §10）：span 超词表 **或** 解码超 500 token → 降级到轻量档网格策略并标 `structure_confidence=low`。**这是正常路径，不是异常兜底** —— 真实数据里这两类表占比非零（E7 已量化）。
- [x] **manifest 登记位置定案**：07 §14 未决项。注意 `models/configs/manifest.json` 是**模型集清单**（`{id, family, version, languages, detector, recognizer, dictionary, classifier}`），schema 与本用途不匹配 → 选定新位置或扩展 schema，条目含 `file_name` / `sha256` / `source_url` / `size_bytes` / `role=table_structure`。（**E18 更新**：该文件已按用户决定**删除**——没有任何代码读它，`src/model_source.rs:322-328` 明确拒绝该形状；表格模型最终登记在 `models/table/manifest.json`，`models/` 下现在只剩 `serve` 与 `table` 两份清单。）
- [x] 更新 `THIRD_PARTY_NOTES.md`：来源 URL、版本、摘要、**精确大小**、模型与代码许可、再分发结论。**未确认前写 `<待确认>`，不得把模型文件标成项目 Apache-2.0。**

### 14.2 验收

- [x] HTML 属性 / 文本转义、跨行跨列、空表、Markdown 退化规则、**网格占满性校验触发的降级标记**均有测试
- [x] 模型加载路径走 `model_verify::verify_sha256`（不是只在下载时校验或只在 `/api/models` 显示校验）
- [~] ~~`cargo package --allow-dirty=false` 检查包内没有模型、数据集、开发机绝对路径~~ —— 该命令**按字面不可执行**（`--allow-dirty` 是**布尔开关**而非取值选项，`cargo package --allow-dirty=false` 报 `error: unexpected value 'false' for '--allow-dirty' found; no more were expected`），且本仓库工作树**先于本阶段**就是脏的，不带 `--allow-dirty` 的 `cargo package` 必然在检查内容之前就拒绝。改为断言**包内容清单**，见 §14.3.5。

### 14.3 完成记录（E13，已收口）

#### 14.3.1 交付物

| 文件 | 变更 | 作用 |
| --- | --- | --- |
| `src/table/emitter.rs` | 重写（原 `structure_to_html` → 四个函数 + 三个枚举） | 属性从单元格表**重建**（不回显模型 token）、`TextPolicy` 显式转义策略、HTML/Markdown 共用一套校验、降级档由空洞与截断推出 |
| `src/table/decode.rs` | 新增 `GridCoverage` / `StructureTermination`；`decode_logic_points` 与 `decode_token_steps` 各加一个返回值 | 在**唯一持有占用集合**的位置算出网格占满性；把"见到 eos"与"跑满步数"分开 |
| `src/table/registration.rs` | 新建（14 条测试） | 读模型旁边的 `manifest.json`，按 `role=table_structure` 取期望哈希，委托 `model_verify::verify_sha256` |
| `models/table/manifest.json` | 新建 | `models/table/slanet-plus.onnx` 的登记条目（sha256 / size_bytes / source_url） |
| `src/model_set.rs` | `ModelRole::TableStructure`（`ALL: [Self; 8]`，`as_str` → `"table_structure"`） | 让登记有合法的 role 拼写 |
| `src/table/structure.rs` | `TableStructureConfig::expected_sha256`；`open` / `probe` 在**创建任何 ORT 会话之前**校验 | 损坏的模型到不了 ORT |
| `src/evaluation/table/run.rs` | `Prediction` 携带置信档与降级列表；报告新增顶层 `structure_confidence` 段 | 让降级可观测，且不改 `result.mean` 的口径 |
| `tools/build_table_teds_baseline.py` | 新增 `structure_confidence` 透传 | 基线自己声明覆盖率，不靠文档散文 |
| `THIRD_PARTY_NOTES.md` | 新增「SLANet-plus table structure model」段 | 来源 / 版本 / 精确大小 / 许可（权重写 `<待确认>`） |
| `Cargo.toml` | `exclude` 扩到 7 项 | 包内不含模型、数据集、采集产物、原型 |
| `tools/run_{windows_baseline,thread_matrix,windows_provider_matrix,page-flow-cdp}.ps1` | 默认路径改为从 `$PSScriptRoot` 推导 | 去掉 8 处开发机绝对路径 |

#### 14.3.2 三条设计判断（都不是"顺手改好"，每条都有代价）

1. **属性从单元格表重建，不回显 `node.tag`**。模型 token 流里可能把同名属性写两遍（实测 7,104 个样本里有 **1 例** `<td rowspan="2" colspan="2" rowspan="2">`），原样拼接会产出非法 HTML。代价：`StructureNode.tag` 与 `CellSlot.rowspan/colspan` 变成**两处真值**，所以加了 `check_node_matches_cell` 交叉核对——不一致时报错而不是静默选一边（它挡的是手工构造的 `StructureResult`，解码器产物同源）。
2. **`TextPolicy` 是显式策略、没有默认值**。E7 曾刻意不转义（理由是 PubTabNet 真值里单元格文本本身就是 HTML 片段），E13 的验收要求"文本转义"。两者**目的不同**：评测要保住与真值的可比性，输出要防 OCR 文本注入。用一个默认值同时满足两者是不可能的，所以让调用方说清楚——评测固定 `Raw`，用户可见输出用 `Escape`。
3. **占满性在 `decode_logic_points` 里算，不在 emitter 里算**。`covered` 是**互异**被覆盖的格子数，**不能**用 `Σ rowspan × colspan` 还原，因为矩形可以重叠：反例是"行 0 第二格 `rowspan=2` 占掉 (0,1) 与 (1,1)，随后行 1 第一格 `colspan=2` 从 (1,0) 起占 (1,0) 与 (1,1)"——面积和是 5，实际覆盖 4 格。有专门测试 `overlapping_rectangles_are_counted_once_not_by_span_area` 钉住这一点。

#### 14.3.3 基线对比：**摘要变了，指标一个字段都没变**

发射器重写会改变输出字节，所以先做逐字节回归，结果是否定的——但**否定的原因必须查清**，否则无法区分"无害的格式变化"与"真的改坏了"。

| 项 | E10（旧发射器） | E13（新发射器） |
| --- | --- | --- |
| 预测文件 SHA-256 | `1ab736e73dfcf6f981f1df01954b03ae658283e4740b2d593d1d86d46ad1b07c` | `df6bd2c0e17bdba5f17bfa74f3d9146c30f30a3ec98f2feae53f38c727d840d2` |
| 预测文件字节数 | 3,744,386 | 3,744,372（**−14**） |
| `result.mean` | 0.5834044577250798 | **0.5834044577250798（逐位相同）** |
| `result.mean_decodable_only` / `min` / `max` / `stddev` / `exact_one` / `exact_zero` / `histogram` | — | **全部逐字段相同** |
| 逐样本 TEDS | — | **7,104 个分数逐个相同** |

**差异归因（`lxml` 解析成 `(tag, sorted(attrs), text)` 序列后比较，7,104 个样本里只有 9 个不同，且 9 个全部"语义完全相同"）**：

- **8 例是属性顺序**：模型给的顺序是 `rowspan` 在前（`<td rowspan="2" colspan="2">`），新发射器固定 `colspan` 在前。字节长度相同。官方 `metric.py` 把属性读进字典，顺序不影响 TEDS——这也是"语义完全相同"成立的原因。
- **1 例是重复属性**：`14_048f59f899b17febc3544a2b24301faf_segR__tR__yf_0_all-line.jpg`，旧输出 `<td rowspan="2" colspan="2" rowspan="2">`（模型 token 流里 ` rowspan="2"` 出现两次），新输出 `<td colspan="2" rowspan="2">`，**−12 字节**。实测两个同名属性取值相同（都是 2），所以旧代码"取第一个"与 `lxml`"取最后一个"结果一致——**在 7,104 个样本上从未出现取值不同**。这一例是**新发射器多修掉的一个真实缺陷**。

→ **"逐字节相同"这个更强的断言对 emitter 不再成立**，任何以此为判据的回归都不能直接套用；判据改为"逐样本分数相同 + 差异逐条归因"。

**基线已重建**（`tests/baseline/windows-baseline/table-teds-full-cpu.json`，**19,797 B**，原 18,955 B）：

- 3 份报告（冷跑为 primary + 两轮热跑），`determinism.runs = 3`、`identical_prediction_digests = true`、三份 `prediction_sha256` 同为 `df6bd2c0…`、`teds_stddev_across_runs = 0.0`；
- `loop_wall_ms = [1346731.8907（冷）, 255920.367, 246862.4517]`（冷热混排是刻意的，见 §11.4.4）；
- 新增顶层段 `structure_confidence`；`stage` 字段用 `--stage` 覆写为 E13（默认值会退回 "E9"，那是**首次定义**该指标的阶段，不是**重建**它的阶段）；
- 与 E10 基线的逐字段 diff 只包含：三份摘要、`prediction_bytes`、`generated_utc`、`stage`、`structure_confidence`（新增）、`page_ocr.cache_path`、以及 `timing` / `backfill_ms` / `page_ocr.ocr_ms` 的 min/mean/p50/p90/max 抖动——**没有任何指标字段或结构字段变化**。

#### 14.3.4 关键结果：降级档的第一次量化

全量 7,104（完整口径、`structure_only=false`）：

| 字段 | 值 | 读法 |
| --- | --- | --- |
| `structure_confidence.high` | 6734 | 逻辑网格铺满 |
| `structure_confidence.low` | **350** | 占可解码样本的 **4.94 %**（350/7084） |
| `degradations.grid_holes` | 350 | 触发降级的**唯一**原因 |
| `degradations.truncated` | **0** | 500 步 trip 上限**从未触发** |
| `holes_total` | 2209 | 350 个样本合计 2209 个空洞（平均 6.3 个/样本） |

两条推论：

1. **触发条件是"网格空洞"而不是"span 数值超上限"**。emitter 看不到词表上限（它只知道 SLANet 的 48 个 token），它能看到的只有"声明的 span 没有把矩形铺满"——这正是模型把 span 截断到词表内之后的**可观测后果**。所以降级判据必须是 `GridCoverage::is_complete()`；拿 span 与 20 比较是另一回事（`span_exceedance` 段单独记）。
2. **`truncated = 0` 意味着"结构长度超上限"这条上界至今没有样本证据**。它只由图的 `Loop` trip 上限推出，属于可能性而非已观测事实——这一点在 07 §10 与 §14 都如实登记了。

#### 14.3.5 `cargo package` 口径修正（原文命令不可执行）

原文判据 `cargo package --allow-dirty=false` 有**两条**独立问题：

1. `--allow-dirty` 是**布尔开关**，写 `=false` 直接报 `error: unexpected value 'false' for '--allow-dirty' found; no more were expected`；
2. 本仓库工作树**先于本阶段**就是脏的（见 §1 的登记），不带 `--allow-dirty` 的 `cargo package` 必然在检查内容之前就拒绝。

**工具层教训**：`cargo package --list` 在**本仓库**会挂住十几分钟——它要遍历 `datasets/`（5.6 GB / 21 万文件）。改用 git 自己的清单（`git ls-files --cached --others --exclude-standard`）秒级返回，**并且这才是真正的判据**：包内容 = 未被 `.gitignore` 排除的未跟踪文件 + 已跟踪文件。

**发现并修掉一个真实的仓库卫生缺陷**：修前清单 **214,934** 个文件（`datasets/` 214,194、`Temp/` 282、`refer/` 56、`models/` 27）。`git ls-files models datasets refer benchmarks` 全为 0（整个未跟踪），但 `.gitignore` **只按扩展名忽略**（`*.onnx`、`*.dll`…），挡不住 `datasets/**/*.jpg`、`Temp/**/*.json`、`models/configs/*.yaml`。原 `Cargo.toml` 只有 `exclude = ["tests/baseline/**"]`。

修法：`exclude` 扩到 `["tests/baseline/**", "datasets/**", "refer/**", "Temp/**", "benchmarks/**", "prototypes/**", "models/**"]`（每项带理由注释）。**修后 `cargo package --list --allow-dirty --offline` exit 0、268 个文件**，七类禁止前缀各 **0**，`tests/fixtures/**` 之外的二进制 **0** 个（30 个 `.onnx` + 1 个 `.jpg` + 9 个 `.png` 全在 fixtures 内）。

**开发机绝对路径：从 14 个文件降到 0 个**：

- 3 个 `benchmarks/model-bench/bench-reference-*-cpu-r3.json` → 由 `benchmarks/**` 排除（未入库的采集产物）；
- 1 个 `prototypes/01-ocr-location-test.html` → 由 `prototypes/**` 排除（浏览器原型，含指向本机 ONNX 的绝对路径）；
- **4 个 tools 的 8 处是真缺陷**（`run_windows_baseline.ps1` 3、`run_thread_matrix.ps1` 2、`run_windows_provider_matrix.ps1` 2、`run-page-flow-cdp.ps1` 1）：默认值写死了 `D:\100_Projects\…`，**脚本只在采集机上可运行**。修法是把 `param()` 默认值改成 `''`，在 `$Crate = Split-Path -Parent $PSScriptRoot` 之后用 `if (-not $X) { $X = Join-Path $Crate '…' }` 推导（**不能**在 `param` 默认值里用 `$PSScriptRoot`）。四个脚本 `Parser::ParseFile` 全部 0 错误，推导出的路径与原来的硬编码路径**逐个 `Test-Path` 为真**，显式传参仍然覆盖默认值 → 本机行为不变；
- **6 个 docs 的 30 处**：24 处是命令片段（`$Repo` / `--config` / `--dataset-root` / `--img-path` 等），换成 `<repo>` 占位符语义不变；1 处是 `docs/09` 里描述一个已不存在的旧仓库路径，换成 `<previous checkout>`；**3 处是实测记录里的字段值**（`docs/04` 的 `runtime_module.path`、`docs/06` 一段错误文本），**只脱敏用户目录前缀并在旁边注明已脱敏**，大小与 SHA-256 逐字保留。
- 全树复扫（`docs/`、`tools/`、`src/`、`tests/`、`assets/`、`README.md`、`NOTICE`、`LICENSE`、`Cargo.toml`）：**零命中**。

#### 14.3.6 验收命令与实测结果

```powershell
cargo fmt --all -- --check                                          # exit 0
cargo clippy --all-targets -- -D warnings                           # 0 warning
cargo test --all-targets                                            # 656 + 2 + 4 + 14 passed, 0 failed
cargo package --list --allow-dirty --offline                        # exit 0, 268 files, 7 类禁止前缀各 0
```

测试计数（`cargo test --all-targets` 的四个 target 分别是 lib / `serve_startup` / 集成 / doc）：

| 阶段 | lib 测试数 | 净增 |
| --- | --- | --- |
| E12 结束 | 614 | — |
| E13 `decode.rs` 占满性 + 截断 | 637 | +23 |
| E13 `emitter.rs` 重写 | 656 | +19 |

分模块：`cargo test --lib table::detection` 25、`table::structure` 29 + 5 条 `real_model_tests`、`table::emitter` 27、`table::registration` 14。

**表格域附加项**：

- 本阶段是否改动了 rec 调用次数（**G2**）：**否** —— `backfill.recognizer_calls` 仍为 **0**（走"整页 det 复用"路线）；
- 本阶段是否改动了结构推理调用次数（**G3**）：**否** —— 每个表格仍恰好一次结构推理，与单元格数无关；
- 本阶段是否新增模型（**G4**）：**否** —— emitter 是纯函数，`registration.rs` 只读文件哈希，都没有引入 CNN 或新 session；
- 新增/更新的 fixture 与基线：`tests/fixtures/table-onnx/` 复用（未新增）；`tests/baseline/windows-baseline/table-teds-full-cpu.json` **重建**（19,797 B）；
- 未决项（07 §14）：**关闭 1 项**（manifest 登记位置）、**新增 4 项**（见 14.3.7）。

#### 14.3.7 未覆盖风险

1. **结构降级档只做到"标记"，没做到"重建"**。低置信样本仍输出 SLANet 结果，**未**降级到线检测重建的规则网格。线检测是轻量档的完整实现，尚未开工。
2. **`truncated` 这条上界从未被触发**（全量 `truncated = 0`），所以"结构长度超 500 token"只有由图的 `Loop` 上限推出的可能性，**没有样本证据**。
3. **`TextPolicy::Escape` 没有被真实 OCR 文本端到端验证过**。单测覆盖了五个 XML 实体，但页面管线里表格**尚未接线**（`src/ocr/pipeline/rapid_structure.rs` 把 `table_structure_infer_ms` 写死 `0.0`），所以"用户可见输出用 Escape"目前是接口而非在跑的行为。
4. **`models/table/manifest.json` 的 sha256 只保护到"文件与登记一致"**，不保护"登记本身被篡改"——登记文件没有自校验，也没有随包分发（`models/**` 被排除出包）。当前威胁模型是"下载损坏 / 磁盘损坏"，不是"本地攻击者"。
5. **`cargo package --list` 用的是 git 清单，不是 cargo 自己的清单**。理由是 `cargo package --list` 在本仓库要遍历 5.6 GB 的 `datasets/`（十几分钟）。两者在 `.gitignore` 语义上应当一致，但**这个等价性本身未被验证**。
6. **`benchmarks/**` 与 `prototypes/**` 被排除出包**，所以它们记录的绝对路径不再影响判据——但这**没有修好那两个目录里的绝对路径**，只是不让它们进包。若将来要让它们入库，必须先清理。
7. **发射器的重复属性处理只在 1 个真实样本上被观测到**。"两个同名属性取值不同"这种更坏的情况**没有样本**；当前实现取第一个（与旧代码一致），`lxml` 取最后一个——**取值不同时行为会分歧**，而这只在 7,104 个样本里从未发生。
8. ~~**定位器（`src/table/detection.rs`）是全表格域唯一没有生产消费者的模块**（见 §14.3.8），因此它的死代码检查是被**局部豁免**的，不是被覆盖的。~~ → **已由 E14 关闭**：整页管线接线后 `detection.rs` 有了生产消费者（`src/ocr/pipeline/rapid_structure.rs` 的 `recognize_with_table`），那条模块级 `#![allow(dead_code)]` 已删除，并且连带清掉了两个真死代码（`TextDistributionDetector::config()` 与 `crop_axis_aligned()`）。全表格域现在都受 `-D warnings` 的死代码保护。见 §14.3.8 的末段。

#### 14.3.8 收尾：移除表格域的模块级 `dead_code` 豁免

`src/table/mod.rs` 自 E2 起带着 `#![cfg_attr(not(test), allow(dead_code))]`，文档在 §3.4 / §4.4 两处写「E9 接线后必须删掉这一行」。E13 实测了这件事能否做到：

**实测（先移除、再看谁报错）**：删掉该行后 `cargo clippy --all-targets -- -D warnings` 报 **21 条 `never constructed` / `never used` 错误，全部落在 `src/table/detection.rs`**（`PageLine` / `Bounds` / `LineRow` / `TableDetectorConfig` / `DetectedRegion` / `TableRegion` / `TableDetector` / `TextDistributionDetector` / `group_rows` / `shared_column_count` / `detect_regions` / `row_height_consistency` / `clamped_rect` / `crop_axis_aligned` / `crop_rect` / `median_of`），其余 8 个模块**一条都没有**。

**结论**：E9/E13 的接线确实把 8 个模块变成了生产代码（`src/evaluation/table/run.rs` 消费它们），模块级豁免因此**已经过时**；仍需要豁免的只有定位器一个模块。

**处置**：
- `src/table/mod.rs` 的 `#![cfg_attr(not(test), allow(dead_code))]` **删除**（8 个模块恢复受 `-D warnings` 的死代码保护）；
- `src/table/detection.rs` 顶部加**模块级** `#![allow(dead_code)]`，注释写明"接线不在 E1–E13 范围内"并指向 §12.4.7 第 1 条与本节；
- 同时**纠正一处失实的归属**：§12.4.7 第 1 条与 `detection.rs` 的模块文档原先都写"接线是 E13 的职责"，但 §14.1 的六项任务里**没有任何一项是页面管线集成**。已改为"E1–E13 范围外的后继工作"。
- 验证：`cargo fmt --all -- --check` exit 0；`cargo clippy --all-targets -- -D warnings` exit 0 / 0 warning；`cargo test --all-targets` → **656 + 2 + 4 + 14 passed / 0 failed**（豁免收窄不改任何行为）。

**为什么值得做**：模块级豁免是**全有或全无**的——它让 8 个已经上线的模块也永远不再被 `-D warnings` 检查死代码。收窄成一个模块的豁免后，"哪个模块还没接线"变成一个编译期可验证的事实，而不是文档里的一句话。

**E14 收口（本条已完全关闭）**：`docs/11-table-page-pipeline-execution.md` 的 E14 把表格能力接进了引擎整页管线（`src/ocr/pipeline/rapid_structure.rs` 的 `recognize_with_table`），`src/table/detection.rs` 因此获得了生产消费者。于是：

- `src/table/detection.rs` 顶部的模块级 `#![allow(dead_code)]` **删除**；
- `src/table/mod.rs` 的模块清单文档补上"生产路径有两条"（整页管线的 `recognize_with_table`，以及离线评测 `src/evaluation/table/run.rs`）；
- 删除豁免后 `cargo clippy --all-targets -- -D warnings` 报出**两个真死代码**并已清理：`TextDistributionDetector::config(&self) -> &TableDetectorConfig`（生产无消费者）与 `pub fn crop_axis_aligned(image, bounds) -> Result<RecImage>`（3 行包装，只被测试使用；`detect()` 自己就需要 `clamped_rect` 的返回值来算 polygon，直接调用即可）。三处依赖它们的测试改为直接调 `clamped_rect`，并把 `the_default_config_is_accepted_by_the_constructor` 拆出一条**用行为差异证明配置真的到达算法**的新测试。

至此**全表格域都受 `-D warnings` 的死代码保护**，没有任何模块级豁免。E14 的完整记录（G5 预算 120 ms、G6 内容回归、8 条未覆盖风险）见 `docs/11-table-page-pipeline-execution.md` §2.5。


---

## 15. 推荐实施顺序与停线条件

### 15.1 推荐顺序

1. **E1** 接入 `TimingLedger`（一切观测的前提）
2. **E2** 多输出契约 + fixture
3. **E3** 解码器纯函数层（可与 **E6** 并行）
4. **E4** 真实模型 golden 钉死坐标系
5. **E5** 形状固定 H/W
6. **E6** TabRecSet detokenize 适配层
7. **E7** 跑通 TEDS 拿 baseline ★
8. **E8** 评测协议定稿
9. **E9** 回填重构 → **E10** det 分辨率 → **E11** 表格定位
10. **E12** 线程与 provider
11. **E13** 输出、降级与登记收尾

**E7 之前不得启动 E9/E10**：两者都是"用精度换性能"的旋钮（07 §2 K1/K2），拿到 baseline 前无法判断是否越界。

### 15.2 必须暂停并重新评估的情况

- 任一阶段使普通 OCR 的 CER / 区域数 / polygon IoU 超出容差（mean CER ≤ 0.03、polygon IoU ≥ 0.85、区域数差 ≤ 5 %）。
- **G1 失败**：表格关闭时 `ocr_total_ms` p50 移出 ±10–15 % 噪声带。
- **G2 / G3 / G4 任一失败**：结构性不变量被打破（说明有人把回填改回了逐 cell rec，或让结构推理按单元格调用，或引入了新的整页 CNN）。
- **R5 的线程改动落在噪声带内** → 回退，不是"再调一调"。
- provider 只能通过 CPU fallback"通过"时，必须标记 provider 不可用，不得继续优化该结果。
- 任何阶段需要重跑公式全量集时，先确认改动确实触及公式模型调用链，再只跑受影响数据集。

---

## 16. 阶段完成记录模板

每个阶段完成后追加以下记录：

```text
阶段：
日期：
提交：
变更摘要：
执行命令：
关键结果：
基线对比：
门禁判定（G1/G2/G3/G4/G5 中本阶段涉及的）：
未覆盖风险：
是否触发 12 图 bench / TEDS 抽样 / 公式 smoke / 全量评测：
```

表格域附加项：

```text
本阶段是否改动了 rec 调用次数（G2）：是/否，次数 =
本阶段是否改动了结构推理调用次数（G3）：是/否，次数 =
本阶段是否新增模型（G4）：是/否
新增/更新了哪些 fixture 与基线 JSON：
未决项（07 §14）是否有新增或关闭：
```