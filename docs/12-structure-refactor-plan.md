# 12 三种识别的结构审计与重构方案（评审稿 v2 · 已按评审意见修订 · 未实施）

> 本文是**评审稿**：只描述现状证据、目标设计与验证办法，**不含任何代码改动**。
> 每一节都可以单独批准或否决，批准后按 §6 的批次顺序实施。
>
> **v2 说明**：v1 交付后收到逐项评审（`m01562`），评审指出 9 处目标设计与真实语义不一致（其中 7 项要求重写）。
> v2 已按评审重写，并把评审确认的裁决写入 §7。评审提到的行号我已逐条在源码中复验，作为 `[实测]` 记录在 §10。
>
> 读者：决定做不做、按什么顺序做的人（含未来的我）。写法沿用 `docs/11`：结论 → 证据 → 设计 → 验证 → 风险，
> 每条结论标注证据等级（§0.2），并在实施时把「变更摘要 / 命令 / 关键结果 / 基线对比」追加回本文（§9）。
>
> 本文审计的对象是**三种识别（文本 / 表格 / 公式）在引擎与服务里的结构**，不是精度，也不是性能优化清单。
> 性能数字只在三处出现：P0-4（先测量再决定）、P1-5/P2-3（解码次数的像素对拍）、P1-7（payload 体积与序列化耗时）。

---

## 0 前提与约定

### 0.1 这次的触发点

`m01287` 的原问题：三种识别的架构是否存在重复、耦合。

审计结论（详见 §1–§3）：**域之间没有耦合，重复集中在三个跨域中枢与若干平行实现上；编排本身是嵌套委托，不是三份实现。**
真正的问题不是"代码重复"，而是：

1. **页面级组合缺少类型** —— "哪些阶段参与、按什么顺序、在哪个坐标空间"目前是 `OcrRequest` 上的并列字段加硬编码嵌套，
   互斥约束写在两个 `if` 里，正交性靠"就地改写请求、关掉自己"来维持。这是新增第四种识别要改约 14 处的根因（§2.5）。
2. **共享层存在，但被旁路** —— `src/evaluation/stats.rs` 的模块文档自己写着"避免每个 bin 各写一套 percentile"，
   而 `src/evaluation/table/run.rs:1526` 与 `src/bin/bench_warm_e2e.rs:252` 各自又写了一份，且**分位定义不同**（§2.4 L-1）。
3. **模型完整性校验缺少统一凭证** —— 校验能力已经存在且表格/公式都在用，但它分散在"服务启动准入 / 域构造器 /
   底层会话构造器"三层之间，**强度由路径决定而不是由"调用方是否持有可信摘要"决定**（§2.1 I-4/I-5，P0-3 重写）。
4. **已有登记的死代码** —— `src/output/markdown.rs:1-6` 自己写明"E15 试过删、被 `-D warnings` 挡回、待统一清理"（§2.4 L-2）。

### 0.2 证据分级（沿用 `docs/11` §0.2，不混用）

| 标记 | 含义 |
| --- | --- |
| `[实测]` | 本机跑出来的数字 / 我读过原文的代码事实（给出 `文件:行号`） |
| `[推算]` | 由已实测的事实按明确规则推出（写出规则） |
| `[未测]` | 尚未测量；只出现在风险与待办里 |

性能绝对数字只允许来自 `src/bin/bench_warm_e2e.rs`（页面级）、`tools/run_windows_baseline.ps1`（12 图基准）、
`src/bin/table_eval.rs`（表格域）——沿用 `docs/11` §0.2 的规定。

### 0.3 三组证据来源，本文分开标注

四份只读审计（子代理，仅读取、未改任何文件）给出的条目**不逐条复验**，一律标 `[审计]`；
我自己读过原文的标 `[实测]`。**凡本文给出的行号，`[实测]` 的可以直接引用；`[审计]` 的在动手前需再核一次。**

- 审计面 A：共享基础设施（`runtime/`、`vision/`、`model_*`）—— 结论见 §1、§2.3。
- 审计面 B：文本 vs 表格（几何、预处理、解码、字典、归属）—— 见 §2.2、§2.3。
- 审计面 C：公式路径与编排层 —— 见 §2.5。
- 审计面 D：输出与评估层（serve 结果层、评测、统计、错误分层）—— 见 §2.2、§2.4、§2.6、§2.7。

### 0.4 本文的验收标准（沿用 `AGENTS.md` §8）

一次改动只有同时满足才叫完成：根因解决 / 需求正确 / 既有功能未被意外破坏 / 相关验证通过 /
没有为兼容旧实现引入复杂度 / 没有留下过渡代码 / 没有保留无效实现 / 相比改前在正确性·性能·架构·简洁性·可维护性上更合理。

**因此每一批结束时必须给出 §8 的门禁输出，而不是"代码写完了"。**

### 0.5 v2 修订记录（评审 `m01562` 的逐项处置）

| 评审点 | 评审结论 | v2 处置 |
| --- | --- | --- |
| 一.1 `Stats` 目标结构（`quantiles: Vec<(&'static str, f64)>`）不可采用 | 会改变公式评测报告结构；只保护了表格与页面基准的键名 | **重写 P0-2**：`Stats` 保留稳定基础字段，`percentile` 是唯一算法，分位集合由报告层适配器声明，不进入 `Stats` 的序列化结构 |
| 一.2 P0-4 不能用页面 warm benchmark 判定双开 | 探针会话还承担契约 / metadata / 词表 / 动态维度名；`bench_warm_e2e` 测的是请求路径 | **重写 P0-4**：改测 `TableStructureSession::open` 冷启动、引擎构造期会话建立耗时与次数、`fix_input_shape` 两态 |
| 一.3 P0-3 审计结论需修正 | 公式 `from_model_with_hash` 已校验、`table::registration` 不是重复封装 | **重写 P0-3**：根因改为"缺少统一模型凭证"，引入 `VerifiedModel` |
| 一.4 P2-3 与 tile 约束矛盾 | tile 子请求明确关闭公式/表格；统一调度器会同时改解码次数、坐标语义与区域合并 | **P2-3 暂缓**：只做 `MaterializedInput`，保持 tile 互斥；未来设计需先覆盖跨 tile 合并/去重/坐标回映/计时 |
| 二.5 几何层不应一次性合并全部 IoU | 5 份实现语义不同（凸四边形裁剪 / 轴对齐 / 回填 / 类别感知 NMS；且 `>=` 与 `>` 并存） | **重写 P1-1**：只抽 `aabb_from_quad` / `aabb_iou` / `polygon_area`，逐处数值对拍 |
| 二.6 `vision::normalize` 类型与语义有误 | 当前 LUT 是 `[f32; 256]` 不是 `[i8; 256]`；det/rec/cls/table 差异不止归一化公式 | **重写 P1-3**：只合并 rec/cls 的 signed-unit LUT |
| 二.7 PageLine/TextLine 合并需先确认坐标契约 | `PageLine` 存四边形、`TextLine` 存裁剪后 bbox；应放共享几何/表格基础层 | **重写 P1-4**：先显式转换 `PageTextLine::from_region`，字段/裁剪/精度对拍后再删旧结构 |
| 二.8 P1-7 不是单纯重复实现 | `regions` / `items` / `tables` 在协议上用途不同 | **重写 P1-7**：先测 payload 体积与序列化耗时，收益不明显则取消；若改，作为独立破坏性输出协议变更 |
| 二.9 白名单统一不能只靠 Rust 常量 | Rust `const` 不会进入内嵌 JS；两边差异不止 span | **重写 P1-6**：白名单规范文件 + 生成脚本 + fingerprint 测试；span ≤ 64（Q2） |
| 三 页面编排方向对但结构过度抽象 | 只有一个 `CoordinatePremise::OriginalPage`；建议 `ValidatedRequest` + `PageRoute` + 一次 `plan()` | **重写 P2-1/P2-2**，删除 `PageStage`/`CoordinatePremise`，补调度器必须记录的 4 条语义 |
| 四 基线与验收门禁需修正 | `0.583640…` 来自未入库 `Temp/`；入库基线仍是 `0.583404…`；`-DryRunKeys` 不存在 | **重写 §8**：区分"入库契约资产"与"本机质量门禁"，删除杜撰的命令，新增 P0-0 基线契约批 |
| 五 实施顺序 | 先契约 → 死代码 → 分位 → 模型凭证 → PageRoute → 输入物化 → 最后才评估几何/归一化/白名单/JSON | **重写 §6** |
| 结论的批准范围 | 批准 P0-1、P1-8、修订后的 P0-2；重写 P0-3/1-1/1-3/1-5/1-6/1-7；P2 只保留 `ValidatedRequest` + `PageRoute` | 已按此重排，并在每项标注 `状态` |
| 六 裁决 | Q1–Q6 逐条回答 | 见 §7.1（已裁决） |
| 七 遗留 N1–N3 | v2 在 §7.2 留了三处实现细节待确认 | **用户 `m01664` 采纳建议**：§7.2 已改为定案表，正文同步 —— `percentile_method: "linear"`、`assets/table_html_whitelist.json` + `tools/generate-table-whitelist.mjs`、`VerifiedModel` 落 `src/model_verify.rs` |

### 0.6 本轮不做的事（评审明确要求）

- **不改任何代码**：v2 仍是一份文档。
- **不把未入库的实验产物写成硬门禁**（`datasets/`、`Temp/`、本文自身都还不是跟踪状态 → §8.2）。
- **不为对称而重构**：评审缩小了 5 项（P1-1/P1-3/P1-4/P1-6/P1-7）的野心，本方案不把它们的原目标改头换面塞回去。

---

## 1 现状：已经统一、明确不要动的部分

这些是本项目过去 25 个执行阶段真正做对的地方，重构不得破坏它们。

| 层面 | 单一事实源 | 证据 |
| --- | --- | --- |
| ONNX 会话创建 | 全 crate 唯一 builder、唯一 commit | `[实测]` `src/runtime/session.rs:508`（builder）、`:537`（`commit_from_file`）；`Commit::from_array` 全仓 0 命中 |
| 对外输出契约 | 一个 `OcrRegion` 承载三态，一个总分派点 | `[实测]` `src/api.rs:827`、`src/output/json.rs:114 to_output_items` |
| 模型角色与档位 | 8 变体 enum + 单分组函数 | `[实测]` `src/model_set.rs:52`、`src/bin/serve/model_plan.rs:92` |
| 错误类型 | 库内无第二套枚举 | `[实测]` `src/error.rs:8`（`RapidOcrError`，14 变体） |
| 域依赖方向 | 单向、无循环 | `[实测]` `src/table/**` 生产代码提 `crate::ocr::` 0 次（4 处全在 `#[cfg(test)]`） |
| 编排骨架 | 嵌套委托，不是三份实现 | `[实测]` `src/ocr/pipeline/rapid_structure.rs:616` 分派 → `:1058`（公式）→ `:638`（文本底座） |
| 公式/表格的模型会话设置 | 只来自引擎运行时档案与策略，两条路径都不得自建 | `[实测]` `rapid_structure.rs:1288`、`:1329` 都取 `runtime_profile().session_runtime()` |
| 服务端路由开闭判据 | 缺模型就明确拒绝，而不是静默无效 | `[实测]` `src/bin/serve/server.rs:175-196`；`src/bin/serve/model_plan.rs:646`（"不是第二套机制"） |
| 模型登记与信任来源 | 表格有清单推导出的可信摘要 | `[实测]` `src/table/registration.rs:144-156`（读 `manifest.json` → `verify_sha256`）；14 条测试见 `docs/09-table-recognition-execution.md:1609` |

---

## 2 审计发现：重复与耦合（按根因类别，不是按文件）

### 2.1 接口设计

| 编号 | 现象 | 证据 | 影响 |
| --- | --- | --- | --- |
| I-1 | 页面级组合只有并列字段；`roi`/`tile` 与公式/表格的禁令**逐字两份**，理由却只写在两份之间的注释里 | `[实测]` `src/api.rs:299-313`（字段）、`:319-332`（公式禁令）、`:333-338`（注释：同一条坐标前提）、`:339-352`（表格禁令） | 加第四种识别时禁令要写第三份；"为什么"与"什么"分开存放，改一处易漏另一处 |
| I-2 | `FormulaPolicy` 与 `TablePolicy` 的校验逐字同构（`enabled` 短路、`model_path` 必填、阈值区间、`max_regions > 0`） | `[实测]` `src/api.rs:682-709` vs `:793-824` | 两条策略的边界检查逻辑重复；已有测试分别钉住两份 |
| I-3 | 同一条件有**两条不同文案**：`api.rs:689` "formula policy requires \`model_path\` when enabled" vs `rapid_structure.rs:1286` "formula policy requires \`model_path\`" | `[实测]` 两处原文 | 调用方无法按文案稳定判据；将来加 i18n 或错误码要改两处 |
| I-4 | 模型完整性校验的**触发点分散在三层**：表格在会话构造前强制（`verify_registered_model`）；公式的 `*_with_hash` 构造器已校验，但默认构造器传 `None`；文本 `OcrSession::new` 根本不接收摘要，只在服务准入/下载层校验 | `[实测]` `src/table/structure.rs:347`、`src/formula/recognizer.rs:87`/`:90-101`、`src/formula/detect.rs:108`/`:117-125`、`src/ocr/session.rs:40-50`（无摘要参数，`open_unchecked`）、`src/model_verify.rs:405`（`None` 即放行） | **v1 把这一条写成"公式完全没有校验"是错的**（评审一.3）。真实根因：缺少统一凭证类型，于是强度由路径决定 |
| I-5 | `table::registration::verify_registered_model` 与库级 `model_verify::verify_sha256` 并存 | `[实测]` `src/table/registration.rs:144-156` | **不是历史叠加**（评审一.3）：`registration` 负责"从清单推导可信期望摘要"，是信任来源；保留分层，只统一凭证类型 |
| I-6 | 净化白名单两套且不一致：库允许 20+ 标签与 `span ≤ 1000`，页面只允许 9 个标签与 `span ≤ 64` | `[审计]` `src/output/html.rs:350-354`/`:445-447`/`:386` vs `src/bin/web/index.html:3055-3068`/`:3099-3104` | 同一份 `table_html` 经两条净化得到不同结果（页面更严） |

### 2.2 数据结构

| 编号 | 现象 | 证据 | 影响 |
| --- | --- | --- | --- |
| D-1 | 几何工具无共享层。但"5 份 IoU"**不是 5 份同一个函数**：接口语义与边界条件不同（见 §4.2 P1-1 的语义表） | `[实测]` 行号见 §4.2 P1-1 | **v1 把它们当重复是过度概括**（评审二.5）；真正同形的是 polygon→bbox（8 份）与 polygon 面积（2 份） |
| D-2 | 同一页文本行有两个错开的结构：`PageLine`（存四边形）与 `TextLine`（存裁剪后的轴对齐 bbox），由 `rapid_structure.rs:1639` 同时构造 | `[审计]` `src/table/detection.rs:78`、`src/table/cell_content_backfill.rs:81`、`src/ocr/pipeline/rapid_structure.rs:1639`（`:1628-1631` 自认是防分叉的刻意设计） | 字段变更要改两处 + 转换点；E25 的行带逻辑只能看到一个视图 |
| D-3 | `Stats` 把分位硬编码成 `p50_ms`/`p95_ms` 两个字段（无 p90），且**被报告直接序列化** | `[实测]` `src/evaluation/stats.rs:10-18`；序列化点 `src/bin/formula_eval.rs:199-201`/`:687`、`src/bin/formula_bench.rs:73-76`/`:102-105`、`src/table/structure.rs:1278-1282`（打印） | 报告要 p90 只能另写一份统计（L-1）；同时删字段会破坏公式评测报告结构（评审一.1） |
| D-4 | 输入图像有两个表示：`RecImage`（BGR）与 `formula/*` 的 `DynamicImage`（RGB），靠 `formula/preprocess.rs:158` 桥接 | `[审计]` `src/config.rs:283`、`src/formula/preprocess.rs:55`/`:158`、`src/input/image_loader.rs:76`/`:117` | 两条加载出口、两条通道序约定；**影响面最大，本文不提议在 P0/P1/P2 内合并**（见 §5） |

### 2.3 模块边界

| 编号 | 现象 | 证据 | 影响 |
| --- | --- | --- | --- |
| M-1 | HTML 转义三份（库出口、表格发射器、网页 JS），其中表格发射器那份与库出口同形、同在 `src/` 内 | `[实测]` `src/output/html.rs:326`/`:339`、`src/table/emitter.rs:383`/`:399`、`src/bin/web/index.html:2316` | crate 内无理由两份；网页那份是有理由的（纵深防御） |
| M-2 | 输出遍历骨架已统一（HTML/Markdown 都复用 `to_output_items`），但表格 HTML 在同一个 payload 里由三种视图承载（`regions` 内 / `items` / `tables`） | `[实测]` `src/output/json.rs:174-227` | **评审二.8：它们不是重复实现**，而是"事实源 / 统一消费视图 / 表格专用视图"三种协议用途 → 先测量再决定（P1-7 重写） |
| M-3 | 计时原语不导出：`runtime/timing.rs:257 from_timings` 只认 `OcrTimings`；`Instant::now()` 全仓 70 处；`DetTimingBreakdown` 与 `api::StageTiming` 字段相同却是两个类型 | `[审计]` `src/runtime/timing.rs:257`、`src/ocr/det/detector.rs:77`、`src/api.rs:896` | 公式/表格独立评测无法复用守恒校验（`bench_warm_e2e.rs:572 assert_split_identity`） |
| M-4 | fixture 基建三份：`src/test_support.rs` 只被 15 个文件用；`src/bin/serve/*` 零复用、自建假模型与 manifest | `[审计]` `src/test_support.rs`（579 行）、`src/bin/serve/tests.rs:690-885` | 新测试要么复用一个不足的公共层，要么再抄一份 |
| M-5 | 评测入口不对称：`src/bin/table_eval.rs`（150 行薄壳，跑库里同一份 `run_table_evaluation`）vs `src/bin/formula_eval.rs`（1636 行，报告结构全在 bin 内） | `[审计]` `src/bin/table_eval.rs:1-6`、`src/bin/formula_eval.rs` | 公式评测的骨架不可被会话内复用；同类能力重复实现 |

### 2.4 局部实现错误（已登记但未清）

| 编号 | 现象 | 证据 | 影响 |
| --- | --- | --- | --- |
| L-1 | 两种分位定义并存：线性插值 vs 最近秩。同一输入 `[1,2,3,4]` 的 p50 分别是 **2.5** 与 **3.0** | `[实测]` `src/evaluation/stats.rs:50-57` 与 `src/evaluation/table/run.rs:1532-1535`；第三份在 `src/bin/bench_warm_e2e.rs:259`（`p50: s[(len-1)/2]`，中位而非插值，且有 p90） | 同一项目里"p50"有两个意思；`table/run.rs:1582` 的测试只验自洽、**未与共享层对拍** |
| L-2 | 死代码已登记未清：`to_markdown` / `to_markdown_texts` / `center_y` / `to_json_items` / `to_formula_{json,markdown,html}` 生产零调用 | `[实测]` `src/output/markdown.rs:1-6`（自述"E15 试过删、被 `-D warnings` 挡回、待统一清理"）、`:17`/`:34`/`:98`；`src/output/json.rs:53`（仅本文件测试 `:234`/`:246`）；`src/formula/output.rs:26`/`:71`/`:75`（仅本文件测试）、`src/exports.rs:42` 重导出 | `AGENTS.md` §7 明令"不得留下无效旧实现"；模块级 `#![allow(dead_code)]` 会让新死代码静默存活 |

### 2.5 架构设计 / 调用流程

| 编号 | 现象 | 证据 | 影响 |
| --- | --- | --- | --- |
| A-1 | 阶段顺序是硬编码嵌套：`recognize` 里表优先（有注释给理由）→ 公式 → 文本；`recognize_with_table` 内部再选"先公式还是直接文本" | `[实测]` `rapid_structure.rs:616-630`（含 `:618-620` 理由）、`:1378-1383` | 顺序与理由分散；加第四种识别要重写这棵树 |
| A-2 | 正交性靠**就地改写请求**维持：公式路径把 `request.formula` 置默认、表格路径把 `text_request.table` 置默认，以防递归 | `[实测]` `rapid_structure.rs:1091`、`:1377` | 这是 A-1 的补丁形态：一个函数能跑哪几个阶段，取决于调用者恰好改掉了哪个字段 |
| A-3 | `recognize_tiled_request`（`:1856`）是**第四条**独立管线；tile 子请求**明确关闭**公式与表格 | `[实测]` `rapid_structure.rs:1856`、子请求构造 `:1994-2004`（`roi: None` + `FormulaPolicy::default()` + `TablePolicy::default()`）、`ImageInput` 物化阶梯三份 `:651-699`/`:1179-1220`/`:1880` | 同一"把请求变成像素"的动作写三遍；**tile 与页面级阶段的互斥是坐标空间决定的语义约束，不是待接管线**（评审一.4） |
| A-4 | 公式自建一条完整输出三通道（JSON/Markdown/HTML），其中三个函数生产零调用，真实 HTML 走另一条路 | `[审计]` `src/formula/output.rs:26`/`:58`/`:71`/`:75`；真实路径 `src/output/html.rs:149-156`、`src/output/markdown.rs:167`/`:216` | 两条形态并存的"公式输出"；读者会以为生效的是 `to_formula_html` 的 `<div class="formula">` |

### 2.6 调用流程：懒加载会话的身份缓存写了两遍

`[实测]` `rapid_structure.rs:1281-1310 formula_recognizer` 与 `:1316-1346 table_session` 逐行同形：
缺 `model_path` → 报错；`FileIdentity::of`；`is_none_or(|(current, _)| *current != identity)` 判重建；
重建时取 `runtime_profile().session_runtime()`；存 `Some((identity, x))`；末尾 `&mut` + "not initialized"。

差别只有两处：公式多设 `set_max_batch_size`，表格多组 `TableStructureConfig`。

### 2.7 跨域耦合的量化（回答"耦合到什么程度"）

| 方向 | 量化 | 判定 |
| --- | --- | --- |
| `src/table/**` → 公式 | 6 处提及（`metadata.rs` 2、`structure.rs` 2、`cell_content_backfill.rs` 1、`decode.rs` 1） | 都是文档/命名层面的提及，无调用 |
| `src/formula/**` → 表格 | 15 处，其中 13 处在 `formula/ftfy.rs`（unicode 表） | 无关耦合 |
| `src/table/**` → `crate::ocr::` | 生产代码 **0** 次（唯一例外 `table/ocr_resolution.rs:58` 引 `EngineConfig`） | 单向、可接受 |
| 中枢文件对两域的提及次数 | `rapid_structure.rs` 231/174、`serve/server.rs` 133/114、`serve/model_plan.rs` 55/166、`output/json.rs` 135/37、`api.rs` 102/43 | **中枢承载两域**是设计选择（一个引擎、一份输出契约），代价是中枢变大 |

结论：**不存在"文本模块被表格污染"这类耦合**。要治理的耦合都在中枢（`api.rs`、`rapid_structure.rs`、`serve/*`）里，
治理手段是给中枢一个类型（§4.3 的 `PageRoute`），而不是把两域拆开。

---

## 3 有理由的重复与不对称：明确**不合并**，并在此登记理由

| 项 | 证据 | 为什么不合并 |
| --- | --- | --- |
| `FormulaDetectorSpec` 与 `TableModelSpec` 形状相同却声明两次 | `[审计]` `src/bin/serve/model_plan.rs:889-891` 注释：合并会抹掉编译期错误 | 编译期能区分的错误比少写 10 行更重要 |
| 公式 JSON 词表 vs 表格换行词表两种格式 | `[审计]` `src/table/metadata.rs:5-16`（+ `:59 parse_character_metadata`、`:92 validate_vocabulary_width`） | 表格词表要拒空 token/单行 JSON，宽松解析会让坏模型静默通过 |
| 公式 Pillow 仿真预处理不复用共享 resize | `[审计]` `src/formula/preprocess.rs:8-9` 显式声明；`:684` golden `max_abs ≤ 1e-5` | 与 Python 参考的逐像素一致性优先于代码复用 |
| 两份净化（库字符串扫描 + 页面 DOMParser） | `[实测]` `src/output/html.rs:386`、`src/bin/web/index.html:3069` | 纵深防御：引擎出口净化 + 渲染前再净化。**只统一白名单规范，不合并实现** |
| 被表格消费的文本行仍留在 `regions` | `[实测]` `docs/07-table-recognition.md:347`（叠加而非替换） | 剔除会让文本通道丢内容；不对称是有意的 |
| 表格域不产文本指标（TEDS 交给 `tools/table_teds.py`） | `[审计]` `src/bin/table_eval.rs:16-20` | Rust 不实现第二份 TEDS 口径 |
| 表格 `detection.rs` 的文本分布启发式（零像素处理） | `[审计]` `src/table/detection.rs:1-16` 模块文档；门禁 G4 | 故意的架构约束（零新增整页 CNN），不是待补的短板 |
| `evaluation/table/run.rs` 与 `evaluation/ocr.rs` 的评测并列 | `[审计]` `src/evaluation/table/mod.rs:3-5`、`formula/mod.rs:6-9` | 两域指标不可混成一个默认汇总（`evaluation/mod.rs:3-9`） |
| `table::registration` 与 `model_verify::verify_sha256` 分层 | `[实测]` `src/table/registration.rs:144-156`、`src/model_verify.rs:405` | 前者是"从清单推导可信期望"的信任来源，后者是纯校验原语；合并会丢掉"谁有权声明摘要" |
| 三种 JSON 视图（`regions` / `items` / `tables`） | `[实测]` `src/output/json.rs:174-227` | 事实源 / 统一消费视图 / 表格专用视图，服务端页面、导出与 CLI 各自在用（P1-7 先测量） |

---

## 4 目标设计与逐项 diff 设计

### 4.0 分批原则

| 批次 | 判据 | 允许的行为变化 |
| --- | --- | --- |
| **P0** | 契约固化 + 口径统一 + 死代码清零 + 一致性收敛；每项都能用一条命令证明 | 允许（分位定义、公开 API 收缩），但必须逐条列明 |
| **P1** | 抽共享层；默认**行为必须等价**（同输入同输出） | 默认不允许；`P1-5` 的"解码次数"变更属例外，需像素对拍并单列 |
| **P2** | 编排重写：给页面级组合一个类型，删掉防递归改写 | 允许（内部结构），产品输出必须等价；阶段语义（抹白/原图/计时守恒）必须逐条回归 |

每项都按同一形状写：**现状 → 目标设计（类型签名）→ 调用链改法 → 行为预期 → 验证 → 风险与回退 → 测试清单**，
并在标题后标注 `状态`（批准 / 修订后批准 / 重写 / 暂缓）。

### 4.1 P0：契约固化、口径统一与死代码清零

#### P0-0 基线契约固化（评审四的前置批，**新增**）

**为什么先做这一批**：评审指出本方案原来的门禁里有未入库的产物（`Temp/` 的 TEDS 数字、`datasets/` 的四张回归图），
而"每批可证伪"必须建立在**已跟踪**的判据上。这一批不改产品行为，只把判据变成可复现的代码与文件。

**目标产物**

1. **输出契约键集合测试**（三份，分别覆盖三类报告）：

   | 报告 | 现有关键键 | 位置 |
   | --- | --- | --- |
   | 页面基准 | `stats.<metric>.count/avg/p50/p90/min/max` | `src/bin/bench_warm_e2e.rs:252-259` |
   | 表格报告 | `count/min/p50/p90/max/mean` | `src/evaluation/table/run.rs:1526-1545` |
   | 公式评测 | `Stats` 的整体序列化（`samples/min_ms/max_ms/mean_ms/p50_ms/p95_ms/stddev_ms`） | `src/bin/formula_eval.rs:199-201`、`src/bin/formula_bench.rs:73-76`/`:102-105` |

2. **统计方法标记**（N1 已裁决为 `percentile_method`）：报告顶层新增 `percentile_method: "linear"`——
   它直接声明"分位是怎么算的"，不需要再套一层整数 schema 版本；
   旧报告按"无标记 = 旧口径"解读。
3. **入库的小型回归 fixture**（Q6 裁决）：4 张回归图裁小后入库（含期望的文本字符多重集），
   放在 `tests/fixtures/` 下，使"没丢文字"这条判据不依赖 `datasets/`。
4. **模型摘要清单**：把三个域各自用到的模型路径 + 期望 SHA-256 写进一份测试可读的清单（复用既有 manifest 更好）。

**行为预期**：无产品行为变化；仓库新增测试与 fixture。

**验证**

```powershell
cargo test --all-targets
cargo test --bin rapidstruct --features serve
node tools\check-page-result-contract.mjs      # 页面结果契约（BODY_SHA256 钉点）
```

**风险与回退**：fixture 入库会增大仓库体积（裁小后可控）；键集合测试若写得过严，未来加键会误红 →
测试用"包含断言 + 明确的键集合快照"两种，前者不阻碍新增。

**测试清单**

- [ ] 三份报告各自的键集合断言（`[1,2,3,4]` 这类定值 + 真实报告的键集合）
- [ ] 小型回归 fixture 的字符多重集断言（只增不减）
- [ ] 页面契约脚本在 CI/本机可跑通，且不依赖 `datasets/`

---

#### P0-1 删除已登记的死代码（L-2） · 状态：**批准**（评审：删，但测试必须迁移）

**现状**（`[实测]`）

- `src/output/markdown.rs:1-6` 的模块注释原文登记了这件事："E15 试过删掉这一行：`-D warnings` 立刻报出
  「`to_markdown` is never used」「`to_markdown_texts` is never used」「field `center_y` is never read」——
  第三条是前两条的连带（`center_y` 只在 `to_markdown` 里被读）……故保留并在此登记。"
- 生产零调用清单（`[实测]` 全仓 grep）：
  - `src/output/markdown.rs:34 to_markdown`、`:98 to_markdown_texts`、`:17`+`:29`+`:71 BoxProps::center_y`
  - `src/output/json.rs:53 to_json_items`（仅本文件测试 `:244` 调用，调用点 `:246`）
  - `src/formula/output.rs:26 to_formula_json`、`:71 to_formula_markdown`、`:75 to_formula_html`（仅本文件测试，外加 `src/exports.rs:42` 重导出）
- **生产实际走的路**（这是删得掉的理由）：JSON 走 `to_output_json` → `serde_json::to_value(OcrOutput)`；
  Markdown 走 `to_markdown_block`（`src/output/markdown.rs:167`/`:216`）；HTML 的公式分支在 `src/output/html.rs:149-156` 生成 `<li><code class="latex">`。

**目标设计**

1. 删除上述 6 个函数 + `src/output/markdown.rs:1` 的模块级 `#![allow(dead_code)]` + `BoxProps::center_y`（连带删 `BoxProps` 中不再被读的字段，若有）。
2. `src/exports.rs:42` 移除 `to_formula_html, to_formula_json, to_formula_markdown` 三项重导出。
   保留 `to_markdown_block` 与 `order_formula_results`（生产路径在用；**动手前用 grep 再确认一次**）。
3. 不新增"兼容包装"。

**调用链改法**

- 无生产调用链需要改（这正是它可删的证据）。
- **测试迁移（评审明确要求：不允许通过删测试来满足 `-D warnings`）**：被删函数的断言必须改挂到它本来包裹的生产入口上。

| 被删函数 | 现有断言位置 | 迁移到 | 覆盖的东西 |
| --- | --- | --- | --- |
| `to_formula_markdown` | `src/formula/output.rs:149,156,165,172,178,184,199`（**7 条用例**） | `to_markdown_block`（`src/formula/output.rs:58`） | `normalize_display_math`、`TRUNCATED_MARKER`、换行折叠、空串、Markdown 特殊字符 |
| `to_formula_html` | `src/formula/output.rs:205,213,224,236`（**4 条用例**） | `src/output/html.rs` 的公式分支（新增 `latex` 含 `<>&"'` 的用例） | HTML 转义、截断标记、`eos_index` |
| `to_formula_json` | `src/formula/output.rs:136`（**1 条用例**，2 处断言 `:137`/`:142`） | `to_output_json` 的 formula 字段用例 | `include_token_ids` 两态 |
| `to_json_items` | `src/output/json.rs:244`（**1 条用例**，调用点 `:246`） | `to_output_items` | 空集与三态合并 |
| `to_markdown` | `src/output/markdown.rs:289`（**1 条用例** `markdown_empty_message`，调用点 `:291`） | `to_output_markdown` 的空输入用例 | 空输入不 panic、返回占位消息 |
| `to_markdown_texts` | **0 个调用点（连测试都没有）** | — | 无需迁移：它今天完全不可达（`center_y` 也只被 `to_markdown` 的 `:71` 读） |

**行为预期**

- 产品输出**逐字节不变**（三通道各存一份改前快照，改后对比）。
- 库公开面收缩：`to_markdown*` / `to_json_items` / `to_formula_{json,markdown,html}` 不再可调用。

**验证**

```powershell
cargo clippy --all-targets --features serve -- -D warnings   # 必须 0；#![allow(dead_code)] 删掉后它就是守卫
cargo test --all-targets
cargo test --bin rapidstruct --features serve
# 三通道对拍：对 tests/fixtures/serve/result-real-42-regions.json 反序列化出的 OcrOutput，
# 分别跑 to_output_json / to_output_html / to_output_markdown，与改前保存的快照逐字节比对
```

**风险与回退**

- 风险：如果有仓外消费者（仓内证据只有 `src/exports.rs:42` 一处重导出，`[实测]`），删掉会破坏它。
- 回退：把被删函数恢复为 1 行薄包装（`self.to_markdown_block(...)`），代价是继续背一个公开面 —— 属于"放弃 P0-1"而不是局部回退。
- 裁决：见 §7.1 Q1（已裁决：删除生产零调用 API，但**保留其语义测试**并迁移到实际输出函数）。

**测试清单**

- [ ] 迁移上表 **14 条用例**（7 + 4 + 1 + 1 + 1）的全部断言；`to_markdown_texts` 因零调用点直接删，无需迁移，`cargo test --lib output` / `--lib formula` 全绿
- [ ] 新增：三通道快照对拍（P0-1 与 P1-7 共用）
- [ ] `cargo clippy ... -D warnings` 输出为空（守卫）
- [ ] 断言"删掉的函数不再出现在 `src/exports.rs`"（防止重导出回潮）

---

#### P0-2 分位口径单点化（L-1 / D-3） · 状态：**重写后批准**（评审一.1）

**现状**（`[实测]`，三份定义）

| 位置 | 定义 | 对 `[1,2,3,4]` 的 p50 | 对 `[1,2,3,4]` 的 p90 |
| --- | --- | --- | --- |
| `src/evaluation/stats.rs:50-57` | `position = fraction*(len-1)` 线性插值 | **2.5** | 无 p90 字段 |
| `src/evaluation/table/run.rs:1532-1535` | `sorted[((len-1) as f64 * q).round() as usize]` 最近秩 | **3.0** | `sorted[round(2.7)]` = 4 |
| `src/bin/bench_warm_e2e.rs:259` | `p50: s[(len-1)/2]`（中位）、`p90` 最近秩 | **2** | 4 |

`Stats` 的字段是 `samples/min_ms/max_ms/mean_ms/p50_ms/p95_ms/stddev_ms`（无 p90），并且**被报告直接序列化**：
`src/bin/formula_eval.rs:199-201`/`:687`、`src/bin/formula_bench.rs:73-76`/`:102-105`、`src/table/structure.rs:1278-1282`（打印）；
`src/bin/formula_bench.rs:476` 的断言还把 `p50_ms == 2.5` 钉死。`src/evaluation/table/run.rs:1582` 的自洽测试**从未与共享层对拍**。

**v1 的设计错在哪（评审一.1）**

1. `pub quantiles: Vec<(&'static str, f64)>` 把"调用方想要哪些分位"塞进了 `Stats` 的序列化结构 ——
   带 `&'static str` 的动态 `Vec` 不适合作为可反序列化的公共报告字段。
2. 删除 `p50_ms`/`p95_ms` 会改变**公式评测**报告结构，而 v1 只保护了表格与页面基准的键名。

**目标设计（重写）**

```rust
// src/evaluation/stats.rs —— 全 crate 唯一的分位实现
/// 分位的唯一定义：线性插值（`position = q * (n - 1)`），与 numpy 默认 `linear` 口径一致。
pub fn percentile(sorted: &[f64], q: f64) -> f64;

/// **字段保持稳定**：现有报告结构与键名不变（评审一.1）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stats {
    pub samples: usize,
    pub min_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub stddev_ms: f64,
}

impl Stats {
    pub fn from_samples(values: Vec<f64>) -> Self;      // 保持不变
    /// 需要额外分位时的通用入口：**不给 `Stats` 增加序列化字段**，只取数。
    pub fn quantile(values: &[f64], q: f64) -> f64;
}
```

报告层要哪些分位，由**该报告自己的适配器**声明，而不是由 `Stats` 声明：

```rust
// src/evaluation/table/run.rs（表格报告侧）
fn summary_value(values: &[f64]) -> Value {
    // 今天那组键名一个都不改：count/min/p50/p90/max/mean
    json!({ "count": …, "min": …, "p50": percentile(&sorted, 0.50),
            "p90": percentile(&sorted, 0.90), "max": …, "mean": … })
}

// src/bin/bench_warm_e2e.rs（页面基准侧）
fn stats_json(s: &[f64]) -> Value {
    // 今天那组键名一个都不改：count/avg/p50/p90/min/max
}
```

**调用链改法**

- `src/evaluation/table/run.rs:1526-1545`：删本地 `percentile` 闭包（**最近秩**），改调 `stats::percentile`（线性插值）；
  键名映射留在本文件的 `summary_value` 适配器里。
- `src/bin/bench_warm_e2e.rs:252-259`：删本地 `stats()`，改调 `stats::percentile`；输出键名不变。
- `src/bin/formula_eval.rs` / `src/bin/formula_bench.rs` / `src/table/structure.rs:1278`：**不改结构**（`Stats` 字段保留），
  因此公式评测报告不变 —— 这正是评审要求的结果。
- `src/bin/formula_bench.rs:476` 的断言保留（`p50_ms == 2.5` 与线性插值一致）。

**行为预期（报告口径变更，必须列明）**

- p50：页面基准从"下中位"改为线性插值；表格报告从"最近秩"改为线性插值。
  `[推算]` 对 12 图基准（样本数 ≥ 12）的位移不超过一个相邻样本差，但**不是零**。
- p90：表格报告从最近秩改为插值；页面基准第一次有统一的 p90。
- **公式评测报告不变**（`Stats` 字段与序列化结构不变）—— 与 v1 相反。
- **跨版本不可比**：`tools/check_provider_claims.ps1:476-484` 的比较前提是"same fingerprint"；
  同时按 P0-0 给报告加 `percentile_method: "linear"` 标记（N1 裁决），旧报告按"无标记 = 旧口径"解读。

**验证**

```powershell
cargo test --lib evaluation::stats          # 新定义被钉住：[1,2,3,4] → p50=2.5、p90=3.7、p95=3.85
cargo test --lib evaluation::table          # 表格报告键名与数值的契约用例
cargo test --bin formula_bench              # 公式基准既有断言仍绿（证明报告结构未被改）
# 端到端：跑一次 12 图基准（真实运行，不假设有 dry-run 开关），核对 JSON 键集合不变
powershell -File tools\run_windows_baseline.ps1
```

**风险与回退**

- 风险 1：脚本按旧键读 → 用 P0-0 的键集合断言挡住。
- 风险 2：已入库基线 JSON（`tests/baseline/windows-baseline/*.json`）的数字来自旧口径。
  用户已裁定**不重建**（`docs/11` §4.12 + §7.1 Q5），因此本项只需在引用点补一句"含分位口径变化"，不得把新数字与旧数字并列比较。
- 回退：`percentile` 的实现切回最近秩即可恢复旧数值语义（一处改动）；但"两套定义并存"会回来，等同于放弃 P0-2。

**测试清单**

- [ ] `percentile` 定义用例（`[1,2,3,4]`、空集、单样本、含 NaN 拒绝）
- [ ] **对拍用例**（消除 L-1 的"未对拍"）：同一组输入上，表格报告的 p50/p90 与 `Stats`/`percentile` 的值一致
- [ ] 三份报告的 **JSON 键集合测试**（页面基准 / 表格报告 / 公式评测）—— 评审明确要求
- [ ] `percentile_method` 标记存在且取值 `"linear"`（N1）
- [ ] `src/bin/formula_bench.rs:476` 的既有断言不变

---

#### P0-3 模型完整性校验：引入统一凭证（I-3 / I-4 / I-5） · 状态：**重写**（评审一.3）

**现状**（`[实测]`，v1 的表述已按评审修正）

- 校验原语已存在：`src/model_verify.rs:405 verify_sha256(path, expected: Option<&str>) -> Result<Option<String>>`
  —— 给出即校验、不匹配报 `RapidOcrError::HashMismatch`（路径 + 期望 + 实际）、`None` 表示"调用方没有可信摘要"。
- **公式其实已经在校验**：`src/formula/recognizer.rs:90 from_model_with_hash`（`:101` 调 `verify_sha256`）
  与 `src/formula/detect.rs:117 from_model_with_hash`（`:125` 调 `verify_sha256`）；
  但默认构造器 `from_model`（`:87` / `:108`）传 `None`，等价于"不校验"。
- **表格的信任来源是清单**：`src/table/registration.rs:144-156 verify_registered_model(model_path, expected)` 读模型旁边的
  `manifest.json` 按 `role=table_structure` 取期望摘要，再委托 `verify_sha256`；`src/table/structure.rs:347` 在**创建任何 ORT 会话之前**调用它。
- **文本没有这一层**：`src/ocr/session.rs:40-50` 的 `OcrSession::new(model_path, runtime_cfg, kind)` 不接收摘要，
  直接 `OrtSession::open_unchecked`；文本模型的完整性只在服务准入/下载层保证。
  （**现状记录，已修**：P0-3 二阶段把会话构造器改为收 `&VerifiedModel`；而“外部调用方仍可用
  `open_unchecked` 整条绕过”这一残留由复审 P1-2 关闭，见 §9 与本文件末尾的更新。）
- 同一条件两条文案（`src/api.rs:689` vs `rapid_structure.rs:1286`）。

**根因（评审一.3 的表述）**

> 模型完整性校验分散在**服务启动准入、域构造器、底层会话构造器**之间，缺少统一的**模型凭证类型**，
> 于是"校验强度"由调用路径决定，而不是由"调用方是否持有可信摘要"决定。

**目标设计（重写）**

```rust
// src/model_verify.rs —— 新增凭证类型；校验仍只有 verify_sha256 一个实现
#[derive(Debug, Clone)]
pub struct VerifiedModel {
    path: PathBuf,
    /// 期望摘要：来自清单/模型集/CLI 的部署声明；`None` = 没有可信期望
    expected_sha256: Option<String>,
    /// 实际摘要：`verify_sha256` 的返回值；`None` = 未校验（无期望）
    actual_sha256: Option<String>,
}

impl VerifiedModel {
    /// 唯一的构造入口：**任何模型文件进入 ONNX 会话之前都经过它**。
    pub fn verify(path: &Path, expected: Option<&str>) -> Result<Self>;
    pub fn path(&self) -> &Path;
    pub fn expected_sha256(&self) -> Option<&str>;
    pub fn actual_sha256(&self) -> Option<&str>;
    pub fn is_verified(&self) -> bool;   // actual.is_some()
}

// 会话构造器接收凭证，而不是裸路径 —— 结构上不存在"忘记校验"的旁路
impl OcrSession {
    pub fn new(model: &VerifiedModel, runtime: &RuntimeConfig, kind: OcrSessionKind) -> Result<Self>;
}
impl FormulaSession {
    pub fn new(model: &VerifiedModel, runtime: &RuntimeConfig) -> Result<Self>;
}
impl FormulaRecognizer {
    /// 删除 `from_model`；只保留 `from_model_with_hash`（可改名为 `from_model`）
    pub fn from_model(model: &VerifiedModel, runtime: &RuntimeConfig) -> Result<Self>;
}
impl FormulaDetector { /* 同上 */ }
impl TableStructureSession { /* `TableStructureConfig.expected_sha256` 保留（部署参数），内部改成构造 VerifiedModel */ }
```

> **【复审末段 · 记录回正】** 代码块里那句“结构上不存在‘忘记校验’的旁路”是**设计目标**，不是当时的事实：
> P0-3 二阶段（批次 4）把会话构造器都改成收 `&VerifiedModel`，但 `OrtSession::open_unchecked` 当时仍是 `pub`
> 并随 `OrtSession` 对外可见，库的使用者可以整条绕过完整性边界，所以 CHANGELOG 与本节那句话当时都说过头了。
> 该残留由复审修复批 P1-2（`86cc76e`）关闭：`open_unchecked` 改为 `#[cfg(test)] pub(crate)`（非测试构建里根本不存在），
> 公开面上的会话构造入口只剩 `open_verified`，另有 `tests/session_open_surface_guard.rs` 钉住不许改回公开。
> **至此**这句话就整个 crate 而言才成立；它不改变文件身份判定的启发式性质与中段修改盲点（§4.2 已登记，另见下文 P1-2 `LazyModel` 一节的标注）。

**`None` 是否允许，由调用场景决定，而不是由每个域自行决定**（评审一.3 的原话）：

| 调用场景 | 期望摘要来源 | 是否允许 `None` |
| --- | --- | --- |
| serve（模型集/CLI 声明） | `declared_sha256`（`src/bin/serve/model_plan.rs:146`/`:502`/`:530`/`:550`） | 允许，但**必须记录**并显示（现有 `/api/models` 已有 `declared_sha256` + `has_declared_digest`，见 §7.1 Q3） |
| 表格结构模型 | 模型旁 `manifest.json`（`registration`） | **不允许**（今天就是强制：`docs/07` 的"没有可追溯哈希的模型不该被静默接受"） |
| 评测（table/formula eval） | `require_registered_model_hash` / `--expected-sha256` | 不允许（可追溯性优先） |
| 库用户自备模型 | 由调用方给 | 允许 |

**调用链改法**

- `src/ocr/session.rs:45`、`src/formula/session.rs:28`：改收 `&VerifiedModel`；调用方是否持有摘要需核实
  （`models/configs/*.yaml` 与模型集清单今天似乎不带 → 若确实没有，构造 `VerifiedModel::verify(path, None)`，语义是"如实记录无法校验"）。
- `src/formula/recognizer.rs:87` / `src/formula/detect.rs:108`：删掉无摘要的默认构造器，改为"凭证必填"。
- `src/table/structure.rs:347`：保留 `registration` 分层（I-5），但把它产出的 `(path, expected)` 交给 `VerifiedModel::verify`。
- `rapid_structure.rs:1286`/`:1321`：文案改为单点函数 `missing_model_path_error(domain)`。

**行为预期**

- 机制统一：**强度由凭证决定**；结构上不再存在"调用方忘了校验"的路径。
- `FormulaRecognizer`/`FormulaDetector` 各少一个公开构造器。
- 若某条真实配置的清单摘要与磁盘不符：从"能跑"变成"明确报 `HashMismatch`" —— 正确变化，但必须先冒烟。
- **不新增 `sha256_verified` 字段**（§7.1 Q3）：现有 `declared_sha256` + 实际摘要 + 验证状态已够。

**验证**

```powershell
cargo test --lib model_verify           # 既有 "no expectation means no verification" 用例改写为"记录为未校验"
cargo test --lib ocr::session formula::session formula::recognizer formula::detect
cargo test --test serve_startup --features serve
cargo test --bin rapidstruct --features serve
.\target\release\rapidstruct.exe --help  # CLI 冒烟：模型仍能被加载
```

**风险与回退**

- 风险：清单摘要过期 → 原本可用的配置开始报错。用真实配置冒烟作为验收的一部分。
- 风险：`VerifiedModel` 传染面大（所有会话构造点）→ 分两次提交：先落凭证类型与 `verify`，再逐域切换。
- 回退：保留裸路径重载（`new_unverified`）作为显式逃生口，但**必须在注释里写明为什么允许**；否则等于放弃 P0-3。

**测试清单**

- [ ] 凭证正/负例：摘要一致 → `is_verified()`；不一致 → `HashMismatch` 且**会话未被创建**
- [ ] 三个域各一条"路径存在但摘要不符"的拒绝用例
- [ ] 文案单点：两处错误的 `to_string()` 相同
- [ ] `table::registration` 的 14 条测试不变（证明分层被保留）
- [ ] 删除无摘要构造器后 `-D warnings` 为 0（守卫）

---

#### P0-4 表格加载的双开会话（测量项） · 状态：**重写**（评审一.2）

**现状**（`[实测]`）

`src/table/structure.rs:334-398 open` 的流程是：

```
:340 is_file 检查 → :347 verify_registered_model（早于任何 ORT 会话）
:354 effective_runtime_config
:358 探针会话 open_unchecked(model_path, &runtime_cfg, &[])      ← 第一次开
:359 probe_io → :360 validate_table_structure_contract
:365-371 metadata_custom("character") → :372 parse_character_metadata → :375-380 词表
:381 dimension_overrides(&probe, model_path)                     ← 从探针读动态维度符号名
:382 drop(probe_session)
:384 正式会话 open_unchecked(..., &overrides)                     ← 第二次开
:386 require_applied → :387-388 input_dims/session_threads
```

**为什么 v1 的测法是错的（评审一.2）**：探针会话不只"多开一次"，它还承担
**输入输出契约读取、character metadata、词表、动态维度名称**四件事；
而 `src/bin/bench_warm_e2e.rs` 测的是**请求路径**，量不到会话创建成本。

**目标：先测量，再决定（Q4 裁决：接受"测得可忽略则不做"）**

| 测量项 | 方法 | 判据 |
| --- | --- | --- |
| `TableStructureSession::open` 冷启动耗时（含探针） | 专用测量（新 bin 或 `#[ignore]` 用例，循环 N 次取分位，用 `stats::percentile`） | 相对整页 p50 的占比 |
| 引擎构造期中表格会话的建立耗时 | `RapidOcrEngine::new` 内加计时（或从外部对构造调用计时） | 是否落在初始化热路径 |
| 每次引擎构造的会话次数 | 在 `Session::builder` 或 `open_unchecked` 上加**测试专用计数**（`#[cfg(test)]`） | 是否为 2（探针 + 正式） |
| `fix_input_shape = false` 与 `true` 两态 | 两态分别测（`dimension_overrides` 在两态下都会开探针，见 `:233-238`/`:278-291`） | 两态是否都双开 |

**可能的结论与各自的动作**

| 结论 | 动作 |
| --- | --- |
| 双开只发生在引擎初始化，且相对页级 p50 可忽略（阈值 1%） | **登记为已知成本，关闭本项**（评审一.2 明确接受） |
| 双开落在请求热路径 | 才考虑设计：把"从探针读符号名"改成"清单/配置声明符号名" —— 但这会反转 `docs/09-table-recognition-execution.md:554` 的刻意设计（"符号名从探针读，不写死"），**必须带着新证据才做** |

**行为预期**：本项**默认不改代码**；只新增测量设施（若用 `#[cfg(test)]` 计数则零产品影响）。

**验证**

```powershell
cargo test -p rapid-structure-rs --lib table::structure -- --ignored   # 冷启动测量用例
# 引擎构造耗时：一次真实 serve 启动的日志/计时（同 provider、同模型）
```

**风险与回退**：测量用例若依赖真实模型文件，需跳过缺失场景而不是失败；回退 = 删掉测量设施。

**测试清单**

- [ ] 冷启动耗时分位用例（`#[ignore]`，需真实模型）
- [ ] 会话次数计数断言（`#[cfg(test)]`）：一次引擎构造期间表格模型被开 2 次（现状即断言）
- [ ] 测量命令与结果写进 §9 的实施记录

---

### 4.2 P1：抽共享层（默认行为必须等价）

P1 的每项都以同一句话为验收：**同输入同输出**。验证基线分两类（§8.2）：
已入库契约（`BODY_SHA256`、小型回归 fixture、`tests/baseline/windows-baseline/*.json`）与**本机质量门禁**
（全量 TEDS、四图字符多重集 —— 依赖未入库 `datasets/`）。

#### P1-1 几何基础函数单点化（D-1） · 状态：**重写**（评审二.5）

**现状语义表**（评审指出的关键：它们**不是同一个函数**）

| 实现 | 语义 | 边界 | 是否合并 |
| --- | --- | --- | --- |
| `src/evaluation/ocr.rs:174 polygon_iou` | 凸四边形裁剪后的**真实 polygon IoU**（被生产调用：`rapid_structure.rs:2170` 平铺去重） | — | **不合并**（保留独立函数） |
| `src/formula/route.rs:69 Rect::iou` | 轴对齐框 IoU | `>=` | 不合并（语义不同，见下） |
| `src/table/cell_content_backfill.rs:293 rectangle_iou` | 表格回填口径（f64） | `>=`/`<` 混用 | 不合并 |
| `src/formula/detect.rs:614 iou` | 参与**类别感知 NMS** | `>` | 不合并（NMS 的 tie-break 与类别偏移 `:653 offset_box` 绑定） |
| `src/table/detection.rs:1272 iou` | 仅 `#[cfg(test)]` | — | 可删（测试改用公共函数） |

**目标设计（缩小到无争议的基础函数）**

```rust
// src/vision/geometry.rs（新模块；`vision/` 已是共享层）
/// 唯一的 polygon → 轴对齐外接框实现（今天 8 份）
pub fn aabb_from_quad(q: &Quad) -> Aabb;
pub fn aabb_of(points: &[[f32; 2]]) -> Aabb;

/// 唯一的轴对齐 IoU（仅在"语义确实是轴对齐 IoU"的调用点替换）
pub fn aabb_iou(a: Aabb, b: Aabb) -> f64;

/// 唯一的 polygon 面积（今天 2 份：`det/postprocess/mod.rs:1764`(f32) / `:1778`(f64)）
pub fn polygon_area(points: &[[f32; 2]]) -> f64;
```

**不做的部分（评审明确要求）**：`polygon_iou`、`formula/detect.rs` 的 NMS（含类别偏移）、`covered_ratio`、
各域的贪心/抑制逻辑**保留各自独立函数**，除非有真实数据证明口径相同。

**调用链改法**

1. 先落 `geometry.rs` 三个函数 + 单元测试（**不改任何调用点**）。
2. 逐处替换，**每次替换前建立旧实现与新实现的数值对拍**（用真实 bbox：四张回归图 + `tests/fixtures/` 里的检测结果）。
3. 对拍一致才删旧实现；不一致则**保留旧实现并在本文登记差异原因**（不是"顺手统一"）。

**行为预期**：等价。但有一处必须实测：**f32 → f64 会让"恰好卡在阈值上"的比较翻转**（IoU 判定是阶梯函数）。
因此**边界语义（`>=` vs `>`）在替换时必须逐处保留原样**，不得因为"统一"而改成同一种。

**验证**

```powershell
cargo test --all-targets
# 数值对拍：旧实现 vs 新实现（含退化矩形、零面积、containment、相等框、恰好等于阈值的构造）
```

**风险与回退**：替换顺序错就会引入难查的判定漂移 → 一次只替换一类调用点，一次提交。回退 = 逐处还原，`geometry.rs` 继续保留（它本身无害）。

**测试清单**

- [ ] `aabb_from_quad` / `aabb_iou` / `polygon_area` 的边界用例（退化、零面积、containment、相等、阈值相等）
- [ ] 每个替换点的"旧 vs 新"数值对拍（在删除旧实现之前跑通）
- [ ] 全量 TEDS 与四图字符多重集（§8.2，本机门禁）

---

#### P1-2 `LazyModel<T>`：懒加载会话的身份缓存单点化（§2.6） · 状态：**批准**

**现状**（`[实测]`）`rapid_structure.rs:1281-1310` 与 `:1316-1346` 逐行同形（缺 `model_path` → 报错 / `FileIdentity::of` /
`is_none_or(|(current, _)| *current != identity)` 判重建 / 重建时取 `runtime_profile().session_runtime()` / 存 `Some((identity, x))` /
末尾 `&mut` + "not initialized"）。

**目标设计**

```rust
// 归属：只依赖 FileIdentity → 放 `src/model_verify.rs` 旁（`src/runtime/` 依赖 ORT，不合适）
pub struct LazyModel<T> {
    label: &'static str,
    slot: Option<(crate::model_verify::FileIdentity, T)>,
}

impl<T> LazyModel<T> {
    pub const fn new(label: &'static str) -> Self;
    /// `path == None` 时返回与 `label` 对应的结构化错误（文案单点，见 P0-3）。
    /// `build` 只在身份变化时被调用；它拿不到 `&mut self`，因此不存在"半初始化"状态。
    pub fn get<F>(&mut self, path: Option<&Path>, build: F) -> Result<&mut T>
    where F: FnOnce() -> Result<T>;
    pub fn is_loaded(&self) -> bool;   // 诊断/测试：断言"没有重复建会话"
}
```

**调用链改法**

- `RapidOcrEngine` 的两个字段 `formula_recognizer: Option<(FileIdentity, FormulaRecognizer)>`、
  `table_session: Option<(FileIdentity, TableStructureSession)>` 变成 `LazyModel<…>`。
- `formula_recognizer(&mut self, policy)` 变成三行：取 `policy.model_path.as_deref()` → `get(path, || { …现有重建代码… })`。
- 表格同理；`TableStructureConfig` 的组装留在闭包里。
- 顺带消除 I-3：错误文案来自 `LazyModel::get` 的 `label`（与 P0-3 的 `missing_model_path_error` 同一处）。

**行为预期**：等价（同一身份判据、同一重建时机、同一错误类型）。

> **【复审 P1-1 · 记录回正】** 本节的“同一身份判据（只看 `FileIdentity`）”已被推翻：只按身份作键时，
> “第一次用正确摘要加载 → 第二次同路径同身份但换一个**错误**声明”会让旧会话直接回答，声明从未参与比对，
> 与 `api.rs` 上“给出摘要即校验”的契约直接冲突。三个缓存点现在的键是
> `SessionCacheKey { identity, declared }`（见 §9 复审 P1-1 行）。另：`LazyModel<T>` 这个结构本身**至今未实施**
> （字段仍内联在 `RapidOcrEngine` 上，本节仍是待办设计）；若将来实施，键必须带上声明摘要，不得退回只看身份。
> 还有一层不得混写：键里的 `identity` 只能给出“没有观察到变化”，**中段修改（同体积 + 同 mtime + 首尾 64 KiB 不变）
> 仍是已登记的启发式盲点**，不能把它描述成安全边界或内容完整性证明。
>
> **【复审 P1/P2 · 就地补充】** 上面这一条不被推翻，但加一条**收敛**：校验缓存条目被**加载期证伪**时会
> 被**删除**（`VerifiedModel::verify` 手上的完整摘要与条目不符即作废，比对失败那条错误路径同样作废），
> 所以一条被中段修改骗过的缓存结论最多活到下一次带可信摘要的加载；会话缓存同理仍按身份判定重建，
> 因此“已建好的会话”仍只能由 `POST /api/models/reverify` 刷新。它仍**不是**实时保证，也仍不能当安全边界引用
> （实施记述见 §9“状态缓存的加载期证伪”行）。

**验证**：`cargo test --all-targets`；serve 两条 `--features serve` 套件；
**加一条"两次相同调用只建一次会话"的断言**（`LazyModel::is_loaded`）。

**风险与回退**：风险低。唯一要小心的是闭包里的错误时机（今天 `model_path` 缺失在取 runtime 之前报错，
抽象后同样在 `get` 内先判 `None`，顺序不变）。回退 = 展开回两份。

**测试清单**

- [ ] `LazyModel` 单元测试：缺失路径报错 / 首建 / 同身份不重建 / 身份变化重建（改 mtime）
- [ ] 引擎级：公式与表格各一次"重建只发生一次"的断言
- [ ] `cargo test --bin rapidstruct --features serve` 全绿

---

#### P1-3 `vision::normalize`（只合并 rec/cls 的 LUT） · 状态：**重写**（评审二.6）

**现状**（`[实测]`）

- rec 与 cls 有一份**同形**的有符号单位化 LUT：`src/ocr/rec/preprocess.rs:228 normalize_to_signed_unit` + `:232 signed_unit_lut() -> &'static [f32; 256]`、
  `src/ocr/cls/preprocess.rs:155` + `:159`（**`[f32; 256]`，不是 `[i8; 256]`** —— v1 写错了类型）。
- 共享层只有 resize（`src/vision/resize.rs:212`、`vision/image_backend.rs:7`）。
- 各域归一化差异**不止公式**：resize 尺寸策略、CHW 写入顺序、padding 语义、table 的 ImageNet mean/std、
  det 的 AVX2 路径（`src/ocr/det/preprocess.rs:101` + `:251`）、table 的内联 `Array4::zeros` + `(raw*SCALE-mean)/std`（`src/table/preprocess.rs:139-150`）。

**目标设计（缩小）**

```rust
// src/vision/normalize.rs —— 只收编 rec/cls 这一份确定同形的实现
pub fn signed_unit_lut() -> &'static [f32; 256];
pub fn normalize_signed_unit_into(dst: &mut [f32], src: &[u8]);
```

**调用链改法**：`src/ocr/rec/preprocess.rs:228-236` 与 `src/ocr/cls/preprocess.rs:155-163` 改调公共函数；
**det 的 AVX2 归一化与 table 的归一化暂时保留**（评审明确要求），等逐像素对拍后再逐个决定。

**行为预期**：等价。`[推算]` 浮点运算顺序若变化，可能出现 1 ulp 差异 → 用**逐像素对比**验证，不只看指标。

**验证**：`cargo test --all-targets`；cls/rec 的既有黄金用例；表格域 TEDS（本机门禁）。

**风险与回退**：低。回退 = 还原两份局部实现。

**测试清单**

- [ ] 0..=255 全域 LUT 数值逐项相等（旧 vs 新）
- [ ] rec/cls 既有预处理用例全绿
- [ ] table 的 ImageNet 归一化未被触碰（diff 断言）

---

#### P1-4 页文本行单一表示（D-2） · 状态：**重写**（评审二.7）

**现状**（`[审计]` + `[实测]`）

- `table::detection::PageLine`（`src/table/detection.rs:78`）保存**四边形**；
- `table::cell_content_backfill::TextLine`（`src/table/cell_content_backfill.rs:81`）保存**裁剪后的轴对齐 bbox**；
- bbox 生成时包含边界裁剪与类型转换；两者由 `rapid_structure.rs:1639` 同时构造（`:1628-1631` 自认是防分叉的刻意设计）。

**目标设计（重写：先转换、后合并）**

1. **不放进 `src/ocr/types.rs`**（评审二.7）：规范页文本行类型放在共享几何 / 表格基础层
   （例如 `src/vision/geometry.rs` 旁或 `src/table/` 内的公共模块），因为它服务的是页面级装配，不是 OCR 会话内部类型。
2. 先提供**显式转换**，保留旧结构：

```rust
/// 页级文本行的规范视图：一次装配、多处只读。
pub struct PageTextLine {
    pub index: usize,
    pub text: String,
    pub polygon: Quad,
    pub bbox: Aabb,     // 由 polygon 按既有裁剪规则导出
    pub score: f32,
}

impl PageTextLine {
    /// 显式转换：字段、裁剪规则、浮点精度逐项对齐后才允许替换旧结构。
    pub fn from_region(region: &OcrRegion) -> Option<Self>;
}
```

3. 只有当"字段 / 裁剪规则 / 浮点精度"三者对拍完成后，才删除 `PageLine`/`TextLine` 中的一份。

**调用链改法**：`rapid_structure.rs:1639` 先改为"构造规范行 + 由它导出两个旧视图"（转换点单点化，行为不变）；
下游消费者分两批迁移，每批一次提交。

**行为预期**：第一批（引入规范类型 + 单点转换）等价；第二批（删除旧结构）等价，前提是对拍通过。

**验证**：`cargo test --lib cell_content_backfill`（E25 的 33 条用例是最好的回归网）+ 表格接线用例 + §8.2 门禁。

**风险与回退**：`f32` vs `f64` 的差异会改变比较精度 → 逐字段核对；回退 = 保留双结构（`docs/07` 的既有说明仍然成立）。

**测试清单**

- [ ] `PageTextLine::from_region` 的字段/裁剪对拍（对 E25 硬编码生产图的 6 条行）
- [ ] `cargo test --lib cell_content_backfill`（33 条）
- [ ] 表格接线用例（`table_routing_*`）

---

#### P1-5 输入物化入口单点化（A-3 的等价部分） · 状态：**重写**（评审二 + 五.6）

**现状**（`[实测]`）同一"把请求变成像素"的动作写三遍：
`rapid_structure.rs:651-699`（文本）、`:1179-1220 decode_source_image`（公式/表格，注释说明它只负责"未抹白的原始像素"）、`:1880`（tile）。

**目标设计（本批只统一入口，保持两次解码）**

```rust
// src/ocr/pipeline/rapid_structure.rs
struct MaterializedInput {
    rec: crate::config::RecImage,
    original_size: crate::api::ImageSize,
    offset: (i32, i32),                     // roi 偏移，今天由 `to_bgr(roi)` 返回
    dynamic: Option<image::DynamicImage>,   // 页面级阶段才需要；懒取得
}

impl RapidOcrEngine {
    fn materialize(&self, request: &crate::api::OcrRequest) -> Result<MaterializedInput>;
    fn raw_dynamic(&self, m: &MaterializedInput) -> Result<image::DynamicImage>;
}
```

- 本批 `raw_dynamic` **保留今天的语义**（`Encoded` 仍走 `loader.load_dynamic_with_limit`，即第二次解码），
  因此**行为严格等价**；三个调用点改吃同一个入口。
- **"解一次"是独立变更**（评审五.6）：它同时影响解码次数、像素来源与坐标语义，必须**像素级对拍**
  证明"由 `RecImage` 转出的 `DynamicImage` 与今天的第二次解码逐像素一致（或差异已被解释）"。
  `[未测]` 已知风险：`docs/04` 有 `image` vs turbojpeg 的 A/B 记录（含"输出差异"栏）→ 两条路径可能不是同一个解码器。
  这项变更放 P2-3（已暂缓）或单列，不混在 P1-5 里。

**行为预期**：本批等价（`raw_dynamic` 的实现在本批不变）。

**验证**：§8.2 + 五个 `ImageInput` 变体的既有用例。

**测试清单**

- [ ] 五个 `ImageInput` 变体各一条物化测试（含 `Pixels` 的 `max_decode_pixels` 限制、`Url` 的既有 fixture）
- [ ] roi 偏移一致性（`to_bgr(roi)` 的 offset 语义不变）
- [ ] 一次"解码次数不变仍为 2"的断言（证明本批没有偷偷改行为）

---

#### P1-6 转义与净化白名单（M-1 / I-6） · 状态：**重写**（评审二.9）

**现状**（`[实测]`/`[审计]`）

- 库内有第二份 `escape_html`（`src/table/emitter.rs:383`，同形五映射，由 `:372 encode_text(text, policy)` 在 `TextPolicy::Escape` 时调用）。
- 净化白名单两套且不一致，且**差异不止 span**：
  - 库：`src/output/html.rs:350-354`（标签集合更大）+ `:445-447`（放行 `align`/`valign`）+ `:386`（字符串扫描净化）+ span `1..=1000`；
  - 页面：`src/bin/web/index.html:3055-3067`（9 个标签）+ `:3068`（只有 `colspan`/`rowspan`）+ `:3069`（DOMParser 净化）+ `:3099-3104`（`n > 1 && n <= 64`）。

**目标设计（重写）**

1. `src/table/emitter.rs` 的私有 `escape_html` 删除，改调 `crate::output::html::escape_html`
   （`escape_markdown_cell` 保留，它转义的是 Markdown，语义不同）。
2. **白名单规范化**（评审二.9；**N2 已裁决为「规范文件 + 生成脚本，生成物入库」**）：
   新增一份**受版本控制的白名单规范文件** `assets/table_html_whitelist.json` 作为唯一权威，
   由生成脚本 `tools/generate-table-whitelist.mjs` 产出两份派生实现并**一并入库**：
   Rust 侧 `src/output/table_whitelist.rs`（`src/output/html.rs` 只消费它），
   JS 侧 `src/bin/web/index.html` 中由标记区（`// <generated:table-whitelist>` … `// </generated:table-whitelist>`）界定的清单。
   用 **fingerprint 测试**断言两份生成物与规范文件一致（而不是靠"记得同时改两边"）：
   测试从规范文件重算指纹，与两份生成物里的指纹比对，任一不一致即失败。
   （标记区注入 vs 独立 JS 文件这种生成细节可在实施时调整，但"规范文件是唯一权威 + 两份生成物 + 指纹断言"不变。）

   ```jsonc
   // assets/table_html_whitelist.json（示意）
   {
     "tags": ["table", "thead", "tbody", "tr", "td", "th", "b", "i", "em", "strong", "span", "p", "br"],
     "attrs": ["colspan", "rowspan"],
     "max_span": 64,
     "text_policy": "escape"
   }
   ```

3. 两份净化实现**保留**（纵深防御，§3），只共享规范。
4. 若按安全交集统一（推荐，Q2 已裁决 span ≤ 64），**必须**新增：标签边界测试、属性边界测试、
   `colspan=64/65`、`Rust 清洗一次后页面再清洗结果一致`、现有页面契约回归。

**行为预期（产品可见的收紧，必须列明）**

- 库净化会开始拒绝 `colspan=65..1000`，并且不再放行 `align`/`valign`（取决于最终规范）。
- `[推算]` 真实表格不会出现 >64 列，但这是产品可见变化，必须在 §9 记录。

**补充（审核点 5 · 前后对比证据与确认范围）**

上面的 `[推算]` 不再当作结论使用，已改由**入库的可复现证据**界定。收紧的产品影响因此
**只声明到已纳入测试的数据与模型**，不是“任意输入都不会变”：

- `src/output/html.rs` 把净化判据抽成 `SanitizerRules`：生产判据 `TABLE_RULES` 与测试用的
  `legacy_rules()`（旧引擎判据：`align`/`valign` 放行、span `1..=1000`）跑**同一份解析内核**
  `sanitize_table_html_with`。前/后输出之差只可能来自白名单本身，不掺入第二份实现的偏差。
- `whitelist_tightening_leaves_the_committed_corpus_unchanged`：仓库内全部跟踪的表格 HTML
  （`p17-table-real.json` 的 `html`/`table_html`、`p17-table-synthetic.json` 的 16 条、
  `table-golden/manifest.json` 的 `reference.html`）逐字节 before == after。
- `tests/fixtures/table-whitelist-before-after.json`：**变更前后实际 HTML 输出的入库对比产物**
  （真实捕获 + 黄金参考存完整文本，另存 4 条构造样本）；守卫用例
  `the_saved_before_after_record_is_current` 会重新加载语料、重算两侧输出并比对，产物陈旧即失败；
  重生成走 `#[ignore]` 探针 `whitelist_before_after_record_regeneration`。
- `the_before_after_comparison_is_discriminating`：钉住上述“零差异”不是恒真 —— 四种被收紧的
  取值在旧判据下必留、新判据下必丢，同时语料里真实出现过的合法 span 在两侧都保留。
- `the_golden_vocabulary_cannot_emit_a_removed_attribute`：把“真实表格没有超过 64 列”换成模型侧
  硬约束 —— 黄金产物的解码词表里 span token 取值 2..=20（≤ 新上界 64），词表内**没有**
  `align`/`valign` token，即被删属性模型根本发不出来。
- **分开验收**：页面渲染侧（`src/bin/web/index.html`）的改动方向与引擎侧相反（标签集 11→28、
  colspan 下界 `>1`→`>=1` 属**放宽**；`align`/`valign` 本来就不在页面白名单），它不经过 Rust 内核，
  因此不在本组用例范围内，由 fingerprint 三份对拍 + `node tools\check-page-result-contract.mjs`
  验收。“等价重构”与“产品决策”不再混在同一条结论里。

**验证**

```powershell
cargo test --all-targets
cargo test --bin rapidstruct --features serve
node tools\check-page-result-contract.mjs
```

**风险与回退**

- 风险：收紧属性白名单会让某些历史 `table_html` 被清洗掉 → 用"清洗前后差异清单"评估，先在测试里量化。
- 回退：规范文件里把被收紧的项加回（一处改动，两边同时生效）。

**测试清单**

- [ ] `escape_html` 合并前后的逐字符对照（`&<>"'`、NULL、非法 UTF-8 边界）
- [ ] fingerprint 测试：规范文件 → Rust 常量与页面清单一致
- [ ] span 边界：1 / 64 / 65 / 1000（页面与库判据一致）
- [ ] 双重净化幂等：Rust 清洗结果再经页面清洗不变
- [ ] （审核点 5）真实/黄金/合成语料 before == after；入库前后对比产物与当前语料、当前判据一致；
      构造样本 before != after；黄金词表发不出被删属性

---

#### P1-7 表格 JSON 视图（M-2） · 状态：**取消（测量后）**（评审二.8；量测结果见 §9）

**现状**（`[实测]`）`src/output/json.rs:174-227`：
`regions`（完整事实源，含 `table`）、顶层 `items`（`:178-186`，统一消费视图）、顶层 `tables`（`:203-227`，表格专用视图，10 字段）。
服务端页面、导出接口与 CLI 都在用它们。

**目标设计（重写：先测量，收益不明显就取消）**

1. **先测**：对真实页面（含大表格）测 payload 体积与 `serde_json` 序列化耗时。
   裁决补充（审核点 3）：投影重构的定义就是**保持输出字节不变**，因此不存在“改后字节”可比；
   判据落在**成本结构**上 —— 分别量视图构造时间、序列化时间、负载体积三项。
2. 若收益不明显（例如 < 5% 且绝对值 < 数毫秒）→ **取消本项**，只在本文登记测量结果。
   首次登记（`f713ded`）的样本是无表格页，审核点 3 判定其不能支撑表格场景结论；
   已补表格场景重测（入库夹具 + 保留探针，见 §9 审核修复行），重测后取消结论维持。
3. 若确实要改：
   - 保留 `regions` 为唯一事实源；`items`/`tables` 改为**从 `output.regions` 派生的投影**，不再各自遍历多个字段；
   - 这属于**独立的破坏性输出协议变更**（不是 P1 等价重构），需要自己的版本标记与对外说明；
   - 快照对拍必须证明字段集合与内容不变。

**行为预期**：等价重构时输出不变；破坏性变更时输出结构变化（需单列）。

**验证**：`tests/fixtures/serve/result-real-42-regions.json` 反序列化 → 再序列化 → 逐字节比对；
`src/bin/serve/tests.rs:7456 assert_result_satisfies_page_contract` 必须仍绿。

**测试清单**

- [x] 测量脚本/命令与结果（体积、耗时、成本结构）写进 §9（首次量测用临时 `#[ignore]` 探针，量测后已 `git checkout` 还原；审核点 3 的重测探针 + 两份表格夹具**已入库**，可重复跑）
- [x] 三通道快照对拍（与 P0-1 共用）：首次量测时 `json.rs` 未被触碰；重测批只在 `json.rs` 参加 `#[cfg(test)]` 探针，`to_output_json`/`to_output_items` 生产体零改动，快照护栏与 `assert_result_satisfies_page_contract` 保持绿
- [x] 若做投影重构：`items`/`tables` 与 `regions` 的一致性断言 —— **未做**：首次量测（无表格样本）命中取消判据；审核点 3 要求补表格场景重测，重测（真实捕获表页 + 8×100 格大 HTML 合成页）同样命中（大表页派生视图总增量 0.61–0.65ms，其中可被投影重构消掉的只有视图遍历本体 <0.4ms，投影保持输出字节不变），三视图按协议用途原样保留（数据见 §9）

---

#### P1-8 评测摘要工具三处合一（M-3/M-5 的周边） · 状态：**批准**

**现状**（`[审计]`）`digest_hex` 三份：`src/evaluation/formula/sampling.rs:46`、`src/evaluation/table/fixture.rs:539`
（`:240` 注释自认"口径与 formula 侧一致"）、`src/evaluation/table/run.rs:1520`（内联）。
**补充（审核点 4）**：迁移后实测发现第四处同形内联 —— `src/bin/formula_eval.rs` 的
`compare_with_reference`（报告字段 `reference_sha256`）。P1-8 批按"改动不超计划规模"的裁决
未动并如实登记为后续候选；审核点 4 判定它属计划范围内的生产摘要实现，已并入单点。

**目标设计**：`src/evaluation/digest.rs` 提供 `pub fn digest_hex(bytes: &[u8]) -> String`，四处改调。

**行为预期**：等价（同一算法、同一输出）。

**测试清单**：一条"多处调用同一函数"的旁证（对同一输入输出相同）+ 既有 fixture 测试不变；
审核点 4 追加 `formula_eval` 的报告摘要回归用例（预定答案向量 + 同一记录换字节写法必然换摘要）。

---

### 4.3 P2：编排重写（根因）

> **评审结论（三）**：`PageRoute` 方向是对的，但 `PageStage { name, premise, reason }` 过度抽象
> （当前只有一个 `CoordinatePremise::OriginalPage`，该实体没有实际信息量）。
> P2 只保留 `ValidatedRequest` + `PageRoute`；tile 与页面级阶段合并**暂缓**（§5）。

#### P2-1 `PageRoute` + `ValidatedRequest`：给"页面级组合"一个类型 · 状态：**重写**（评审三）

**现状**（`[实测]`）

```rust
// src/api.rs:299-313 —— 组合是并列字段
pub struct OcrRequest { input, roi, scale_hint, stages, preprocess, detection, recognition, output, formula, table }

// src/api.rs:319-352 —— 同一条坐标前提，两份逐字禁令，理由夹在中间注释里
if self.formula.enabled { /* roi 禁令 + tile 禁令 */ }
// 表格路由与公式路由共享同一条坐标前提，因此共享同一条约束。…
if self.table.enabled { /* 同样的两个禁令，文案不同 */ }

// src/ocr/pipeline/rapid_structure.rs:616-630 —— 顺序是 if 嵌套
if request.table.enabled { return self.recognize_with_table(request, policy); }
if request.formula.enabled { return self.recognize_with_formula(request, policy); }
self.recognize_text(request)

// :1091 / :1377 —— 正交性靠就地改写请求
request.formula = FormulaPolicy::default();       // 防递归
text_request.table = TablePolicy::default();      // 防递归
```

**目标设计（重写，去掉过度抽象）**

```rust
// src/api.rs
/// 本次请求实际要跑的页面级阶段。**顺序不在这里**（顺序由调度器按依赖决定）。
#[derive(Debug, Clone, Default)]
pub struct PageRoute {
    formula: Option<FormulaPolicy>,
    table: Option<TablePolicy>,
}

impl PageRoute {
    pub fn runs_formula(&self) -> bool;
    pub fn runs_table(&self) -> bool;
    pub fn formula(&self) -> Option<&FormulaPolicy>;
    pub fn table(&self) -> Option<&TablePolicy>;
}

/// 校验与路由的**唯一产物**：不可变的计划交给调度器，调度器不再看 `OcrRequest` 的策略字段。
#[derive(Debug, Clone)]
pub struct ValidatedRequest {
    request: OcrRequest,
    route: PageRoute,
}

impl ValidatedRequest {
    /// 一次完成：校验所有策略 → 推导路由 → 校验坐标前提（理由与禁令同处一地）。
    pub fn plan(request: OcrRequest) -> Result<Self>;
    pub fn request(&self) -> &OcrRequest;
    pub fn route(&self) -> &PageRoute;
}
```

- **删除** v1 的 `PageStage { name, premise, reason }` 与 `CoordinatePremise`（只有一个变体的实体没有信息量）。
- 禁令消息用**一个模板 + 每阶段自己的 `reason`**（模板统一，理由仍是各自原文），既消掉重复判据，又不丢"为什么"。
- `plan()` 内部对每个启用的阶段做同一件事：检查 `roi`/`preprocess.tile` 并给出该阶段的理由。

**调用链改法（调度器）**

```rust
// src/ocr/pipeline/rapid_structure.rs
impl crate::api::OcrEngine for RapidOcrEngine {
    fn recognize(&mut self, request: OcrRequest) -> Result<OcrOutput> {
        let planned = crate::api::ValidatedRequest::plan(request)?;   // 校验 + 路由，一次
        self.recognize_page(planned)
    }
}

impl RapidOcrEngine {
    /// **阶段顺序在整个 crate 里只出现这一次。**
    /// 表格在公式之外的理由（表格要吃"公式抹白之后"的页级文本行）写在这里。
    fn recognize_page(&mut self, planned: crate::api::ValidatedRequest) -> Result<OcrOutput> {
        let request = planned.request();
        let mut output = match planned.route().formula() {
            Some(policy) => self.recognize_text_with_formula(request, policy)?,
            None => self.recognize_text(request.clone())?,
        };
        if let Some(policy) = planned.route().table() {
            self.attach_table_regions(&mut output, request, policy)?;
        }
        output.validate()?;
        Ok(output)
    }
}
```

**调度器必须记录的四条语义（评审三：这是 P2 的核心回归，不能只测最终区域数量）**

| 语义 | 今天的行为 | 回归方式 |
| --- | --- | --- |
| 公式的**文本通道**用抹白图 | `src/formula/route.rs:176-206` 抹白后才跑文本 | 断言公式区域内文本为空/被移除，且区域外文本不变 |
| 表格的**定位与结构推理**用**原图** | `recognize_with_table` 先拿原图（`:1386`） | 断言表格检测结果与"原图"一致（对拍） |
| 公式与表格同时启用时，公式区域内的表格文本可能是 `Unmatched` | `src/api.rs:742-745` 与 `rapid_structure.rs:1357-1360` 的已知边界 | 断言这种组合下 `TableCellMatch::Unmatched` 的具体计数与区域（不是"能跑就行"） |
| `formula_ms` / `table_detect_ms` / `table_structure_infer_ms` / `table_backfill_ms` / `total_ms` 的守恒关系 | 今天由 `recognize_with_table` 的返回结构累加 | 断言字段集合与含义不变、两阶段耗时仍非零且相加关系成立 |

**行为预期**：产品输出等价；**两条防递归改写被删除**（不再有递归：每个阶段由调度器调用至多一次）。

**验证**：§8.2 全部门禁 + 上表四条语义的断言 + `serve` 的 M4 公式队列与 E16.2 表格路由两块用例。

**风险与回退**

- 风险 1：`roi`/`tile` 的禁令消息变化 → serve 里若有按文案断言的测试会红（**先 grep 文案**）。
- 风险 2：调度器从"互相调用"变成"顺序调用"后，任何依赖"子调用顺带做了什么"的隐含行为都会暴露 ——
  这是重构的主要价值，也是主要风险，故必须全门禁。
- 回退：保留 `recognize_with_table`/`recognize_with_formula` 两个私有函数，只把 `recognize` 换成调度器（半途回退点）。

**测试清单**

- [ ] `formula_routing_rejects_roi_and_tile` / `table_routing_rejects_roi_and_tile`（`rapid_structure.rs:2424`/`:3383`）改为断言新消息
- [ ] `PageRoute` 的 4 种组合（无 / 仅公式 / 仅表格 / 都有）× `roi`/`tile` 的矩阵测试（今天散落在 4 个用例里）
- [ ] 上表四条语义的断言（抹白 / 原图 / `Unmatched` / 计时守恒）
- [ ] serve：`src/bin/serve/tests.rs:2339`（M4 公式队列）、`:2504`（E16.2 表格路由）两块必须全绿
- [ ] 四象限输出快照（公式两态 × 表格两态）

---

#### P2-2 阶段函数改"追加"，删除防递归改写（A-2） · 状态：**重写**（与 P2-1 **同一批/同一次提交**）

**目标签名**

```rust
fn recognize_text_with_formula(&mut self, request: &OcrRequest, policy: &FormulaPolicy) -> Result<OcrOutput>;
fn attach_table_regions(&mut self, output: &mut OcrOutput, request: &OcrRequest, policy: &TablePolicy) -> Result<()>;
```

- `recognize_with_table` 这个名字消失：表格不再是"一条会自己先跑公式的路由"，而是"往已完成的文本输出上追加表格区域"。
- `recognize_with_formula` 只保留一层：抹白 → 文本 → 公式区域。
- 抹白仍**只作用于文本通道**，表格结构推理仍读原图 —— 与 P2-1 上表的第 1、2 条一致。

**测试清单**：与 P2-1 共用；另加"公式开关两态 × 表格开关两态"的四象限输出快照对比。

---

#### P2-3 tile 与页面级阶段合并 · 状态：**暂缓**（评审一.4）

**评审结论**：这不是简单的管线复用问题，而是**坐标空间与页面级语义**问题。
`recognize_tiled_request` 的子请求明确关闭公式与表格（`rapid_structure.rs:1994-2004`）。
若按 v1 的设计把 tile 接到同一个 `recognize_page`，会**同时**改变解码次数、坐标语义与区域合并行为，无法满足"行为等价"。

**本方案的范围**

1. 只做 `MaterializedInput`（P1-5）：统一"编码图像 → `RecImage`"的物化入口。
2. **保持 tile 与页面级公式/表格互斥**。
3. tile 路径的其余部分（独立管线形态）**本轮不动**。

**将来要合并时，必须先完成的设计（否则不开工）**

| 必须设计的东西 | 为什么 |
| --- | --- |
| 跨 tile 的公式/表格区域合并与去重 | 相邻 tile 会各自检出同一区域（overlap 区域） |
| 坐标回映 | tile 局部坐标 → 整页坐标的映射与 ROI 偏移 |
| 阶段计时 | 一次页面级阶段跨多个 tile 时的 `formula_ms`/`table_*_ms` 归属 |
| 区域边界语义 | 跨 tile 边界的表格会被切成两个不完整表 |

---

#### P2-4 验收指标：新增第四种识别要改几处 · 状态：**条件性**（仅在真的准备加阶段时执行）

**现状**（`[审计]` + `[实测]`）约 **14 个点、5 个文件族**：`src/api.rs` 6 点（`RegionKind:493`、`OcrRegion`/`validate:828-843`/`:1096-1145`、
`OcrRequest:300-313`、`roi`/`tile:319-352`、`StageReports`/`OcrTimings:918-959`）；
`rapid_structure.rs` 4 点（分派 `:616-630`、新 `recognize_with_X`、会话访问器、区域构造 `:1135`/`:1473`、计时 `:1120`）；
`model_set.rs` 3 点；`src/bin/serve` 5 点；`src/output` 3 文件。

**目标**：P2 之后应降到 **≤ 6 点**（每个概念恰好一处）：

| 概念 | 只在一处 |
| --- | --- |
| 区域类型 | `RegionKind` + `OcrRegion` payload + `validate` 臂（`src/api.rs`） |
| 路由与坐标前提 | `PageRoute` 的启用判据（`src/api.rs`） |
| 阶段顺序 | `recognize_page` 的调度体（`rapid_structure.rs`） |
| 模型角色 | `ModelRole` + `model_plan` 分组（`model_set.rs` / serve） |
| 输出分派 | `to_output_items` 的一个新臂（`json.rs`） |
| 计时字段 | `OcrTimings` 一个字段（`api.rs`） |

**验收方法（可证伪、不需要真的实现第四种识别）**：做一次**空阶段演练** —— 临时加一个返回零区域的桩阶段，
走完上面 6 处，确认能编译、`cargo test --all-targets` 全绿、然后删除桩；演练的实际改动点清单写进 §9。
**只有这份清单 ≤ 6 点，P2-4 才算达成。** 评审未反对这个指标本身，但它不是本轮必做的实施项。

---

## 5 明确不做的事 / 暂缓的事（附理由）

| 不做 / 暂缓 | 理由 |
| --- | --- |
| **tile 与页面级阶段的合并**（原 P2-3 的主体） | 评审一.4：会同时改变解码次数、坐标语义与区域合并；前置设计清单见 §4.3 P2-3 |
| **"解一次"（`raw_dynamic` 走 `RecImage` 转换）** | 像素来源改变，须先做逐像素对拍；`docs/04` 记录过 `image` vs turbojpeg 的输出差异 |
| **几何层全量合并（polygon IoU / NMS / covered_ratio）** | 评审二.5：语义与边界不同，只有真实数据能证明同口径 |
| **det AVX2 归一化与 table 归一化的合并** | 评审二.6：差异不止公式，需逐像素对拍后再定 |
| **JSON 视图重构（原 P1-7 的默认动作）** | 评审二.8：三种视图是协议用途；先测量，收益不明显则取消 |
| 合并输入表示双轨（`RecImage` BGR vs `DynamicImage` RGB，D-4） | 影响面最大（`formula/*` 全部裁图与预处理、`input/` 两条加载出口）；收益是整洁而非正确性 |
| 计时原语重构（`Timer`/`Span`，M-3） | 70 处 `Instant::now()`；触发器应是"公式/表格独立评测需要守恒校验"这个具体需求 |
| fixture 基建统一（M-4） | 收益局限在测试内部；`serve/*` 的假模型/HTTP 基建与 `test_support` 取向不同 |
| `formula_eval.rs` 骨架下沉（M-5） | 等"公式评测需要在会话内复用"的需求出现。**不要为了对称而重构** |
| 合并 §3 登记的任何一项 | 那些重复都有明确理由（含评审一.3 澄清的 `registration` 分层） |
| 把校验缓存改为全文件校验 / 每次状态查询都强制重验 | **复审 P1/P2 裁决**：566 MB 模型在页面 8 s 轮询下每次重读是不可接受的代价（冷验证实测值就记在 `/api/models` 的 `verification` 块里）。改走第三条：把口径写清楚（它是性能启发式，不是完整性保证）+ 加载期证伪收敛（`VerifiedModel::verify` 作废被磁盘推翻的条目）。要确定性结论仍只有 `--reverify-models` 与 `POST /api/models/reverify` 两个入口（记述见 §9） |
| 把状态升级为实时保证，或把“禁止越界措辞”留给人复查 | **复审 P1 残余风险的裁决（第四轮）**：中段修改仍属启发式盲区，本批**不**升级为实时保证（代价见上一行），而是接受这条**已声明**的限制；同时把“不得把 `present` 或缓存命中写成已核实当前磁盘内容”从口头纪律改成机器判据 `tools/check_model_state_claims.ps1`（句子级判据 + 必须句 + `-SelfTest` 自证咬合力），已纳入 §8.1 |
| 引入第四种识别 / 版面模型 / 方向分类 | 用户已裁决：当前不再新增功能（`m01287`） |
| 重建 `tests/baseline/windows-baseline/*.json` | 用户已裁定不重建（`docs/11` §4.12 + §7.1 Q5）；P0-2 只在引用点补"含分位口径变化" |

---

## 6 批次顺序、依赖与规模（按评审五重排）

| 批次 | 项 | 依赖 | 触达（`[推算]`） | 判据 |
| --- | --- | --- | --- | --- |
| **1** | P0-0 基线契约固化（键集合 / 标记 / 小型 fixture / 模型摘要清单） | — | 测试 + `tests/fixtures/` + 3 个报告 | 三份键集合测试绿；小型 fixture 入库 |
| **2** | P0-1 死代码清理（含测试迁移） | 1 | `output/markdown.rs`、`output/json.rs`、`formula/output.rs`、`exports.rs` | `-D warnings` 0；三通道快照不变；上表 14 条用例已迁移 |
| **3** | P0-2 分位口径统一（保持报告结构 + 适配器） | 1 | `stats.rs` + 2 个 bin + 1 个 run.rs | 对拍绿；公式报告结构不变；标记就位 |
| **4** | P0-3 模型凭证（`VerifiedModel`） | — | `model_verify.rs`、`ocr/session.rs`、`formula/{session,recognizer,detect}.rs`、`table/structure.rs` | 三域拒绝用例绿；`registration` 14 条不变 |
| **5** | P2-1 + P2-2 `ValidatedRequest` + `PageRoute`（删防递归改写） | 1–4 | `api.rs`、`rapid_structure.rs` | 四象限快照 + 四条阶段语义 + serve 两块用例 |
| **6** | P1-5 输入物化入口（保持两次解码） | 5 | `rapid_structure.rs`（3 段合一） | 五变体用例 + "解码次数仍为 2"断言 |
| **7** | 之后才评估：P1-1 几何基础函数 / P1-3 rec·cls LUT / P1-4 页文本行 / P1-6 白名单规范 / P1-7 JSON 体积测量 | 1–6 | 见各节 | 每项自己的对拍/测量判据 |
| **随时** | P0-4 双开测量（只测量） | — | 测量设施（可能零产品改动） | 测量结果 + 结论（做/不做）写入 §9 |
| **随时** | P1-2 `LazyModel` | P0-3（共用文案单点） | `rapid_structure.rs` + 新模块 | 重建只发生一次 |
| **随时** | P1-8 `digest_hex` 三处合一 | — | `evaluation/{digest.rs,formula/sampling.rs,table/fixture.rs,table/run.rs}` | 输出一致 |
| **条件性** | P2-4 空阶段演练 | P2-1/P2-2 | 仅临时桩 + §9 记录 | 改动点清单 ≤ 6 |

**每批 = 一次提交**，提交信息里带 §8.3 的门禁输出摘要。**不允许两批混在一次提交里** —— 否则出问题时无法二分定位。
批 5 与批 4 的间隔建议留一次完整门禁（它是本轮唯一的产品输出结构改动）。

---

## 7 裁决与待确认

### 7.1 已裁决（评审 `m01562`）

| 编号 | 问题 | 裁决 |
| --- | --- | --- |
| Q1 | P0-1：删除生产零调用的公开函数，还是保留薄包装？ | **删除**生产零调用 API，但**保留其语义测试**并迁移到实际输出函数（不允许删测试换 `-D warnings` 通过） |
| Q2 | 净化白名单的 `span` 上限统一到哪个值？ | 统一到 **64**，前提是**先完成双实现的规范化测试**（规范文件 + 生成 + fingerprint） |
| Q3 | P0-3 之后要不要在 `/api/models` 暴露 `sha256_verified`？ | **不新增**：现有 `declared_sha256`（`src/bin/serve/model_plan.rs:146`）+ 实际摘要 + 验证状态已足够 |
| Q4 | P0-4 的判据接受"测量后可结论为不做"吗？ | **接受**（判据见 §4.1 P0-4 的 1% 阈值） |
| Q5 | `tests/baseline/windows-baseline/*.json` 是否保持原样？ | **保持现有入库基线、不重建**；另加**统计方法说明**（`percentile_method: "linear"`，见 §7.2 N1） |
| Q6 | 回归门禁依赖未入库 `datasets/`，是否把 4 张回归图裁小后入库？ | **保留本地全量门禁**，**同时增加小型入库回归 fixture**（P0-0 第 3 项） |

### 7.2 实现细节裁决（N1–N3 · 已按建议定案）

用户 `m01664` 采纳建议，三处不再保留备选：

| 编号 | 问题 | 裁决 | 理由 |
| --- | --- | --- | --- |
| N1 | 统计方法标记的字段名与取值 | **`percentile_method: "linear"`** | 标记的是"分位怎么算"这一事实本身；整数 schema 版本多一层间接，且在只有一种算法的今天无法自证 |
| N2 | 白名单规范的载体与生成方式 | **`assets/table_html_whitelist.json` 为唯一权威 + `tools/generate-table-whitelist.mjs` 生成 Rust/JS 两份（生成物入库）** | 评审二.9 已否定"Rust 常量当权威"（`const` 不会进入内嵌 JS）；规范文件入库才可审计、可 review、可指纹断言 |
| N3 | `VerifiedModel` 的落脚模块 | **`src/model_verify.rs` 内**（与 `verify_sha256`、`FileIdentity` 同处一室） | 凭证是校验的产物，放一起才能保证"校验只有一个实现"；若将来凭证要携带 role/provenance 等域信息，再拆独立模块 |

---

## 8 验证与回归门禁（v2 重写）

### 8.1 命令清单（每批都要跑）

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --features serve -- -D warnings
cargo test --all-targets
cargo test --bin rapidstruct --features serve
cargo test --features serve --test serve_startup
node tools\check-page-result-contract.mjs
# 声明口径守卫（复审 P1 残余风险）：先自证判据仍会咬人，再扫全仓库措辞
powershell -NoProfile -ExecutionPolicy Bypass -File tools\check_model_state_claims.ps1 -SelfTest
powershell -NoProfile -ExecutionPolicy Bypass -File tools\check_model_state_claims.ps1
```

### 8.2 两类基线资产：**已入库的契约** vs **本机质量门禁**

**（a）已入库、可直接复用的契约资产**

| 资产 | 值 / 位置 | 用途 |
| --- | --- | --- |
| serve 页面契约 | `tests/fixtures/serve/result-real-42-regions.json`，`BODY_SHA256 = fda2f713…`（钉点 `src/bin/serve/tests.rs:7647`、`tools/check-page-result-contract.mjs:39`） | 页面读取路径没变 |
| Windows 基准 JSON | `tests/baseline/windows-baseline/*.json`（**入库基线 = `0.5834044577250798`**，表格域） | 历史可比性（**不重建**，Q5） |
| 表格 golden | `tests/fixtures/table-golden/`（含 `input_bgr_resized.bin`） | 表格推理逐像素回归 |
| 页级延迟预算 | `docs/07` R11；`table_structure_budget = 120 ms`（`docs/11` §2.5） | 性能退化判定 |
| 小型回归 fixture | **P0-0 新增**（4 张图裁小后入库 + 期望字符多重集） | "没丢文字"不依赖 `datasets/` |

**（b）本机质量门禁（依赖未入库数据，不得作为跨机器硬门禁）**

| 资产 | 值 | 说明 |
| --- | --- | --- |
| 四图字符多重集 | real 23 行 / region61 26 / region62 22 / table.jpg 10（**只增不减**） | 依赖 `datasets/text/`、`datasets/table/`（未入库） |
| 全量 TEDS（7,104 样本） | E25 实测 `0.5836405337617874` | `[实测]` 但**产物在 `Temp/`（未入库）**；入库基线仍是 `0.5834044577250798` |
| 预测确定性 | 同一二进制连跑两次预测摘要逐字节相同（`ee528ea737e909fd`，3,744,387 B） | 判定差异来自代码而非噪声 |

**关于 `0.583640…`（评审四）**：若将来要把它作为**跨机器**的硬门禁，**必须先提交**：
对应预测摘要（`ee528ea737e909fd`）、数据版本（`pubtabnet_val_gt_html.json` 的摘要）、模型摘要（`slanet-plus.onnx` 的 SHA-256）、
运行配置（`--content page-reuse --ocr-config … --intra-threads 14`）。在此之前，它只是**本机证据**。

**本文自身**：`docs/12-structure-refactor-plan.md` 目前**未跟踪** → 实施第一批时应与代码一起入库，否则方案与实现会脱节。

### 8.3 每批的通过判据

| 批次 | 通过判据 |
| --- | --- |
| 1（P0-0） | 8.1 全绿；三份报告键集合测试绿；小型回归 fixture 入库且字符多重集断言绿 |
| 2（P0-1） | 8.1 全绿；三通道快照逐字节不变；14 条用例已迁移；`exports.rs` 无回潮 |
| 3（P0-2） | 8.1 全绿；分位对拍绿；**公式评测报告结构不变**；`percentile_method: "linear"` 标记就位（N1） |
| 4（P0-3） | 8.1 全绿；三域拒绝用例绿；`registration` 14 条不变；真实配置冒烟通过 |
| 5（P2-1/2-2） | 8.1 全绿；四象限快照；四条阶段语义断言；serve M4/E16.2 两块用例；本机 TEDS 不低于 `0.5836405337617874` |
| 6（P1-5） | 8.1 全绿；五变体用例；"解码次数仍为 2"断言 |
| 7（评估类） | 每项自己的对拍/测量判据 + 8.1；P1-6 另需规范文件指纹与两份生成物一致（N2） |
| 随时（P0-4） | 测量结果与结论写入 §9（可结论为"不做"） |

---

## 9 实施记录（留待实施时填写）

| 批次 | 提交 | 命令 | 关键结果 | 基线对比 | 门禁判定 | 未覆盖风险 |
| --- | --- | --- | --- | --- | --- | --- |
| 7 · P1-1 | `61cffe8` | `vision/geometry` 几何基础函数单点化 + 逐处对拍委托 | aabb/quad 几何函数收敛单点；各调用点逐位对拍 | 三通道快照 + `--all-targets` | §8.1 全绿 | det AVX2 归一化路径未动 |
| 7 · P1-3 | `421e400` | `vision/normalize` signed-unit LUT 单点化 | 0..=255 全域 LUT 含符号位对拍、端点/无 -0.0/单例稳定 | lib 722 绿 | §8.1/§8.2 全绿 | table ImageNet 归一化按裁决暂缓 |
| 7 · P1-4 | `819f6d0` | `PageTextLine` 规范视图 + 装配点单点转换（第一批） | bbox 唯一生成点 = `aabb_from_flat8`，与旧逐位一致；旧结构原样保留 | lib 724、serve 串行 289 | §8.1/§8.2 全绿 | 删旧结构属第二批，本批未做 |
| 7 · P1-6 | `bc4dbb6` | 权威 `assets/table_html_whitelist.json` + 生成器 + fingerprint 三份对拍 | **产品可见收紧（§4.2 行 864 要求登记）**：标签统一 28（页面 11→28）、移除 align/valign、引擎 span 1000→64、页面 colspan 下界 >1→>=1；escape 全链单点 | lib 728、serve 串行 289、serve_startup、node 契约 | §8.1/§8.2 + `generate-table-whitelist.mjs --check` 全绿 | 捕获的 42 区域页面 fixture 不含表格，大表格双重净化路径未由真实捕获体覆盖（由构造用例覆盖）；当时只给了“新规则生效”的边界测试，**没有**变更前后对比（审核点 5，见下一行） |
| 7 · P1-6（审核点 5 补证） | 审核修复批 | 净化判据抽成 `SanitizerRules`（生产 `TABLE_RULES`）+ `sanitize_table_html_with` 内核，测试侧 `legacy_rules()` 复用同一内核复现收紧前判据；新增 4 条用例 + 入库对比产物 `tests/fixtures/table-whitelist-before-after.json` + `#[ignore]` 重生成探针 | 真实捕获（`p17-table-real.json` 的 `html` 341B / `table_html` 315B）+ 黄金参考（`reference.html` 拼接 920B）+ 合成大表 16 条均 before == after；产物存 7 条记录（3 unchanged + 4 tightened，构造样本），守卫用例会重算比对，陈旧即失败；黄金词表 span token 取值 2..=20（≤64）且无 `align`/`valign` token | 迁移前的“前后”由旧判据在同一内核上重算得出（与 `bc4dbb6^` 的 `sanitize_attrs` 逐字比对过判据），不是重新跑旧版本二进制 | §8.1 全绿（lib 738 passed + 2 ignored、serve 并行 289/串行 289、serve_startup、两个 node 检；首跑 clippy 报 `collapsible_if`，改 match guard 后全套重跑） | 确认范围只到**已纳入测试的数据与模型**（上述三份仓库内语料 + 黄金词表），不是“任意输入都不会变”；页面 JS 侧改动（标签集 11→28、colspan 下界放宽）不经过本内核，本组用例不覆盖，另由 fingerprint 对拍 + node 契约检验收 |
| 7 · P1-7 | 无代码改动（取消） | 临时 `#[ignore]` 探针 `cargo test --lib --features serve p17_measure_json_view_payload_and_time -- --ignored --nocapture`，量测后 `git checkout` 还原 | debug min-of-300：`to_value`=232µs、`to_output_json`=728µs、视图构造增量 **496µs（0.496ms）**；`result-real-42-regions.json` 派生视图占 full 30366B 的 **39.4%**（items 10340B + text 1582B 复制 regions；此页无表格、formulas 空） | 新量测（无改前/改后对比：投影重构保持输出字节不变） | **结论：取消投影重构** —— 收益 <0.5ms（debug 最坏值，release 更低）；【复审 P2-4 就地更正】原写的“且相对完整序列化路径 <5%”是人工推算、分母口径不一致，按记述里声明的分母 `to_output_json` 重测本机为 46.55%（真实页）/12.96%（大表页）；取消结论改立在“字节冗余只能靠破坏性协议变更消除 + 时间收益仅覆盖 JSON 构造一个步骤”两条前提上（见下面两行）；三视图按协议用途原样保留 | payload 39.4% 冗余真实存在，但删 items/text 属**破坏性输出协议变更**（评审二.8 排除出 P1，需独立版本标记与对外说明），另议 |
| 7 · P1-7（审核点 3 重测） | 审核修复批 | 保留探针（`#[ignore]`，不再还原）+ 入库夹具 `tests/fixtures/json-output/p17-table-{real,synthetic}.json`：`cargo test --lib --features serve p17_measure_json_table_views_payload_and_time -- --ignored --nocapture`；合成夹具由 `Temp/p17_build_fixtures.py` 构造（真实表 region 为模板，未入库） | debug min-of-300×3 轮一致：**真实捕获表页**（74 regions、1 张 10 格表，full 36560B/源 25733B，派生视图 29.6%）`to_value`=253–279µs、`to_output_json`=707–713µs、构造增量 **427–460µs**；**合成大表页**（8 张 100 格表、每表 HTML+table_html ≈63KB，full 1.44MB/源 667KB，派生视图 54%）`to_value`=2.134–2.140ms、`to_output_json`=2.753–2.792ms、构造增量仅 **613–652µs（占完整路径 22–24%；【复审 P2-4 就地更正】原写的“可被重构消掉的视图遍历本体只占 2.2–5.9%”为人工推算且分母口径不一致，按声明分母 `to_output_json` 重测本机为 46.55%/12.96%）**，items/text/tables 视图本体构造 16/133/126µs | 旧测量（无表格页 496µs）不能代表表格场景；新测量分列成本结构后：可被投影重构消掉的只有视图遍历本体（大表页 16–133µs/视图，最坏合计仍 <0.4ms debug），而派生负载自身的序列化（把 items 264KB + tables 508KB 再走一遍 `to_value`）在**大表页**里占增量大头（剩余 492µs vs 本体 403µs），真实页则相反（本体 335µs vs 剩余 103µs），且字节不变约束下重复字节无法消 | §8.1 全套通过（fmt/clippy/`--all-targets` lib 734+serve 289/串行/serve_startup/两个 node 检）；本批另修一处 serve 顺序敏感断言（见下一行），修前串行首跑在该用例失败 | **重判定：取消结论维持**，判据改为成本结构而非前后字节对比；合成夹具是模板放大的压力样本非真实页面，其绝对耗时随机器与 CPU 负载浮动；协议变更（删 items/tables 重复字段）仍按裁决排除在重构外，另议；临时构造脚本在 `Temp/` 未入库，入库资产为两份夹具 + 探针本身 |
| 门禁可信性（审核清单外，用户批准） | 审核修复批 | `src/bin/serve/tests.rs`：`Scripted.tables` 由 `Vec<TablePolicy>` 改 `Vec<(String, TablePolicy)>`，请求体与策略在 `recognize` 的**同一次加锁**内成对记录；`the_table_stage_runs_from_the_model_set_when_requested` 的 `policies[0]/[1]/[2]/[3]` 下标断言改为按请求体（`plain`/`text+table`/`formula+table`/`explicit off`）取策略，并保留“恰好两次 enabled”的计数断言 | 根因：记录顺序是**后端进入顺序**，多 worker 下不等于提交顺序；`?table=1` 落在哪一格随机，故下标断言偶发失败（既可在并行复现，**也可在 `--test-threads=1` 复现** —— 推翻此前“串行恒绿”的登记）。改后断言强度不降反升：`table=0` 被当开、或公式队列漏接 `table=1` 都会被具体请求体钉住 | 改前该用例在 `--test-threads=1` 下首跑即失败（推翻此前“串行恒绿”的登记），而隔离复跑 12 次 0 失败 → 说明是顺序竞态而非逻辑回归；本批生产代码字节不变，故无输出/性能前后对比 | 单测隔离复跑 10/10 通过（改后）；随后 §8.1 全套七项通过 | `formulas()` 上存在同类下标断言（同文件 `the_formula_queue_runs_…`，两队列各一单，尚未复现失败），本批按最小改动原则**未动**，登记为后续候选；修复手段是脚本化最小替换 + `git diff` 逐字节核验（该文件禁止用编辑工具批量改）。**该候选已由复审 P2-3 闭合**，见后面的“公式队列去顺序依赖”行 |
| 随时 · P1-8 | （本批） | `evaluation/digest.rs` 单点 `digest_hex`，三处改调（含 run 侧第二处同形内联 engine_fingerprint） | 行为等价；已知答案向量 + digest_file 一致性旁证；sampling/fixture 既有 manifest 测试不变 | lib 730（新增 2 条旁证）、serve 串行 289、全套件测试通过（r6 回正：当时写的是“全套件无回归”） | §8.1/§8.2 全绿（本次提交） | `bin/formula_eval:1134` 是审计未列入的**第四处生产拷贝**（bin 链接公开 `evaluation::digest` 可达），本批按"不超规模"裁决未动，后续候选（**审核点 4 已并入，见下一行**）；测试内的 sha256_hex 参照实现（model_store/json/formula-preprocess golden）有意不合并 |
| 随时 · P1-8（审核点 4 补齐） | 审核修复批 | `src/bin/formula_eval.rs`：`compare_with_reference` 的内联 `Sha256` 块改调 `evaluation::digest::digest_hex`（并删去局部 `use sha2::{Digest, Sha256}`）；新增用例 `the_reference_digest_goes_through_the_single_digest_helper` | 语义比对逐字同形（同算法、`format!("{:x}")` 小写 64 字符、同一 `raw.as_bytes()` 输入），报告字节不变；用例同侧钉住：① 两个预定答案向量（与任意 SHA-256 实现可重算）；② 同一记录换键顺序/空白写法 → 摘要必变而记录条数不变（证明钉的是内容不是路径）；③ 与单点 `digest_hex` 逐字相等 | 迁移前该字段由本地内联产生（已用 `git show` 比对旧实现确认算法/编码一致）；新用例直接跑 `compare_with_reference`，不依赖模型资产 | §8.1 全套（本批提交） | 分片合并的 `same_source()` 判据本身已有用例（`reference_comparison_aggregates_across_shards`），本批不重复；测试内的 `TempGuard` 是 bin 局部最小临时目录工具（库内 `test_support` 是 `#[cfg(test)]` 私有，bin 不可用）；M-5（`formula_eval.rs` 骨架下沉）仍按 §5 暂缓 |
| 会话缓存键 · 复审 P1-1 | 复审修复批 | `ocr/pipeline/rapid_structure.rs`：三个懒加载会话的键由 `FileIdentity` 改为 `SessionCacheKey { identity, declared }`，重建判定收口到唯一判据 `needs_rebuild`；三处构造点（公式识别器/公式检测器/表格会话）同步。`model_verify.rs` 新增 `normalize_expected_sha256` 单点，`api.rs` 策略文档补写“跨请求也成立” | 原契约漏洞：声明只在**重建分支**被读，所以“热会话 + 同一路径 + 换一个错误摘要”会让新声明静默通过（审核给的三步复现序列在旧实现下不会报错）；声明进入键后即按这次的声明重校。`declared` 经规范化只折叠 `verify` 本来就忽略的大小写；trim/去前缀会把两个**不同判定**折叠成同一个键，等于保留同一类洞 | lib 738 → 746（新增 8 条 = `model_verify` 规范化同形 1 + 无模型判据 4 + 需真实模型的端到端 3）、serve 并行/串行 289 | §8.1 全套七项实跑全 0（本批提交：fmt / clippy `--all-targets --features serve -D warnings` / `--all-targets` / serve 串行 / serve_startup / 两个 node 检） | 判别力：把 `needs_rebuild` 改回“只看身份”后 5 条新用例**全部失败**（已实跑后精确还原）。残余盲点不变：体积/mtime 写回原值的**中段修改**仍属启发式盲区（§4.2 已登记），本批不把它描述成安全边界；3 条端到端用例缺资产时提前返回（静默跳过），故判据另由 4 条无模型守卫钉住 |
| 公式队列去顺序依赖 · 复审 P2-3 | 复审修复批 | `src/bin/serve/tests.rs`：`Scripted.formulas` 由 `Vec<FormulaPolicy>` 改为 `Vec<(String, FormulaPolicy)>`（请求体与策略成对记录，与 `tables` 同一手法），新增 `formula_policy(body)`；M4 用例 `the_formula_queue_runs_the_formula_pipeline_from_the_model_set` 的 `policies[0]/[1]` 改为按请求体（`text image` / `formula image`）取策略。脚本化最小替换（4 处均断言命中次数 == 1）+ `git diff` 逐字节核验 —— 该文件工作树是 CRLF，禁止用编辑工具批量改 | 审核点：上一条行登记过“同类风险仍在”。`recognize` 的记录顺序是**后端进入顺序**，不等于提交顺序，所以下标断言既可能偶发失败，也可能把策略关联到错误的请求（后者更阴：断言依旧全绿但钉的是另一件事）。`formula_policy` 找不到对应请求体时直接 panic，所以“漏跑”也算回归 | 判别力三组（均实跑后还原）：① 把记录改成 `insert(0, …)`（进入顺序倒转）→ 新断言仍通过；② 同一倒转下改用下标断言 → 在 `!text_policy.enabled` 处 FAILED（证明旧断言确实依赖顺序，不是理论风险）；③ 把记录的请求体换成错误值 → panic `no formula policy recorded for request body "text image"`（证明关联是活的）。另：单测隔离复跑 10/10 通过 | lib 746 不变，serve 并行/串行仍 289（本轮无新增用例，只把两条断言换锐）；§8.1 全套（本批提交） | 本批改的是**测试证据的可靠性**，不是生产逻辑：没有证据表明公式队列本身错位。serve 侧其余顺序敏感断言已逐行扫过（`served` 只用于说明，不按 `served[n]` 断言），全文件不再有 `policies\[\d\]` |
| 不经凭证的会话入口 · 复审 P1-2 | 复审修复批 | `runtime/session.rs`：`pub fn open_unchecked` → `#[cfg(test)] pub(crate) fn open_unchecked`；`exports.rs` 写明公开面上只剩 `open_verified`；`config.rs` 的 `effective_session_threads` 文档去掉指向它的 intra-doc 链接；新增 `tests/session_open_surface_guard.rs`（4 条）；更正 §10 里 `src/ocr/session.rs` 那一行已过期的结论 | 审核点：生产域会话虽全走 `open_verified`，但外部调用方仍可直接拿 `OrtSession::open_unchecked(path, …)` 整条绕过完整性边界，所以 P0-3 二阶段写的“结构上不存在忘记校验的旁路”当时并不成立。开发期无兼容约束，按建议从公开面拿掉，而不是把结论降级为“域会话入口不可绕过”。编译器保证两件事（crate 外拿不到 + 非测试构建里不存在），守卫保证第三件事（不许改回公开） | 判别力（实跑后精确还原）：改回 `pub` → 守卫 ①② 双失败；删 `#[cfg(test)]` 门 → ① 失败。`cargo check --all-targets` 能过 = 非测试目标里没人调用它（守卫 ② 的编译期对拍）。连带修正既有守卫 `every_run_entrance_takes_an_exclusive_borrow` 的分界：它拿“第一个 cfg 测试门”当生产/测试边界，新增的门恰好在前 → `--all-targets` 报“`.run(inputs` 必须恰好两处”失败（门禁抓到）；改成“门 + 紧跟模块声明”后覆盖反而变强 | lib 仍 746 passed + 2 ignored；新增集成测试目标 `session_open_surface_guard`（4 条，不需模型）；§8.1 全套（本批提交） | 守卫是源码扫描（与 `exports_dead_code_guard`、`serve/mod.rs` 的 `dependency_boundary` 同族，不引入 trybuild 类依赖）：它钉的是名字与形状，不是链接器视图；若彻底删掉 `open_unchecked`，本守卫与它共存亡。不改变：文件身份仍是启发式，中段修改盲点照旧（§4.2） |
| P1-7 判据入库为结构化记述 · 复审 P2-4 | 复审修复批 | 新增入库记述 `tests/fixtures/json-output/p17-cost-record.json`（由探针在 `RAPID_STRUCTURE_P17_WRITE_RECORD=1` 时写出，包含方法/判据/两份样本的字节分项与时间分项与重算字段）+ 非 ignored 守卫用例 `the_saved_p17_cost_record_is_recomputable_and_self_consistent`；探针改为分列打印并直接生成同一记述（min-of-300×3 轮取最小） | 复审点：判据原来只靠人工解释一段打印输出，“取消”不是机器强制的结论。守卫**不重跑计时**（绝对耗时阈值保持机器相关，钉在 CI 里就是伪判据），只钉四件事：① 记述声明的样本必须存在且仍能加载、结构计数与记述一致；② 字节分项在当前代码上重算逐项相等（输出协议一飘就红），且 `full − source = views_total + key_overhead` 必须成立；③ 算术自洽（增量 = 全路径 − 基础路径、本体 = 四视图之和、**本体 ≤ 增量**（这是每条记录的算术要求，故无条件断言而不挂在结论分支下）、占比 = 本体/声明分母）；④ “取消”结论的前提在数据里成立（字节冗余非破坏性不可消、未声明机器无关绝对阈值） | 判别力 9 组变异（均实跑后还原，还原后记录文件与改前逐字节一致）：改 `views_total`±1（→ `json.rs:1134`）/ 改 `increment_ns`（→ 1157）/ 改单个视图时间（→ 1162）/ 把四个视图时间一起抬过整体增量、同时保持“本体 = 四视图之和”自洽（→ 1167，即无条件的 `本体 ≤ 增量`）/ 把夹具名指向不存在的文件（→ 879）/ 改走占比分母（→ 1077）/ 删一个判据字段（→ 1052）/ 把“非破坏性可消冗余”改成 true（→ 1190）/ 改结构计数（→ 1103）→ 守卫**全部失败**；还原后通过。首跑时旧记述（带着已废弃的 `eliminable_share_threshold_percent`、缺少两个前提字段）当场被判据块字段集断言拦住（`json.rs` 里那条 `assert_eq!(judgement_keys, …)`），证明 schema 漂移不会漏 | lib 746 → 747（新增 1 条非 ignored 守卫），`#[ignore]` 仍 2 条；serve 并行/串行仍 289；§8.1 全套七项实跑全 0（本批提交） | 本机入库数字：视图本体 0.336ms（真实页）/0.403ms（大表页），占 `to_output_json` 的 46.55%/12.96%，绝对值随机器与负载浮动；**本批没有重测 JSON 构造在整页请求里的占比**（仓库内无可引用的端到端 bench），所以取消结论只建立在“字节冗余需破坏协议才能消 + 时间收益仅限这一个构造步骤”上；若日后证明该步骤在端到端里可观，应重新评估 |
| 记录回正（审核结论末段 · r6） | 审核修复批 | `log.txt`（本地总结，未跟踪）：标题"提交总览（12 次）"与实际 13 行的矛盾 → 按列表计数并补入 r1–r5（共 18 次）；"serve 289 恒绿，零回归" → 拆成可复现表述；"计划内全部任务完成" → 限定为修订后清单；P1-6 的 `[推算]` 与"4 项收紧" → 指向 `1fbdb6d` 的前后对比证据 + "引擎收紧/页面放宽"分开验收；新增"审核修复批 r1–r5"一节逐条列事实与未修边界 | 逐条与 `git log`、§9 各行、CHANGELOG 对拍；测试数演进 712→…→738（lib 738 passed + 2 ignored、serve 并行/串行 289、`formula_eval` 15） | 回正前的写法是“提交总览（12 次）”配 13 行列表，以及“serve 289 恒绿，零回归”“计划内全部任务完成”“全程未发生门禁失败”；回正后按实际列表计数（当时 18 次），并把每一句换成具体命令 + 结果 + 实际出现过的失败 | 本批仅改文档（含未跟踪的 log.txt），源码与 `1fbdb6d` 字节一致，未重跑全套七项；只跑 fmt --check 与两个 node 检 | "零回归"与"串行恒绿"这两个措辞在任何跟踪文档里都不再出现；"确认范围 = 已纳入测试的数据与模型"已写进 §4.2/§9 与本行 |
| 记录回正 · 复审末段（不可绕过措辞与缓存盲点） | 复审修复批 | 无行为改动，只收窄措辞：① `src/model_verify.rs` 的 `FileIdentity` 文档把“相等即‘缓存里的摘要仍然描述这个文件’”改为“相等 = **没有观察到变化**”，并写明中段修改盲区与“要钉内容必须走 `VerifiedModel` 的字节快照”；② `rapid_structure.rs` 的 `SessionCacheKey` 文档新增一节“键里的文件身份是启发式，不是安全边界”；③ §5 P0-3 的“结构上不存在忘记校验的旁路”与 §5 P1-2（`LazyModel`）的“同一身份判据”两段加就地标注，§10 前言的已标注清单同步；④ §9 补齐两行列数（“门禁可信性”与“记录回正 r6”两行此前只有 6 个单元格，表头 7 列）；⑤ `log.txt`（未跟踪）补“第二轮复审”一节并改掉两行过期事实 | 复审点：跟踪文档把启发式的缓存判据写成了内容完整性证明（“相等即摘要仍描述这个文件”），而 P0-3 时期的“结构上不存在忘记校验的旁路”当时并未成立（`open_unchecked` 仍 `pub`）。前者不能当安全边界引用，后者只能从 `86cc76e` 起引用 | 与 `git log` 逐条对拍的复审批计数：`11ff6e6`（缓存键）/ `86cc76e`（会话入口）/ `de44a40`（公式队列）/ `810091a`（P1-7 记述）；测试数演进 738 → 746 → 747 passed + 2 ignored，serve 并行/串行 289 未变；旧数字 2.2–5.9% / <5% 已从 §9 就地更正（见 P1-7 两行） | 本批改了 `src/` 的文档注释（会重编 lib），因此提交前重跑全套七项（fmt `--check` / clippy `-D warnings` / `--all-targets` / serve 串行 / serve_startup / 两个 node 检），**退出码均为 0**；测试计数不变：lib **747 passed + 2 ignored**、serve 并行与串行各 **289**（串行 327.49s）。另跑 `cargo doc --no-deps --features serve`（退出 0；仓库本身带 67 条 rustdoc 警告，但**无一条**指向本批新增的 `FileIdentity` / `SessionCacheKey` / `VerifiedModel` 链接，即“新增链接可解析”而不是“文档无警告”） | 只回正措辞，不改变事实范围：**中段修改（同体积 + 同 mtime + 首尾 64 KiB 不变）仍是已登记的启发式盲点**（§4.2），本批没有加新的检测手段；`log.txt` 未跟踪，不能作为仓库门禁证据；§9 两行的列数不齐只影响渲染，内容本来无误，补齐不翻结论 |
| 表格会话缓存键 · 复审 P1 | 复审修复批（本批） | `ocr/pipeline/rapid_structure.rs`：`SessionCacheKey` 新增 `config: SessionConfigKey`（`EngineOnly` / `Table(TableStructureConfig)`），构造器拆为 `engine_only` / `table`（15 处调用点同步）；`table_session` 先组装 config，**同一份**既进键又交给 `TableStructureSession::open`；`api.rs` 的 `TablePolicy` 文档补“配置跨请求也生效” | 复审给的复现序列在旧实现下确实成立：边长 488 的热会话 + 第二次只改配置 → 旧会话被复用、新配置不生效。危害不是“稍慢”而是推理本身错位：同一张请求的预处理已按新边长生成张量。实测（把调用点的键退回 `engine_only`）`Got invalid dimensions for input: x … index: 2 Got: 600 Expected: 488`。键存**整个** `TableStructureConfig`而不是三字段，是为了新增旋钮自动进键 | 判别力（实跑后精确还原，还原后文件哈希与改前一致）：退回后端到端用例 FAILED（上述 ORT 错误）、无模型规则用例仍绿 —— 证明两类守卫不可互替（前者钉调用点，后者钉键类型）；规则用例另断“表格键 ≠ 公式键（同身份同声明）”，删 `config` 字段即红 | lib 747 → **749 passed + 2 ignored**（+2：端到端 1 + 无模型规则 1）、serve 并行/串行各 **289**（串行 278.56s）、serve_startup 2、`session_open_surface_guard` 4、两个 node 检全绿；§8.1 全套七项**退出码均为 0**（本批提交） | 只修了调用方**显式声明摘要在场**时的配置跨请求问题：`expected_model_sha256 = None` 回落 manifest 登记摘要那条路径仍用原始 `None` 进键（复审 P1-b，下一批）；文件身份仍是启发式，中段修改盲点照旧（§4.2）；两条端到端用例缺资产时仍静默 skip，所以规则另由无模型守卫钉住；**登记摘要回落那一半已由下一行闭合** |
| 表格登记摘要回落 · 复审 P1-b | 复审修复批（本批） | `ocr/pipeline/rapid_structure.rs`：`table_session` 在取文件身份之前先调 `registration::resolve_registered_expected`，**同一个有效摘要**既进键又交给 `verified_registered_model`（传已解析值，因此 registration 内部的回落分支不重读清单）；`SessionCacheKey.declared` 文档从“声明摘要”改为“有效期望摘要”；`api.rs` 的 `TablePolicy::expected_model_sha256` 同步 | 与 P1-1 同构的洞：策略里 `None` 不等于“这一次没有期望摘要”，表格域还有一条清单回落。旧实现把原始 `None` 直接进键，于是“登记值被改掉”整个类别落在键外：声明不变 + 身份不变 -> 旧会话继续回答，新登记值一次也没重校。**校验语义未变**（显式非空仍 `trim` 后胜出，登记仍走 registration 的小写化与合法性判定） | 新增端到端 `a_changed_table_registration_is_not_answered_by_a_hot_session`（真实模型+真实表格页，策略留 `None`）三段：登记正确→通过且有真表格；登记改掉（模型一个字节没动）→ `HashMismatch(expected=清单值, actual=磁盘实测)`；改回来→又能跑（键变化双向）。规则用例另补 `None → Some` 方向。判别力：键退回原始声明后端到端用例在 `expect_err` 处 FAILED，而规则模块仍全绿（它钉键类型、钉不了调用点）—— 两类守卫不可互替已写进模块文档；还原后文件哈希与改前一致 | lib 749 → **750 passed + 2 ignored**（+1 端到端）、serve 并行/串行各 **289**（串行 277.84s）、serve_startup 2、`session_open_surface_guard` 4、两个 node 检全绿；§8.1 全套七项**退出码均为 0**（本批提交） | **行为变化已如实登记**：清单解析从“仅在重建分支”变为“每条表格请求”，因此“模型旁边有清单但未登记该模型/角色不对/哈希为空”现在在任意一次请求上报错（更正确的失败），代价是每请求多一次小 JSON 读盘 —— 本批**未为此做端到端计时**，不声明性能影响幅度；文件身份仍是启发式，中段修改盲点照旧（§4.2，复审第三项）；**该复审项已由下一行闭合**；端到端用例缺资产时仍静默 skip |
| 状态缓存的加载期证伪 · 复审 P1/P2 | 复审修复批（本批） | `src/model_verify.rs`：新增私有 `invalidate_if_falsified(path, actual)`（与条目不符则 `remove`，返回是否真删），`VerifiedModel::verify` 在算出完整摘要后调用它，**匹配与 `HashMismatch` 两条分支都调**；模块文档新增两节（“状态是什么 / 不是什么”与“加载期证伪”）并扩写 `ReverifyCause::FirstSight`；`model_set.rs` 的 `state_in` 与 `ModelFileState::Present`、`model_plan.rs::report()`、`server.rs::admit_ocr` 的措辞按两层拆开；`docs/05` §10 第 10 条改列**三个**确定性入口 | 复审点：`verify` 完整读盘却**既不写也不废**身份缓存，于是同体积 + 同 mtime + 首尾 64 KiB 不变的中段修改下，“状态报告”与“实际加载结果”可以**无限期**分叉（`/api/models` 报旧摘要甚至 `present`，同一次加载已按新内容拒绝会话或按新内容建会话）。修法选**证伪即作废**而不是覆盖：这条路径上没有任何一次观察把当前身份与当前摘要一起取过来（`verify` 不取 `FileIdentity`，取也只能是读盘**之后**），覆盖会写进一条从未被一起观察过的记录，中间状态也可能被钉成“已验证”；删除只放弃一次加速机会。**未改为全文件校验或强制重验**：566 MB 模型在 8 s 轮询下重读不可接受，本批按复审给的第一条路走（把它当性能启发式写清口径）外加一条加载期收敛 | 新增 4 条用例：收敛（作废 -> 下一次 `computed == true` 且给出新摘要 -> 第三次重新变热，钉住“不是一次性失效循环”）、失败路径同样作废（`HashMismatch(expected=旧声明, actual=新内容)` 后状态必须重读）、`expected = None` **不作废**（无证据，限制被钉成测试）、三态直测底层函数（相符保留 / 不符删除 / 无条目不动作，含大小写不敏感）。判别力（实跑后精确还原，还原后文件哈希与改前一致）：摘掉那一次调用 -> 两条收敛用例 FAILED 于 `model_verify.rs:1288:9`（"a load that falsified the cached digest must not leave a hot entry behind"）与 `:1336:9`（"a refused load falsifies the entry all the same"），而“限制”与“三态”两条仍绿 —— 四条钉的是不同件事 | lib 750 -> **754 passed + 2 ignored**（+4）、serve 并行与串行各 **289**（34.57s / 277.43s）、serve_startup 2、`session_open_surface_guard` 4、`exports_dead_code_guard` 1、`text_min_multiset` 1、bench_warm 5 / formula_bench 4 / formula_eval 15 不变；两个 node 检全绿（白名单 fingerprint `32cf61e1…` 与上批一致）；§8.1 全套七项**退出码均为 0**（本批提交） | **命中仍不等于当前内容**：没被加载过、也没被 reverify 过的文件，中段修改照样不进状态；本批把分叉从无限期收窄到“至多到下一次带可信摘要的加载”，**不是实时保证**。作废只管**状态**不管**已建好的会话**（会话缓存同样按身份判定是否重建），运行期要同时刷新两者仍只有 `POST /api/models/reverify`。`expected = None` 无证据不作废。本批无端到端计时，不声明收敛带来的成本幅度；`model_set.rs` 里 `ModelFileState` 的 enum 拆行是 rustfmt 强制的附带格式化 |
| 声明口径机器化 · 复审 P1 残余 | 复审修复批（本批） | 新增 `tools/check_model_state_claims.ps1`（15 条越界短语 + 6 份文件的必须句 + 7 例 `-SelfTest`）；`README.md` § Model integrity 一节补“命中报告的是上一次完整校验的摘要 / 三个确定性入口 / 不是实时保证 / 该禁令由脚本机器执行”，并把“admission check 也走摘要缓存”这句过期表述改为存在性-only；`CHANGELOG.md` 三处引用旧错误原句的地方补上“越界 / 不成立”标注 | 第四轮复审结论：两个表格缓存缺陷已闭合，剩余的“中段修改仍属启发式盲区”作为**已声明的启发式限制**被接受，但要求“继续禁止把 `present` 或缓存命中描述为已核实当前磁盘内容”。这类禁令只靠人复查一轮就会漂移，所以做成机器判据（扫描 220 个文本文件，现存 0 条越界措辞） | 判据选型全部是实跑出来的三次纠正：① 第一版用“短语前 220 字符窗口”找否定词，在真实文件末尾注入一句越界注释后**守卫仍返回 0**（长中文文档里前向窗口几乎必然撞到某个“不”字），遂改为**同一句内**判否定；② 英文整段不按 `.` 切会让上一句的 `not` 给下一句的宣称兜底，故 `.` 与 `…` 也作边界；③ “必须句”判据在 Windows 下因反斜杠相对路径与字典键（正斜杠）不匹配而**静默失效**，归一化后第一次真正生效就抓到三处“我以为写了但其实没写”（`model_verify.rs` 无“性能启发式”字样、`model_plan.rs` 的口径句被文档注释折行拆开、`docs/05` 写的是“只有三个入口”），因此必须句改按仓库里**实际在场**的写法取值 | lib **754 passed + 2 ignored**、serve 并行/串行各 **289** 不变（本批零行为改动，只加工具与文档）；§8.1 七项 + 新增两项守卫**退出码均为 0**；判别力：注入越界注释 → exit 1 并点名 `src/model_verify.rs:1609` 三条，删掉 `model_set.rs` 的口径句 → exit 1 报 `missing-statement`，两者还原后 exit 0 且文件哈希与改前一致 | 守卫扫的是**措辞**不是行为：它钉不住“代码将来真的把缓存当完整性用”，只能钉住越界写法与口径句在场；短语作为判据数据所在的文件靠文件头 `claim-guard: phrases-are-data` 跳过第 1 条（本脚本自身与被豁免的测试文件即属此类）；脚本必须带 UTF-8 BOM，否则 Windows PowerShell 5.1 会把中文短语按 ANSI 误读→判据静默失效（`-SelfTest` 连这件事一起钉）；残余 P1 风险本身**未消除**：中段修改依旧不实时进状态，收敛仍需一次带可信摘要的加载或两个 reverify 入口 |
| 域外新增 · 逐词/逐字框接通（Step 1，非本文批次） | 外部计划批（本批） | `POST /api/ocr` 增第三个正交参数 `?words=off\|words\|chars`（缺省 `off`）：`http.rs::parse_words` → `OcrJobParams { max_side, table, words }`（提交那刻固定）→ `server.rs::recognize` → `text_request(…, words)`；`WordOutputMode` 补 `as_str`/`from_name`/`ALL`/`kind_for` 与 `#[default] Off`，`WordKind` 线上拼写改 `snake_case`；`rapid_structure.rs` 两条分支的 `kind` 改调单一映射；CLI `--words/--chars` 与 evaluate 路径同词表；新增 12 条用例与 `docs/05` §4.2.3 | 本行不属于本文的页面编排重构批次，登记在此只为防止同一个能力被三处反复重述（按裁决分域：**输出契约** = 本批；**Web Demo 呈现层** = Step 2；**CTC 字符定位算法** = Step 3；**字符定位评测** = Step 4）。修复的事实错误：库里一直有字符级框，只是 serve 写死 `Off`，因此“只能区域级”不成立 | 端到端 6 条用例的几何是**脚本化**的（行框 `(1,1)-(10,5)`、`original_size 100x50`），证明的是“原样透传文本/标签/坐标、不重缩放、不改顺序、非法档位在建任务前被拒”，**不**证明字符框准确度；判别力靠 `Scripted.word_modes` 按请求体成对记录（沿用复审 P2-3 手法，避免进入顺序≠提交顺序）与 `word_mode()` 找不到即 panic（参数被丢掉也算回归） | lib 754 → **757 passed + 2 ignored**（+3）、serve 并行 **298** / 串行 **298 passed**（321.75s，+9）、serve_startup 2、两个 node 检、守卫 selftest + 正式扫描：§8.1 全套九项**退出码均为 0**（本批提交） | 字符框仍为 CTC 后处理估计（列索引当中心 + 整行平均字宽），仓库内**无**字符定位基准数据，故不得据本批声称像素级精确；`src/bin/web/index.html` 本批未动，`chars` 档只有 API 侧可达性；`rec-only`（`stages.detect=false`）分支 `word_boxes` 恒为空——本批拆掉硬编码陷阱但**未**改变该分支行为；`?words=chars` 的响应体放大未做端到端测量，仅登记 413 `result_too_large` 的路径后果；`CoordinateSpace` 线上值仍是 `"Image"`（被 SHA-256 冻结夹具钉住），与 `WordKind` 的不对称已登记在 `docs/05` §4.2.3 |

---

## 10 附录：证据索引

`[实测]` = 本文作者读过原文（含评审 `m01562` 提出、我已复验的行）；`[审计]` = 四份只读审计报告给出、动手前需再核。

本索引钉的是**动手分析当时**的代码现状（行号也一样会漂）；后续批次改写过的结论以 §9 为准。
已被后续批次推翻或需要限定范围的行就地标注（目前：`src/ocr/session.rs` 一行由复审 P1-2 更正；§5 P0-3 的“结构上不存在忘记校验的旁路”与 §5 P1-2 的“同一身份判据”两段由复审末段加注）；
未被标注的行**不代表它对当前代码仍成立**，引用前请重读原文。

| 文件 | 行 | 结论 | 级别 |
| --- | --- | --- | --- |
| `src/api.rs` | 226-240 | `StagePlan` 已被文本管线三阶段占用，`PageRoute` 不得与它同名 | `[实测]` |
| `src/api.rs` | 283-287 | 已有 `CoordinateSpace`（输出坐标），命名需避让 | `[实测]` |
| `src/api.rs` | 299-352 | 组合字段 + 两份 roi/tile 禁令 + 中间理由注释 | `[实测]` |
| `src/api.rs` | 634-710 / 746-825 | 两个策略对象的字段与逐字同构的 validate | `[实测]` |
| `src/api.rs` | 742-745 | 公式区域内的表格文本可能是 `Unmatched` 的已知边界 | `[实测]` |
| `src/ocr/pipeline/rapid_structure.rs` | 616-630 | 分派与顺序理由 | `[实测]` |
| `src/ocr/pipeline/rapid_structure.rs` | 638-709 | 文本底座 + 输入物化第一份 | `[实测]` |
| `src/ocr/pipeline/rapid_structure.rs` | 1091 / 1135 / 1175-1222 / 1281-1346 / 1377-1383 | 防递归改写、公式区域内联构造、`decode_source_image` 第二份物化、两个懒加载、表格路由头 | `[实测]` |
| `src/ocr/pipeline/rapid_structure.rs` | 1856 / 1994-2004 | tile 是第四条管线；**tile 子请求关闭公式与表格**（P2-3 暂缓的依据） | `[实测]` |
| `src/table/structure.rs` | 166-205 / 233-238 / 278-291 / 335-398 | 配置、overrides 提前返回、`require_applied`、双开会话（探针承担契约/metadata/词表/维度名） | `[实测]` |
| `src/table/registration.rs` | 144-156 | `verify_registered_model`：从 `manifest.json` 推导可信期望摘要并委托 `verify_sha256`（**不是重复封装**） | `[实测]` |
| `src/formula/recognizer.rs` | 87 / 90-101 | 默认 `from_model` 传 `None`；`from_model_with_hash` 已调用 `verify_sha256` | `[实测]` |
| `src/formula/detect.rs` | 108 / 117-125 | 同上（对称入口） | `[实测]` |
| `src/ocr/session.rs` | 44-51 | `OcrSession::new` 收 `&VerifiedModel` 并走 `open_verified`；本行原文写的是**重构前现状**（“不接收摘要，直接 `open_unchecked`”），已于复审 P1-2 更正 —— 那条不经凭证的入口现在是 `pub(crate)` + `#[cfg(test)]` | `[实测]` |
| `src/evaluation/stats.rs` | 1-5 / 10-18 / 42-57 / 87 | 模块自述"避免每个 bin 各写一套"、字段无 p90、线性插值、既有断言 | `[实测]` |
| `src/evaluation/table/run.rs` | 1526-1545 / 1582 | 最近秩 `summarize`、只验自洽未对拍 | `[实测]` |
| `src/bin/bench_warm_e2e.rs` | 252-259 | 第三份统计口径（中位 + 最近秩） | `[实测]` |
| `src/bin/formula_eval.rs` / `src/bin/formula_bench.rs` / `src/table/structure.rs` | `formula_eval:199-201`,`687`；`formula_bench:73-76`,`102-105`,`476`；`structure:1278-1282` | `Stats` 被报告直接序列化/打印，且测试钉住 `p50_ms == 2.5` → P0-2 不得改字段 | `[实测]` |
| `src/ocr/rec/preprocess.rs` / `src/ocr/cls/preprocess.rs` | `rec:228-236` / `cls:155-163` | signed-unit LUT 是 **`[f32; 256]`**（v1 写成 `[i8; 256]` 已更正） | `[实测]` |
| `src/bin/serve/model_plan.rs` / `src/bin/serve/server.rs` | `plan:146`,`502`,`530`,`550`；`server:1001-1002`,`2913-2914` | `/api/models` 已暴露 `declared_sha256` + `has_declared_digest`（Q3 依据） | `[实测]` |
| `tools/*.ps1` | `run_windows_baseline:103`、`run_thread_matrix:65`、`run_windows_provider_matrix:90`、`refingerprint_windows_baseline:101`、`check_provider_claims:477` | 基准 JSON 的键名依赖（`stats.<metric>.p50`），P0-2 不得改键名（该脚本**没有** dry-run 开关） | `[实测]` |
| `src/model_verify.rs` | 405-418 / 703 测试 | 校验唯一入口与"无期望即不校验" | `[实测]` |
| `src/output/markdown.rs` | 1-6 / 17 / 34 / 98 | 死代码登记原文 | `[实测]` |
| `src/output/json.rs` | 53 / 234-246 / 174-227 | 死代码；三种视图（事实源 / 消费视图 / 表格视图） | `[实测]` |
| `src/formula/output.rs` | 26 / 58 / 71 / 75 / 137-237 | 公式输出四通道与各自测试 | `[实测]` |
| `src/exports.rs` | 42 | 公式输出三函数的重导出点 | `[实测]` |
| `docs/09-table-recognition-execution.md` | 554-555 / 1609 | "符号名从探针读不写死"+"`require_applied` 不能省"；`registration` 14 条测试 | `[实测]` |
| `docs/11-table-page-pipeline-execution.md` | 45 / 314 / 1219 | 性能数字来源约定、词表同次探针读出、确定性证据 | `[实测]` |
| `docs/07-table-recognition.md` | 347 | 被表格消费的行"叠加非替换" | `[实测]` |
| 几何/预处理/字典/评测的其余行号 | 见 §2 各表 | 由四份审计给出 | `[审计]` |
