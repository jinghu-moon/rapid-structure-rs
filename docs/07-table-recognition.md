# 表格识别功能设计（v2：性能与精度平衡）

本文是 RapidStructure-RS 的表格识别落地设计。它基于当前仓库代码、`refer/RapidTable-main` 的实现阅读结果、`models/table/slanet-plus.onnx` 的实际探针，以及 `datasets/table/tabrecset/` 的落地核验。文档描述的是待实现模块，不代表 `src/table/` 已经存在。当前项目处于开发期，允许按本文目标直接调整模块边界和公共接口，但必须先建立旧功能基线，再验证新增管线和回归结果。

## 0. 怎么读这份文档

### 0.1 v2 相对 v1 的变更

| 变更 | 内容 |
| --- | --- |
| **补齐精度轴** | v1 只有模块骨架，没有任何精度目标。v2 新增 §1（性能预算与精度目标）与 §2（权衡旋钮表）。没有这两节就无法讨论"平衡" |
| **并入实测证据** | 原 `docs/07-table-recognition-perf-review.md` 的实测事实基础成为本文 §5，11 条改进项成为本文 §12。本文自包含，不再需要交叉阅读两份文档 |
| **关闭 4 类 `<待确认>`** | 模型存在性 / SHA / 精确字节数、输出数量与名称、metadata 格式、488 输入契约 —— 均已实测（见 §5.2、§8） |
| **更正 v1 的量化事实** | "约 6.8M" 的归属、"63.69" 的口径、"SLANet-plus 比 SLANet 快" 的假设 —— 见 §3.1 |
| **新增构造性精度上界** | 词表 span ≤ 20、Loop trip 上限 500 token，两条都由实测常量推出，决定了 TEDS 天花板 < 100（见 §1.2） |
| **新增数据落地实测** | TabRecSet 的实际 schema 与标注编码（见 §11.1） |
| **性能设计改为可断言的不变量** | §1.3 的 G2/G3/G4 是单测可断言的结构约束，不依赖反复计时 |

原评审文档保留为推导过程记录，本版取代其结论地位。

### 0.2 证据分级（本文档强制标注）

| 标记 | 含义 | 可用范围 |
| --- | --- | --- |
| **[实测]** | 来自 Rust harness、模型探针或数据核验 | 可作决策依据 |
| **[推算]** | 由实测的成本模型外推 | **只可信其形状（比例关系）**，绝对值必须用 Rust harness 重测 |
| **[未测]** | 尚未量化 | 是空缺，不得当作已定结论 |

### 0.3 方法学警告：本机有两个 ONNX Runtime，只有一个可信

| 路径 | 版本 | 用途 | 可信度 |
| --- | --- | --- | --- |
| Rust 链接（静态） | API 1.28.0 | 仓库正式管线 | **唯一可用于性能结论** |
| Python `onnxruntime` | 1.20.1（`C:\A_Softwares\Python\python.exe`，Python 3.12.8） | 临时探针 | **绝对延迟不可用** |

同一批模型，两者相差约 20–30×，无法用线程数解释：

| 测量 | Python 探针（scratch） | Rust 已入库基线 `bench-cpu.json` | 倍差 |
| --- | --- | --- | --- |
| rec 推理 | ≈1.37 ms / 像素列（H=48） | 全页 274 ms / 37 区域 ≈ 7.4 ms/区域 | ~30× |
| det 推理 | ≈5000 ms/Mpx | 570 ms/页（max_side_len 2000，约 3 Mpx） | ~20× |

**结论**：本文所有带 **[推算]** 的 Python 探针数字只用于描述**成本模型形状（比例关系）**；任何绝对值都必须用 `src/bin/bench_warm_e2e.rs` 重测。这正是 `AGENTS.md` §4「性能优化必须有依据，禁止为了臆测性能而优化」的直接适用。

### 0.4 两种精度口径不可混用

官方资料里存在两套互不兼容的表格精度口径，混用会直接导致错误的选型判断：

- **legacy PubTabNet 口径**：**只对 SLANet 存在** —— Acc 76.31 / **TEDS 95.89** / 766 ms（CPU, MKL）/ 9.2M（来源：[ppstructure/table/README.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/f8b41a62/ppstructure/table/README.md)）。
- **modern 内部集口径**：PaddleX 模块文档的 **Accuracy** —— SLANet 59.52 / SLANet_plus 63.69 / SLANeXt 69.65，**这是 PaddleX 内部数据集上的准确率，不是 TEDS**（来源：[table_structure_recognition.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/refs/heads/main/docs/version3.x/module_usage/table_structure_recognition.en.md)）。

`src/evaluation/mod.rs` 已明确"两类指标不得混合成一个默认汇总"。表格域是第三个域，同样不得混入 ocr/formula 汇总（见 §11.2 L3 第 6 条）。

---

## 1. 性能预算与精度目标

### 1.1 精度指标口径与目标值

**指标集合**（各指标职责不同，不得合并成一个数）：

| 指标 | 层次 | 实现位置 | 说明 |
| --- | --- | --- | --- |
| **TEDS / TEDS-Struct** | 结构主指标 | PubTabNet [`src/metric.py`](https://raw.githubusercontent.com/ibm-aur-nlp/PubTabNet/master/src/metric.py)（`class TEDS`，`structure_only=True` → TEDS-Struct；基于 APTED 树编辑距离） | 表格结构评测的事实标准 |
| cell 文本 CER | 内容 | 复用 `src/evaluation/ocr.rs::character_error_rate` | 逐单元格文本错误率 |
| cell 级精确匹配 | 内容 | 对齐 PaddleOCR [`ppocr/metrics/table_metric.py`](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/ppocr/metrics/table_metric.py) 口径 | **PaddleOCR 自己的表格指标是结构 token 串精确匹配，不是 TEDS**。保留它是为了能与官方数字对话 |
| 端到端页面 CER | 端到端 | 复用 `src/evaluation/ocr.rs` | 与现有 OCR 评测同一口径 |
| 区域检测 P/R/F1 + mean polygon IoU | 定位 | 复用 `src/evaluation/ocr.rs::polygon_iou` | 表格定位层单独观测 |

**目标值口径（关键，且是刻意的）**：**首轮实测前不设定绝对精度目标。**

第一性理由：官方唯一可比的 PubTabNet 数字（SLANet TEDS 95.89）是在**英文 PubTabNet 测试集**上取得的；SLANet_plus 的 63.69 是**内部集 Accuracy**，两者不可比。本项目在 TabRecSet val（7,548 张）上跑出的 TEDS 将是**第一个与本项目代码路径对应的真实数字**——在那之前写下的任何绝对目标都是猜，违反 `AGENTS.md` §4。

> **首轮实测已完成（E7，见 `docs/09-table-recognition-execution.md` §8.5）**：TabRecSet val 的 **7,104** 个 pubtabnet 名单内样本上，**TEDS-Struct = 0.706518**（失败样本按 0 计入；只算可解码的 7,084 张为 0.708513）。完整 TEDS **未测**，因为它需要单元格文本，而预测单元格文本依赖 §12 R1 的回填机制。基准文件：`tests/baseline/windows-baseline/table-teds-cpu.json`；该数字**不能**与 95.89 或 63.69 对比（不同数据集、不同 split、structure-only）。
>
> **完整口径实测已完成（E9，见 §10.5.7）**：同一 7,104 样本上，复用整页 OCR 文本 + 几何匹配回填后，**完整 TEDS = 0.570710**（decodable-only 0.572321 / 7,084），比同集合的 TEDS-Struct 低 **0.135808**。回填本身几乎免费（`backfill_ms` p50 **0.0077 ms**），且 `recognizer_calls = 0` —— 复用路线**没有**为单元格文本多跑一次识别器。基准文件：`tests/baseline/windows-baseline/table-teds-full-cpu.json`。
>
> ⚠️ **上面这个 `0.570710` 是 E9 当时的口径**（默认 det 分辨率 `736/"min"/2000`）。E10 把表格路径的默认分辨率改成 `736/"max"/736` 之后，同一份基线的完整 TEDS 升到 **0.583404**，同时 `page_ocr.ocr_ms` p50 从 549.159 ms 降到 **108.907–112.736 ms**。**两个数字属于不同口径，不可直接当成前后回归对比**；判定依据见 §12 R2 与 `docs/09-table-recognition-execution.md` §11.4.4。
>
> ⚠️ **这两个基线文件（`table-teds-cpu.json` / `table-teds-full-cpu.json`）里的数字都测于 E25 之前。** E25 改了单元格行归属，同一 `page-reuse` 口径变成 `0.583640`（`+0.00024`，预测 3,744,387 B）。按 `docs/11` §4.12 的裁定**不重建**入库文件，但引用时必须连着"测于 E25 之前"一起读。
>
> 同一轮实测还量出了两件影响后续判断的事：① **`metric.py` 不以下界 0 为界** —— 预测树远小于真值树时编辑距离可超过归一化项，实测 `min = −0.1364`；② **主导误差是"漏掉单元格/行"而不是跨度属性** —— 预测单元格数与真值完全相等的只有 **46.73 %**（少算 2,807 / 多算 967），而"真值去掉全部 `colspan`/`rowspan`"的消融仍有 **0.9743**、"去掉 `<thead>`/`<tbody>`"仍有 **0.9007**。→ 结构改进的优先级应落在行/单元格计数上，不是跨度建模。

因此在首轮实测前，**唯一可写死的是回归型门禁**：

```text
TEDS_candidate ≥ TEDS_baseline − ε
```

其中 `ε` 由评测集规模与抽样稳定性决定。确定方法（而不是拍一个数）：同配置重复跑 **3 次**，取 `src/evaluation/stats.rs::Stats::from_samples` 的 `stddev_ms` 作为 `ε` 的下界，再乘以安全系数。抽样必须用 `src/evaluation/formula/sampling.rs` 的 `SampleStrategy::Hash`（稳定哈希，明确禁止"取前 N"），否则 `ε` 会被抽样漂移污染。

> **`ε` 的实测下界是 0**：E7 的同配置三次重跑产出**逐字节相同**的预测文件（2,364,628 B，`sha256 = 2cfafefff31be2eee3e7dae13ef52371c4755f39208776dec6698d89d85288f0`），因此三次的 TEDS 完全相同、stddev = **0.0**。CPU 上这条路径**当前是确定的**；安全系数若有意义，它容忍的是**未来引入**的抖动，不是当前的抖动。
>
> **E8 已据此定稿协议**：`ε = 0`，且 0 的含义是**逐位相同**而不是"差得很小"。协议全文在 **`docs/10-table-evaluation-protocol.md`**（§1 ε / §2 网格规则 / §3 构造性上界扣除 / §4 报告 schema 与域判别 / §5 命令 / §6 未确认项）。本节下面关于"取 `stddev_ms` 乘安全系数"的描述是**定稿前的推导**，被协议 §1 取代：安全系数在 stddev 恒为 0 时不产生作用，与其保留一个不起作用的乘数，不如把 0 的含义写清楚。**推论：一次重复跑出现任何非零差异都是缺陷，不是噪声。**

绝对目标在首轮实测后由产品确认。

### 1.2 精度预算分解

| 层 | 输出 | 主要误差来源 | 约束 |
| --- | --- | --- | --- |
| 定位 | `TableRegion[]` | 启发式在无线框 / 嵌套 / 跨页表格上退化 | **已实测**（`docs/09-table-recognition-execution.md` §12.3/§12.4）：多块并排页面**假合并**（真表格内部空隙 404.8 px > 并排块 gutter 159.7 px，无阈值可分开）；无文本表格**原理上不适用**（det 只有 1 个框）；跨页表格**未测**。单表格强项：`table.jpg` 上 IoU 0.957 |
| 结构 | `StructureTree` | **词表 span ≤ 20**、**Loop trip 上限 500 token** | **两条构造性上界（见下）** |
| 文本 | `CellContent[]` | det 分辨率下降导致小字号漏检；rec 是单行识别器，多行单元格被压扁 | [未测] |
| 输出 | HTML / Markdown / JSON | Markdown 无法表达 rowspan 语义 | 设计约束，非误差。**已由 E15 落地三个通道**（§7.6）：Markdown 的跨度损失**不再静默**（`has_span` 时前后各加一行 HTML 注释说明），HTML 是无损输出，JSON 的 `items[].table` + 顶层 `tables[]` 与 `formulas[]` 并列；面向用户的 HTML 走 `TextPolicy::Escape` 并经 `sanitize_table_html` 白名单净化，评测口径的 `Raw` 刻意不净化（保与 PubTabNet 真值的可比性） |

**构造性上界之一 —— 结构 span 超出词表。**
`slanet-plus.onnx` 的 `character` metadata 实测只到 `rowspan="20"` / `colspan="20"`（48 token + sos/eos = **50 类**，[实测]，见 §5.2）；而 TabRecSet 的 `WORDMAP_STRUCTURE.json` 中存在 `rowspan="41"`、`rowspan="23"`、`rowspan="22"`、`colspan="24"`、`colspan="21"`（[实测]，见 §11.1）。→ 这类表在 SLANet-plus 上**必然结构错误**，属模型表达能力上限。**TEDS 天花板 < 100 是构造性的。**

**构造性上界之二 —— 结构长度超出解码上限。**
ONNX 图中 `Loop` 的 trip 上限常量为 `assign_0.tmp_0.0 = 500`（[实测]），即最多解码 500 个 token。结构串里每个单元格至少占 `<td>` 与 `</td>` 两个 token，故**单元格数超过约 250 的表格结构必然被截断**（[推算]，由实测常量与 token 结构推出；具体阈值取决于 `<tr>`/`<thead>` 等开销量与 `merge_no_span_structure` 的编码方式）。

→ 评测协议必须**显式扣除**这两项，否则会把构造性上界误判成代码回归。这是"精度预算"里最容易被忽略、也最容易造成误判的两条。

### 1.3 性能预算与门禁

**现状**（[实测]，`tests/baseline/windows-baseline/bench-cpu.json`，release，12 页 × 3 轮 = 36 样本，max_side_len 2000，intra_threads 16）：`ocr_total_ms` p50 = **990.11 ms**、p90 = **1136.14 ms**；平均 **34.8** 个文本区域/页（p50 37，p90 61）。

**预算分解式**（表格路径按 §12 R1 的设计路线）：

```text
page_total_ms = page_ocr_ms + table_detect_ms + table_structure_infer_ms + table_backfill_ms
```

设计目标：

- `table_detect_ms` ≈ 对已有 `regions` 做一次 O(regions) 聚类，**不引入新的整页 CNN** → 边际成本接近 0；
- `table_backfill_ms` = **净增 rec 调用为 0**（复用页面 OCR 已有的 `LineResult`）；
- `table_structure_infer_ms` = **表格数 × 单次 488² 成本**，与单元格数**无关**。

**门禁**（前三条是结构性不变量，可用单测断言，不依赖计时）：

| 门禁 | 断言 | 类型 |
| --- | --- | --- |
| **G1** | 表格开关关闭时，既有 `ocr_total_ms` p50 落在基线 ±10–15 % 噪声带内 | 计时 |
| **G2** | 回填路径的 rec 调用次数增量为 **0** | **结构不变量（单测可断言）** |
| **G3** | 结构推理调用次数 **= 表格数**，且不随单元格数增长 | **结构不变量（单测可断言）** |
| **G4** | 表格定位不新增整页 CNN（模型数量不增加） | **结构不变量（代码审查）** |
| **G5** | 整页 p50 增量 ≤ `table_structure_budget`（数值待 §12 R9 接入 `TimingLedger` 后的首轮实测确定） | 计时 |

G2/G3/G4 的价值：**把性能设计写成可断言的结构约束，而不是需要反复重测的数字。** 一旦有人把回填改回"逐 cell 喂 rec"，G2 会立刻失败；一旦有人让结构推理按单元格调用，G3 会立刻失败。这比事后对比 p50 更早、更确定地发现性能退化。

---

## 2. 权衡旋钮表（本文档的核心）

"性能与精度平衡"的操作定义就是下面这张表：每个旋钮的左右两侧都必须有实测。**右列大量为 [未测] 是当前的真实状态** —— 这也是为什么现在只能谈性能、不能谈平衡。

| # | 旋钮 | Δ性能 | Δ精度 | 证据状态 |
| --- | --- | --- | --- | --- |
| K1 | 表格路径 det 分辨率（`2000` → `736`，`limit_type=min`） | `detector_infer` 570.57 ms 占页面 **57.9 %**；det 对面积近似线性 → [推算] 约 0.1× | [未测]。风险是小字号单元格漏检 → cell 文本 CER 上升 | [推算] / [未测] |
| K2 | 回填路线：整页 det 复用 + 几何匹配（R1）↔ 逐 cell 裁剪喂 rec | 逐 cell 路线每格约 7.4 ms/区域（Rust 口径），40 格表 ≈ **+300 ms/页（+30 %）**；R1 路线净增 **0** | R1 **消除多行压扁**（精度↑）；但引入"det box ↔ cell 匹配"这一新的错误来源 | [实测] 性能 / [未测] 精度 |
| K3 | 结构推理输入固定 H/W = 488（R4） | [未测]。仅知消除动态形状告警与重规划；在 DirectML 上是前提条件 | 应为 0（形状固定不得改变数值，须逐位断言验证） | [未测] |
| K4 | 结构 session intra threads（16 → 4） | 单次约 20 %，但**落在 ±10–15 % 噪声带内** | 0（CPU 归约顺序确定） | **[实测] 判定为噪声 → 不改**（E12，6 次重复：偏差 −2.60 % / −6.89 %，均小于 `default` 自身 9.30 % 的离散度；见 §12 R5 引用块） |
| K5 | 结构模型批处理（bs 1 / 2 / 4） | 收益上限 **12 %**（th=8, bs=2）；单线程下批处理**劣化**（59.3 → 90.5） | 0 | [实测] 比例 |
| K6 | rec 批次组成 / 宽度上限（`max_wh_ratio`） | 批成本 ≈ `batch_size × 批内最大宽度`；整批 pad 到 640 时每格成本 **9.4×** | 0（只影响速度） | [实测] 比例 |
| K7 | rec 是否裁断超宽单元格 | 线性 1.37 ms/像素列 | 裁断 → 长文本 CER 上升 | [未测] |
| K8 | 精度档 VLM 开关 | 同级模型 594 MB 量级 → 单页耗时**推入秒级** | 边际收益未证明（见 R11） | [未测] |
| K9 | 表格定位策略：文本分布启发式（R8）↔ 专用整页 CNN | 启发式边际 ≈ 0；专用 CNN **+30~100 %** 页面耗时 | 启发式在无线框 / 嵌套 / 跨页表格退化 | [推算] / [未测] |
| K10 | 结构 token 上限 L：固定 500 ↔ 保持动态 | 固定 500 会**强制跑满解码步**，成本远超收益 | 无增益（L 动态实测 27–150，已满足需求） | [实测] → 保持动态 |

**读法**：K4 已经实测判定为噪声带内，**直接不改**；K5/K6 是 rec/结构的既有成本形状，属于"不要做错"的约束；K1、K2、K9 是高收益项，但**每一个都缺精度侧的实测**，必须先补 L3 数据集再动手（见 §13）。

---

## 3. 选型结论

### 3.1 默认结构模型：SLANet-plus

默认结构模型选择 PaddleX/RapidTable 使用的 **SLANet-plus**，文件名 `slanet-plus.onnx`。默认本地管线为：

```text
输入页
  -> 表格定位（复用已有 det 的文本框分布）
  -> 表格裁剪
  -> SLANet-plus 结构推理
  -> HTML token 解码（含 rowspan/colspan）
  -> 复用 PP-OCRv6 整页 det 结果按几何匹配回填单元格文字
  -> HTML / Markdown
```

候选方案按项目硬约束筛选：模型必须能由 ONNX Runtime 在本地运行，纯 CPU 可用，并能接入现有 `runtime`、`ocr`、`output` 和模型校验层。

| 方案 | 本地 ONNX/CPU | 与现有架构 | 取舍 |
| --- | --- | --- | --- |
| **SLANet-plus** | 已有 ONNX 制品；本机文件 7,758,305 B；CPU 路径成立 | 输入输出可封装为独立 ORT session，文字回填复用现有 OCR 结果 | **默认**。结构与文字解耦，便于分别诊断和替换 |
| RapidTable 的 ppstructure 中英文模型 | ONNX；约 7.3M/7.4M | 与 SLANet-plus 相同 | 可作为语言/精度对照，暂不作默认 |
| UniTable | 配置为 `encoder.pth`/`decoder.pth`，约 500M，PyTorch | 不符合纯 ONNX 默认路径 | 不纳入。且它未在官方 PP-Structure 榜单出现，**作为"对照"会引入不可比基准** |
| GLM-OCR、MinerU、LightOnOCR、Surya 等端到端大模型 | 本仓库没有已核验的本地 ONNX 契约，资源与运行时成本不明确 | 需要新的 VLM/远程抽象 | 不纳入默认路径，仅作 feature-gated 精度档（§10） |

**必须更正 v1 的三处量化事实**（它们会影响选型判断）：

| v1 说法 | 官方事实 | 出处 |
| --- | --- | --- |
| SLANet-plus "约 6.8M" | 6.8M 是 `en_ppocr_mobile_v2.0_table_structure`（**TableRec-RARE**，不是 SLANet-plus）；SLANet 为 9.2M(en)/9.3M(ch)；SLANet_plus 的 HF 模型卡记 6.9M（`inference.pdiparams` = 7,666,515 B）。本机文件 **7,758,305 B** | [models_list.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/docs/version2.x/ppstructure/models_list.en.md)、[HF PaddlePaddle/SLANet_plus](https://huggingface.co/PaddlePaddle/SLANet_plus) |
| "SLANet-plus 63.69"（隐含可比 PubTabNet TEDS） | 63.69 是**内部数据集 Accuracy**，不是 TEDS（见 §0.4） | [table_structure_recognition.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/refs/heads/main/docs/version3.x/module_usage/table_structure_recognition.en.md) |
| 隐含"SLANet-plus 比 SLANet 快" | 官方 A100 + ONNX Runtime E2E 延迟：SLANet **23.81 ms** vs SLANet_plus **23.42 ms**，差 **0.39 ms**，在噪声内 | 同上 |

**对选型的影响**：官方资料里**没有**"SLANet-plus 优于 SLANet"的性能证据。选 SLANet-plus 的正当理由只能是"内部多语种/中文表格 Accuracy 更高"或"该 ONNX 是本地唯一可用的两输出制品"，**不能是速度**。

### 3.2 结构与文字解耦

SLANet-plus 只负责结构，不负责单元格文字。文字回填复用现有 PP-OCRv6 结果，使结构和内容的失败可以分别诊断，也避免把大模型引入基础 CPU 路径。

---

## 4. 现有实现可复用的事实

公式域是最接近的参考骨架。`src/formula/detect.rs` 和 `src/formula/session.rs` 都通过 `runtime::session::OrtSession` 打开模型，先做 IO probe，再执行领域契约校验；`src/formula/contract.rs` 将输入固定为 `FLOAT32 [N,1,384,384]`，输出固定为 `INT64 [N,L]`。公式检测还在加载时调用 `require_requested_provider`，因此请求 DirectML/CUDA 后发生 CPU 回退会失败，不会静默改变语义。

模型完整性走 `model_verify::verify_sha256`。该校验复用进程级身份缓存（路径、体积、mtime 和首尾各 64 KiB 局部摘要），在模型实际加载入口再次校验期望 SHA-256。新表格模型必须沿用同一入口，不得只在下载时校验或只在 `/api/models` 显示校验。

现有 OCR rec 的关键事实来自 `src/ocr/rec/preprocess.rs` 和 `src/ocr/rec/recognizer.rs`：输入为 `RecImage`，预处理生成 CHW `FLOAT32`，每批按宽高比排序后调用 `OcrSession`，CTC decoder 返回 `LineResult { text, score }`。

公式骨架中还有两条可直接借鉴的契约：`formula::detect` 的页面检测输入是 `FLOAT32 [N,3,768,768]`，采用保持比例的 768 letterbox、`114/255` 填充和 RGB NCHW；已导出的检测输出是 `FLOAT32 [N,6,A]`，6 个通道为绝对像素框和两类分数，之后做类别感知 NMS 并缩放回原图。`formula::recognizer` 则在加载时先校验模型 SHA，再由 `FormulaSession` 校验 `[N,1,384,384] -> [N,L]`，读取 `character` metadata，使用独立 tokenizer 解码，不读取普通 OCR 字典。表格结构 session 应复用这条"共享 ORT + 领域契约 + metadata 显式失败"的模式，但**不能套用公式的 384/768 尺寸或 token 语义**。

**关于 manifest（更正 v1 的假设）**：`models/configs/manifest.json` **不是模型注册表**，而是模型集清单——`{id: "pp-ocrv6-medium-multilingual", family, version, languages, detector, recognizer, dictionary, classifier}` 四键结构，各带 `file_name` / `sha256` / `source_url`。表格模型本来就不属于它。**已更正（E18 复核）**：`models/serve/manifest.json` **是存在的**（1,193 B），v1 写"不存在"是错误假设——它的 `id` 是 `pp-ocrv6-medium-plus-formula`，声明了五个 role（detector / recognizer / dictionary / formula_recognizer / formula_detector），是"页面 OCR + 公式"那一个模型集。**另一处更正**：`models/configs/manifest.json` 到今天仍然**没有任何代码读它**（`src/model_source.rs:322-328` 明确拒绝它那种 legacy 四字段形状并给出迁移提示），是遗留死文件——**已在 E18 按用户决定删除**（`models/` 下现在只剩 `models/serve/manifest.json` 与 `models/table/manifest.json` 两份清单）。→ 表格模型的登记位置需要重新决定，**不能假设沿用 `models/configs/manifest.json`**（见 §8）。

**评测基础设施（决定 §11 的测法）**：仓库**没有** `[dev-dependencies]`、**没有** `[[bench]]`、**没有** criterion、**没有**任何 `[profile.*]`（`Cargo.toml` 全默认）。唯一的端到端 harness 是 `src/bin/bench_warm_e2e.rs`；基线工具是 `tools/run_windows_baseline.ps1`。`tests/` 下只有 `tests/serve_startup.rs`，其余 700 个 `#[test]` 都是 `src/**` 内联的 `#[cfg(test)] mod tests`。→ 表格域的测试必须沿用这套口径，**不得新引入 benchmark 框架**。

---

## 5. 实测事实基础

### 5.1 页面级耗时构成（[实测]，权威）

来自 `tests/baseline/windows-baseline/bench-cpu.json`：

| 分量 | ms/页 | 占比 |
| --- | --- | --- |
| **detector_infer** | **570.57** | **57.9 %** |
| **recognizer_infer** | **273.53** | **27.7 %** |
| input_other（未命名） | 79.86 | 8.1 % |
| recognizer_postprocess | 41.33 | 4.2 % |
| unattributed | 6.75 | 0.7 % |
| detector_postprocess | 4.49 | 0.5 % |
| input_resize | 3.82 | 0.39 % |
| recognizer_preprocess | 2.84 | 0.29 % |
| detector_preprocess | 1.43 | 0.15 % |
| input_crop | 1.14 | 0.12 % |
| page_postprocess | 0.02 | 0.002 % |
| **total** | **985.79**（p50 990.11 / p90 1136.14） | |

派生量：`ort_inference_ms` = 844.10（85.6 %）、`rust_ms` = 134.93（13.7 %）。

**第一性事实**：表格识别新增的模型是 SLANet-plus，488×488 单次推理量级约 25–31 ms（§5.3），即**约占页面预算的 3 %**。表格模块的性能天花板远低于页面级 det/rec 的既有开销——**所以最高收益的优化不在表格模块内**（这是 §12 R1/R2 排序的依据）。

### 5.2 SLANet-plus ONNX 图与契约（[实测]，`models/table/slanet-plus.onnx`）

- 文件 **7,758,305 字节**；SHA-256 = `d57a942af6a2f57d6a4a0372573c696a2379bf5857c45e2ac69993f3b334514b`，**与本文 §8 声明的来源配置逐位一致** → v1 的"落地前仍需重算 SHA" `<待确认>` **可以关闭**。
- graph name `PaddlePaddle Graph 0`，**830 节点**，opset `('',14)`，ir_version 7，producer 为空。op 直方图：`Constant 484, Conv 77, BatchNormalization 73, HardSwish 73, Cast 31, Concat 23, Reshape 15, Slice 13, Shape 8, Squeeze 8, ConstantOfShape 5, Resize 3, Add 3, GlobalAveragePool 2, Relu 2, HardSigmoid 2, Mul 2, Transpose 1, Less 1, Not 1, And 1, Loop 1, Softmax 1`。顶层 initializer 元素数为 0（权重全内联在 484 个 `Constant` 节点中）。
- 输入名 **`x`**，FLOAT，形状**全动态** `[DynamicDimension.0, 3, DynamicDimension.1, DynamicDimension.2]`。
- **两个输出**：`save_infer_model/scale_0.tmp_0` FLOAT `(N,L,8)`（bbox，8 = 4 点 xyxyxyxy）、`save_infer_model/scale_1.tmp_0` FLOAT `(N,L,50)`（结构概率，50 类）。**→ v1 的"实际输出数量和名称需 probe" `<待确认>` 可以关闭。**
- `metadata_props` 只有 `character`，是**长度 582 的换行分隔（非 JSON）**字符串，split `\n` 恰好 **48 个 token**：`<thead> </thead> <tbody> </tbody> <tr> </tr> <td > </td> colspan="2"…colspan="20" rowspan="2"…rowspan="20" <td></td>`。+`sos`/`eos` = **50 类**，与输出最后一维一致。
  - ⚠️ **token 是片段，不是整标签**（E2 逐字节复测后更正本条）。按下标：`6 = "<td"`（**没有 `>`**）、`7 = ">"`、`8 = "</td>"`、`47 = "<td></td>"`，`9..27 = ' colspan="2"'…' colspan="20"'`、`28..46 = ' rowspan="2"'…' rowspan="20"'`（**都带前导空格**）。所以 `<td>` = 下标 6 + 7 两段拼成，`<td colspan="2">` = 6 + 9 + 7，`</td>` 收尾；空单元格是单 token `<td></td>`。
  - **早先"含 `<td></td>` 而不含 `<td>`（`merge_no_span_structure=True` 已烘焙）"的说法不准确**：`<td` 确实在词表里（下标 6），只是没有完整的 `<td>`。结论仍然成立——`merge_no_span_structure` 语义确已烘焙——但**理由不同**。
  - **对解码器的硬约束**：必须**按序拼接 token 片段后再解析 HTML**，禁止把 token 与整标签做字符串相等匹配。另：`rowspan`/`colspan` 的 token 都**从 2 起**（1 是隐含默认值，无 token），到 20 止 —— 与 §94 行的构造性上界一致。
- **L 是动态的**：零输入 → L=27；随机输入 → L=150。`Loop` 的 trip 上限常量是 `assign_0.tmp_0.0 = 500`，提前退出由 `logical_and` 依 eos 控制。batch 1/2/4/8/16 全部被接受。
- ORT 对动态形状输出会打印告警：`Expected shape from model of {} does not match actual shape of {1} for output logical_and_0.tmp_0`（`execution_frame.cc:870 VerifyOutputSizes`），仅告警。
- ⚠️ **metadata 格式与公式域不兼容**：`src/formula/model_info.rs` 的 `FormulaModelInfo::probe` 要求 `character` 是**合法 JSON**，否则返回 `Config` 错误。SLANet-plus 的 metadata 是换行分隔字符串 → **直接复用公式域的 metadata 解析会失败**，表格域必须有自己的解析器（见 §7.2）。

### 5.3 SLANet 线程 / 批处理 / 优化级别（[推算]，仅比例可用）

线程与批处理（488×488，ms/图）：

| threads \ batch | bs=1 | bs=2 | bs=4 | bs=8 | bs=16 |
| --- | --- | --- | --- | --- | --- |
| 1 | 59.3 | 61.4 | 74.4 | 79.0 | 90.5 |
| 4 | 36.6 | 35.6 | 38.5 | 41.0 | 45.7 |
| 8 | 30.8 | **27.2** | 29.6 | 31.1 | 34.3 |

- **批处理收益上限约 12 %**（th=8，bs=2），bs≥4 开始回升；**单线程下批处理反而劣化**（59.3 → 90.5）。
- ORT 优化级别（488²，th≈4）：`DISABLE_ALL 79.2 / BASIC 69.2 / EXTENDED 66.3 / ALL 43.9 ms` → **ALL 相对 DISABLE_ALL 约 1.8×**。`src/runtime/session.rs:248` 已设 `GraphOptimizationLevel::Level3`（= ALL），**这一项已经是对的，不要动**。
- `enable_cpu_mem_arena` 开 41.8 vs 关 49.3 ms → 默认开是对的。`enable_mem_pattern` 关 43.1 vs 开 43.1 ms → 无差异。

### 5.4 rec 的成本模型（[推算]，仅比例可用）

`H=48, bs=1` 单行：W=16 → 26.4 ms；W=64 → 96.8；W=320 → 471.3；W=640 → 881.9。**近似线性，斜率约 1.37 ms/像素列，截距约 3.9 ms**。

混合批（bs=32）——**这是最关键的一条**：

| 批内容 | ms/cell | 相对全短批 |
| --- | --- | --- |
| 全短 W=64 | 76.9 | 1.0× |
| 全短 W=96 | 112.4 | 1.5× |
| **31 短 + 1 长 → 整批 pad 到 320** | **386.2** | **5.0×** |
| **31 短 + 1 长 → 整批 pad 到 640** | **721.6** | **9.4×** |
| 全宽 W=320 | 374.2 | 4.9× |

**第一性事实**：rec 的批成本 ≈ `batch_size × 批内最大宽度`，与"有效内容像素"无关。

---

## 6. 分层与数据流

新增领域模块 `src/table/`，只依赖共享层、`ocr` 和 `runtime`，不让 `runtime` 反向依赖表格域：

```text
TableEngine
  ├─ detection              页面 -> TableRegion[]
  ├─ structure_ort          TableCrop -> StructureRaw -> StructureTree
  ├─ cell_content_backfill  CellRect[] + 页面 LineResult[] -> CellContent[]
  └─ html_emit               StructureTree + CellContent[] -> HTML/Markdown
```

一次请求的数据流：

1. `detection` 在原图坐标系产生候选表格矩形/多边形，并裁剪出表格图像；每个 region 保留原图偏移，便于最终结果定位和可视化。**复用页面 OCR 已有的文本框分布**推断表格区域，不新增整页网络（§12 R8）。
2. `structure_ort` 对每个 crop 进行 `488×488` 预处理，运行 SLANet-plus，取回**两个输出**（结构 token 概率 + 与 token 对齐的 bbox 预测）；解码器产生 DOM-like 结构和单元格几何框。
3. `cell_content_backfill` 用**几何匹配**把页面 OCR 已有的 `LineResult` 归到各个 cell，而不是逐 cell 再跑一次 rec（§12 R1）。识别失败的单元格保留空文本和错误状态，而不是伪造结构 token。
4. `html-emit` 根据结构树顺序插入转义后的文本，输出完整 HTML；Markdown 从同一结构树按行列网格生成，**不能从已拼好的 HTML 做正则替换**。

> **与 v1 的关键差异**：v1 第 3 步写的是"将每个单元格框映射回 crop，生成 `RecImage`，批量调用 recognizer"。v2 改为几何匹配复用。理由是 v1 路线的成本正比于 `单元格数 × 批内最大宽度`（§5.4），且 rec 是单行识别器会把多行单元格压扁——见 §12 R1 的完整根因。

当前代码没有专用 table detector。**表格结果公开类型已由 E14 定案**（`src/api.rs` 的 `RegionKind::Table` + `TableOutcome` / `TableCellOutcome`，见 §6.1）；Markdown 对 rowspan/colspan 的最终呈现规则**仍属 `<待确认>`**（E14 落地的 `TableOutcome.markdown` 沿用"span 位置留空"的既有规则，尚未端到端核对，见 §14）。

### 6.1 公开类型与整页接线（E14 定案）

E14 把表格能力从"离线评测可用"接到**引擎整页管线**，公开面落在 `src/api.rs`（对外线契约，与 `src/table/` 这个私有实现模块分开）：

```rust
pub enum RegionKind { Text, Formula, Table }   // 新增第三路

pub struct TableOutcome {
    pub html: String,          // 完整 <html><body><table>…
    pub table_html: String,    // 只有 <table>…
    pub markdown: String,
    pub rows: usize, pub cols: usize,
    pub confidence: TableStructureConfidence,   // High | Low
    pub degradations: Vec<TableDegradation>,    // GridHoles { holes } | Truncated
    pub holes: usize,
    pub cells: Vec<TableCellOutcome>,
    pub source_line_indices: Vec<usize>,        // 被消费的页级文本行（仍留在 regions 里）
    pub model_id: String,
}
pub struct TableCellOutcome {
    pub row: usize, pub col: usize, pub rowspan: usize, pub colspan: usize,
    pub text: String, pub score: Option<f32>,
    pub match_kind: TableCellMatch,             // BlankBox | Unmatched | Single | Multiple
    pub polygon: Option<Polygon>,
}
```

**关键语义（每条都是一个可以写错的判断）**：

- **表格区域不携带 `recognition`**。单元格文本来自**页级几何匹配**（复用已有的 rec 结果），不是新识别出来的；若同时挂上 CTC 结果，"识别出来的"与"匹配进来的"就不可分辨了。`OcrOutput::validate` 的 `Table` 臂强制这一点（也禁止表格区域带公式结果）。
- **`source_line_indices` 里那些文本行仍留在 `regions` 中**，不删除、不改写。表格区域是**叠加**而不是**替换**：删掉它们会让"表格文本"只能从 `TableOutcome` 里取，而文本通道（CER、复制全文、文本排序）会失去内容。
- **`api` 里另立 serde 类型**（`TableStructureConfidence` / `TableDegradation` / `TableCellMatch`）而不是复用 `src/table/emitter.rs` 的 `StructureConfidence` / `StructureDegradation` 与 `cell_content_backfill::CellMatchKind`。两条理由：① `mod table;` 在 `src/lib.rs` 是**私有**的，把私有模块的类型放进公开结构体字段会触发 `private_interfaces`；② `api` 是线契约（要对客户端稳定），`table` 是实现（可以随便重构）。
- **`TablePolicy` 与 `roi` / `tile` 互斥**，且与公式路由同形拒绝（`OcrRequest::validate`）。页级文本 polygon 会被 `+ offset` 映射回原图，所以坐标**恰好**仍能对上——这不是一次会崩的错，而是会**静默返回调用方没问过的结果**的错。
- **表格回填不使用 `BackfillSource::Crop { image, recognizer }`**，只用 `BackfillSource::Reuse { lines, offset }` → `BackfillOutcome::recognizer_calls` **恒为 0**（门禁 G2/G7）。这是"删除逐 cell 裁剪喂 rec"（§12 R1）在类型层面的落实。
- **公式 + 表格同时启用时，表格在外层、定位与结构模型吃原图**：结构模型看的是线条与版式，公式像素被抹白只会污染输入；而"表格消费的文本行"来自文本/公式阶段的输出区域，两者都在原图坐标系，所以坐标一致。**该组合的端到端行为尚未实测**（没有"表格内含公式"的测试图），见 §14。

**整页路径的实测结果（E14，`docs/11-table-page-pipeline-execution.md` §2.5）**：12 图、release、`max_side_len=2000` 下，表格三分量（detect + 结构推理 + 回填）之和的中位数是 **56.491 ms**（极差 2.514 ms），预算定为 **120 ms**；峰值工作集稳定上升约 **161 MB**（+12 %，**没有门禁约束**）。单图 `04表格与键值对.png`（3200×2000）定位出 2 个区域（其中 1 个是 §7.1 登记的假合并），单表结构推理 ≈31 ms，与 §5.3 的 25–31 ms 相符。

---

## 7. 模块落地草案

### 7.1 `src/table/detection.rs`

职责是页面级定位、裁剪和坐标变换，不参与结构 token 解码。接口形态（**E11 已落地并纠正了原稿的签名**）：

```rust
pub struct TableRegion {
    pub polygon: [[f32; 2]; 4], // 原图坐标
    pub score: f32,
    pub crop: RecImage,
    pub origin: [f32; 2],
}

/// 定位器的输入行：**必须自带几何**。
pub struct PageLine {
    pub polygon: [[f32; 2]; 4], // 原图坐标
    pub text: String,
    pub score: f32,
}

pub trait TableDetector {
    fn detect(&mut self, image: &RecImage, page_lines: &[PageLine]) -> Result<Vec<TableRegion>>;
}
```

> ⚠️ **原稿写的是 `&[LineResult]`，那不可实现。** `src/ocr/types.rs:22-27` 的 `LineResult { text, score, word_info }` **不带任何几何字段**；带几何的是 `src/ocr/det/detector.rs:70` 的 `DetOutput::boxes: Vec<Quad>`，它与 `src/ocr/pipeline/types.rs` 的 `ExecutionOutput { boxes, txts, scores, .. }` 里的 `txts`/`scores` **同序并行**。表格域因此自定义 `PageLine`，**不引入 `pub(crate)` 的 `ExecutionOutput`**。定位器的实测结论（三张图、假合并机制、无文本表格不适用）见 `docs/09-table-recognition-execution.md` §12.4。

**默认定位策略**：从已有 det 文本框的分布推断表格区域（表格区域内文本框呈规整行列、行高一致、x 坐标成列）。这是零额外模型成本的做法。只有在实证表明启发式召回率不达标时，才引入专用模型——官方 PP-TableMagic 管线确实有 `PP-LCNet_x1_0_table_cls`（6.6 MB）与 `RT-DETR-L_*_table_cell_det`（124 MB）（来源：[table_recognition_v2.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleX/refs/heads/release/3.7/docs/pipeline_usage/tutorials/ocr_pipelines/table_recognition_v2.en.md)），但那是**全功能管线**的目标，不是本项目的性能目标。

**E11 实测结论（见 `docs/09-table-recognition-execution.md` §12.3/§12.4）：启发式在多块并排页面上会假合并，且不是可调参数问题** —— det 框紧贴文字而非单元格，真表格内部列间空隙实测可达 **404.8 px**，而两块并排结构之间的 gutter 只有 **159.7 px**，没有任何间距阈值能把二者分开。另有**无文本表格原理上不适用**（`table_without_txt.jpg` 上 det 只有 1 个框）。因此专用模型保持为**备选**，等待真实业务页面的失败率数据再决定。

v1 写"复用现有 PP-OCRv6 文本框并结合横/竖线检测做候选合并，**或**引入独立 ONNX detector"——v2 直接选定前者，并把后者降级为"实证不达标时的备选"。

**E14 接线后的生产消费者**：`src/ocr/pipeline/rapid_structure.rs` 的 `recognize_with_table`（整页管线）与 `src/evaluation/table/run.rs`（离线评测）。定位器的**输入**是 `table_source_lines(&output.regions)`——它只取 `kind == Text` 且有 `polygon` + `recognition` 的区域，并且**从同一处同时构造**四边形与轴对齐盒（各建一份会让"定位看到的行"与"回填用的行"因一次过滤规则分叉而不再对应）。因此定位器看到的文本框分布就是页级文本通道的产出，坐标天然同系（整页像素），无需换算。`detection.rs` 的模块级 `#![allow(dead_code)]` 已在 E14 删除（`docs/09` §14.3.8）。

**E23 修正：折行单元格不得打断行区间**（完整记录见 `docs/11-table-page-pipeline-execution.md` §4.10）。这条是**语义级**的，不是调参：

- **第一性事实**：定位器的输入是 **OCR 文本框**，而一个文本框是**一行视觉文本**，不是一个单元格。"哪些框构成一行"靠 `group_rows` 按 `center_y` 聚类；一个折行单元格的第二行落在自己的 y 上，于是**一行视觉文本被拆成两个行**。
- **旧的失效路径**：行区间延展要求相邻两行 `shared_column_count ≥ min_columns`，而一条折行的细行最多只能匹配 1 个 → **每一处折行都打断一次区间**。用户那张 12 行的生产记录表因此只检出 2 个 3 行片段（另 6 行退回普通文本区域，"不到一半"）。
- **现行语义**：细行**不再打断区间，而是区间的填充** —— 列匹配只在**锚行**（`column_count() >= min_columns`）之间做，细行照旧计入 `bounds` 与 `members`；检出判据是 **锚行数 ≥ `min_rows`**；`row_height_consistency` 只喂锚行（细行更矮，算进来会把"更像表格"罚成"更不像表格"）。**没有新增任何配置项。**
- **`DetectedRegion.rows` 因此是真实行数的下界**（锚行数）：折行的细行被计入包围盒与成员，却不计入这个数。**权威的行列网格由结构模型给出**（`TableOutcome.rows`/`cols`/`cells`），定位器的行数只用来判断"这里像不像表"。
- **两处已知遗留**（都登记在 §14）：① 表格框线**之上**的跨列表题与表头之间没有列结构，几何上与"表格上方的标题"不可区分 → 表题留在区域之外；② 折行单元格的**续行**会被回填匹配到上一行（模型预测的单元格框整体比真实行低 8–15 px，IoU 差距只有 1 %，保序约束救不了它 —— 见 §14 与 `docs/11` §4.10）。

### 7.2 `src/table/structure-ort.rs`

该模块对应公式域的 `contract.rs` + `session.rs` 组合：

- 打开模型前检查文件存在并调用 `model_verify::verify_sha256`；
- 用 `OrtSession` 检查 IO 契约：**输入 1 个、输出 2 个**（实测为 `(N,L,8)` 与 `(N,L,50)`，见 §5.2）。**现有的 `run_arrayd_view_with` 只取第一个输出，必须先扩展**（§12 R3）；
- 用共享 `RuntimeConfig` 和 `runtime::provider` 选择 CPU/DirectML/CUDA；表格域默认允许 CPU，若产品策略要求严格加速，则像公式域一样调用 `require_requested_provider`；
- 从 ONNX custom metadata 读取 `character` token 列表，**用表格域自己的换行分隔解析器**（**不要复用 `src/formula/model_info.rs` 的 JSON 解析**，见 §5.2）；缺失或损坏必须显式报错，不能回退到 PP-OCR 字典。

参考实现的输入张量契约已由代码确定：

```text
dtype: FLOAT32
layout: NCHW
shape: [N, 3, 488, 488]
resize: 保持比例，长边缩放到 488，cv2.resize 默认插值
pad: 右侧/下侧补零至 488×488
normalize: (pixel / 255 - [0.485, 0.456, 0.406]) / [0.229, 0.224, 0.225]
shape_list: [原高, 原宽, ratio, ratio, 488, 488]
```

`TablePreprocess` 的顺序是 resize -> normalize -> pad -> HWC 转 CHW。这里的 `shape_list` 必须和每个 batch 样本一一对应，不能在 Rust 中用"推理输入尺寸"代替原图尺寸。

**形状固定**：用 ORT 官方的 free-dimension override 把 `x` 的 H/W 固定为 488（**不改模型文件**），**L 保持动态**：

```rust
// 概念示意：per-dimension override，按名字定位 free dim
session_builder.with_free_dimension_override_by_name("x", ...);
```

> **关键约束（第一性事实）**：**不要固定 L**。实测零输入 → L=27、随机输入 → L=150，`Loop` 的 trip 上限常量为 500 但依 eos 提前退出。把 L 固定成 500 会强制解码 500 步，代价可能远超收益——这是"禁止臆测性能"的直接反例（§2 K10）。

### 7.3 HTML token 解码

RapidTable `TableLabelDecode` 的实际步骤如下，Rust 实现应逐步测试而不是调用 Python：

1. 读取 `character` metadata；在词表前后加入 `sos`、`eos`。当启用 `merge_no_span_structure` 时，确保 `<td></td>` 存在，并移除 `<td>`（本模型的 metadata 已烘焙该语义：词表里没有完整的 `<td>`，但**有片段 `<td`（下标 6）与 `>`（下标 7）**，见 §5.2）。
2. 对结构概率张量沿类别维 `argmax`，同时保存最大概率作为 token score。
3. 跳过 `sos`/`eos`；遇到第一个位置之后的 `eos` 停止。其余 token 按 metadata 映射成 HTML token，并包装为 `<html><body><table>...</table></body></html>`。
4. 对 `<td>`、`<td rowspan="...">`、`<td colspan="...">` 和 `<td></td>` 记录与 token 位置相同的 bbox。bbox 先按 `shape_list` 的原始高宽反归一化；SLANet-plus 再按参考实现的 `resized=488` 比例还原到原图 crop。
5. 丢弃全零占位 bbox。`rowspan`/`colspan` 属性必须保留在结构节点上，**不能只把 token 拼成字符串后再猜测网格**。

参考代码返回两个并行结果：`table_structs`（token 包装后的 HTML 结构及平均结构置信度）和 `cell_bboxes`（单元格框）。**本模型的两个输出名与最后一维已实测确定**（§5.2）；**bbox 坐标的归一化范围已实测钉死**（`docs/09-table-recognition-execution.md` §5.4）：模型输出归一化到**补零后（左上对齐）**的 488×488 画布，经 `_bbox_decode` 与 `rescale_cell_bboxes` 两步复合后得到**原图（crop）像素**坐标，等于 `归一化输出 × max(h, w)`。

建议内部类型：

```rust
pub struct StructureNode {
    pub tag: String,                 // table/tr/td 等
    pub rowspan: Option<u32>,
    pub colspan: Option<u32>,
    pub cell_index: Option<usize>,
    pub bbox: Option<[f32; 4]>,      // crop 坐标
}

pub struct StructureResult {
    pub root: StructureNode,
    pub cells: Vec<CellSlot>,
    pub confidence: f32,
}
```

**解码器的错误路径必须覆盖 §1.2 的两条构造性上界**：span 超出词表（>20）时模型只会给出词表内的最近 token，解码器**无法从输出中察觉**这一点——因此需要在 emitter 侧对"网格占满性"做校验（见 §7.5），并把这种情况标记为 `structure_confidence=low` 降级，而不是当作正常结果。

### 7.4 `src/table/cell-content-backfill.rs`

该模块只做**几何匹配**，不重新跑 rec：

- 输入是 §5.1 已有的页面级 `LineResult` 列表与结构解码得到的 cell bbox；
- 对每个 det box 计算与所有 cell box 的 `(distance, 1-IoU)`，按 `(1-IoU, distance)` 取最优建立 `matched[cell_idx] = [det_idx…]`。参考实现见 `refer\RapidTable-main\rapid_table\table_matcher\main.py` 的 `TableMatch.match_result(cell_bboxes, dt_boxes, min_iou=0.1**8)`，其中 `if sorted_distances[0][1] >= 1-min_iou: continue` 是拒绝匹配的判据；
- 逻辑网格（含 rowspan/colspan 占位）用同文件 `decode_one_logic_points` 的 `occupied_cells` + `is_occupied/mark_occupied` 推导，这是生成 Markdown 网格的正确参考；
- 匹配失败的 cell 保留空字符串与错误枚举，**不得伪造结构 token**；结构置信度与文字置信度分开保存，不把结构分数塞入 CTC 分数字段。

> **E25 定案：行边界由文本行推，不由模型框推**（`<待确认>` 已在 §14 关闭）。参考实现只做上面那一步"对所有单元格取最优 IoU"，而结构模型的单元格框**整体比真实行低 8–15 px**，于是折行单元格的续行会被判给**上一行**（实测 IoU `0.195` 对 `0.189`，谁大纯属抖动）：症状是"上一行的单元格里混进下一行的第一句"，且从出错处起每行串一格。修法是在取最优之前插入**行带**阶段 —— `assign_row_bands` 把纵向与表格范围相交的文本行按中心 y 排序，取相邻中心距**最大的 `rows-1` 个间隙**作行边界（`rows` 从单元格网格数出来，含 `rowspan`），于是"行数用结构模型的、行边界用文本行的"；行带内再按 `(1-IoU, distance)` 取列。判据无魔数（只比较选中/未选中两个间隙集合的上下确界，并列即视为不可信），实测间隙分离度是行内 `12.5–15.5 px`、行间 `31.5–59 px`。两条退路保证它**只改归属、不丢文字**：间隙并列 → 退回逐格匹配；带内够不着任何框 → 该行退回全集。记录见 `docs/11-table-page-pipeline-execution.md` §4.12。

公式识别的复用边界：`src/formula` 是独立 bounded context，普通 OCR 不直接依赖公式识别器。表格单元格若要识别公式，应在表格域提供可选的 cell post-process：先由公式 detector/路由识别候选区域，再调用现有 `formula::recognizer`，并把结果作为 cell 内的富文本片段。**不得把 LaTeX 伪装成 `LineResult`**；该整合协议目前为 `<待确认>`。

### 7.5 `src/table/emitter.rs`（E13 定案；文档原写 `html-emit.rs`）

emitter 接收结构树和已按 `cell_index` 对齐的内容，进行 HTML 属性和文本转义，再输出 HTML。属性只允许来自结构解码器验证过的正整数 `rowspan`/`colspan`；非法或超过网格上限的值返回结构错误。输出至少包括 `<table>`，是否附加完整 `<html><body>` 包装由公共输出策略决定。

> **E13 定案（`<待确认>` 已关闭）**：API 落为四个函数，**不**与 `src/output/html.rs` 合并——后者面向页面级 OCR 报告（`render_report` / `render_output_report`），输入是 `OcrOutput`；emitter 的输入是结构树 + 单元格文本，两者没有共享状态，强行合并只会让页面报告依赖表格域类型。
>
> ```rust
> pub enum TextPolicy { Raw, Escape }
> pub enum StructureConfidence { High, Low }
> pub enum StructureDegradation { GridHoles { holes: usize }, Truncated }
> pub struct RenderedTable { pub rendered: String, pub confidence: StructureConfidence, pub degradations: Vec<StructureDegradation> }
>
> pub fn emit_html(result: &StructureResult, cell_texts: &[String], policy: TextPolicy) -> Result<RenderedTable>
> pub fn emit_table_html(result: &StructureResult, cell_texts: &[String], policy: TextPolicy) -> Result<RenderedTable>
> pub fn emit_markdown(result: &StructureResult, cell_texts: &[String]) -> Result<RenderedTable>
> pub fn structure_degradations(result: &StructureResult) -> Vec<StructureDegradation>
> ```
>
> **属性从 `CellSlot::rowspan`/`colspan` 重建**，不回显 `node.tag`：模型 token 流里可能把同名属性写两遍（实测 7,104 个样本里有 1 例 `<td rowspan="2" colspan="2" rowspan="2">`），原样拼接会产出非法 HTML。顺序固定为 `colspan` 在前、`rowspan` 在后（`tools/table_teds.py` 用的官方 `metric.py` 把属性读进字典，顺序不影响 TEDS，固定只为可复现）。
>
> **`TextPolicy` 是显式策略、无默认值**，因为两个调用方的目的不同：评测口径必须 `Raw`（PubTabNet 单元格文本本身就是 HTML 片段——E6 字符普查实测 `<` 与 `>` 各出现 4,171 次且完全成对——转义会破坏与真值的可比性）；用户可见输出用 `Escape`（OCR 文本不可信，直接拼进 HTML 是注入面）。一个默认值无法同时满足两者。
>
> **Markdown 从逻辑网格生成**（把每个单元格放到 `matrix[row_start][col_start]`，再按 `grid.cols` 输出），不做 HTML 正则替换。转义规则：`\` → `\\`、`|` → `\|`、换行 → `<br>`。跨行跨列的信息在 Markdown 里**丢失**（格式能力边界），HTML 仍是无损输出。
>
> **E15 落地**：跨度损失**不再静默**。只要网格里存在 `rowspan > 1 || colspan > 1`，`emit_markdown` 就在表格**前后各加一行** `const MARKDOWN_SPAN_NOTE`（`<!-- 表格含跨行/跨列，Markdown 无法表达；已按逻辑网格展开，跨过的位置为空位。无损表示见 HTML 字段。 -->`）；无跨度时一个字都不加。用 HTML 注释是因为它在**渲染后**的 Markdown 里不可见（不污染阅读）、在**原文**里可见（可被 diff 与测试命中）。
>
> **为什么门控是 `has_span` 而不是 `degradations`**：`degradations` 描述的是**另一种**丢失（网格有洞 `GridHoles`、token 被截断 `Truncated`）。`04表格与键值对.png` 上两张真实表格的 `degradations == []`、`holes == 0`、`confidence == high` —— **结构完全良构却仍然丢跨度**。若把说明行挂在降级档上，这两张表就会静默丢失跨度信息。

**新增职责（由 §1.2 的构造性上界推出）**：emitter 是**唯一能察觉 span 超词表**的位置。它必须校验逻辑网格的占满性——若网格出现空洞（说明某处的 span 被模型截断到词表内），输出应带 `structure_confidence=low`，而不是静默产出一个看似正常的表格。

> **E13 落地**：占满性由 `decode::GridCoverage`（`cells` / `covered` / `holes()` / `is_complete()`）提供，**在持有占用集合的 `decode_logic_points` 里算出**——不能用 `Σ rowspan × colspan` 还原，因为矩形可以重叠（E13 有反例测试）。截断信号由 `decode::StructureTermination { Eos, Exhausted }` 提供（两个出口的 token 序列看起来都合法，只有"有没有见到 eos"能分开）。
>
> `structure_degradations` 把两者映射成 `GridHoles { holes }` / `Truncated`，非空即 `Low`，但**文本照样产出**——降级是正常路径而不是异常兜底。**实测（全量 7,104，`structure_only=false`）**：`high 6734` / `low 350`（4.94 %）/ `holes_total 2209` / `truncated 0`。`truncated = 0` 说明 500 步 trip 上限在这份数据上不是瓶颈；350 例网格空洞就是"span 超词表"症状的第一次量化。

### 7.6 输出三通道（E15 定案）

三个通道都从**同一份 `OcrOutput`** 出发，各自由一个纯函数产出（没有一条通道解析另一条的产物）：

| 通道 | 入口 | 表格区域怎么出 |
| --- | --- | --- |
| JSON | `src/output/json.rs::to_output_json` / `to_output_items` | `items[i].kind == "table"`，`items[i].txt` = **Markdown**（表格区域的唯一文本表示），`items[i].table` = `OcrTableItem { rows, cols, confidence, degradations, holes, cells, table_html }`；顶层另有自包含的 `tables[]`（多出 `region` / `html` / `markdown` / `model_id`）。与 `formulas[]` 并列，且**无条件写入**（没有表格时是 `[]`） |
| HTML | `src/output/html.rs::render_report` | 真的 `<table>`（不是 `<pre>` 转义块）：`tables_section` 加 `<h2>表格 (n)</h2>` 与每表一个 `<figure>`；样式在独立 `const TABLES_STYLE`（**不并进共享 `<style>`**，于是"没有表格的报告逐字节不变"是结构保证） |
| Markdown | `src/output/markdown.rs::to_output_markdown` | 表格区域渲染成 Markdown 表格（由 `src/table/emitter.rs::emit_markdown` 生成，见 §7.5）；**被表格消费的行被抑制**（`consumed_by_tables` 位图） |

**净化的唯一落点**：`src/ocr/pipeline/rapid_structure.rs::finalize_table_html(html, table_html, TableTextPolicy)`。`Escape` 分支调 `src/output/html.rs::sanitize_table_html`（白名单 `ALLOWED_TABLE_TAGS`）并重建 `<html><body>` 外壳；`Raw` 分支原样返回。`sanitize_table_html` 经 `src/exports.rs` 导出，所以网页侧（E16.3）**不需要第二份白名单**。

> **为什么 `Raw` 刻意不净化**：`Raw` 的消费者是 lxml + 官方 `metric.py`（见 §11），改字节会破坏与 PubTabNet 真值的可比性。**为什么净化目前不改变任何字节**：`TextPolicy::Escape` 已把单元格文本的 `<` 变成 `&lt;`，而 emitter 只写白名单标签与 `rowspan`/`colspan`/`align`/`valign` —— 实测同一表格在 `Escape` 与 `Raw` 下 `table_html` 逐字相同。所以净化是**纵深防御**，不是当下的行为变更；这条由 `rapid_structure.rs::tests::sanitizing_is_a_no_op_on_whitelisted_emitter_output` 钉住。

**`plain_text` 的口径（E15.4 定案）**：**按阅读顺序输出行，表格区域自身不贡献文本**。因为表格区域**没有 `recognition`**（文本活在 `cells` 里），而 `plain_text` 只取 `recognition` —— 所以被表格消费的行**恰好出现一次**，不是"被特殊处理掉的"。反过来，`to_output_markdown` 必须**显式抑制**被消费的行，否则同一段文字在 Markdown 里会出现两遍（一次作为行、一次作为单元格）。理由是 `plain_text` 的消费者是 CER 评测、网页复制全文与搜索，`|` / `---` 是标记不是文字。

**端到端实测**（`[实测]`，`datasets\text\04表格与键值对.png` 3200×2000，`--table-model models\table\slanet-plus.onnx`；命令与完整数字见 `docs/11` §3.5.3）：json 134,221 B / markdown 1,845 B / html 37,014 B / plain 1,051 B；`items` 61（关表格）vs 63（开）；HTML 里 `<script` 0、`<table>` 2、`<td` 59（31+28）、`&lt;td` 0；不给 `--table-model` 时 `class="tables"` / `<table>` / `table-polygon` 全为 0。

> **CLI 入口**：`rapidstruct.exe run --img-path <图> --config <yaml> [--table-model <onnx>] --format plain|json|html|markdown`。`--table-model` 给了才 `TablePolicy::enabled = true`。**`evaluate` 刻意不接表格开关** —— `--manifest` 量的是纯文本 CER 与 polygon IoU，而表格路由会多产出一个没有 `recognition` 的区域，改变区域数与 IoU 口径，与 `tests/baseline/windows-baseline/evaluation-cpu.json` 不可比（门禁 G6）。

---

### 7.7 网页侧消费（E16 定案）

`src/bin/serve/` 是结构的第四个消费者（前三个是 JSON / HTML / Markdown 三通道，§7.6）。落地要点：

| 关注点 | 落地 | 为什么 |
| --- | --- | --- |
| 模式选择 | `#modeRow` + `#modeOpts`（四个 `role="radio"` 选项，`renderModeRow()` 渲染） | 旧页面只有一个公式复选框，表达不了"文本 + 表格"。表格是**正交的第二维**，所以状态是 `state.mode`（`text` / `formula` / `text-table` / `formula-table`）而不是第二个布尔量 |
| 请求表达 | `POST /api/ocr?queue=<text\|formula>` 加正交的 `&table=1` | 表格阶段不占独立队列，理由见 `docs/05-local-web-demo-implementation.md` §4.2.2 |
| 净化 | `sanitizeTableHtml()`：`DOMParser` 解析后**白名单重建**（`TABLE_ALLOWED_TAGS` / `TABLE_ALLOWED_ATTRS`），非白名单元素降级为文本、属性一律丢弃 | `TableOutcome.html` 是**模型产物**。服务端输出层已经用 `TextPolicy::Escape`，但前端不能假设服务端版本 —— 这是最后一道防线（门禁 **G8**）。**刻意不用正则**：HTML 的容错解析规则用正则写不对，漏一个变体就是一个注入点 |
| 单元格文本 | 只从 `table.cells[].text` 读，按文本渲染 | 单元格文本本身可能是 HTML 片段（PubTabNet 真值里 `<` / `>` 各 4,171 次且成对），§14 的 E15 条目记录了 `markdown` 与 `cells[].text` **不**过净化器 |
| 跨度 | 只保留 `colspan` / `rowspan`，且必须满足 `Number.isInteger(n) && 1 < n <= 64` | 越界值一律**丢弃、不 clamp**：`colspan="1e9"` 会让表格不可渲染，`colspan="1"` 等价于没写 |
| 低置信 | `confidence === "low"` 渲染成**文字可见**的徽标「低置信（网格空洞或截断）」 | 不只靠颜色；`low` 是**正常路径**（§10 结构降级档，全量实测占 4.94 %） |
| 被消费行 | `source_line_indices` 指向的页面行仍留在 `regions` 里，默认**折叠**并标注「已归入表格 N」 | 与 §6.1 的"叠加非替换"一致；折叠是为了让"同一段文字既在表格里又在正文里"这件事可见而不是被静默藏起来 |
| 复制全文 | 表格按阅读顺序用 `TableOutcome.markdown` 插入，被消费的行**跳过** | 否则同一段文字会出现两次 |
| 渲染时机 | 表格卡片默认**不渲染**：只显示 HTML 原文 + 一个「渲染表格」按钮（E24）；点击才由 `sanitizeTableHtml()` 把表格放进 `.rt-wrap`，再点收回并**清空** `innerHTML` | 与公式"默认原文、按需渲染"同构（用户能先核对模型产物），也避免一张几百格的表在结果一出来就常驻 DOM。**只改时机、不改通路**：渲染路径上唯一的赋值语句仍是 `wrap.innerHTML = sanitizeTableHtml(...)`。原文与渲染输入同源（只存在 `data-table-html` 属性里，原文行由 `getAttribute()` 写回），否则两份会分叉 |

**E24 的点击路径实测**：`src/bin/serve/tests.rs::the_table_render_toggle_is_lazy_and_stays_sanitized` 在真实浏览器里驱动页面自己的 `tableRegionHtml()` + `toggleTableRender()`，逐项断言"默认折叠 / 原文可见 / `esc()` + `getAttribute` 逐字节还原模型 HTML / 点击后得到真 `<table>` 且 `<script`·`<img`·`onerror` 一个都不剩 / 合法 `colspan` 存活 / 再点后隐藏且被清空"。按钮在 `regionList` 的点击委托里**先于**区域选中被拦下（否则点按钮等于选中区域），这一条由页面结构断言把守。

**G8（注入面）实测**：11 个 payload（`<script>`、`<img onerror>`、`javascript:` 链接、`onclick`、`<iframe>`、`<svg/onload>`、`style` 里的 `javascript:`、合法 `colspan`/`rowspan`、越界 `colspan`、`<b>/<sub>/<sup>/<br>`、已转义文本）经净化后**没有一个可执行节点**，而合法跨度与白名单内联标签**全部保留**（反向对照，防止"全删掉"也能通过）。用例 `src/bin/serve/tests.rs::the_table_sanitizer_produces_no_executable_nodes` 在**真实浏览器**里跑（`RAPID_STRUCTURE_TEST_BROWSER` 或探测到的 Edge / Chrome），无浏览器时按仓库口径 skip。

## 8. 模型来源、manifest 与 license

RapidTable `default_models.yaml` 给出的 SLANet-plus 来源为：

```text
https://www.modelscope.cn/models/RapidAI/RapidTable/resolve/v2.0.0/slanet-plus.onnx
SHA-256: d57a942af6a2f57d6a4a0372573c696a2379bf5857c45e2ac69993f3b334514b
```

**已实测核实**（v1 的 `<待确认>` 可关闭）：本机 `models/table/slanet-plus.onnx` = **7,758,305 B**，SHA-256 **逐位一致**。文件确为两输出 SLANet 结构模型（§5.2）。

⚠️ **来源性质必须记清**：官方**不存在**已发布的 SLANet ONNX——HF 仓库文件列表只有 `.gitattributes / README.md / config.json / inference.json / inference.pdparams / inference.yml`，ONNX 必须自行用 `paddle2onnx` 转换（官方默认 `--opset_version 7`，失败时自动升版；来源 [paddle2onnx.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleX/release/3.7/docs/pipeline_deploy/paddle2onnx.en.md)）。本项目使用的制品来自 **RapidAI/RapidTable v2.0.0**，是**第三方重打包**，其转换参数（opset、是否裁剪）**无官方文档可查**。

落地步骤：

1. 模型放在模型存储目录，不把大模型直接塞入 crate；
2. 用 `model_store::sha256_file` / `model_verify` 计算实际摘要；
3. **决定表格模型的 manifest 登记位置**（**E18 已定案**：登记在 `models/table/manifest.json`；当时提到的 `models/configs/manifest.json` 是 legacy 死文件，已删除）；
4. 让 table model resolver 在加载和重新校验时都读取该条目，并把期望 SHA 传到 `verify_sha256`；
5. 更新 `THIRD_PARTY_NOTES.md`，记录来源 URL、版本、摘要、精确大小、模型/代码 license 和是否允许再分发。

> **E13 定案（`<待确认>` 已关闭）**：登记位置 = **`models/table/manifest.json`（与模型同目录）**，schema 沿用 `src/model_source.rs` 的 v1 五字段（`schema_version` / `id` / `family` / `version` / `languages` / `files[]`，`files[]` 每条含 `name` / `role` / `sha256` / `size_bytes` / `source_url`）。`role` 用新增的 `ModelRole::TableStructure`（`"table_structure"`）。
>
> 为什么是**同目录**而不是扩展 `models/serve/manifest.json`：`src/model_source.rs` 的规则是"`<model_dir>/manifest.json` 存在时它是该目录的**唯一权威来源**"。把表格登记放进 serve 目录会让加载路径必须同时知道两个互不相关的目录，而 `models/serve/manifest.json` 描述的是页面 OCR 那一套模型。放在模型旁边则加载时只需 `ModelManifest::load_from_dir(model_path.parent())`。
>
> 校验时机（§14.2② 的落地）：`TableStructureSession::open` 与 `TableStructureModelInfo::probe` 都在**创建任何 ORT 会话之前**调 `src/table/registration.rs::verify_registered_model`，后者委托 `model_verify::verify_sha256`。`expected_sha256` 可显式覆盖登记值（属于部署参数，因此是 `TableStructureConfig` 的字段）。评测路径用 `require_registered_model_hash`，**要求**有登记——没有可追溯哈希的模型不该被静默接受。
>
> 登记缺失、`role` 不符、`sha256` 为空、哈希不符**各有独立错误消息**（`registration.rs` 有 14 条测试）；坏清单是**错误**而不是"没有登记"（否则会静默回落到不校验）。

RapidTable 仓库和其代码文件带 Apache-2.0 许可，但这**不能自动证明**下载的 `slanet-plus.onnx` 权重具有相同再分发条款。PaddleX/PaddleOCR 导出件的模型许可、训练数据和 ModelScope 页面条款必须逐项确认；未确认前在 notes 中写 `<待确认>`，**禁止把模型文件标成项目 Apache-2.0**。若代码、权重或 metadata 出现冲突来源，应在 `THIRD_PARTY_NOTES.md` 保留冲突双方原文和结论，而不是覆盖其中一方。

---

## 9. Provider 与失败语义

表格结构 session 使用与公式 session 相同的 `RuntimeConfig`。CPU 是默认可用路径；DirectML/CUDA 只有在相应 Cargo feature 和运行库可用时选择。

**provider API 的 `selected_ep` 只表示交给 ORT 的 EP 链头部**，不能据此宣称模型所有节点都在 GPU 上。既有教训：本机 CUDA 因缺 cuDNN 而节点全落 CPU，API 察觉不到（实测 p50：CPU 1039.5 / DirectML 508.7 / CUDA 1034.2）。**任何加速结论必须来自实测 P50/P90。**

默认表格基础路径允许 accelerator 不可用时回退 CPU，但若调用方明确要求严格 provider，应设置 `fail_if_provider_unavailable=true` 或调用 `require_requested_provider`。

**per-EP session 配置是必需的，不是优化**（[官方明文](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)）：DirectML EP **不支持 memory pattern 优化，也不支持并行执行**，调用方必须 `DisableMemPattern` 且把 `execution_mode` 设为 `ORT_SEQUENTIAL`，否则 session 创建报错；此外 DML **不支持对同一 session 的多线程 `Run`**，要求 **opset ≤ 20**，并偏好静态形状。→ 在 `open_session` / `resolve_execution_providers` 里按 `selected_ep` 分支：

- **DML** → `DisableMemPattern` + `ORT_SEQUENTIAL` + 固定 H/W（§7.2）+ 拒绝并行 `Run`；
- **CUDA** → 官方默认 `arena_extend_strategy = kNextPowerOfTwo`、`cudnn_conv_algo_search = EXHAUSTIVE`（来源：[ORT CUDA EP](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html)）；
- **CPU** → 保持现状（Level3 + arena 开，已实测最优，§5.3）。

> **E12 落地形态（2026-10）**：策略落在 `src/runtime/session.rs` 的 `SessionBuildPolicy { memory_pattern, parallel_execution, must_be_explicit }` + `SessionBuildOption` 上，`for_provider(selected_ep)` **只对 `DirectMl`** 返回偏离项（`Cpu | Cuda` 返回 `ORT_DEFAULTS` → 一条 setter 都不调用，因此"CPU 路径逐条调用不变"是可断言的）。`resolve_execution_providers` 被**提前到 builder 创建之后、`commit_from_file` 之前**（EP 特有选项必须在那之前施加）。opset 约束由 `require_direct_ml_opset`（`DIRECT_ML_MAX_OPSET = 20`）检查。**"拒绝并行 `Run`"不是新增的运行时检查**：所有 run 入口都取 `&mut self`（`Session` 虽 `Send + Sync`，但共享只能经 `MutexGuard` 取得 `&mut`），新增测试 `every_run_entrance_takes_an_exclusive_borrow` 把这条钉成静态不变量。
>
> ⚠️ **但 DirectML 对表格模型实测不可用**：会话能开、推理必失败（`Loop.0` 体内的 `Cast.73`，DML 报 `E_INVALIDARG`/`0x80070057`）。本节的 DML 分支仍是正确性要求，但**不得对外承诺表格模型能在 DML 上运行**。见 §12 R7 的实测引用块与 doc 09 §13.5.2。

模型缺失、SHA 不匹配、IO 契约不符、metadata 缺失和结构解码失败都应是可定位的错误，不能回退到普通 OCR 并伪造"表格成功"。表格模型未启用时，普通 OCR 和现有公式路径应保持原行为。

---

## 10. 分档与退化方案

### 轻量档（无 SLANet-plus）

只使用现有 PP-OCRv6 文本框，再增加水平/垂直线检测：

1. 对文本框按 y/x 聚类，估计行和列；
2. 用横竖线交点修正边界，合并相邻框；
3. 生成没有可靠 rowspan/colspan 的规则网格；
4. 复用 §7.4 的几何匹配回填，输出 HTML/Markdown。

该档应明确标记 `structure_confidence=low`，对复杂合并单元格可能退化。线检测、聚类阈值和公开结果字段均为 `<待确认>`，实现后必须以合成表格 fixtures 验证。

> 注：v2 的默认定位策略（§7.1）已经复用文本框分布，所以轻量档与默认路径在**定位**上共用同一套启发式，差别只在**结构**是从 SLANet-plus 得来还是从线检测得来。

> **E16 落地（网页侧如何暴露分档）**：`src/bin/web/index.html` 的模式选择器把四条路径显式化成选项 —— `文本`（`queue=text`）、`文本 + 公式`（`queue=formula`）、`文本 + 表格`（`queue=text&table=1`）、`文本 + 公式 + 表格`（`queue=formula&table=1`）。表格两项的可用性**只**由 `/api/models` 的 `table.routing` 与 `pipelines.table.plan` 决定；**本档（无 SLANet-plus）下这两项被禁用并点名缺 `表格结构`**（`src/bin/web/index.html` 的 `modeAvailability()` → `tableBlockedText()`），而不是发出一个服务端会静默忽略的参数。这与轻量档的语义一致：没有 SLANet-plus 时表格结构能力**整体不可用**，而不是悄悄退化成某个更差的默认值。

### 结构降级档（有 SLANet-plus，但结果不可信）

由 §1.2 的两条构造性上界触发：

- span 超出词表（数据里存在 `rowspan="41"`、`colspan="24"`）→ 模型必然给出错值，`emitter` 的网格占满性校验会捕捉到空洞；
- 结构长度超出 500 token 上限 → 解码被截断。

两种情况都应降级到轻量档的网格策略并标记 `structure_confidence=low`，**这是正常路径，不是异常兜底**（真实数据里这两类表占比非零，见 §11.1）。

> **E13 实测（全量 7,104，完整口径）**：`structure_confidence.high = 6734`、`low = 350`（占可解码样本的 **4.94 %**）、`degradations.grid_holes = 350`、`degradations.truncated = 0`、`holes_total = 2209`。
>
> 三条由此确定的事实：
> 1. **触发条件是"网格空洞"，不是"span 数值超上限"**。emitter 看不到词表上限，它只能看到"声明的 span 没有把矩形铺满"——这正是模型把 span 截断到词表内之后的**可观测后果**。所以降级判据是 `GridCoverage::is_complete()`，而不是拿 span 与 20 比较。
> 2. **`truncated = 0`**：500 步 trip 上限在这份数据上从未触发，所以"结构长度超上限"这条上界**尚未被任何样本验证**，只是由图的 `Loop` 上限推出来的可能性。
> 3. **降级不等于不输出**：350 个低置信样本仍然产出 HTML 并参与 TEDS 计分。把它们排除掉会抬高分数但掩盖退化——所以报告里 `structure_confidence` 是**并列的独立段**，不改 `metric.mean` 的口径。

**尚未实现的部分**：本档要求"降级到轻量档网格策略"（即用线检测重建规则网格），当前实现只做到**标记**低置信并照常输出 SLANet 结果。线检测重建是轻量档的完整实现，尚未开工。这是登记项，见 §14。

### 精度档（可选 VLM/远程后端）

VLM 只作为 feature-gated 或远程后端，不进入本地默认路径。后端接口应接收裁剪图并返回经过 schema 校验的结构树/HTML；远程请求必须显式启用，超时、网络错误、不合规 HTML 和服务端版本写入结果错误。本地 SLANet-plus 结果应可作为回退，不能让远程后端替换现有 CPU 默认语义。

**性能前提**：本机已有 `models/serve/pp_formulanet_plus_m.onnx` = **593,915,961 B（594 MB）**、`models/formula/detection/pix2text-mfd-1.5.onnx` = 80 MB；SLANet-plus 只有 7.8 MB。同量级的 VLM 会把单页耗时推入秒级（对照：官方 SLANeXt 标 351 MB、CPU 501.66 ms，来源 [table_recognition_v2.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleX/refs/heads/release/3.7/docs/pipeline_usage/tutorials/ocr_pipelines/table_recognition_v2.en.md)）。→ **该档必须自带 CPU 延迟预算断言**（例如"启用后 p50 不得超过 X ms/页"），否则 feature-gate 一开就会把整页耗时从 1 s 量级推到不可接受区间。这不是优化建议，是防止性能回归的门禁。

具体模型、服务协议、隐私策略、成本和 license 全部为 `<待确认>`。

---

## 11. 测试与验收

### 11.1 数据集：TabRecSet 落地实测

**先说结论**：**中文表格没有"许可无争议 + 全部维度齐备"的公开数据集**。最合适的组合是 **TabRecSet 作主力 + 少量自建补充集只覆盖它缺失的维度**。

TabRecSet 已下载并解压到 `datasets/table/tabrecset/`，实测结构：

| 目录 | 内容 | 规模 |
| --- | --- | --- |
| `pubtabnet_format/` | 12 个 `.json` + 2 个 `*_IMAGE_PATHS.txt`（`train_` / `val_` 各一套） | 14 个文件 |
| `cropped_datasets_1Tab_per_img/train/` | 30,633 个 `.json` + 30,633 个 `.jpg`（**配对差 0，无缺图**） | 2,116.6 MB |
| `cropped_datasets_1Tab_per_img/val/` | 7,548 个 `.json` + 7,548 个 `.jpg`（**配对差 0**） | 418.3 MB |

**① `pubtabnet_format/` 不是"含 HTML 的 PubTabNet jsonl"，需要适配层。**
它是 **tokenized 训练格式**，8 类文件：`WORDMAP_STRUCTURE.json`（**53 项**，token 字符串 → id；`<pad>/<start>/<end>/<unk>` 之外是结构标签，例如 `rowspan="12"` → 5、`colspan="24"` → 27、`<td>` → 34、`<tr>` → 50、`</tr>` → 43）、`WORDMAP_CELL.json`、`CAPTIONS_STRUCTURE.json`、`CAPTIONS_CELL.json`、`CAPLENS_STRUCTURE.json`、`CAPLENS_CELL.json`、`NUMBER_CELLS_PER_IMAGE.json`、`IMAGE_PATHS.txt`。
→ TEDS 本身就是在**结构 token 序列**上做树编辑距离，所以这条路是通的；但官方 `metric.py` 解析的是 jsonl schema，**必须先写一个 detokenize 适配层**把 id 序列经 `WORDMAP_STRUCTURE` 还原成 token 串。**不要把"能算 TEDS"误读成"可直接喂官方脚本"。**

**② `cropped_datasets_1Tab_per_img` 的标注比 README 描述的更适合直接做真值。**
实测（400 张 train 采样 / 10,516 个 cell）：格式为 **LabelMe 4.5.9**（`version: "4.5.9"`、`imagePath`、`imageWidth/Height`、`shapes[]`），`shapes[].shape_type` **100% 为 `polygon`**；`shapes[].label` 的编码是

```text
<row>-<col>-<rowspan>-<colspan>-<cell 文本>
```

字段顺序证据：① 400/400 张图的全部 label 均符合该形态（尾随 `-` 且文本为空 = 空单元格）；② 按「字段3 = rowspan」解释时逻辑网格铺满率 **331/400 = 82.8 %**，按「字段3 = colspan」只有 **301/400 = 75.3 %**；③ 语义上 `1-1-1-3-ACTIVITY BOOSTERS (KITCHEN)` 是跨 3 列标题远比跨 3 行合理，`2-3-1-4-二级菜单` 同理。

⚠️ 但铺满率**只有 82.8 %**，说明它并非严格的"网格完全分区"标注。→ 用它做真值前**必须定义未标注网格位置的处理规则**，否则 TEDS/GriTS 对标注完备性敏感、指标会抖动。**这条是评测协议的前置条件。**

**③ SLANet-plus 的词表装不下 TabRecSet 的 span** —— 详见 §1.2 的构造性上界，这是把 TabRecSet 数据与模型能力对照后才能发现的约束。

**逐文件下载清单**（figshare 记录版本 **v9**，`published_date` 2025-07-21，总计 **5,673,915,294 B**；元数据端点 `https://api.figshare.com/v2/articles/20647788`）：

| 文件 | 大小 | MD5 | 下载地址 |
| --- | --- | --- | --- |
| `pubtabnet_format.zip` ⭐ | 9,769,549 B | `602d54889fde4177066e4d1ddd29bbc4` | https://ndownloader.figshare.com/files/56418491 |
| `cropped_datasets_1Tab_per_img.zip` ⭐ | 2,101,449,958 B | `3ab78fbf0324ec376e02f7b54bf172d3` | https://ndownloader.figshare.com/files/56419934 |
| `TabRecSet (CurveTabSet).zip` | 3,412,457,623 B | `048fd48511a4989c2b0da3627237fc38` | https://ndownloader.figshare.com/files/40332520 |
| `tencent OCR.zip` | 20,018,363 B | `959804228a93f46a779abd9220137095` | https://ndownloader.figshare.com/files/39445096 |
| `README.md` | 2,725,577 B | `59ebf9140d51a8bc652504b2e2e05aa0` | https://ndownloader.figshare.com/files/39439612 |
| `TableMe-*.exe` | 127,494,224 B | `8ce957dfb5a73a4d3a65670a9d777598` | https://ndownloader.figshare.com/files/56060744 |

⚠️ **许可口径冲突（未解决）**：GitHub README 写 **CC BY-SA 4.0**；figshare 记录的 license 字段写 **CC BY 4.0**。二者差别在 **ShareAlike 传染性**（CC BY 4.0 允许以更宽松条款再分发，CC BY-SA 4.0 要求衍生作品沿用同许可）。**两个第一方来源互相矛盾**，落地为产品资产前必须澄清。仅作内部评测、不随产品分发时风险可控。

**其它公开数据集对照**（英文为主，许可多为未知或冲突）：

| 数据集 | 规模 | 官方指标 | 许可 |
| --- | --- | --- | --- |
| [PubTabNet](https://github.com/ibm-aur-nlp/PubTabNet) | 500,777 train / 9,115 val / 9,138 test | **TEDS / TEDS-Struct** | ⚠️ HF 写 `cdla-sharing-1.0`，PaddleOCR 写 CDLA-Permissive，冲突 |
| [PubTables-1M](https://huggingface.co/datasets/bsmock/pubtables-1m) | 575,305 标注页 / 947,642 完整表格 | GriTS(+ COCO AP) | CC-BY-4.0 |
| [FinTabNet.c](https://huggingface.co/datasets/bsmock/FinTabNet.c) | 97,475 表格 | TEDS / GriTS / DAR | cdla-permissive-2.0 |
| FinTabNet | 112,875 表格 | TEDS / GriTS | ❌ 原始 IBM 页面已失效 |
| [TableBank](https://github.com/doc-analysis/TableBank) | 417,234 表格 | **TSR = 4-gram BLEU（不是 TEDS）** | ⚠️ HF `apache-2.0` vs README "请勿再分发" |
| SciTSR | 15,000 | 关系 P/R/F1 | ❌ 未找到 |
| ICDAR 2013 Table Competition | 158 表 | DAR | ❌ 未找到 |
| cTDaR (ICDAR 2019) | 1000+1000 图 / 80 文档 | Track A IoU / Track B 邻接 | ❌ 官方 GitHub 404 |
| [WTW](https://github.com/wangwen-whu/WTW-Dataset) | 14,581 图 | TEDS（官方评测代码 "To be updated"） | ❌ 未找到 |
| SynthTabNet | 600k PNG | TEDS / GriTS | ❌ 未找到 |
| TAL（好未来） | 16,000 训练图 | 竞赛指标 | ❌ 未找到 |

**中文覆盖**：TabRecSet（17.7K）是唯一许可明确可用的；TAL（16K）与 WTW 中文场景集许可未知；PubTabNet / PubTables-1M / FinTabNet / TableBank / SciTSR **全为英文**。PaddleOCR 自建的中文表格集（7,835 图）与 59.52/63.69 的评测集**均不可下载**。

**官方指标实现位置**（避免自己重造）：

- **TEDS / TEDS-Struct**：[PubTabNet `src/metric.py`](https://raw.githubusercontent.com/ibm-aur-nlp/PubTabNet/master/src/metric.py)；论文 [arXiv 1911.10683](https://arxiv.org/abs/1911.10683)（ECCV 2020）。
- **GriTS**：[table-transformer `src/grits.py`](https://raw.githubusercontent.com/microsoft/table-transformer/main/src/grits.py)；论文 [arXiv 2203.12555](https://arxiv.org/abs/2203.12555)。
- **cTDaR Track A/B**：[ctdar_measurement_tool](https://github.com/cndplab-founder/ctdar_measurement_tool)。
- ⚠️ **PaddleOCR 本身的表格指标不是 TEDS**（见 §1.1）。

### 11.2 分层测试计划

**L1 — 契约与形状单测（零外部资产，入 `tests/fixtures/`）**

- 仿 `tests/fixtures/formula-onnx/`（12 个 KB 级 `.onnx`）造一个 **2 输出**最小 ONNX，钉住 `(N,L,8)` / `(N,L,50)` 契约与 48-token 换行分隔 metadata 的解析；
- 逻辑网格推导的纯函数单测：构造 rowspan/colspan/空单元格/对角表，断言 `occupied_cells` 网格与 HTML/Markdown 输出。参考 `decode_one_logic_points` 的语义；
- cell↔det-box 几何匹配的纯函数单测（对角/合并/空单元格 case），断言匹配矩阵；
- **§1.3 G2/G3 的门禁断言**也放在这一层：G2 断言回填路径 rec 调用增量 = 0；G3 断言结构推理调用次数 = 表格数；
- **已有可复用资产**：`refer\RapidTable-main\tests\test_files\table.jpg`（59,389 B）与 `table_without_txt.jpg`（34,907 B）。

**L2 — 真实页面回归（项目内已有，立即可用）**

- `datasets\text\04表格与键值对.png`（417,157 B）已在 `datasets\text\golden-manifest.json` 中，`text` 字段是整页真值、`annotation_scope = "source-dom-visible-text"`。可直接作端到端回归样本（当前 `boxes` 全为 `[]`，所以只有 CER 没有 IoU）；
- 局限：**只有 1 页真实表格**，且无表格结构真值（无 cell bbox、无 HTML）。**不够**用于结构评测。

**L3 — 表格结构评测集（先接 TabRecSet，再按缺口自建）**

**顺序很重要**：先写 detokenize 适配层 → 用 TabRecSet val 的 **7,548 张**跑通 TEDS → 确认指标口径无误后，**再决定**自建多少页。这样自建量最小，且自建集与公开集共用同一套指标实现，结果可比。

1. **来源**：① TabRecSet 的 `cropped_datasets_1Tab_per_img.zip` + `pubtabnet_format.zip`（§11.1）；② 项目自有文档页面截图；③ 其它公开可再分发的中文表格。⚠️ 许可口径冲突见 §11.1；
2. **标注格式**：结构真值采用 PubTabNet 的 jsonl schema（`{filename, split, imgid, html:{structure:{tokens[]}, cell:[{tokens[], bbox[x0,y0,x1,y1]}]}}`），使 TEDS 参考实现可**直接跑**；但**必须注意** TabRecSet 的 `pubtabnet_format` 本身是 tokenized 格式，需要 §11.1 ① 的 detokenize 适配层，**不存在"拿来即跑"**；
3. **规模建议**：**自建部分**首期 30–50 页（TabRecSet 已覆盖的维度不必重复），覆盖矩阵必须显式包含 TabRecSet 覆盖不足的项：中文扫描歪斜、无线框表格、**rowspan ≥ 2 与 colspan ≥ 2 的嵌套组合**、多行单元格、跨页表格；
4. **标注方式**：结构用人工校对的 HTML 真值；cell bbox 允许半自动（现有 det 文本框 + 表格线检测生成后人工修正）；cell 文本用整页 OCR 结果 + 人工校对，而不是逐格录入（成本降一个量级）；
5. **抽样必须稳定哈希**：复用 `src/evaluation/formula/sampling.rs` 的 `SampleStrategy::Hash`（数据集相对路径 + 真值稳定哈希，**明确禁止"取前 N"**），并把 `manifest_sha256` / `sample_set_sha256` / `content_sha256` 写进 manifest；
6. **指标挂载**：新增 `src/evaluation/table/`，与 `src/evaluation/{ocr,formula}.rs` 平级 —— `src/evaluation/mod.rs` 已明确"两类指标不得混合成一个默认汇总"，表格是第三个域，同样不得混入 ocr/formula 汇总；
7. **评测协议前置项（不可跳过）**：
   - 定义**未标注网格位置的处理规则**（铺满率 82.8 %，见 §11.1 ②），否则指标会抖动；
   - 显式**扣除 §1.2 的两条构造性上界**，单独统计"因 span 超词表而必然出错的表格"占比，把它们从回归判据中剔除（或单列），否则会被误判成代码回归；
8. **报告口径复用**：`src/bin/rapidstruct.rs:334` 的 `evaluation_report_value(&EvaluationSummary) -> Result<serde_json::Value,_>` 是报告序列化的唯一入口（CLI `rapidstruct evaluate` 与 `POST /api/evaluate` 共用），表格域应挂到同一出口。

**L4 — 性能基线（复用既有工具，不新建框架）**

- `pwsh -NoProfile -File tools/run_windows_baseline.ps1` → 写 `tests/baseline/`；
- 判据用 `src/evaluation/stats.rs` 的 `Stats`（p50/p95 的唯一口径）；
- provider 对比必须用 `tools/check_provider_claims.ps1 -Reference <cpu.json> -Reports <other.json>`，因为 `selected_ep` 只表示 EP 链头部（§9）。

### 11.3 验收顺序

按 `AGENTS.md` 的开发期规则执行：

1. **基线**：先运行现有 `cargo test --all-targets`，并记录普通 OCR、公式检测/识别、provider 和模型校验相关测试结果；
2. **契约测试**：提交 SLANet-plus fixture 或外部资产探针，覆盖 input/output 数量、dtype、rank、metadata、SHA 不匹配和 provider 回退；
3. **前处理 golden**：固定不同宽高比、空白边、纯色图，比较 Rust 张量的 shape、统计量和参考实现输出；插值差异若不能 bit-exact，记录允许误差；
4. **解码测试**：覆盖 `sos/eos`、`<td>`/`<td></td>`、rowspan、colspan、全零 bbox、非法 token 和 bbox 反变换；
5. **回填测试**：验证 cell↔det-box 匹配矩阵、批处理重排后结果仍回到原 cell、空 cell 和匹配失败语义；
6. **输出测试**：HTML 属性/文本转义、跨行跨列、空表、Markdown 退化规则、**网格占满性校验触发的降级标记**；
7. **回归**：再次运行完整既有测试，并用同一基线图片比较普通 OCR 与公式输出；新表格开关关闭时不得改变旧结果；
8. **性能记录**：记录 CPU provider 下结构、回填和总耗时；DirectML/CUDA 仅报告实测，不从 `selected_ep` 推断加速。

---

## 12. 性能改进项（按 收益/成本 排序）

> 本节每一项都保持"现象/瓶颈 → 根因（第一性事实） → 具体改法 → 预期收益 → 所需测试 → 风险"的结构。所有绝对值均来自 §5 的 [实测]；带 [推算] 的探针数字只用其形状。

### R1 — 表格回填改为"整页 det 复用 + 几何匹配"，删除逐 cell 裁剪喂 rec 的路线

| 项 | 内容 |
| --- | --- |
| **成本** | 高（`src/table/cell_content_backfill` 的整体设计变更） |
| **收益** | 高（回填额外 rec 成本 → 0，并消除多行压扁的精度损失） |

**现象/瓶颈**：v1 §3 规划的 `cell_content_backfill` 是"逐单元格裁剪 → 调 rec 回填"。其成本正比于**单元格数 × 批内最大宽度**，且与内容量无关。

**根因（第一性事实）**：

1. rec 的输入契约是 `[N, 3, 48, W]`。`src/ocr/rec/preprocess.rs` 的 `batch_shape_for(max_wh_ratio, rec_image_shape)` 里 `dst_width = ((img_height as f64) * max_wh_ratio) as usize`，而 `max_wh_ratio` 取自**批内最大值**（`src/ocr/rec/recognizer.rs` 每批先算 `max_wh_ratio` 与 `wh_ratio_list`，再 `batch_scratch.resize(total_len, 0.0)` 整批 pad 到同一宽度）。所以只要表格里出现一个跨多列的合并单元格，**同一批所有单元格都被拉宽**。实测比例见 §5.4 表：整批 pad 到 640 时每格成本是 W=64 时的 **9.4×**。
2. rec 是**单行**识别器：输入被等宽拉伸到固定高 48 px。多行单元格（换行表头、地址、备注）被压成一行，信息不可恢复。
3. 逐 cell 裁剪意味着每个单元格一次 crop + 一次 resize + 一次归一化（`src/vision/resize.rs`、`src/ocr/rec/preprocess.rs` 的 `signed_unit_lut()` 归一化），这些在整页路径里已经付过一次。

**具体改法**：改为参考实现/官方路线 —— **整页 det 一次得到全页文本框，表格结构模型给出 cell bbox，用几何匹配把 det box 归到 cell**。参考实现 `refer\RapidTable-main\rapid_table\main.py` 的 `RapidTable.__call__` 就是先 `table_structure(imgs)` 再对**整页图**调 `self.ocr_engine(img)`，最后交给 `TableMatch`；`refer\RapidTable-main\rapid_table\table_matcher\main.py` 的 `TableMatch.match_result(cell_bboxes, dt_boxes, min_iou=0.1**8)` 对每个 det box 算与所有 cell box 的 `(distance, 1-IoU)`，按 `(1-IoU, distance)` 取最优，建立 `matched[cell_idx] = [det_idx…]`。逻辑网格（含 rowspan/colspan 占位）用同文件 `decode_one_logic_points` 的 `occupied_cells` + `is_occupied/mark_occupied` 推导。

**预期收益**：回填的**净增 rec 成本为 0**（复用页面 OCR 已有的 `LineResult`），并消除多行压扁。对比逐 cell 路线——即使按最乐观的每格 7.4 ms/区域（Rust 口径）估算，一张 8×5 = 40 格的表也要多出约 **300 ms/页，相当于把整页耗时提高 30 %**。这一项同时落在 §1.3 的 **G2** 门禁上。

**所需测试**：
- 基线（改动前）：`datasets\text\04表格与键值对.png` 走现有 pipeline，记录 `regions`、`ocr_total_ms` p50/p90（口径同 `tests/baseline/windows-baseline/bench-cpu.json`）；
- 新增：cell↔det-box 匹配的纯函数单测（对角/合并/空单元格 case），断言匹配矩阵与 `decode_one_logic_points` 的逻辑网格；
- 回归：新路线在同一批页面上的 HTML 结构逐 token 一致；cell 文本非空率；总耗时不得高于基线。

**风险**：cell bbox 的坐标系换算必须先钉死 —— 参考实现里 `ratio = min(488/h, 488/w)`、`w_ratio = 488/(w*ratio)`、`h_ratio = 488/(h*ratio)`，在横表 `w >= h` 时 `w_ratio = 1`，语义反直觉。**先用 golden 把这一步钉死，再谈性能**，否则"匹配错"会被误判成"检测差"。

> **已实测确认**（`docs/09-table-recognition-execution.md` §5.4，真实模型 golden）：横表 `w_ratio == 1` 属实，且两步复合后 x 与 y 的总因子都是 `488 / ratio = max(h, w)`，即 `after_rescale = 模型归一化输出 × max(h, w)`，量纲是**原图（crop）像素**而非 488 画布坐标 —— 中间的 `_bbox_decode` 在代数上被完全抵消，**只调用其中一个才是错的**。模型输出归一化到**补零后**的 488×488 画布（左上对齐）。
> `w_ratio == 1` 是**中间量**的性质，净效果是各向同性缩放；不要据此推断"横向不需要缩放"。
> 另注：参考实现把结构模型直接跑在**整张图**上（`refer/RapidTable-main/rapid_table/main.py:98-110` 不裁剪），故其 cell 坐标天然是页面坐标；本仓库走"页面 crop 里的小表"路线，**crop 偏移量必须另加**。

### R2 — 页面级 det 分辨率：表格路径不应默认继承 `max_side_len = 2000`

| 项 | 内容 |
| --- | --- |
| **成本** | 低（配置/参数层面） |
| **收益** | **最高**（最高可达数百 ms/页） |

**现象/瓶颈**：`detector_infer_ms = 570.57`，占页面总耗时 **57.9 %**。

**根因（第一性事实）**：det 是整页稠密卷积网络，计算量正比于输入像素数；`max_side_len` 直接决定像素数。当前基线用 2000。而 PaddleOCR 官方表格评估/推理脚本用的是 `--det_limit_side_len=736 --det_limit_type=min`（来源：[ppstructure/table/README.md §4.3](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/f8b41a62/ppstructure/table/README.md)）。探针实测 det 延迟对面积近似线性（`736² 3015 ms → 2048² 19857 ms`，约 5000 ms/Mpx；**[推算]，绝对值不可用，仅说明"面积线性"这一形状**）。

**具体改法**：为表格识别路径引入独立的 det 限制，而不是复用页面级的 `max_side_len`。最小改动是在表格路径的 `RuntimeConfig`/pipeline 配置里单独传 `limit_side_len` + `limit_type`；符合 `AGENTS.md`「可以破坏旧接口」——不必为兼容旧配置加分支。

**预期收益**：面积从 2000² 量级降到 736² 量级，理论上限约 7×（按面积），**但表格单元格文字通常小于正文，降分辨率的精度代价必须先量出来**。这是一个必须用数据决定的旋钮（§2 K1），不是无条件优化。

**所需测试**：
- 基线：`datasets\text` 12 页在 `max_side_len ∈ {2000, 1280}` 的 CER（`tests/baseline/windows-baseline/evaluation-cpu.json` 已是现成对照）；
- 新增：表格页专用集（§11.2 L3）在多个 det 分辨率下的区域召回 / cell 文本 CER / 单元格文本完整率；
- 验收口径可直接复用仓库既有容差文件 `benchmarks/image-bench/api-comparison-small-medium.json`：mean CER ≤ 0.03、polygon IoU ≥ 0.85、区域数差 ≤ 5 %。

**风险**：表格里的小字号内容漏检；降分辨率后单元格文本回填失败率上升。**若 CER 上升超过容差，就放弃这一项**——`AGENTS.md` 的决策优先级是 正确性 → 根因 → 性能。

> **已实测确认**（`docs/09-table-recognition-execution.md` §11.4，全量 TabRecSet val 7,104 张，`content_source = page-reuse`）：本条**采纳**，且实测方向与预期相反 —— 精度**上升**、耗时**下降**，是严格改进而非取舍。
>
> **本节原稿的根因不完整。** 决定 det 输入像素数的**不是一个旋钮，而是三处代码**：① `global.max_side_len` 的前置缩放（只缩不放）；② `det.limit_side_len` + `det.limit_type` 的 `compute_resize_hw`（`"min"` 只放不缩）；③ `src/ocr/det/detector.rs:210-224` 的 `resolve_limit_side_len_like_python` —— **`limit_type != "min"` 时它直接丢弃配置的 `limit_side_len`**，改用 `max_wh` 派生的 `960 / 1500 / 2000` 阶梯（复刻 PaddleOCR 行为，由 `src/ocr/det/detector.rs:236-240` 的测试钉死）。所以 `limit_type = "max"` 下 `limit_side_len` 是**死字段**。
>
> 官方 `--det_limit_type=min` 的隐含前提是"输入是整页、短边本来就在 736 附近"，**在 TabRecSet 上不成立**：val 长边中位 448 px、短边中位 150 px，**短边 < 736 的有 7,055 张（99.3 %）**，长边 > 2000 的只有 13 张（0.18 %）。一张 448×150 的裁剪表会被 `min` 放大到约 2208×736（**约 24 倍面积**）再喂 det —— 这既是主要成本，也把输入推离了训练分布。
>
> | 档位（`limit_side_len` / `limit_type` / `max_side_len`） | **生效策略** | 完整 TEDS | ΔTEDS | `page_ocr.ocr_ms` p50 | 加速比 | `regions_seen` | Δregions |
> | --- | --- | --- | --- | --- | --- | --- | --- |
> | 736 / min / 2000（原默认） | 短边放大到 736 | 0.570710 | — | 549.159 | 1.00× | 140,373 | — |
> | **736 / max / 736**（新默认） | **永不放大，长边 ≤ 736** | **0.583404** | **+0.012694** | **108.907 / 112.736** | **4.87–5.04×** | 145,377 | **+3.56 %** |
> | 1280 / max / 1280 | 永不放大，长边 ≤ 1280 | 0.583932 | +0.013222 | 111.376 | 4.93× | 145,684 | +3.78 % |
> | 320 / min / 2000 | 短边放大到 320 | **0.590587** | **+0.019877** | 200.599 | 2.74× | 148,883 | **+6.06 % ✗** |
>
> **默认值判定为 `736 / "max" / 736`**，依据：① 唯一可测门槛（区域数差 ≤ 5 %）淘汰了精度最高的 `min320`（+6.06 %）；② 通过的两档之间 TEDS 差 0.000528（0.09 % 相对）低于有意义分辨率，而 `ocr_ms` 与墙钟代价实打实，奥卡姆取更紧更快的；③ 长边上限从 736 放到 1280（影响长边 > 736 的那 11 % 图）只换来 +0.000528 TEDS —— 这是"det 输入超过 736 无增益"的直接证据；④ **`"max"` 给 det 输入一个硬上界（长边 ≤ 736，≤ 0.54 Mpx），`"min"` 给不了**：`min` 只约束短边下界，一张 1280×100 的图会被放大到 4096×320，生产路径上延迟不可预测。这条独立于门槛。
>
> `page_ocr.ocr_ms` 列的两个数（108.907 / 112.736）是**两次独立冷跑**同一配置的 p50，极差 3.5 %，属整轮墙钟抖动；所以加速比报区间 **4.87–5.04×** 而非单点。入库基线 `tests/baseline/windows-baseline/table-teds-full-cpu.json` 记的是 112.736。
>
> **`"max"` 反而更准的解释**：`min` 把小裁剪表放大到分布之外，det 给出的文本框既多又碎（`unmatched_cells` 反而更多）；`max` 让输入保持原尺度。`min320` 比 `max736` 高 0.0072，说明**最极小的那些表**（短边 p10 = 60 px）确实需要一点放大，但它的收益被区域数超容差挡在门外，且它没有输入上界。
>
> **验收口径的偏离**：本节列的 `mean CER ≤ 0.03` 与 `polygon IoU ≥ 0.85` **在 TabRecSet 上不可测** —— 该数据集只有结构 token 与单元格多边形，**没有文本框与文本真值**。已改用**完整 TEDS**（直接测最终产物）+ `unmatched_cells` + `non_empty_rate` + `regions_seen`。这不是放宽门槛，是换成更直接的指标。另注：**表格路径没有独立的 det 计时**（`content_source = page-reuse` 跑的是整页引擎），所以可比量是 `page_ocr.ocr_ms`。
>
> **已知设计债与遗留风险**（完整七条见 §11.4.7）：`limit_side_len` 在默认档位下是死字段，只被文档与测试标注、**未被类型系统禁止**；`"min"` 的放大没有上界而 `"max"` 有，但"上界存在"这个语义没有单独的测试（若有人把 `apply` 改成不覆盖 `global.max_side_len`，上界会静默消失）；未测长边上限低于 736 的档位，所以"再往下压也无损"是**推测不是实测**；若将来判定"区域数差 ≤ 5 %"不适用于表格路径，默认值应重评为 `320/min/2000`。
>
> 附带修掉一个静默缺陷：页级 OCR 缓存（`src/evaluation/table/run.rs` 的 `OcrCache`）原先只以样本路径为键，**换分辨率或换配置后会被静默复用**，把另一份引擎的文本记到本次报告上。现已加引擎指纹（整份解析后配置的规范化 JSON 的 sha256），指纹不符**报错拒绝复用**。这条守卫在本轮真实触发过一次。

### R3 — `OrtSession` 必须支持多输出（否则表格结构无法实现）

| 项 | 内容 |
| --- | --- |
| **成本** | 低（约 20 行） |
| **收益** | 无直接加速；是功能前提 |

**现象/瓶颈**：`src/runtime/session.rs:145` 的 `run_arrayd_view_with` 只取第一个输出：

```rust
let outputs = self.session.run(inputs![input_tensor]).map_err(ort_error)?;   // :144  已拿到全部输出
let output_name = self.output_names.first().ok_or_else(|| { ... })?;         // :145  立刻丢掉其余
```

`output_i64_2d` 在 `src/runtime/session.rs:107` 同样 `output_names.first()`。

**根因（第一性事实）**：现有模型契约（`src/formula/contract.rs` 的 `FLOAT32 [N,1,384,384] → INT64 [N,L]`）都是**单输出**，API 据此简化。SLANet-plus 有 **2 个输出**（§5.2），单输出 API 无法表达该契约。注意 `session.run(inputs![...])` 本身已经返回全部输出，只是被 API 丢弃了——不需要改 ORT 调用方式，只需把闭包的入参从"单个 array"泛化为"整个 `SessionOutputs`"。

**具体改法**：把 `run_arrayd_view_with` 的闭包签名从 `FnOnce(ArrayViewD<'a, f32>) -> Result<T>` 改为接收 `&SessionOutputs`（或新增一个 `run_arrays_view_with` 多输出变体），并在 `src/runtime/contracts.rs` 增加"输出对"契约（类似现有 `require_single_output`，新增 `require_outputs(&[&str])`）。按 `AGENTS.md` 可直接改签名，不需要保留旧 API。

**预期收益**：无直接加速。但缺了 bbox 输出就只剩结构 token，没有 cell 坐标 → 既无法做 R1 的几何匹配，也无法出 Markdown 网格。

**所需测试**：仿照 `tests/fixtures/formula-onnx/` 构造一个 2 输出最小模型，加契约单测；对真实 `slanet-plus.onnx` 加 golden 测试断言 `(N,L,8)` / `(N,L,50)` 的形状与 dtype，并断言两路输出的数值与参考实现一致。

**风险**：`SessionOutputs` 的生命周期绑定到 `run` 的返回值，闭包签名要处理借用；这是编译期问题，不会引入运行时风险。

### R4 — 不要固定 L（token 维），只固定 H/W

| 项 | 内容 |
| --- | --- |
| **成本** | 低 |
| **收益** | 中（消除 ORT 动态形状告警与重规划；DirectML 上为必要条件） |

**现象/瓶颈**：输入形状全动态，ORT 打印 `Expected shape from model of {} does not match actual shape of {1} for output logical_and_0.tmp_0`。

**根因（第一性事实）**：`GraphOptimizationLevel::ALL` 的形状推断在动态图上退化；ORT 官方文档说明优化级别与 EP 分区依赖形状可知性（来源：[ORT graph-optimizations](https://onnxruntime.ai/docs/performance/model-optimizations/graph-optimizations.html)）。

**具体改法**：见 §7.2 的 free-dimension override。

**关键约束（第一性事实）**：**不要固定 L**（§2 K10）。把 L 固定成 500 会强制解码 500 步。

**预期收益**：消除告警 + 让 ORT 走静态形状路径；在 DirectML 上从"退化"变为"能跑"。[未测] H/W 固定本身能否加速。

**所需测试**：断言固定 H/W 前后、同一输入下两路输出的**数值逐位一致**（形状固定不应改变语义）；再记录 `bench_warm_e2e` 的 p50 变化。

**风险**：若后续要支持 488 以外的输入尺寸，这个 override 必须随之调整；建议把它做成 `TableStructureConfig` 的字段而不是硬编码常量。

### R5 — 表格结构 session 要用较小的 intra threads，且不要继承页面级 16 线程

| 项 | 内容 |
| --- | --- |
| **成本** | 低 |
| **收益** | 低–中（原估约 20 % 单次结构推理；更重要的是避免线程超订） |
| **实测结论（E12，2026-10）** | **回退：没有任何线程设置被证明优于自动推导值 → 默认保持继承（本机生效 14）**。机制（声明字段 + 可覆盖）已交付，改的是"不设默认值"这件事。见下方引用块与 doc 09 §13.5.3。 |

**现象/瓶颈**：SLANet 488² 在 4 线程下 36.6 ms，8 线程下 30.8 ms（bs=1）——8 线程确实更快；但另一轮在**充分预热**下 4 线程测到 **25.1 ms**，优于 8 线程的 30.8 ms。两个口径互相矛盾，说明收益落在噪声区。

**根因（第一性事实）**：

1. SLANet 是 830 节点的小网络（77 Conv / 73 HardSwish），batch=1 时并行度不足，线程越多同步开销占比越高；
2. ORT 官方线程默认值 `session.intra_op.allow_spinning = "1"`（来源：[ORT threading](https://onnxruntime.ai/docs/performance/tune-performance/threading.html)）——**空闲线程自旋而非休眠**。仓库当前已有 det + rec 两个 session（+ 公式），表格模块会再加一个。若每个都拿 16 个 intra 线程，在 20 核级 CPU 上会产生自旋争抢；
3. 仓库自己的 `tests/baseline/windows-baseline/thread-matrix.json` 表明：本机线程调整的 p50 波动整体落在 **±10–15 % 噪声带**内，`src/runtime/profile.rs` 的文档也明确写"这是简化，不是加速"。

**具体改法**：让 `RuntimeConfig` 支持 per-session 覆盖，表格结构 session 用固定的较小 intra threads（例如 4），或在 `RuntimeProfile` 里为表格 session 单独解析。**不要**为表格模块修改全局 `auto_tuned_thread_budget()`。

> 注：`src/runtime/profile.rs` 当前的 `RuntimeProfile::session_runtime(&self) -> RuntimeConfig` 恒 `auto_tune_threads=false`，并复用同一份解析结果。最干净的做法是在 `TableStructureConfig` 里声明 `intra_threads: Option<usize>` 并在构造 `OrtSession` 时用它覆盖。

**预期收益**：单次结构推理 ~20 %（噪声内，需实测确认）；避免多 session 超订带来的隐性退化。

**所需测试**：按仓库既有口径跑线程矩阵（`tools/run_thread_matrix.ps1`，p50 对比、判定带 ±10–15 %）。**若落在噪声带内就不改**。

> **E12 实测（2026-10）——判定：回退，默认保持继承。**
>
> `tools/run_thread_matrix.ps1` 走的是页级 `bench_warm_e2e`，而**表格尚未接进页面管线**（`src/ocr/pipeline/rapid_structure.rs` 把 `table_structure_infer_ms` 写死 `0.0`）→ 该脚本**原理上观测不到 R5**。实测改走表格域：`target\release\table_eval.exe --limit 500 --content structure-only --intra-threads N`，**6 次重复**交错 default / 4 / 8。
>
> | 设置 | `infer_ms.p50`（ms，6 次） | 均值 | 自身 spread | 相对 default |
> | --- | --- | --- | --- | --- |
> | `default`（生效 **14**） | 27.742 / 26.437 / 27.279 / 28.896 / 28.516 / 28.861 | 27.955 | **9.30 %** | — |
> | `intra=4` | 27.811 / 26.397 / 30.970 / 25.825 / 29.675 / 22.695 | 27.229 | **36.46 %** | −2.60 % |
> | `intra=8` | 27.306 / 25.308 / 25.989 / 28.089 / 24.221 / 25.254 | 26.028 | 15.97 % | −6.89 % |
>
> 最大偏差 −6.89 % 既小于 10 % 带底，也小于 `default` 自身的 9.30 % 离散度 → **按"落在带内就回退改动"保持 `TableStructureConfig::intra_threads` 默认 `None`**。
>
> ⚠️ **判定带本身需要修正**：3 次重复时看到的是 "`intra=4` 慢 21.76 %、`intra=8` spread 45.64 %"，**两者都是噪声**。同一配置 6 次重复的离散度就有 9.3 %（`intra=4` 达 36 %），远超 n=500 时 p50 的抽样误差（约 2 %）→ 主导方差是**重复间系统漂移**。**±10–15 % 对本机表格模型偏乐观**，表格域任何"小幅提升"的判定必须先建立该配置自身的离散度。
>
> ⚠️ **本结论只对"表格单独跑"成立**：§13.1 的第一性依据（与 det/rec 的自旋争抢）**没有被实测**，因为表格还没接进页面管线。接线后必须重测。

**风险**：低。唯一风险是把噪声当收益，因此必须有"改前/改后同一 harness 同轮次"的对照。

### R6 — 不要为结构模型引入多表格批处理排队

| 项 | 内容 |
| --- | --- |
| **成本** | 低（=不做） |
| **收益** | 避免负收益 |

**现象/瓶颈**：批处理收益上限实测约 **12 %**（th=8, bs=2：27.2 vs 30.8 ms/图），bs≥4 开始回升；**单线程下批处理显著劣化**（59.3 → 90.5 ms/图）。

**根因（第一性事实）**：SLANet 是 compute-bound，批处理只摊薄 per-run 固定开销，不改变总 FLOPs；单线程下大 batch 破坏缓存局部性。

**具体改法**：一页多表时直接串行，或最多 bs=2。**不要**为了批处理引入跨页排队、结果聚合等复杂度。

**所需测试**：若坚持要批处理，需在同一 harness 下给出 bs∈{1,2,4} 的 p50 对比，并证明收益超出噪声带。

**风险**：无（这是"不做"的建议）。

### R7 — DirectML 路径必须走 per-EP 的 session 配置，否则会创建失败

见 §9 的完整 per-EP 分支。**成本** 中；**收益**：DML 上从"session 创建失败"变为"可运行"。

**所需测试**：`tools/run_windows_provider_matrix.ps1` + `tools/check_provider_claims.ps1 -Reference <cpu.json> -Reports <other.json>`。

**风险**：⚠️ **本机 Python 环境只有 `AzureExecutionProvider` + `CPUExecutionProvider`，没有 DirectML DLL，无法在本机验证任何 DML 行为**。另外 SLANet 含 `Loop` 控制流算子，**DML 对控制流子图的支持未验证** → 不要写进对外承诺，必须实测。

> **E12 实测（2026-10）——per-EP 分支已落地，DirectML 实测确认"不能跑表格模型"。**
>
> **原文的风险段有一条前提是错的**：本机**并非**没有 DirectML DLL —— 那是 **Python** `onnxruntime` 的 provider 列表。仓库的 Rust 构建静态链接自己的 ONNX Runtime 1.28 并加载 `DirectML.dll`（E5 已实测 `selected_ep == direct_ml`、`fallback_to_cpu == false`）→ 这是**工具链错配**，不是硬件缺失，DML 路径在本机**可以**通过 Rust 二进制实测。
>
> **实测结果（`cargo test --features directml-provider --lib table::structure::direct_ml_experiment -- --nocapture`）**：
>
> ```
> DIRECT_ML_VERDICT=session_opened
> DIRECT_ML_SELECTED_EP=DirectMl
> DIRECT_ML_FALLBACK=false
> DIRECT_ML_OPSET=Some(14)
> DIRECT_ML_VERDICT=inference_failed
> DIRECT_ML_ERROR=decoding failed: ONNX Runtime error: Non-zero status code returned while running Loop node.
>   Name:'Loop.0' ... while running Cast node. Name:'Cast.73' ...
>   DmlExecutionProvider\src\MLOperatorAuthorImpl.cpp(2870)... Exception(1) ... 80070057
> ```
>
> 1. **失败在运行期，不在创建期** —— 会话开得成，第一次推理才失败（`Loop.0` 体内的 `Cast.73`，DML 报 `E_INVALIDARG`/`0x80070057`）。→ **任何"能开会话就报告 DirectML 可用"的检查都会假阳性。**
> 2. **opset 约束不是障碍** —— SLANet-plus 是 **14 ≤ 20**。真正的障碍是 DML 对 `Loop` 体内 `Cast` 算子的支持。
> 3. 因此 §9 的分支**保留**（`DisableMemPattern` + `ORT_SEQUENTIAL` 仍是 DML 的正确性要求），但**不得对外承诺表格模型在 DML 上可用**。落地形态见 doc 09 §13.5.1（`SessionBuildPolicy` + `SessionBuildOption` + `require_direct_ml_opset`）。
>
> **未做的决定（留给 E13）**：是否在 `TableStructureSession::open` 时就拒绝 DirectML，或做一次预检推理把运行期失败转成清晰的 `Config` 错误。不做的理由：预检要跑一次推理（`Loop` trip 上限 500，最坏 500 步），且硬编码"DML 不支持"会在 ORT 修好后误拒。

### R8 — 表格定位不要引入第三个整页 CNN

| 项 | 内容 |
| --- | --- |
| **成本** | 中 |
| **收益** | 高（避免 +30~100 % 页面耗时） |

**具体改法**：走 §7.1 和 §10 轻量档的思路——**从已有 det 的文本框分布推断表格区域**。这是零额外模型成本的做法。只有当实证表明启发式召回率不达标时，才引入专用模型。这一项落在 §1.3 的 **G4** 门禁上。

**所需测试**：用 `datasets\text\04表格与键值对.png` + `refer\RapidTable-main\tests\test_files\table.jpg`、`table_without_txt.jpg` 做区域召回/精确率断言；在整页 bench 上确认 `ocr_total_ms` 无增量。

**风险**：启发式在无线框表格、跨页表格、嵌套表格上会退化。必须用 §11.2 L3 的表格测试集量化，而不是靠直觉。

### R9 — 把表格阶段纳入 `TimingLedger`（其余一切的前置条件）

| 项 | 内容 |
| --- | --- |
| **成本** | 低 |
| **收益** | 使所有性能结论可证伪 |

**现象/瓶颈**：`src/runtime/timing.rs` 的 `TimingLedger` 目前没有表格分量。

**根因（第一性事实）**：没有命名分量就无法证明收益；`AGENTS.md` §4 要求"性能优化必须有明确依据"，§6 要求"验证必须真实有效"。

**具体改法**：照 `formula_ms` 的先例，在 `src/runtime/timing.rs` 的 `TimingLedger` 里新增命名分量（如 `table_detect_ms` / `table_structure_infer_ms` / `table_backfill_ms`），并在表格阶段写入 `OcrTimings`；`TimingLedger::from_timings` + `mean`/`shares`/`conservation` 无需新类型。`src/bin/bench_warm_e2e.rs` 已输出 `stages` / `timing_ledger` / `timing_split`，会自然带上新分量。

**预期收益**：让 R1/R2/R5 的收益可被证实或证伪，并提供 §1.3 G5 的数值。

**所需测试**：
- `src/runtime/timing.rs` 已有的守恒测试必须继续通过；
- 新增：表格分量出现在 `shares()` 中且 `Σshare = 1`；`timing_split` 恒等式 `ort_inference_ms + rust_ms + input_overflow_ms + unattributed_ms == denominator_ms` 仍成立。

**风险**：低。注意 `conservation` 是**诊断不是验收**：release 残差 −6.75 ms/页（0.69 %），debug 大一个数量级，别把残差当 bug 追。

### R10 — 量化事实更正

见 §3.1 的更正表与 §0.4 的口径说明。**成本** 低（文档层面）；**收益**：避免基于错误数字做选型。

### R11 — 精度档（VLM）必须先证明边际收益，再谈性能

见 §10 精度档。**成本** 中；**收益**：防止性能回归。

---

## 13. 建议的执行顺序

| 序 | 动作 | 前置 | 判据 |
| --- | --- | --- | --- |
| 1 | R9 把表格分量接入 `TimingLedger` | 无 | `shares()` 求和 = 1；`timing_split` 恒等式成立 |
| 2 | R3 `OrtSession` 支持多输出 + 契约测试 | 无 | 2 输出契约单测 + `slanet-plus.onnx` golden |
| 3 | 用 golden 钉死 `rescale_cell_bboxes` 的坐标系 | R3 | cell bbox 与人工标注一致 —— **✅ 已完成**（`docs/09-table-recognition-execution.md` §5.4） |
| 4 | R4 固定 H/W（不固定 L）+ 数值逐位一致断言 | R3 | 输出逐位相等；告警消失 |
| 5 | 写 TabRecSet detokenize 适配层 → 跑通 TEDS | 无（纯离线） | val 7,548 张产出 TEDS 数字 = **§1.1 的 baseline** —— **✅ 已完成**（E6 适配层 + E7 基线，`docs/09-table-recognition-execution.md` §7.4 / §8.5；TEDS-Struct = 0.706518，完整 TEDS 顺延 E9） |
| 6 | 定义 L3 评测协议（未标注网格位规则 + 扣除构造性上界） | 5 | 重复 3 次的 stddev 给出 ε |
| 7 | R1 回填改"整页 det 复用 + 几何匹配" | 3、4 | **G2 通过**；cell 文本非空率不降 —— **✅ 已完成**（`docs/09-table-recognition-execution.md` §10.5）：G2 通过（全量 7,104 样本 `recognizer_calls = 0`）；非空率判据**已按实测撤回**并替换为"完整 TEDS 不低于对照实现"（实测 +0.0445，逐样本胜 139/200）；**全量完整 TEDS = 0.570710**（基线 `tests/baseline/windows-baseline/table-teds-full-cpu.json`）；L2 回归顺延 E11 |
| 8 | R2 表格路径独立 det 分辨率（用 L3 数据集定量） | 6 | CER 在容差内（mean CER ≤ 0.03）且 p50 下降 —— **✅ 已完成**（`docs/09-table-recognition-execution.md` §11.4）：CER/IoU 判据**在 TabRecSet 上不可测**（无文本框与文本真值），改用完整 TEDS + `unmatched_cells` + `non_empty_rate` + `regions_seen`；区域数差 ≤ 5 % 淘汰了 `min320`（+6.06 %）；默认值定为 **`limit_side_len=736 / limit_type="max" / max_side_len=736`**，完整 TEDS **0.570710 → 0.583404**（+0.012694）而 `page_ocr.ocr_ms` p50 **549.159 → 108.907/112.736**（**4.87–5.04×**），两轴均为严格改进 |
| 9 | R8 表格定位走文本分布启发式 | 3 | **G4 通过**；区域召回达标；`ocr_total_ms` 无增量 —— **✅ 已完成**（`docs/09-table-recognition-execution.md` §12.4）：**G4 通过**（行为面 + 静态面两层断言）；召回判据**已按实测撤回并替换为逐图判据**（三张图没有区域级真值）；`ocr_total_ms` 增量按构造为 0（**尚未接进页面管线**），定位器自身 **1.713 ms/call**（约整页预算的 0.17 %）。**实测退化了**：多块并排页面上假合并（真表格内部空隙 404.8 px > 并排块 gutter 159.7 px，无阈值可分开），且无文本表格原理上不适用 |
| 10 | R5 表格 session 线程数（仅在超出噪声带时才改） | 1 | 线程矩阵 p50 超出 ±10–15 % —— **✅ 已完成**（`docs/09-table-recognition-execution.md` §13.5.3）：判据**实测后修正**（`run_thread_matrix.ps1` 走页级、表格未接线 → 观测不到，改走 `table_eval` 6 次重复）；**判定回退** —— 最大偏差 −6.89 % 小于 10 % 带底、也小于 `default` 自身的 9.30 % 离散度 → `intra_threads` 默认保持 `None`（继承）。机制（字段 + 可覆盖）已交付 |
| 11 | R7 DirectML per-EP 配置（仅在有 DML 设备时） | 8 | `check_provider_claims.ps1` 通过 —— **✅ 已完成**（`docs/09-table-recognition-execution.md` §13.5.5）：per-EP 分支已落地（`SessionBuildPolicy` / `SessionBuildOption` / `require_direct_ml_opset` / `resolve_execution_providers` 提前）；`check_provider_claims.ps1` 对已入库三报告的判定与本阶段前**逐字相同**；**DirectML 对表格模型实测不可用**（`Loop.0` 体内 `Cast.73` 报 `E_INVALIDARG`）→ 不构成加速声明 |
| 12 | 输出、降级与登记收尾（emitter 定案 + 网格占满性 + 降级档 + manifest 登记 + `THIRD_PARTY_NOTES`） | 5、7、9 | 属性/转义/跨行跨列/空表/Markdown/降级标记均有测试；加载路径走 `verify_sha256`；包内容清单干净 —— **✅ 已完成**（`docs/09-table-recognition-execution.md` §14.3）：emitter 四函数定案（`emit_html` / `emit_table_html` / `emit_markdown` / `structure_degradations`），`TextPolicy` 显式化解决"评测要 Raw、输出要 Escape"的冲突；`GridCoverage` / `StructureTermination` 提供占满性与截断信号；**全量实测 `low = 350`（4.94 %）/ `truncated = 0`**；登记落为 `models/table/manifest.json` + `ModelRole::TableStructure`；`cargo package --list` 268 个文件、包内**零**模型/数据集/开发机绝对路径 —— **E17 复核时改为 272 个**（271 → 272：`tests/fixtures/table-golden/input_bgr_resized.bin` 曾被 `.gitignore` 的 `*.bin` 吃掉，见 §14 的 E17 登记项） |

**注意第 5 步的位置**：它在 R1/R2 之前。理由是第一性排序 —— R1/R2 都是"用精度换性能"的旋钮（§2 K1/K2），**在拿到 baseline TEDS 之前无法判断它们是否越界**。先有数字，再动性能。

**贯穿全程的硬约束**：每一步都必须有"改前基线 → 改动 → 新行为验证 → 既有行为验证 → 对比"的完整闭环（`AGENTS.md` §6/§9），并且**性能数字一律来自 Rust harness**，不得引用本文档中带 [推算] 的 Python 探针绝对值。

---

## 14. 未确认项登记（不编造）

**关于模型**

- SLANet_plus / SLANeXt 的**独立论文**：不存在。SLANet 本身没有独立论文，只在 **PP-StructureV2**（[arXiv 2210.05391](https://arxiv.org/abs/2210.05391)）内提出；其**同行评审会议/期刊未找到**（官方目录仅列为 arXiv preprint）；
- SLANet_plus 在 **PubTabNet / cTDaR / WTW 上的 TEDS**：未找到（官方只有内部集 Accuracy 63.69）；
- PaddleOCR/PaddleX **ONNX 模型的 INT8 量化官方指引**：未找到；**ONNX Runtime 后端的 FP16**：未找到（FP16 仅记录于 TensorRT 后端）；
- **官方发布的 SLANet ONNX**：不存在（见 §8）；
- **PP-OCRv6 det 被官方特别推荐用于表格**：未找到（官方表格管线列的是 PP-OCRv5_server/mobile_det）；
- 名称冲突：另有一个无关的 "SLANet"（Qasim/Mahmood et al., ICDAR 2019），一手 venue/DOI 未核实；
- **S-TEDS** 指标的官方定义/实现：未找到。

**关于数据集**

- FinTabNet 原始一手下载：未找到（IBM 页面已失效）；
- cTDaR 可用下载：未找到（官方 GitHub 404）；
- SciTSR / WTW / cTDaR / ICDAR-2013 GT / SynthTabNet / TAL 的**许可**：均未找到；
- PubTabNet 许可冲突（`cdla-sharing-1.0` vs "CDLA-Permissive"）、TableBank 许可冲突（`apache-2.0` vs "请勿再分发"）、**TabRecSet 许可冲突（CC BY-SA 4.0 vs CC BY 4.0）**：**官方两侧说法不一致，均未解决**；
- PubTabNet 是否发布 test 集真值：未确认；
- WTW 官方评测代码：未找到（repo 标 "To be updated"）。

**关于本项目**

- ~~**`rescale_cell_bboxes` 的坐标系语义：未实测。** 这是 R1 的前置阻塞项，也是 §13 第 3 步；~~ **已实测关闭**（`docs/09-table-recognition-execution.md` §5.4）：坐标系是**原图（crop）像素**，即 `模型归一化输出 × max(h, w)`，见 §12 R1 注；
- ~~**DirectML 下 SLANet 的 `Loop` 控制流算子是否可执行：未验证**（本机无 DirectML DLL）；~~ **已实测关闭**（`docs/09-table-recognition-execution.md` §13.5.2）：**不可执行** —— 会话能开（`selected_ep == DirectMl`、`fallback_to_cpu == false`、opset 14 ≤ 20），但推理必失败于 `Loop.0` 体内的 `Cast.73`（DML 报 `E_INVALIDARG`/`0x80070057`，源头 `MLOperatorAuthorImpl.cpp(2870)`）。**原文"本机无 DirectML DLL"是工具链错配**：那是 Python `onnxruntime` 的 provider 列表，Rust 构建静态链接的 ORT 1.28 已加载 `DirectML.dll`。→ **"能开会话"不能作为 DML 可用性证据**；E13 需决定是否在 `open` 时拒绝 DML；
- **表格路径 det 长边上限低于 736 的档位（如 512 / 320）：未实测**。E10 只证了"736 与 1280 在完整 TEDS 上无差别（差 0.000528）"，**不能推出"再往下压也无损"** —— TabRecSet 长边中位 448 px，压到 512 很可能开始损精度，但这只是推测（`docs/09-table-recognition-execution.md` §11.4.7 第 5 条）；
- **表格路径 det 输入的"上界存在"这个语义：没有单独的测试**。它靠 `TableOcrResolution::apply` 覆盖 `global.max_side_len` 实现，若有人把它改成只覆盖 `det.*`，上界会**静默消失**（§11.4.7 第 3 条）；
- **`TableOcrResolution.limit_side_len` 在默认档位下是死字段**（`limit_type = "max"` 时 `src/ocr/det/detector.rs:210-224` 直接丢弃配置值）：只被文档与测试标注，**未被类型系统禁止**，是已知设计债（§11.4.7 第 4 条）；
- **固定 H/W 后 SLANet 的实际加速比：未实测**（Python 环境数字不可用；E5 的 debug 计时差 0.16 ms / 0.3 % 落在噪声带内，不下结论）；
- ~~**表格结构模型的最优线程数：未定**（4 vs 8 线程两个口径互相矛盾，落在噪声带内 → §2 K4 的结论是"不改"）；~~ **已实测关闭**（`docs/09-table-recognition-execution.md` §13.5.3）：6 次重复下 `intra=4` 与 `intra=8` 相对 `default`（生效 14）的偏差是 −2.60 % / −6.89 %，**都小于 10 % 带底，也小于 `default` 自身的 9.30 % 离散度** → 判定**回退**，`TableStructureConfig::intra_threads` 保持 `None`（继承）。机制保留、默认值未改；
- **`infer_ms.p50` 在表格域的重现性差：新登记的测量风险**。同一配置 6 次重复的离散度就有 **9.30 %**（`intra=4` 达 **36.46 %**），远超 n=500 时 p50 的抽样误差（约 2 %）→ 主导方差是**重复间系统漂移**。**§2 K4 的 ±10–15 % 判定带对本机表格模型偏乐观**；表格域任何"小幅提升"的结论必须先建立该配置自身的离散度，3 个样本会得出错误结论（E12 实测到过 21.76 % 的假信号）；
- **R5 的线程争抢前提未被实测**：§12 R5 的第一性依据是"表格 session 与 det/rec 的 `allow_spinning` 自旋争抢"，但表格**尚未接进页面管线**（`src/ocr/pipeline/rapid_structure.rs` 把 `table_structure_infer_ms` 写死 `0.0`），所以 E12 的线程实测只覆盖"表格单独跑"。接线后必须重测；
- **DirectML 分支在本机没有端到端可用路径**：`SessionBuildPolicy` 的 `must_be_explicit` 字段是为 DML 的**正确性要求**加的（`ORT_SEQUENTIAL` 恰好等于 ORT 默认值，不能让上游默认值替我保证），但 DML 已知跑不通表格模型 → 该分支只有单测覆盖，"这两项被真实下发"未被会话级验证（`docs/09-table-recognition-execution.md` §13.5.5 第 5 条）；
- **CUDA 分支未加任何 CUDA 专属选项**：§9 说 CUDA 用官方默认（`kNextPowerOfTwo` / `EXHAUSTIVE`），既然就是默认值则不下发。但本机 CUDA 缺 cuDNN、节点全落 CPU，**"CUDA 分支正确"在本机不可验证**（§13.5.5 第 6 条）；
- **表格模型在 `manifest` 中的登记位置：已定案**（`docs/09-table-recognition-execution.md` §14.3）：落为**与模型同目录**的 `models/table/manifest.json`（`src/model_source.rs` 的 v1 schema），`role` 用新增的 `ModelRole::TableStructure`（`"table_structure"`）；`TableStructureSession::open` / `TableStructureModelInfo::probe` 在**创建任何 ORT 会话之前**走 `src/table/registration.rs::verify_registered_model` → `model_verify::verify_sha256`。`models/table/manifest.json` **不在** `models/**` 的 gitignore 范围内（`.gitignore` 只按扩展名忽略 `*.onnx` 等），清单可以入库而权重不入库；
- **表格模型权重的再分发许可：`<待确认>`**（`THIRD_PARTY_NOTES.md` 的「SLANet-plus table structure model」段）：RapidTable **仓库**自带 Apache-2.0，但**权重**是第三方重打包、转换脚本与源 checkpoint 无文档、ONNX 自身也不带许可元数据（实测 `metadata_props` 只有 `character`）→ **不得再分发、不得标成本项目 Apache-2.0**；
- **结构降级档只做到"标记"，没做到"重建"**（§10）：低置信样本仍输出 SLANet 结果，**未**降级到线检测重建的规则网格。线检测是轻量档的完整实现，尚未开工；
- **`truncated` 这条上界从未被触发**（§10）：全量 7,104 样本里 `degradations.truncated = 0`，所以"结构长度超 500 token"只有由图的 `Loop` 上限推出的可能性，**没有样本证据**；
- **`decode_logic_points` 的空洞计数是"互异覆盖格数"而不是"span 面积和"**：矩形可以重叠（E13 有反例测试 `overlapping_rectangles_are_counted_once_not_by_span_area`，`Σ rowspan×colspan = 5` 而 `covered = 4`）→ 用面积和反推空洞会高估，**不可用**；
- **发射器重写改变了预测文件摘要**（`docs/09-table-recognition-execution.md` §14.3.3）：7,104 个样本里 9 个字节不同（8 个属性顺序、1 个重复属性被去掉），**语义与逐样本 TEDS 全部不变**。→ 任何以"逐字节相同"为判据的回归都不能直接套用在 emitter 上；
- **`cargo package` 的原始验收命令按字面不可执行**（§14.2③）：`--allow-dirty` 是布尔开关；且本仓库工作树先于本阶段就是脏的。改为断言**包内容清单**（见 `docs/09-table-recognition-execution.md` §14.3.5）；
- **公式与单元格富文本的整合协议：未定**（§7.4）；
- ~~**Markdown 对 rowspan/colspan 的最终呈现规则：未定**（§7.5）。~~ **已由 E15.3 定案**（`docs/11-table-page-pipeline-execution.md` §3.5）：span 语义在 Markdown 里**仍然丢失**（这是格式的能力边界，不是实现缺陷，没有改变），但**不允许静默丢失** —— 只要网格里存在 `rowspan > 1 || colspan > 1`，`src/table/emitter.rs::emit_markdown` 就在表格**前后各加一行** `const MARKDOWN_SPAN_NOTE`（`<!-- 表格含跨行/跨列，Markdown 无法表达；已按逻辑网格展开，跨过的位置为空位。无损表示见 HTML 字段。 -->`）；无跨度时一个字都不加。说明行是 **HTML 注释**：在渲染后的 Markdown 里不可见（不污染阅读），在原文里可见（可被 diff 与测试命中）。`<br>` 之类的替代表示**仍然未采纳** —— 需要 fixtures 与产品确认，本轮没有做；
- ~~**表格结果公开类型：未定**（§6）；~~ **已由 E14 定案**（§6.1）：`RegionKind::Table` + `TableOutcome` / `TableCellOutcome` / `TableStructureConfidence` / `TableDegradation` / `TableCellMatch` / `TablePolicy`，全部落在 `src/api.rs` 这个线契约模块；`src/table/` 保持私有实现。**未定的是 Markdown 对 rowspan/colspan 的呈现**（E14 填了 `TableOutcome.markdown`，但沿用"span 位置留空"的规则，未端到端核对）；
- **`input_other_ms = 79.86`（8.1 %）的构成：未定位**。已定位的候选窗口是 `src/ocr/pipeline/rapid_structure.rs:156-162`（resize ≈4.7 ms/页）、`:189-195`（vertical padding 0.8–3.5）、`:210-212`（crop 1.5–2.5）、`src/ocr/rec/recognizer.rs:89-98,222-231`（批次装配/排序/bidi 0.2–2.1）、`rapid_structure.rs:106-132`（≈0.2），但这些加起来远小于 80 ms，**说明还有一个未识别的输入侧区段**。这一项建议先做定位再谈优化。
- **表格定位器的假合并未修**（`docs/09-table-recognition-execution.md` §12.4.2）：多块并排页面把左下键值对与右下表格并成一个区域。机制已量化（真表格内部空隙 404.8 px > 并排块 gutter 159.7 px），但**没有解法**；生产路径上回填的几何匹配会跨块错配；
- ~~**表格定位器尚未接进页面管线**：所以"整页 `ocr_total_ms` 无增量"是按构造为 0，不是端到端实测；接线后需按 G5 复测（§12.4.7 第 1 条）。~~ **已由 E14 关闭**：整页管线已接线（`src/ocr/pipeline/rapid_structure.rs` 的 `recognize_with_table`，派发顺序表格优先于公式），G5 已复测并定值 **120 ms**（可归因分量之和的中位数 56.491 ms），`src/table/detection.rs` 的模块级 `#![allow(dead_code)]` 已删除；完整记录见 `docs/11-table-page-pipeline-execution.md` §2.5。**仍然成立的部分**：E14 的表格开关只在 `OcrRequest` 与 `bench_warm_e2e` 的 `--table-model` 上，**尚未接进 YAML 配置 / CLI / serve**（那是 E16 的范围）；
- **`TableDetectorConfig` 的 6 个默认值只在 3 张图、共 136 个文本框上验过**：`min_columns = 2` 有第一性依据，其余 5 个（`row_center_tolerance` / `column_overlap_ratio` / `max_row_gap` / `region_margin_ratio`）是工程默认值，**没有实测支撑**，也未做参数敏感性分析（§12.4.7 第 5 条）；
- **跨页表格未测**：定位器只在单张图上工作，跨页拼接既无接口也无测试（§12.4.7 第 6 条）；
- **无文本表格（`table_without_txt.jpg` 类）目前是"检不到"而不是"降级处理"**：专用模型本轮未引入（§12.4.3 给了三条不引入的实测依据）。

**E14（整页管线接线）新登记的未确认项**（完整记录见 `docs/11-table-page-pipeline-execution.md` §2.5.6）：

- **假合并缺陷在整页路径上复现且未修，影响比 E11 估计的更大**。`04表格与键值对.png` 上 2 个区域里有 1 个横跨两块并排结构；整页文档里并排栏位比 E11 的纯表格验证图更常见。E14 的端到端测试把这个现状**钉成断言**（`bounds.left <= 200 && bounds.right >= 2900`），修好会大声失败、必须同步文档，但缺陷本身没动。需要 E17 用 E8 的测试集量化失败率；
- **公式 + 表格同时启用没有端到端实测**。语义已定案（表格在外层、定位与结构模型吃原图、公式区域由 `kind == Formula` 过滤掉），但没有"表格内含公式"的测试图。风险是：落在表格内的公式区域**不**参与回填 → 对应单元格为空，而公式区域本身仍在 `regions` 里，消费者看到"空单元格 + 一条公式"而不是"含公式的单元格"。这不是 bug 而是**未定义的产品行为**；
- **峰值工作集上升约 161 MB（+12 %）没有预算**。G5 只管时间。增量来自表格会话（SLANet-plus + ORT arena）与整页 `RecImage` 的复制（`decode_source_image` + `rec_image_from_dynamic` 各复制一份 3200×2000×3 ≈ 19 MB）。**未尝试复用文本阶段已解码好的图** —— 后者需要把解码结果向 `recognize_text` 之外暴露，是一个明确的后续优化点；
- **`table_structure_budget = 120 ms` 只在 12 图、`max_side_len=2000`、`intra_threads=16`、CPU 上测过**。其它分辨率、其它 provider、其它图集上没有依据。它是**第一版**阈值，可被后续实测修正；
- ~~**`TableOutcome.markdown` 在 E14 就填了，但 Markdown 表达不了 `rowspan`/`colspan`**（§7.5）：span 损失尚未在任何通道上做过端到端核对（E15.3 的职责）；~~ **已由 E15.3 关闭并端到端核过**：`04表格与键值对.png` 上两张表（cells 31 + 28）的 `degradations == []`、`holes == 0`、`confidence == high`，**结构完全良构却仍然丢跨度** —— 这是"说明行必须挂在结构层、由 `has_span` 驱动"而不是"由降级档驱动"的实测依据。真实产物的 markdown 里每张表前后各有一行说明（`docs/11` §3.5.3）；
- **`table_source_lines` 只消费 `kind == Text` 且有 `polygon` + `recognition` 的区域**。因此 `RegionSource::Input`（整图当一张文本）与无 polygon 的区域都不参与回填 → "整图识别"模式下表格会全是空单元格。这是刻意的（没有几何就无从匹配），但**没有测试也没有文档警告**；
- **DML + 表格模型仍不可用**（§14 上方的实测条目）。E14 未做任何缓解，也没有在 `Session::open` 时拒绝 —— 用户拿到的是一条运行期错误，错误信息点名 `Loop.0` / `Cast.73`。是否要在 `open` 时拒绝（代价：可能误拒 ORT 修好后的未来版本）留到 E17 定案；
- **表格路由的 `roi` / `tile` 拒绝是"请求级"而不是"策略级"**：`TablePolicy` 看不到 `roi`/`tile`，所以这个不变量住在 `OcrRequest::validate` 里。若将来有人绕开 `validate` 直接构造请求，互斥就会失效（目前所有入口都先调 `validate`）。

**E15（输出三通道：JSON / HTML / Markdown）新登记的未确认项**（完整记录见 `docs/11-table-page-pipeline-execution.md` §3.5.6）：

- **`TableOutcome.markdown` 与 `TableOutcome.cells` 之间没有不变量**。`markdown` 在解码期一次性生成；消费者若改了 `cells`，`markdown` 会静默分叉。E15 只钉住了 `html` ↔ `table_html` 的恒等式（`src/ocr/pipeline/rapid_structure.rs::finalize_table_html` 的 `debug_assert_eq!`），而那个断言**只在 debug 构建生效**，release 静默容忍；
- **净化只覆盖 `html` / `table_html`，不覆盖 `markdown` 与单元格 `text`**。面向用户的 `markdown` 里，单元格文本中的 HTML 片段（PubTabNet 真值里 `<` / `>` 各 4,171 次且成对）是**原样保留**的（与 `TextPolicy::Raw` 同理，见 `emit_markdown` 的文档）。若下游把 `markdown` 喂给未净化的 HTML 渲染器，这是一个注入面；
- **`sanitize_table_html` 的白名单没有做过 fuzz**。现有证据是"发射器产物 + 合成攻击样本 + 幂等性"，不是"任意输入"；"裸 `>` 截短"等边界行为只有单测覆盖；
- **`TableOutcome.model_id` 是模型路径原文**（`models\table\slanet-plus.onnx`）：会泄漏本地路径，也让同一模型的两次识别在 JSON 上不可比。E14 的既有行为，E15 原样保留；
- **三通道一致性只在一份 2×2 合成夹具上被断言**（`src/output/json.rs::tests::the_three_channels_agree_on_the_table_grid_and_its_cells`）。**没有**在带洞 / 带降级档 / 带跨度的真实 `TableOutcome` 上做跨通道断言 —— 而这三类恰恰是各通道最容易分歧的地方；
- **`plain_text` 会丢掉"不在页面行里的表格文本"**：页面路径只用 `BackfillSource::Reuse`，被消费的行仍以行的形式留在 `plain_text` 里。但若将来启用 `BackfillSource::Crop`（逐 cell 识别），那些**只存在于表格里**的文本不会出现在 `plain_text` 中，而 `plain_text` 正是 CER 与网页复制全文的来源；
- ~~**网页侧（E16）尚未消费新结构**：`src/bin/serve/` 一行未改，`/api/ocr` 的 JSON 里 `items[].table` 与顶层 `tables[]` 已就位，但页面还不渲染表格；~~ **E16 已关闭**：页面新增 `kind === "table"` 渲染分支与 `sanitizeTableHtml()`（门禁 G8）、四态模式选择器、常驻模型清单面板；`src/bin/web/index.html` 从 211,933 B / 5430 行增到 253,135 B / 6195 行。完整记录见 `docs/11-table-page-pipeline-execution.md` §4.8；**E24 又改了一次渲染时机**：表格从"无条件渲染"改成"默认原文 + 「渲染表格」按钮"（记录见 `docs/11` §4.11），净化仍是唯一入口；
- **`cuda` / `direct_ml` 下表格的三种输出未被端到端核对**：E15 的 provider smoke 只跑 `bench_warm_e2e`（**不开表格路由**），所以"表格 HTML 在 DirectML 下是否逐位相同"仍是空白（与 E12 登记的 DML 项同一来源）；
- **开启表格后 HTML 报告的内存峰值没有测过**：E14 的 +161 MB 峰值是 `bench_warm_e2e` 口径；三通道都在同一份 `OcrOutput` 上工作、没有引入新的常驻缓冲，但"渲染完整 HTML 报告"这条路径没有内存数据。

**E16（本地服务与网页：模型清单 / 模式选择 / 表格渲染）新登记的未确认项**（完整记录见 `docs/11-table-page-pipeline-execution.md` §4.8.6）：

- **"真实提交一张含表格的图、在浏览器里看结果"这条链路没有用鼠标走过**。E16 的页面验收是**无头浏览器 + 真实页面脚本 + 伪造的 `TableOutcome`**（实测 `document.title` = `items=4 | tables=1 | wraps=1 | realTables=1 | tds=3 | spanKeep=1 | dropped=0 | folds=1 | badge=低置信（网格空洞或截断） | barHidden=false | regions=4`）。`?table=1` 的**服务端**链路有端到端测试，但人工点一次这件事没有做； **手工验收由用户负责**（用户声明「本地网页测试由我负责」），故本条按交接项处理。
- **前端净化只覆盖 `table_html` / `html`**：`cells[].text` 与 `markdown` 不经 `sanitizeTableHtml`（与本文档 E15 那条同源）。当前页面只把单元格文本按文本渲染，所以这条路径是安全的，但"净化器覆盖了全部注入面"这个**更强的命题不成立**；
- **`sanitizeTableHtml` 的白名单没有 fuzz**：11 个手写 payload 覆盖了 `<script>` / `<img onerror>` / `javascript:` / `onclick` / `<iframe>` / `<svg/onload>` / `style` 七类，mXSS、命名空间混淆、`<template>` 内容、`data:` URL 等变体**未覆盖**；
- **`?table=` 这个新契约面只在两处登记**（`docs/05-local-web-demo-implementation.md` §4.2.2 与本文档 §6.1 / §7.7）：更早的文档里"队列类别就是管线选择"这句仍然成立的字面表述没有逐处复核；
- **网页在 `cuda` / `direct_ml` 下未被端到端核对**：E15 的 provider smoke 不开表格路由，E16 的页面自检是离线预览（DEMO）——没有一次"非 CPU provider + 表格 + 网页渲染"的实测。

**E17（收尾与登记）新登记的未确认项**

- **`tests/fixtures/table-golden/input_bgr_resized.bin` 曾被 `.gitignore` 的 `*.bin` 静默吃掉**：`cargo package --list` 里没有它（271 个条目），干净 clone 的逐位比对会**退化成 skip**。E17 已修 —— `.gitignore` 增加 `!tests/fixtures/table-golden/input_bgr_resized.bin`，包清单 271 → **272**。**但没有测试断言这个文件的存在**：它再次被忽略时只会安静地 skip，只能靠人工跑 `cargo package --list` 发现；
- **本阶段（`docs/09` + `docs/11`）的全部产物尚未提交 git**：`git ls-files src/table` = **0**、`git ls-files tests/fixtures/table-golden` = 0。`cargo package --list --allow-dirty` 之所以通过正是因为有 `--allow-dirty`；**干净 clone 既拿不到代码也拿不到 fixture**。release 之前必须先提交；
- **`THIRD_PARTY_NOTES.md` 只补齐了表格域的 fixture 来源**：`table.jpg`（上游 RapidTable 的测试图，该仓库 `LICENSE` 为 Apache-2.0，11,337 B，已在本地核实）是 E17 唯一新增记录的第三方测试资产；**全仓其余 fixture 的来源/许可没有重新清点**；
- **`TableOutcome.model_id` 仍是模型路径原文**（`models\table\slanet-plus.onnx`，见本文档上文那条 E14 登记项）：会泄漏本地路径，也让同一模型的两次识别在 JSON 上不可比。修它要动 `src/api.rs` 的公开字段，所以**不在收尾阶段顺手改**；
- **DirectML 下是否要在 `TableStructureSession::open` 时拒绝表格模型：仍未定案**（E14 登记，E17 复核时仍未改）。用户现在拿到的是一条运行期错误（`Loop.0` 体内的 `Cast.73`），错误信息点名了算子 —— 但"能开会话"这个更早的假信号还在。

**E23（折行单元格把一张表切碎）新登记的未确认项**

- ~~**折行单元格的续行会被回填匹配到上一行**~~ **E25 已按下面的方向修好**：`r6 c1` 塞进三行、`r7 c1` 塞进两行、`r8 c1` 只剩一行（实测于用户那张生产记录表）。机制是**结构模型预测的单元格框整体比真实行低 8–15 px**（模型行带 r6 `333-398` / r7 `392-460` / r8 `452-518`，真实行约 `318-380` / `380-445` / `440-500`），于是续行 `y383-407` 与 `r6c1` / `r7c1` 的 IoU 是 **0.1989 / 0.1884**（差 1 %）、下一行首行 `y443-468` 是 **0.2154 / 0.2072**。**"保序约束"不解决它**：E23 实测"行号非降下最大化总 IoU"的动态规划给出的指派与旧贪心**逐字段相同**，因为正确指派的总 IoU 反而更低（`1.3479` 对 `1.3666`），且按 y 排序后两种指派的表行号本来就都是非降的（那次改动已**完整回退**）。**E25 采用的方向**（原记录在此）：从文本行序列本身推行使区间（行边界的间隙实测 32 / 31.5 px，行内只有 12.5–15.5 px），而不是信任漂移的模型框 —— 已实现为 `assign_row_bands`（行数用结构模型的、行边界用文本行的），见 §7.4 的 E25 定案与 `docs/11` §4.12；**残留**：折行续行落在**哪个模型行**现在稳定了，但"续行与首行在同一个单元格里如何拼接"仍只是空格连接（`join_line_texts`），没有插入换行；
- **表题（表格框线之上的跨列标题）留在表格区域之外**：它与表头之间没有列结构，几何上与"表格上方的标题"不可区分，因此作为普通文本区域输出。后果是 `DetectedRegion.rows` 比真实行数少 1，且用户在结果里会看到一个看似多余的文本区域。**没有解法被验证**（`region_margin_ratio` 只是把边界往外推，会把标题之外的正文也吞进来）；
- **定位器改动对全量 TEDS 是恒等变换，因此这份指标对定位器零覆盖**：E23 改动后重跑全量 7,104 张（`table_eval --content page-reuse` + `tools/table_teds.py`）得 `mean TEDS = 0.5834044577250798`，预测文件 `sha256 = df6bd2c0…`、3,744,372 B，**与入库基线逐字节相同**。原因不是"改动无效"，而是**离线评测不调用定位器**：`src/evaluation/table/` 里零引用 `table::detection`，它把**整张图**喂给结构模型（`src/evaluation/table/run.rs:1437-1459`），而 TabRecSet val 是"一张表一张图"的裁剪集；定位器的唯一生产消费者是 `src/ocr/pipeline/rapid_structure.rs:1391`（整页管线）。**后果**：入库基线 `table-teds-full-cpu.json` 对**定位器**的改动是恒等的（不需要为 E23 重跑）；但 **E25 的改动（单元格行归属）它没有覆盖** —— 同一口径在 E25 之后是 `0.583640`（`+0.00024`，预测 3,744,387 B），该入库文件仍停在 E25 之前的值上，引用时按 §14 的提示连着"测于 E25 之前"读（裁定见 `docs/11` §4.12）。要度量"表格被切碎 / 漏检"必须换整页数据集或自建分层集 —— 现有协议量不出来（登记于 `docs/11` §4.10）；
- **E9 时代的 OCR 缓存 `Temp/ocr-cache-val.json` 已被引擎拒绝**（`missing field \`engine_fingerprint\``）：缓存格式在 E13 前后变过，现在只有带 `engine_fingerprint` 的缓存可复用（`Temp/e13-cache-baseline.json`，`d9a39222…`）。热跑约 4 分钟、`cache_hits 7104`、`regions_seen 0`（热跑不给页级 OCR 计时证据）；
- **只在 `test-config-small.yaml`（page_reuse）+ 用户那张图 + 既有夹具上验证过**：`min_columns = 2` 时"细行"的定义依赖页级文本通道的产出；若某页的折行把**左列**也拆开（当前样本里左列永远是一行），锚行会减少，`min_rows` 的语义需要重新检验；
- **`detect_regions` 的锚行判据用的是 `column_count() >= min_columns`（成员框个数）**：一个跨两列的单元格在视觉行上只贡献 1 个框，所以"列数"是**下界**——这条从 E11 起就是这样，本轮没有改，但折行修正把它的影响从"切碎"（召回）转移到了"锚行判定"（精度），值得在真实业务页面上重新量化；
- **新增的两条测试都依赖硬编码坐标**（一条合成 7 框、一条用户真实页的 27 框）：它们是**当前失效输入的回归钉子**，不是性质测试 —— 若将来换掉 `group_rows` 的容差或聚类方式，断言值需要重新导出（脚本 `Temp/e23-emit-test-lines.py` 是导出方式；它是 Temp 下的一次性脚本，**未入库**）。