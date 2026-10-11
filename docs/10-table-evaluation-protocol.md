# 10 表格识别评测协议

本文件是表格识别评测的**协议**。它回答三个问题：重复跑之间允许多大差异（ε）、
逻辑网格以哪份标注为准、以及哪些样本的失败是模型在构造上不可能避免的。

本文件只放**论证与实测数字**；能被编译期常量、纯函数与单测钉住的那一半在
`src/evaluation/table/protocol.rs`。分成两半的理由：文档里的数字会腐烂，代码里的断言不会。
`span_limits` 的教训正是"把从词表推出来的上限硬编码"会在换模型时静默失效。

> **依据等级**：`[实测]` = 本机跑出来的数字（命令与产物路径随文给出）；
> `[推算]` = 由实测数字直接推导；`[未测]` = 尚未验证，明确登记。
> 性能绝对数字只允许来自 `src/bin/bench_warm_e2e.rs` 与 `tools/run_windows_baseline.ps1`，
> 本文件不含性能结论。

---

## 1 ε：重复跑容差

### 1.1 结论

**ε = 0，且 0 的含义是"逐位相同"，不是"差得很小"。**

实现：`src/evaluation/table/protocol.rs` 的 `REPEAT_EPSILON` 与 `within_repeat_epsilon`。
报告里的 `protocol.repeat_epsilon` 与 `protocol.repeat_epsilon_meaning` 会把这个口径
随每份报告一起落盘 —— 一份 2027 年的报告要能自己说清它用的是哪套规则。

### 1.2 实测依据

`docs/09-table-recognition-execution.md` §8.3 要求同配置重跑 3 次。E8 在实现定稿后又重编译并跑了第 4 轮（见 `docs/09-table-recognition-execution.md` §9.4.4 第 2b 步），因此下表是 **4 轮**：

| 项 | 值 |
| --- | --- |
| 轮次 | **4**（3 轮同一二进制 + 1 轮在重编译之后） |
| 配置 | `input_side_len=488`、`fix_input_shape=true`、`intra_threads=14`、`inter_threads=1`、`limit=0`（全量 7,104） |

> `intra_threads=14` 是**本机自动推导值**（`auto_tuned_thread_budget()`），不是调优结果。E12 用 6 次重复对照过 `4` / `8` / 继承，最大偏差 −6.89 %，**小于该配置自身的 9.30 % 重复间离散度** → 判定无更优设置，本协议继续钉住 14（`docs/09-table-recognition-execution.md` §13.5.3）。⚠️ 也因此**不要**把本协议里的 `intra_threads` 当作可调性能旋钮：改它会让报告不可比（§5.3 的缓存指纹把线程数计入），且改动本身落在噪声内。
| 预测文件 | 2,364,628 B，sha256 `2cfafefff31be2eee3e7dae13ef52371c4755f39208776dec6698d89d85288f0` |
| 轮间差异 | **逐字节相同**（4 轮同一个 sha256） |
| TEDS-Struct | 0.706518（4 轮相同） |
| stddev | **恰好 0.0** |
| 整轮墙钟 | 246,894 / 246,429 / 260,943 / 250,266 ms（极差 5.9 %） |

第 4 轮的意义：它证明 E8 期间的两处代码清理（线程日志格式、把类别数检查移到解码之前）**没有改变行为**。第 4 轮是与前 3 轮**不同的二进制**，却产出同一个摘要。

> **`loop_wall_ms` 的 5.9 % 波动与 ε 无关**：那是整轮墙钟（含 I/O、图片解码、进程调度），ε 约束的是**分数**。预测文件逐字节相同 ⇒ 4 轮分数完全相同。按 doc 07 §0.3 / §0.4，单机 wall 数不构成性能结论；性能数字只允许来自 `src/bin/bench_warm_e2e.rs` 与 `tools/run_windows_baseline.ps1`。

`[实测]` 于是任务清单里"取 `Stats` 的 stddev 作下界，乘安全系数"中的下界**就是 0**，
安全系数这个乘数在数学上不产生任何作用。与其保留一个不起作用的乘数，不如把 0 的含义
写清楚。

### 1.3 为什么可以断言"不存在抖动"而不只是"测不出抖动"

机制上的四条理由，每条都能被指到具体代码：

1. **输入形状固定**为 `[1,3,488,488]`（`TableStructureConfig::fix_input_shape`，
   见 `docs/09-table-recognition-execution.md` §6），因此不存在与形状相关的分支。
2. **batch 恒为 1**：本 harness 逐样本推理，不存在批内归约顺序问题。
3. **CPU 上的 ORT 对同一份图与同一份输入是确定的**；provider 解析在会话打开时
   完成一次，不随样本变化。
4. **解码是纯函数**：`argmax_structure` → `decode_token_steps` → `tokens_to_structure`
   → `structure_to_html`，不含原子操作、不含哈希序迭代、不含时间与随机数。

因此 **一次重复跑出现任何非零差异都不是噪声，而是缺陷**。这也是
`within_repeat_epsilon` 在 ε = 0 时退化为**逐位比较**而不是 `<= 0.0` 的原因：
后者会把 `-0.0` 与 `0.0` 判为相等，而这里的 0 主张的是"相同"，不是"差值可忽略"。
两者的强度不同，写法的强度也必须不同。

### 1.4 指标分辨率（与 ε 是两件事）

ε 管的是**同一份配置重复跑**的差异；指标分辨率管的是**指标本身能表达多小的变化**。
两者都影响"这次改动是否让模型变差"的判断，但不能互相替代。

TEDS 的公式是 `1 - 编辑距离 / max(节点数_pred, 节点数_true)`，因此单个节点的增删
造成的最小分数变化是 `1 / max(节点数)`。真值 HTML 的节点数实测（`[实测]`，7,104 个样本，
`len(lxml.xpath(".//*"))`）：

| 统计量 | 值 |
| --- | --- |
| min | 5 |
| p50 | **25** |
| p90 | 64 |
| p99 | 128 |
| max | 196 |
| mean | 33.38 |

`[推算]` 于是**中位表格**的最小可分辨变化约 **1/25 = 0.04**，最大的表格约
**1/196 = 0.0051**。这意味着：

- 一个"只影响少数表格、每张差一个节点"的改动，在 7,104 个样本的平均分上
  可能只移动 0.001 量级 —— **在平均值上看不见，但它是真实的结构变化**。
  因此回归判据不能只看总分，必须同时看**逐样本差值分布**与**预测文件哈希**。
- ε = 0 并不等于"任何差异都是回归"：差异是**确定的**（可复现），
  它只是说明"这次改动确实改变了输出"，方向好坏仍要看分数。

`[实测]` `python tools/table_teds.py --resolution --true <gt> --pred <pred>` 在
7,104 个样本上给出：

| 项 | 值 |
| --- | --- |
| 真值节点数 | min 5 / p50 25 / p90 64 / max 196 / mean 33.38 |
| 单样本分辨率 `1/max(nodes)` | min 0.003802 / **p50 0.040000** / p90 0.071429 / max 0.200000 |
| **数据集均值可见度** | **0.00000589** |

"数据集均值可见度"= `mean(1/max(nodes)) / N`，即**一张表里一个节点变化时数据集均值移动的
幅度**。`[推算]` 因此**数据集均值上小于 5.9e-6 的变化不构成任何证据**：它小于
"单张表动一个节点"的效果，也就是说它连一次最小结构事件都盖不住。

这条与 ε = 0 不矛盾：ε 说的是"同一份配置重复跑必须逐位相同"，
可见度说的是"不同的配置之间，均值差异要多小才算看不出来"。前者是**正确性**判据，
后者是**灵敏度**判据。

`[未测]` 尚未实现按样本类型（行数、列数、单元格数）分层的分辨率报告；
`--resolution` 给出的是数据集级与逐样本级的分辨率，不是分层后的。

---

## 2 网格规则：逻辑网格以哪份标注为准

### 2.1 结论

**权威来源是 token 流**（`AUTHORITATIVE_GRID_SOURCE = GridSource::TokenStream`），
**空洞保留**（`UNCOVERED_POSITION_RULE = UncoveredPosition::KeepHole`）。

### 2.2 数据集里有两套网格信息

| 来源 | 位置 | 性质 |
| --- | --- | --- |
| token 流 | `pubtabnet_format/val_CAPTIONS_STRUCTURE.json` | 顺序扫描 + `rowspan`/`colspan` 占位即可重建，**构造性完整** |
| LabelMe | `cropped_datasets_1Tab_per_img/val/*.json` 的 `label` 字段 | `row-col-rowspan-colspan-text`，**只在部分样本上自洽** |

### 2.3 实测依据

`[实测]`（7,104 个 val 样本，双源交叉验证）：

| 判据 | token 流 | LabelMe 位置字段 |
| --- | --- | --- |
| 单元格数量与对方一致 | 7,104 / 7,104 | 7,104 / 7,104 |
| `(rowspan, colspan)` 多重集吻合 | **7,104 / 7,104** | **0 / 7,104** |
| 网格自洽（无重叠，`sealed`） | — | **5,995 / 7,104（84.4 %）** |

LabelMe 不自洽的原因已定位：跨列超级单元格与其后单元格的起始列会错位半个跨度。
抽检样本 `030d2e51a4dd5cf540101dd1d33ece92ea572d93_0_all-line.json`（108 个单元格、
36 处重叠）里，单元格 `(1,3,1,2)`='商品A' 与 `(1,4,1,2)`='4' 在格 `(1,4)` 重叠 ——
**LabelMe 的 row/col 字段不是全局一致网格**。

因此 LabelMe 位置字段只可用于**几何/多边形**用途，不可用于网格语义。

### 2.4 对 doc 09 §9.1 ② 措辞的纠正

任务清单写"铺满率 82.8 %"，容易被读成"token 流网格有 17.2 % 是空洞"。**不是。**
82.8 %（331/400）是 **LabelMe 在 400 张稳定哈希子集上**、按解释 A（字段 3 = `rowspan`）
的铺满率（解释 B = 301/400 = 75.3 %）。token 流网格的实测空洞率是 **0.34 %**：

| 项 | 值 |
| --- | --- |
| 有空洞的表 | **24 / 7,104（0.34 %）** |
| 无空洞的表 | 7,080 |
| 空洞位置总数 | 132 |
| 单表最多空洞 | 27 |
| 最大网格 | 41 行 × 24 列 = 216 格 |

### 2.5 为什么空洞保留而不是铺满/延伸/剔除

| 候选 | 否决理由 |
| --- | --- |
| 按最小包围盒铺满 | 会**凭空发明**真值里不存在的单元格 |
| 视为 `colspan`/`rowspan` 延伸 | 同样会发明真值里不存在的跨度属性 |
| 剔除该样本 | 丢掉 0.34 % 的样本，且让"这一行比其它行短"这件事从指标里消失 |
| **保留空洞**（采纳） | 空洞在真值里就是空的；指标不该把它当成"漏了一个单元格" |

### 2.6 这条规则对 TEDS 没有影响

TEDS 比较的是 HTML 树，而真值 HTML 本来就如实反映了短行（`<tr>` 里就是少几个 `<td>`）。
`KeepHole` 约束的是**未来**引入网格型指标（GriTS 一类）或单元格几何比对时的行为。
把它现在就定下来，是因为等到引入网格指标时再定，会让"某个历史分数是不是用同一套
网格算的"变成无法回答的问题。

---

## 3 构造性上界扣除

### 3.1 结论

两类样本被**单列**：真值含词表无法表达的跨度（`span_beyond_vocabulary`）、
真值结构串长度超过解码循环的 trip 上限（`beyond_loop_trip_cap`）。

**策略：计入头条指标，排除出回归判据。**

- **计入头条指标**：它们的失败是真实的 0 分。把失败样本从分数里删掉就是把
  "模型答错"变成"样本不存在"，那是只报告对模型有利样本的做法
  （`src/evaluation/formula/sampling.rs` 的模块文档明确禁止）。
- **排除出回归判据**：拿它们判断"这次改动是否让模型变差"，是在判断一个模型
  **无法控制**的性质 —— 真值是否可表达不由模型决定。

实现：`src/evaluation/table/protocol.rs` 的 `UpperBound` / `classify_upper_bound`；
报告里的 `upper_bound` 段给出计数、占比与样例。

### 3.2 第一类：跨度超出词表

`[实测]` 词表上限来自模型自己的 `character` metadata（`rowspan` / `colspan` 各 2..20），
**从词表解析而不是硬编码**，因此换模型会自动更新。

| 项 | 值 |
| --- | --- |
| 超限表 | **3 / 7,104（0.042 %）** |
| 超限单元格 | 6 / 161,925 |
| 实测最大 `rowspan` | 16 |
| 实测最大 `colspan` | **24**（> 上限 20） |
| 样例 | `174515708_ljh_1_no-line.jpg`、`174515708_ljh_1_all-line.jpg`、`174515708_ljh_1_three-line.jpg` |

三条超限样本是**同一张底层表格的三种渲染**，各含 2 个 `colspan="24"`。

`[推算]` 影响极小（0.042 %），不会把头条数字压得看起来像"模型不可用"。
doc 09 §8.4 担心的正是这件事 —— 在该数据集上**不成立**，但统计仍必须单列，
因为它是模型契约的性质而不是数据集的性质。

### 3.3 第二类：超过解码循环的 trip 上限

`LOOP_TRIP_CAP = 500`：`models/table/slanet-plus.onnx` 的 `Loop` 节点携带常量
`assign_0.tmp_0.0 = 500`，每次迭代发射一个 token，遇到 `eos` 提前退出。

| 项 | 值 |
| --- | --- |
| 超限表 | **0 / 7,104（0 %）** |
| 实测最大结构 token 数 | **390** |
| token 数分布 | min 9 / p50 48 / p90 132 / p99 263 / max 390 / mean 65.07 |

`[实测]` 该扣除类在本数据集上**为空**。但仍必须实现，理由同上：这是**模型契约**的
性质，不是本数据集的性质。换一个表格更密集的数据集，这一类立刻非空。

#### 为什么不从模型里探测这个上限

`character` 词表可以通过 ORT 的 `session.metadata_custom("character")` 读到，所以
词表派生的上限是**探测**出来的；但 trip 上限是计算图内部的常量，ORT 不公开图结构，
Rust 侧拿不到。

因此这里用一个**自曝式**常量代替探测：报告里同时记录本数据集实测的**最大结构
token 数**（`upper_bound.largest_structure_tokens`）与这个上限
（`upper_bound.loop_trip_cap`）。两者一旦接近，`by_loop_trip_cap` 的计数就会从 0
开始增长 —— 常量过期会表现为"被扣除的样本变多"，而不是静默失效。

### 3.4 判据来源

两个输入都是**已经算好的事实**，不是估计值：

- `span_beyond_vocabulary` 来自 `span_usage` 对**真值 HTML** 的扫描
  （上限由词表解析出来）；
- `structure_tokens` 来自真值记录里的结构串长度。

`[实测]` 交叉验证：由 token 流重建的单元格总数 = **161,925**，与
`tools/table_detokenize.py` 的 `cells=161925` 逐位吻合；平均单元格数
22.79349662162162 × 7,104 = 161,925。

---

## 4 报告 schema 与域判别

### 4.1 域判别规则

`rapidstruct evaluate --manifest <path>` 按 manifest 的形状判别域：

| manifest 形状 | 域 | 行为 |
| --- | --- | --- |
| **裸 JSON 数组** | OCR | 现状，**一个字节都不改** |
| 对象且 `"domain": "table"` | 表格 | 走 `evaluation::table::run` |
| 其它 | OCR | 尝试按 `Vec<EvaluationCase>` 解析，失败则报错 |

**决定不给 OCR 报告加 `"domain": "ocr"`**：那会改动一个已经发布的 HTTP 契约
（serve 的 `POST /api/evaluate`）而换不到实质好处。

表格报告自带 `"domain": "table"`，消费者不必靠"有没有 `cases` 字段"去猜自己拿到的
是哪一种报告。

### 4.2 表格域 manifest

```json
{
  "domain": "table",
  "dataset": "datasets/table/tabrecset",
  "model": "models/table/slanet-plus.onnx",
  "predictions": "Temp/table-pred-val.json",
  "limit": 0,
  "side_len": 488,
  "fix_input_shape": true,
  "intra_threads": 14,
  "fail_fast": false
}
```

除 `domain` 外全部可选，未给出的字段沿用 CLI 默认值（`TableEvalConfig::default`）。

**路径相对当前工作目录解析**，与同名 CLI 参数一致。OCR manifest 是把 `case.image`
相对 manifest 所在目录解析的，这里**刻意不照抄**：本 manifest 指向的是仓库里的数据集与
模型（不是与 manifest 同级的样本文件），如果相对 manifest 目录解析，同一份 manifest
换一个存放位置就指向不同的数据集 —— 那是纯粹的意外。换句话说，这份 manifest 是
"被存下来的 CLI 参数"，参数是 CWD 相对的，它就也是。

### 4.3 报告顶层键

| 键 | 含义 |
| --- | --- |
| `domain` | 恒为 `"table"` |
| `tool` | `"table_eval"` |
| `metric_scope` | 报了什么、没报什么、以及"不可与官方 SLANet TEDS 对比"的警告 |
| `protocol` | 本文件三节的机器可读副本（ε / 网格规则 / 上界策略） |
| `model` | 路径、字节数、sha256、输入输出名、词表大小、`sos`/`eos` 下标 |
| `config` | 输入边长、是否固定形状、会话实际输入维度、线程数 |
| `dataset` | 根目录、manifest 与 sha256、样本集与内容 sha256、加载/选中/可解码/失败计数 |
| `span_limits` | 从词表解析出的 `rowspan`/`colspan` 上限 |
| `span_exceedance` | 超限表数、占比、超限单元格数、实测最大跨度、样例 |
| `upper_bound` | 两类构造性上界的计数、占比、最大 token 数、策略说明、样例 |
| `structure_confidence` | **E13 新增**：结构可信档的分布（`high` / `low`）、降级原因计数（`degradations.grid_holes` / `degradations.truncated`）、空洞总数 `holes_total`，以及三条说明字段（`scoring` / `why_grid_holes` / `why_truncated`） |
| `timing` | 逐样本墙钟（preprocess / infer / decode / total / cells），**不是 timing ledger** |
| `failures` | 逐条失败（文件名、图像路径、错误消息） |
| `failure_kinds` | 失败消息 → 次数 |
| `outputs.predictions` | 预测文件路径、字节数、sha256 |
| `content_source` | 单元格文本来源（`structure_only` / `page_reuse` / `crop_control`，§5.2.1） |
| `ocr_resolution` | 表格路径自己的 det 分辨率（`limit_side_len` / `limit_type` / `max_side_len`）、**`live_knob`**（哪一只旋钮真正生效）与它作用于哪条路径 |
| `page_ocr` | 页级 OCR 的引擎配置路径、**引擎指纹**、缓存命中/未命中、`ocr_ms` 分位数、见过的区域数；非 `page-reuse` 时为 `null` |
| `backfill` | 回填的匹配拓扑计数、`non_empty_rate`、文字置信度、`recognizer_calls`；非 `page-reuse` 时为 `null` |
| `crop_control` | 对照口径的识别器调用次数与 `recognize_ms`；没跑对照口径时为 `null` |
| `peak_working_set_bytes` / `memory_source` / `ort_runtime` / `ort_runtime_version` | 域无关运行时字段 |

`crop_control` 为 `null` 表示**没跑**对照口径，与"跑了但一次都没调识别器"是两件事，
所以不能用 `recognizer_calls == 0` 代替它。

`ocr_resolution` 里的 `live_knob` 不是装饰：`limit_type = "min"` 时生效的是 `limit_side_len`
（det 把短边放大到它，**永不缩小**），`limit_type = "max"` 时生效的是 `max_side_len`
（前置缩放给长边一个硬上界，**永不放大**），而 **`limit_side_len` 在 `"max"` 下被
`src/ocr/det/detector.rs:210-224` 完全忽略**（它改用 `max_wh` 派生的 960/1500/2000 阶梯，
这是复刻 PaddleOCR 的行为）。报告里点明哪只旋钮是活的，是为了让读基线的人不至于
"改一个死字段然后解释不了为什么数字没动"。

最后四个字段由 `src/evaluation/runtime_report.rs` 的 `attach_runtime_fields` 统一附加。
它**不**复用 `src/bin/rapidstruct.rs` 的 `evaluation_report_value(&EvaluationSummary)` ——
后者的签名绑死 OCR 域汇总类型，字面复用必然要把 TEDS 塞进 OCR 汇总，正好违反
`src/evaluation/mod.rs` 的"三类指标不得混入默认汇总"。而两份报告里**确实共享的**
只有那四个字段，它们的存在理由是"没有它就无法证明两份评估跑在同一份运行库上"。
这是对 doc 09 §9.1 ⑤ 的**有意偏离**。

`structure_confidence` 是**并列的独立段，不是扣除**：`low` 样本仍然计分并留在 `result` 里。
它的用处是让"0.583404 这个数字由什么样的样本组成"可查——`high 6734 / low 350`
（4.94 %，全部来自网格空洞；`truncated` 为 0）与"全部 high"是同一个均值下的两种不同现实。
**不要**把它当成"应该剔除的坏样本"：剔除会抬高分数并掩盖退化。它也不改变本文件 §3 的
上界扣除策略——两者统计的是不同对象（`upper_bound` 数的是"真值无法被词表表达"，
`structure_confidence` 数的是"模型实际没能铺满网格"）。

### 4.4 失败样本写空串，不删除

官方 `metric.py` 的 `evaluate` 对空预测返回 0.0，因此失败会**如实拉低**总分。
把失败样本从预测文件里删掉会让 `batch_evaluate` 拿不到键，从而把"模型答错"变成
"样本不存在"。

`[实测]` E7 全量跑有 20 条解码失败，全部是**模型的系统性结构错误**（漏发闭合标签），
不是解码器缺陷：

| 失败消息 | 次数 |
| --- | --- |
| `table structure ends with 1 unclosed tag(s)` | 12 |
| `table structure ends with 2 unclosed tag(s)` | 5 |
| `table structure closes \`</tbody>\` without a matching open tag` | 2 |
| `table structure has a fragment \` colspan="18"\` … the \`<td\` … is missing` | 1 |

`[推算]` 代价已量出：只压低 **0.002**。但这条规则是**口径**问题，不是量级问题 ——
0.002 也好 0.2 也好，删除失败样本都会让分数不再可解释。

---

## 5 命令

### 5.1 跑一遍并评分

```powershell
# 1. 推理 + 落预测与报告（全量约 4.5 分钟）
cargo run --release --bin table_eval -- `
    --predictions Temp/table-pred-val.json `
    --report Temp/table-report-val.json

# 2. 评分（官方 metric.py，结构口径）
python tools/table_teds.py `
    --true datasets/table/tabrecset/derived/pubtabnet_val_gt_html.json `
    --pred Temp/table-pred-val.json `
    --structure-only --out Temp/table-teds.json
```

### 5.2 走 `rapidstruct evaluate`（§9.2 验收路径）

```powershell
cargo run --release --bin rapidstruct -- evaluate `
    --manifest Temp/table-eval-manifest.json `
    --output Temp/table-report-val.json
```

报告落在 `--output`，同时打到 stdout。表格域**不接受** `--config`（页级 OCR 引擎配置）
与 `--iou-threshold`（多边形 IoU 阈值）：表格域不跑页级 OCR，也没有 IoU 指标，
给了它们说明调用方拿错了 manifest，因此明确报错而不是静默忽略。

### 5.2.1 三种单元格文本来源（`content` 字段）

`TEDS-Struct` 与 `TEDS` 是两个**不可互换**的数字，差别只在单元格文本从哪来。
manifest 的 `content` 字段（或 `table_eval` 的 `--content`）选择来源；未知取值**报错**
而不是回落到默认值。

| `content` | 报告里的 `reported_metric` | 单元格文本从哪来 | 识别器调用次数 |
| --- | --- | --- | --- |
| `structure-only`（默认） | `TEDS-Struct` | 全部空串 | 0 |
| `page-reuse` | `TEDS` | 整页 OCR 结果 + 几何匹配（§10 的 R1 路线） | **0**（门禁 G2） |
| `crop-control` | `TEDS` | 逐单元格裁剪喂识别器 | 每格 1 次 |

`crop-control` 是**对照基线**，不是生产路径。它存在的唯一理由是证明复用路线没有
比"朴素做法"更差。它的 `recognizer_calls` 非零是设计意图，不是门禁 G2 失败 ——
报告里的 `recognizer_calls_note` 会随 `content` 切换判据方向。

`page-reuse` 需要页级 OCR 引擎配置（`ocr_config`）与可选的页级 OCR 结果缓存
（`ocr_cache`）。缓存键是**样本相对路径**而不是绝对路径：换机器、换盘符不该让缓存
全部失效。页级 OCR 与表格模型无关，换结构模型/改回填规则/调结构分辨率都不该让它
重跑一遍（全量是小时级成本），所以缓存是这条路线能反复迭代的前提。

**两种口径各有一份入库基线**，不可互相替代：

| 口径 | 基线文件 | 全量 7,104 的值 | 产出于 |
| --- | --- | --- | --- |
| `structure-only` | `tests/baseline/windows-baseline/table-teds-cpu.json` | `TEDS-Struct = 0.706518`（decodable-only 0.708513） | E7/E8 |
| `page-reuse` | `tests/baseline/windows-baseline/table-teds-full-cpu.json` | `TEDS = 0.583404`（decodable-only 0.585052） | E10，**E13 重建**（19,797 B） |

> ⚠️ **这两个文件里的数字都测于 E25 之前。** E25 改了单元格行归属（`src/table/cell_content_backfill.rs::assign_rows_from_text`），全量预测从 3,744,372 B 变成 3,744,387 B、`page-reuse` 从 `0.583404` 变成 `0.583640`（`+0.00024`）。按 `docs/11` §4.12 的裁定**不重建**这两个入库文件，但引用时必须连着"测于 E25 之前"一起读；将来要一次新的精度主张，先按本文件 §4.3 重建再引。

> **完整口径的基线数字在 E10 变过。** E9 首次建立它时默认分辨率是 `736/"min"/2000`，值 `0.570710`；
> E10 把默认改成 `736/"max"/736` 后同一条命令得到 `0.583404`（+0.012694），同时 `page_ocr.ocr_ms`
> p50 从 549.159 降到 **108.907–112.736**（两次独立冷跑的极差 3.5 %，即 **4.87–5.04×**；入库基线里
> 记的是 112.736）。**这不是回归，是有意的口径变更** —— 所以基线文件里必须
> 自带 `ocr_resolution` 段（含 `live_knob`），否则读者会把两个数字当成同一口径的前后对比。
> 详细判定依据见 `docs/09-table-recognition-execution.md` §11.4.4。

两份基线都用 `tools/build_table_teds_baseline.py` 从**运行报告**推导，不是手抄。工具只在拿到
**≥2 份**报告时才敢写 `identical_prediction_digests: true` 与 `runs_agree_within_epsilon: true`；
单份运行时这两个字段是 `null`，因为一次运行的"逐字节相同"是空洞真命题（单元素集合必然
只有一个元素）。结构口径由 **4 轮**建立。完整口径由 **3 轮**建立 —— 三份预测产物摘要同为
`df6bd2c0e17bdba5f17bfa74f3d9146c30f30a3ec98f2feae53f38c727d840d2`（其中一轮是冷缓存跑，
因为热缓存报告的 `page_ocr.ocr_ms` 只有 `{count: 0}`、`regions_seen` 为 0，而那两段是验收要用的
证据）。

> 完整口径的摘要**变过两次**，两次都不是非确定性：
> - E10：`100d327f206fce0cdddddbe552a2ecd0344dc31e878008f7983ea95d4823c6a8` → `1ab736e7…`，
>   原因是默认分辨率变更（`736/"min"/2000` → `736/"max"/736`，见上）；
> - E13：`1ab736e7…` → `df6bd2c0…`，原因是**发射器重写**（属性从单元格表重建、固定 `colspan`
>   在前，并去掉一个模型 token 流里重复出现的属性）。这一次 `result.mean` 与**逐样本 TEDS
>   全部逐位不变**（7,104 个样本里只有 9 个字节不同，且 9 个全部语义相同）。详细归因见
>   `docs/09-table-recognition-execution.md` §14.3.3。
>
> 所以"预测摘要变了"**不等于**"指标变了"，也**不等于**"不确定"。判据是：同一份配置的多次跑
> 是否逐字节相同（`determinism.prediction_sha256` 集合大小为 1），以及指标字段是否移动。

完整口径的基线还自带 `content_source`、`page_ocr`（含产出文本的配置路径）与 `backfill`
（含 `recognizer_calls`，即门禁 G2 的证据）三段 —— 换一份引擎配置就换一个指标口径，
基线必须能自证这一点，而不是让读者去找那次运行的日志。

**页级 OCR 缓存必须带引擎指纹。** 缓存条目的键是样本相对路径，但文件头还记着产出它的
**引擎指纹**（`sha256` of the resolved `EngineConfig` 的规范化 JSON，含 `global.max_side_len`、
`det.limit_side_len`/`limit_type`、模型路径与 `runtime` 段）。指纹不符时 `OcrCache::load`
**报错拒绝复用**，而不是拿旧文本继续算 —— 否则改分辨率或换 provider 之后，报告里的
`page_ocr.engine` 会记着**这一次**的配置，而单元格文本其实来自另一次，读者无从察觉。
指纹取整份配置而不是配置文件路径：同一条路径的内容可以变；也不是只取模型路径：那会漏掉
分辨率与 provider（provider 会改变浮点累加顺序）。代价是换线程数或 provider 会让缓存
失效一次 —— 这是**有意的**，因为 `docs/10` §1.2 的逐字节确定性本来就只在固定线程与
provider 下取得。做分辨率对照实验时，每个档位给一个自己的 `--ocr-cache` 文件。

### 5.3 确认 ε

```powershell
# 走 table_eval（默认）
pwsh -NoProfile -File tools/run_table_determinism.ps1 -Runs 3
# 走 rapidstruct evaluate（§9.2 的验收路径）
pwsh -NoProfile -File tools/run_table_determinism.ps1 -Runs 3 -Manifest Temp\e8-manifest.json
```

各轮的预测文件 sha256 必须相同。不同即**缺陷**（见 §1.3），不是噪声。脚本从**报告**里读
`outputs.predictions.sha256` 与 `protocol.repeat_epsilon`（不自己复述协议），不一致时打印
`With epsilon = 0 that is a DEFECT, not noise; see docs/10-table-evaluation-protocol.md section 1.3`。

> `-Manifest` 驱动下预测路径由 manifest 固定，每轮覆盖同一个文件；证据是**每轮报告里记录的那一次运行的摘要**，不是磁盘上最后留下的文件。

### 5.4 指标分辨率与消融标尺

```powershell
python tools/table_teds.py --resolution
python tools/table_teds.py --ablate --limit 2000
```

`--ablate` 给出"如果预测只做到某一步"的分数上限，用来判断一个分数是"模型差"还是
"指标太敏感"。`[实测]` 2,000 样本子集上的标尺：

| 消融 | TEDS-Struct |
| --- | --- |
| 恒等（预测 = 真值） | 1.000000 |
| 去掉全部 `colspan` / `rowspan` | 0.974318 |
| 去掉 `<thead>` / `<tbody>` | 0.900738 |
| 平凡单空格 | 0.121960 |
| 空预测 | 0.000000 |

`[推算]` 指标不过敏（去掉全部跨度属性只掉 0.026），因此 E7 的 0.706518 反映的是
**真实的模型差距**，不是指标惩罚。

---

## 6 未确认项与登记

1. `[未测]` **按样本类型分层的指标分辨率**未实现（§1.4）。当前只报数据集级与
   逐样本级。
2. `[未测]` `LOOP_TRIP_CAP = 500` 来自对 ONNX 图常量的读取（Python 侧），
   Rust 侧无法独立复核；自曝机制（§3.3）是缓解手段，不是证明。
3. `[实测]` 已关闭：完整 TEDS（含单元格文本）已产出（§5.2.1 的 `page-reuse` 口径，
   基线 `tests/baseline/windows-baseline/table-teds-full-cpu.json`，全量 7,104 = 0.583404；
   **该文件测于 E25 之前**，E25 之后同一口径是 `0.583640`，见 §5.2.1 的提示）。
   它走"整页 det 复用 + 几何匹配"，不需要逐单元格调识别器（`backfill.recognizer_calls = 0`）。
   **仍然不可**与 PubTabNet 论文里 SLANet 的 TEDS 直接对比——数据集、切分与"用真值文本还是
   识别文本"都不同，理由写在基线文件的 `metric.comparability` 段里。
4. `[未测]` ε = 0 是在**本机 CPU provider** 上验证的。DirectML / CUDA 路径下
   是否同样逐位可复现未验证（E12 的范围）。协议目前只对 CPU 路径成立。
5. `[实测]` 已关闭：任务清单里的"铺满率 82.8 %"是 LabelMe 在 400 张子集上的性质，
   不是 token 流的性质（§2.4）。
6. `[实测]` 已关闭：`colspan` 上限 20 与实测最大值 24 的冲突只影响 3 张表（§3.2）。
7. `[未测]` **`structure_confidence` 的档位没有独立的分辨率标尺**（E13 新增）。
   `high` / `low` 是二值判据（网格是否铺满），不是阈值化的连续量，所以没有"ε 附近抖动"
   的问题；但"低置信样本的 TEDS 分布是否显著差于高置信样本"**没有实测过**——若两者分布
   接近，这个档位就只是诊断标签而不是质量信号。
8. `[未测]` **`truncated` 降级在全量 7,104 上从未触发**（`degradations.truncated = 0`），
   所以这条上界的扣除逻辑**只有单测覆盖，没有真实样本**（E13 §14.3.7 第 2 条）。
9. `[未测]` **`TextPolicy::Escape` 没有被真实 OCR 文本端到端验证过**：表格尚未接进页面
   管线（`src/ocr/pipeline/rapid_structure.rs` 把 `table_structure_infer_ms` 写死 `0.0`），所以
   评测口径固定 `Raw`，"用户可见输出用 `Escape`"目前是接口而非在跑的行为。
