# 表格识别设计文档 —— 性能与速度专项评审

> ⚠️ **本文档的结论地位已被取代。** 其内容已全部并入
> [docs/07-table-recognition.md](07-table-recognition.md) **v2（性能与精度平衡）**：
> §0 方法学警告 → v2 §0.3；§1 实测事实基础 → v2 §5；§2 R1–R11 → v2 §12；
> §3 测试集与数据集 → v2 §11；§4 未确认项 → v2 §14；§5 执行顺序 → v2 §13。
> 本文档**仅保留为推导过程记录**；v2 是自包含的，需要结论时请读 v2。
> v2 另补齐了本文档完全没有的精度轴（指标口径、精度目标、精度预算、构造性上界）与权衡旋钮表。
> 注意：v2 已改写本文档 §3.3 中"`pubtabnet_format.zip` 可直连官方 `metric.py`"的说法——该包是
> **tokenized 训练格式**，需要 detokenize 适配层，不存在"拿来即跑"（见本文档 §3.2.2 ①）。

评审对象：[docs/07-table-recognition.md](07-table-recognition.md)（v1；该文件现已为 v2）
评审范围：性能与速度。正确性/精度只在影响性能结论时提及。
日期基线：`tests/baseline/windows-baseline/bench-cpu.json`（已入库，release，12 页 × 3 轮 = 36 样本，max_side_len 2000，intra_threads 16）。

---

## 0. 方法学警告（先读，否则后面所有数字都会被误用）

本机存在两个 ONNX Runtime：

| 路径 | 版本 | 用途 | 可信度 |
| --- | --- | --- | --- |
| Rust 链接（静态） | API 1.28.0 | 仓库正式管线 | **唯一可用于性能结论** |
| Python `onnxruntime` | 1.20.1（`C:\A_Softwares\Python\python.exe`，Python 3.12.8） | 我的临时探针 | **绝对延迟不可用** |

同一批模型，两者相差约 20–30×，无法用线程数解释：

| 测量 | Python 探针（scratch） | Rust 已入库基线 `bench-cpu.json` | 倍差 |
| --- | --- | --- | --- |
| rec 推理 | ≈1.37 ms / 像素列（H=48） | 全页 274 ms / 37 区域 ≈ 7.4 ms/区域 | ~30× |
| det 推理 | ≈5000 ms/Mpx | 570 ms/页（max_side_len 2000，约 3 Mpx） | ~20× |

**结论**：本文中所有 Python 探针数字只用于描述**成本模型形状（比例关系）**，并已逐条标注；任何绝对值都必须用 Rust harness 重测。这正是 `AGENTS.md` §4「性能优化必须有依据，禁止为了臆测性能而优化」的直接适用。设计文档 §8 第 8 条也要求性能记录走 CPU provider 实测。

探针脚本（未入库 scratch，仅用于复现图形/形状事实）：`Temp\probe_slanet.py`、`Temp\probe_slanet2.py`、`Temp\probe2.py`、`Temp\probe3.py`；输出 `Temp\probe_slanet_out.txt`、`Temp\probe2_out.txt`、`Temp\probe3_out.txt`。

---

## 1. 实测事实基础

### 1.1 页面级耗时构成（权威，来自 `tests/baseline/windows-baseline/bench-cpu.json`）

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

派生量：`ort_inference_ms` = 844.10（85.6 %）、`rust_ms` = 134.93（13.7 %）。每页平均 34.8 个文本区域（p50 37，p90 61）。

**第一性事实**：表格识别新增的模型是 SLANet-plus，488×488 单次推理量级约 25–31 ms（见 1.3），即**约占页面预算的 3 %**。表格模块的性能天花板远低于页面级 det/rec 的既有开销——所以最高收益的优化不在表格模块内。

### 1.2 SLANet-plus ONNX 图与契约（已实测，来自 `models/table/slanet-plus.onnx`）

- 文件 `models/table/slanet-plus.onnx`，**7,758,305 字节**；SHA-256 = `d57a942af6a2f57d6a4a0372573c696a2379bf5857c45e2ac69993f3b334514b`，**与设计文档 §5 声明逐位一致**（该文档中"落地前仍需重算 SHA"的 `<待确认>` 可以关闭）。
- graph name `PaddlePaddle Graph 0`，**830 节点**，opset `('',14)`，ir_version 7，producer 为空。op 直方图：`Constant 484, Conv 77, BatchNormalization 73, HardSwish 73, Cast 31, Concat 23, Reshape 15, Slice 13, Shape 8, Squeeze 8, ConstantOfShape 5, Resize 3, Add 3, GlobalAveragePool 2, Relu 2, HardSigmoid 2, Mul 2, Transpose 1, Less 1, Not 1, And 1, Loop 1, Softmax 1`。顶层 initializer 元素数为 0（权重全内联在 484 个 `Constant` 节点中）。
- 输入名 **`x`**，FLOAT，形状**全动态** `[DynamicDimension.0, 3, DynamicDimension.1, DynamicDimension.2]`。
- **两个输出**：`save_infer_model/scale_0.tmp_0` FLOAT `(N,L,8)`（bbox，8 = 4 点 xyxyxyxy）、`save_infer_model/scale_1.tmp_0` FLOAT `(N,L,50)`（结构概率，50 类）。
- `metadata_props` 只有 `character`，是**长度 582 的换行分隔（非 JSON）**字符串，split `\n` 恰好 **48 个 token**：`<thead> </thead> <tbody> </tbody> <tr> </tr> <td > </td> colspan="2"…colspan="20" rowspan="2"…rowspan="20" <td></td>`。含 `<td></td>` 而不含 `<td>`（即 `merge_no_span_structure=True` 语义已烘焙进 metadata）。+`sos`/`eos` = **50 类**，与输出最后一维一致。
- **L 是动态的**：零输入 → L=27；随机输入 → L=150。Loop 的 trip 上限常量是 `assign_0.tmp_0.0 = 500`，提前退出由 `logical_and` 依 eos 控制。batch 1/2/4/8/16 全部被接受。
- ORT 对动态形状输出会打印告警：`Expected shape from model of {} does not match actual shape of {1} for output logical_and_0.tmp_0`（`execution_frame.cc:870 VerifyOutputSizes`），仅告警。

### 1.3 SLANet 线程/批处理/优化级别（Python 探针，**仅比例可用**）

线程与批处理（488×488，ms/图）：

| threads \ batch | bs=1 | bs=2 | bs=4 | bs=8 | bs=16 |
| --- | --- | --- | --- | --- | --- |
| 1 | 59.3 | 61.4 | 74.4 | 79.0 | 90.5 |
| 4 | 36.6 | 35.6 | 38.5 | 41.0 | 45.7 |
| 8 | 30.8 | **27.2** | 29.6 | 31.1 | 34.3 |

- **批处理收益上限约 12 %**（th=8，bs=2），bs≥4 开始回升；单线程下批处理反而劣化（59.3 → 90.5）。
- ORT 优化级别（488²，th≈4）：`DISABLE_ALL 79.2 / BASIC 69.2 / EXTENDED 66.3 / ALL 43.9 ms` → **ALL 相对 DISABLE_ALL 约 1.8×**。现有 `src/runtime/session.rs:248` 已设 `GraphOptimizationLevel::Level3`（= ALL），**这一项已经是对的，不要动**。
- `enable_cpu_mem_arena` 开 41.8 vs 关 49.3 ms → 默认开是对的。`enable_mem_pattern` 关 43.1 vs 开 43.1 ms → 无差异。

### 1.4 rec 的成本模型（Python 探针，**仅比例可用**）

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

## 2. 改进建议（按 收益/成本 排序）

### R1 — 表格回填改为"整页 det 复用 + 几何匹配"，**删除逐 cell 裁剪喂 rec 的路线**

| 项 | 内容 |
| --- | --- |
| **成本** | 高（结构级重构，`src/table/cell_content_backfill` 的整体设计变更） |
| **收益** | 高（回填额外 rec 成本 → 0，并消除多行压扁的精度损失） |

**现象/瓶颈**：设计文档 §3 规划的 `cell_content_backfill` 是"逐单元格裁剪 → 调 rec 回填"。其成本正比于**单元格数 × 批内最大宽度**，且与内容量无关。

**根因（第一性事实）**：

1. rec 的输入契约是 `[N, 3, 48, W]`。`src/ocr/rec/preprocess.rs` 的 `batch_shape_for(max_wh_ratio, rec_image_shape)` 里 `dst_width = ((img_height as f64) * max_wh_ratio) as usize`，而 `max_wh_ratio` 取自**批内最大值**（`src/ocr/rec/recognizer.rs` 每批先算 `max_wh_ratio` 与 `wh_ratio_list`，再 `batch_scratch.resize(total_len, 0.0)` 整批 pad 到同一宽度）。所以只要表格里出现一个跨多列的合并单元格，**同一批所有单元格都被拉宽**。实测比例见 §1.4 表：整批 pad 到 640 时每格成本是 W=64 时的 9.4×。
2. rec 是**单行**识别器：输入被等宽拉伸到固定高 48 px。多行单元格（换行表头、地址、备注）被压成一行，信息不可恢复。
3. 逐 cell 裁剪意味着每个单元格一次 crop + 一次 resize + 一次归一化（`src/vision/resize.rs`、`src/ocr/rec/preprocess.rs` 的 `signed_unit_lut()` 归一化），这些都在整页路径里已经付过一次。

**具体改法**：改为参考实现/官方路线 —— **整页 det 一次得到全页文本框，表格结构模型给出 cell bbox，用几何匹配把 det box 归到 cell**。参考实现 `refer\RapidTable-main\rapid_table\main.py` 的 `RapidTable.__call__` 就是先 `table_structure(imgs)` 再对**整页图**调 `self.ocr_engine(img)`，最后交给 `TableMatch`；`refer\RapidTable-main\rapid_table\table_matcher\main.py` 的 `TableMatch.match_result(cell_bboxes, dt_boxes, min_iou=0.1**8)` 对每个 det box 算与所有 cell box 的 `(distance, 1-IoU)`，按 `(1-IoU, distance)` 取最优，建立 `matched[cell_idx] = [det_idx…]`。逻辑网格（含 rowspan/colspan 占位）用同文件 `decode_one_logic_points` 的 `occupied_cells` + `is_occupied/mark_occupied` 推导，这是生成 Markdown 网格的正确参考。

**预期收益**：回填的**净增 rec 成本为 0**（复用页面 OCR 已有的 `LineResult`），并消除多行压扁。对比"逐 cell 跑 rec"——即使按最乐观的每格 7.4 ms/区域（Rust 口径）估算，一张 8×5 = 40 格的表也要多出约 300 ms/页，相当于把整页耗时提高 30 %。

**所需测试**：
- 基线（改动前）：`datasets\text\04表格与键值对.png` 走现有 pipeline，记录 `regions`、`ocr_total_ms` p50/p90（口径同 `tests/baseline/windows-baseline/bench-cpu.json`）。
- 新增：cell↔det-box 匹配的纯函数单测（构造对角/合并/空单元格 case），断言匹配矩阵与 `decode_one_logic_points` 的逻辑网格。
- 回归：新路线在同一批页面上的 HTML 结构逐 token 一致；cell 文本非空率；总耗时不得高于基线。

**风险**：cell bbox 的坐标系换算必须先钉死 —— 设计文档 §4.2 自己把 `rescale_cell_bboxes` 的坐标系标为 `<待确认>`（参考实现里 `ratio = min(488/h, 488/w)`、`w_ratio = 488/(w*ratio)`、`h_ratio = 488/(h*ratio)`，在横表 `w >= h` 时 `w_ratio = 1`，语义反直觉）。**先用 golden 把这一步钉死，再谈性能**，否则"匹配错"会被误判成"检测差"。

---

### R2 — 页面级 det 分辨率：表格路径不应默认继承 `max_side_len = 2000`

| 项 | 内容 |
| --- | --- |
| **成本** | 低（配置/参数层面） |
| **收益** | **最高**（最高可达数百 ms/页） |

**现象/瓶颈**：`detector_infer_ms = 570.57`，占页面总耗时 **57.9 %**。

**根因（第一性事实）**：det 是整页稠密卷积网络，计算量正比于输入像素数；`max_side_len` 直接决定像素数。当前基线用 2000。而 PaddleOCR 官方表格评估/推理脚本用的是 `--det_limit_side_len=736 --det_limit_type=min`（来源：[ppstructure/table/README.md §4.3](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/f8b41a62/ppstructure/table/README.md)）。探针实测 det 延迟对面积近似线性（`736² 3015 ms → 2048² 19857 ms`，约 5000 ms/Mpx；**绝对值不可用，仅说明"面积线性"这一形状**）。

**具体改法**：为表格识别路径引入独立的 det 限制，而不是复用页面级的 `max_side_len`。最小改动是在表格路径的 `RuntimeConfig`/pipeline 配置里单独传 `limit_side_len` + `limit_type`；符合 `AGENTS.md`「可以破坏旧接口」——不必为兼容旧配置加分支。

**预期收益**：面积从 2000² 量级降到 736² 量级，理论上限约 7×（按面积），**但表格单元格文字通常小于正文，降分辨率的精度代价必须先量出来**。这是一个必须用数据决定的旋钮，不是无条件优化。

**所需测试**：
- 基线：`datasets\text` 12 页在 `max_side_len ∈ {2000, 1280}` 的 CER（`tests/baseline/windows-baseline/evaluation-cpu.json` 已是现成对照）。
- 新增：表格页专用集（见 §3）在多个 det 分辨率下的 区域召回 / CER / 单元格文本完整率。
- 验收口径可直接复用仓库既有容差文件 `benchmarks/image-bench/api-comparison-small-medium.json`：mean CER ≤ 0.03、polygon IoU ≥ 0.85、区域数差 ≤ 5 %。

**风险**：表格里的小字号内容漏检；降分辨率后单元格文本回填失败率上升。**若 CER 上升超过容差，就放弃这一项**——`AGENTS.md` 的决策优先级是 正确性 → 根因 → 性能。

---

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

**根因（第一性事实）**：现有模型契约（`src/formula/contract.rs` 的 `FLOAT32 [N,1,384,384] → INT64 [N,L]`）都是**单输出**，API 据此简化。SLANet-plus 有 **2 个输出**（§1.2），单输出 API 无法表达该契约。注意 `session.run(inputs![...])` 本身已经返回全部输出，只是被 API 丢弃了——不需要改 ORT 调用方式，只需把闭包的入参从"单个 array"泛化为"整个 `SessionOutputs`"。

**具体改法**：把 `run_arrayd_view_with` 的闭包签名从 `FnOnce(ArrayViewD<'a, f32>) -> Result<T>` 改为接收 `&SessionOutputs`（或新增一个 `run_arrays_view_with` 多输出变体），并在 `src/runtime/contracts.rs` 增加"输出对"契约（类似现有 `require_single_output`，新增 `require_outputs(&[&str])`）。按 `AGENTS.md` 可直接改签名，不需要保留旧 API。

**预期收益**：无直接加速。但缺了 bbox 输出就只剩结构 token，没有 cell 坐标 → 既无法做 R1 的几何匹配，也无法出 Markdown 网格。

**所需测试**：仿照 `tests/fixtures/formula-onnx/`（12 个 KB 级 `.onnx`）构造一个 2 输出最小模型，加契约单测；对真实 `slanet-plus.onnx` 加 golden 测试断言 `(N,L,8)` / `(N,L,50)` 的形状与 dtype，并断言两路输出的数值与参考实现一致。

**风险**：`SessionOutputs` 的生命周期绑定到 `run` 的返回值，闭包签名要处理借用；这是编译期问题，不会引入运行时风险。

---

### R4 — 不要固定 L（token 维），只固定 H/W

| 项 | 内容 |
| --- | --- |
| **成本** | 低 |
| **收益** | 中（消除 ORT 动态形状告警与重规划；DirectML 上为必要条件） |

**现象/瓶颈**：输入形状全动态，ORT 打印 `Expected shape from model of {} does not match actual shape of {1} for output logical_and_0.tmp_0`。

**根因（第一性事实）**：`GraphOptimizationLevel::ALL` 的形状推断在动态图上退化；ORT 官方文档说明优化级别与 EP 分区依赖形状可知性（来源：[ORT graph-optimizations](https://onnxruntime.ai/docs/performance/model-optimizations/graph-optimizations.html)）。DirectML EP 官方明确偏好静态形状，并提供 `AddFreeDimensionOverrideByName` 用于把 free dim 固定（来源：[ORT DirectML EP](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)）。

**具体改法**：用 ORT 官方的 free-dimension override 把 `x` 的 H/W 固定为 488（**不改模型文件**），L 保持动态：

```rust
// 概念示意：per-dimension override，按名字定位 free dim
session_builder.with_free_dimension_override_by_name("x", ...);
```

**关键约束（第一性事实）**：**不要固定 L**。实测零输入 → L=27、随机输入 → L=150，Loop 的 trip 上限常量为 500 但依 eos 提前退出。把 L 固定成 500 会强制解码 500 步，代价可能远超收益——这是"禁止臆测性能"的直接反例。

**预期收益**：消除告警 + 让 ORT 走静态形状路径；在 DirectML 上从"退化"变为"能跑"。H/W 固定本身能否加速需实测（我无法在 Python 环境测出可信数字）。

**所需测试**：断言固定 H/W 前后、同一输入下两路输出的**数值逐位一致**（形状固定不应改变语义）；再记录 `bench_warm_e2e` 的 p50 变化。

**风险**：若后续要支持 488 以外的输入尺寸，这个 override 必须随之调整；建议把它做成 `TableStructureConfig` 的字段而不是硬编码常量。

---

### R5 — 表格结构 session 要用**较小**的 intra threads，且不要继承页面级 16 线程

| 项 | 内容 |
| --- | --- |
| **成本** | 低 |
| **收益** | 低–中（约 20 % 单次结构推理；更重要的是避免线程超订） |

**现象/瓶颈**：SLANet 488² 在 4 线程下 36.6 ms，在 8 线程下 30.8 ms（bs=1）——8 线程确实更快；但 probe1 在**充分预热**下 4 线程测到 **25.1 ms**，优于 8 线程的 30.8 ms。两个口径互相矛盾，说明收益落在噪声区。

**根因（第一性事实）**：

1. SLANet 是 830 节点的小网络（77 Conv / 73 HardSwish），batch=1 时并行度不足，线程越多，同步开销占比越高。
2. ORT 官方线程默认值 `session.intra_op.allow_spinning = "1"`（来源：[ORT threading](https://onnxruntime.ai/docs/performance/tune-performance/threading.html)）——**空闲线程自旋而非休眠**。仓库当前已有 det + rec 两个 session（+ 公式），表格模块会再加一个。若每个都拿 16 个 intra 线程，在 20 核级 CPU 上会产生自旋争抢。
3. 仓库自己的 `tests/baseline/windows-baseline/thread-matrix.json` 表明：本机线程调整的 p50 波动整体落在 ±10–15 % 噪声带内，`src/runtime/profile.rs` 的文档也明确写"这是简化，不是加速"。

**具体改法**：让 `RuntimeConfig` 支持 per-session 覆盖，表格结构 session 用固定的较小 intra threads（例如 4），或在 `RuntimeProfile` 里为表格 session 单独解析。不要为表格模块修改全局 `auto_tuned_thread_budget()`。

> 注：`src/runtime/profile.rs` 当前的 `RuntimeProfile::session_runtime(&self) -> RuntimeConfig` 恒 `auto_tune_threads=false`，并复用同一份解析结果。要为表格 session 单独设线程数，最干净的做法是在 `TableStructureConfig` 里声明 `intra_threads: Option<usize>` 并在构造 `OrtSession` 时用它覆盖。

**预期收益**：单次结构推理 ~20 %（噪声内，需实测确认）；避免多 session 超订带来的隐性退化。

**所需测试**：按仓库既有口径跑线程矩阵（`tools/run_thread_matrix.ps1`，p50 对比、判定带 ±10–15 %）。**若落在噪声带内就不改**——`AGENTS.md` 禁止无依据的性能改动。

**风险**：低。唯一风险是把噪声当收益，因此必须有"改前/改后同一 harness 同轮次"的对照。

---

### R6 — 不要为结构模型引入多表格批处理排队

| 项 | 内容 |
| --- | --- |
| **成本** | 低（=不做） |
| **收益** | 避免负收益 |

**现象/瓶颈**：批处理收益上限实测约 **12 %**（th=8，bs=2：27.2 vs 30.8 ms/图），bs≥4 开始回升；**单线程下批处理显著劣化**（59.3 → 90.5 ms/图）。

**根因（第一性事实）**：SLANet 是 compute-bound，批处理只摊薄 per-run 固定开销（约为 W=16 处的截距量级），不改变总 FLOPs；单线程下大 batch 破坏缓存局部性。

**具体改法**：一页多表时直接串行，或最多 bs=2。**不要**为了批处理引入跨页排队、结果聚合等复杂度（`AGENTS.md` §4：不得为了臆测性能增加不必要的并发机制）。

**预期收益**：避免引入负收益与复杂度。

**所需测试**：若坚持要批处理，需在同一 harness 下给出 bs∈{1,2,4} 的 p50 对比，并证明收益超出噪声带。

**风险**：无（这是"不做"的建议）。

---

### R7 — DirectML 路径必须走 per-EP 的 session 配置，否则会创建失败

| 项 | 内容 |
| --- | --- |
| **成本** | 中 |
| **收益** | 避免运行时失败；让 GPU 路径可用 |

**现象/瓶颈**：现有 `src/runtime/session.rs:243 open_session` 对所有 EP 统一设 `GraphOptimizationLevel::Level3` 并依赖默认的 mem pattern / arena / 并行执行。

**根因（第一性事实，官方明文）**：DirectML EP **不支持 memory pattern 优化，也不支持并行执行**；调用方必须 `DisableMemPattern` 且把 `execution_mode` 设为 `ORT_SEQUENTIAL`，否则 session 创建报错。此外 DML **不支持对同一 session 的多线程 `Run`**，要求 **opset ≤ 20**，并偏好静态形状。（来源：[ORT DirectML Execution Provider](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)）

**具体改法**：在 `open_session` / `resolve_execution_providers` 里按 `selected_ep` 分支：
- DML → `DisableMemPattern` + `ORT_SEQUENTIAL` + 固定 H/W（见 R4）+ 拒绝并行 `Run`（表格模块内保证串行）。
- CUDA → 官方默认 `arena_extend_strategy = kNextPowerOfTwo`、`cudnn_conv_algo_search = EXHAUSTIVE`（来源：[ORT CUDA EP](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html)）。
- CPU → 保持现状（Level3 + arena 开，已实测最优，见 §1.3）。

**预期收益**：DML 上从"session 创建失败"变为"可运行"。

**所需测试**：`tools/run_windows_provider_matrix.ps1` + `tools/check_provider_claims.ps1 -Reference <cpu.json> -Reports <other.json>`（仓库既有工具，后者专门用于核对"provider 声明 vs 实测"）。

**风险**：**本机 Python 环境只有 `AzureExecutionProvider` + `CPUExecutionProvider`，没有 DirectML DLL，我无法在本机验证任何 DML 行为**。另外 SLANet 含 `Loop` 控制流算子，DML 对控制流子图的支持我**未验证** → 不要把它写进对外承诺，必须实测。`src/runtime/provider.rs` 的既有教训也要记住：`selected_ep` 只表示交给 ORT 的 EP 链头部，本机 CUDA 因缺 cuDNN 而节点全落 CPU，API 察觉不到（实测 p50 CPU 1039.5 / DirectML 508.7 / CUDA 1034.2）→ **任何加速结论必须来自实测 P50/P90**。

---

### R8 — 表格定位不要引入第三个整页 CNN

| 项 | 内容 |
| --- | --- |
| **成本** | 中 |
| **收益** | 高（避免 +30~100 % 页面耗时） |

**现象/瓶颈**：设计文档 §3 的 `TableDetector` trait + §1 候选表隐含"需要表格区域检测"。

**根因（第一性事实）**：页面预算已被 det（57.9 %）主导；再加一个整页 CNN 会再叠加一次整页推理。官方 PP-TableMagic 管线确实有 table classification（`PP-LCNet_x1_0_table_cls`，6.6 MB）与 cell detection（`RT-DETR-L_*_table_cell_det`，124 MB）等模块（来源：[PaddleX table_recognition_v2.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleX/refs/heads/release/3.7/docs/pipeline_usage/tutorials/ocr_pipelines/table_recognition_v2.en.md)），但那是**全功能管线**的目标，不是本项目的性能目标。

**具体改法**：优先走设计文档 §7「轻量档」的思路——**从已有 det 的文本框分布推断表格区域**（表格区域内的文本框呈规整行列、行高一致、x 坐标成列）。这是零额外模型成本的做法。只有当实证表明启发式召回率不达标时，才引入专用模型。

**预期收益**：避免第三个整页模型；表格定位的边际成本接近 0（做一次 O(regions) 的聚类，regions p50 = 37，量级可忽略）。

**所需测试**：用 `datasets\text\04表格与键值对.png` + `refer\RapidTable-main\tests\test_files\table.jpg`、`table_without_txt.jpg` 做区域召回/精确率断言；在整页 bench 上确认 `ocr_total_ms` 无增量。

**风险**：启发式在无线框表格、跨页表格、嵌套表格上会退化。必须用 §3 的表格测试集量化，而不是靠直觉。

---

### R9 — 把表格阶段纳入 `TimingLedger`（其余一切的前置条件）

| 项 | 内容 |
| --- | --- |
| **成本** | 低 |
| **收益** | 使所有性能结论可证伪 |

**现象/瓶颈**：`src/runtime/timing.rs` 的 `TimingLedger` 目前没有表格分量。

**根因（第一性事实）**：没有命名分量就无法证明收益；`AGENTS.md` §4 要求"性能优化必须有明确依据"，§6 要求"验证必须真实有效"。设计文档 §8 第 8 条已要求记录 CPU provider 下结构、回填、总耗时。

**具体改法**：照 `formula_ms` 的先例，在 `src/runtime/timing.rs` 的 `TimingLedger` 里新增命名分量（如 `table_detect_ms` / `table_structure_infer_ms` / `table_backfill_ms`），并在表格阶段写入 `OcrTimings`；`TimingLedger::from_timings` + `mean`/`shares`/`conservation` 无需新类型。`src/bin/bench_warm_e2e.rs` 已经输出 `stages` / `timing_ledger` / `timing_split`，会自然带上新分量。

同时注意：仓库**当前没有** `[dev-dependencies]`、`[[bench]]`、criterion、以及任何 `[profile.*]`（`Cargo.toml` 全默认）。表格模块的基线/回归应复用既有口径 —— `pwsh -NoProfile -File tools/run_windows_baseline.ps1`（`bench_warm_e2e` × 3 轮 + `rapidstruct evaluate`）并把产物落到 `tests/baseline/`，而不是新引入一套 benchmark 框架。

**预期收益**：让 R1/R2/R5 的收益可被证实或证伪。

**所需测试**：
- `src/runtime/timing.rs` 已有的守恒测试（`real_world_outer_window_does_not_conserve_the_reported_total`、`documented_input_formula_holds_in_both_branches`）必须继续通过。
- 新增：表格分量出现在 `shares()` 中且 `Σshare = 1`；`timing_split` 恒等式 `ort_inference_ms + rust_ms + input_overflow_ms + unattributed_ms == denominator_ms` 仍成立（`src/bin/bench_warm_e2e.rs` 已有此断言）。

**风险**：低。注意 `conservation` 是**诊断不是验收**：release 残差 −6.75 ms/页（0.69 %），debug 大一个数量级，别把残差当 bug 追。

---

### R10 — 设计文档中需要更正的量化事实（影响选型判断）

| 文档说法 | 官方事实 | 出处 |
| --- | --- | --- |
| SLANet-plus "约 6.8M" | 6.8M 是 `en_ppocr_mobile_v2.0_table_structure`（TableRec-RARE）；SLANet 为 9.2M（en）/ 9.3M（ch）；SLANet_plus 的 HF 模型卡记 6.9M（`inference.pdiparams` = 7,666,515 B）。本机文件 7,758,305 B。 | [PP-Structure models_list.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/docs/version2.x/ppstructure/models_list.en.md)、[HF PaddlePaddle/SLANet_plus](https://huggingface.co/PaddlePaddle/SLANet_plus) |
| "SLANet-plus 63.69"（隐含可比 pubtabnet TEDS） | 两个互不兼容的口径：现代模块文档的 **Accuracy**（SLANet 59.52 / SLANet_plus 63.69 / SLANeXt 69.65）是 **PaddleX 内部数据集**上的准确率，**不是 TEDS**；真正的 PubTabNet 官方数字只对 SLANet 存在：**Acc 76.31 / TEDS 95.89 / 766 ms（CPU, MKL）/ 9.2M**。 | [table_structure_recognition.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/refs/heads/main/docs/version3.x/module_usage/table_structure_recognition.en.md)、[ppstructure/table/README.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/f8b41a62/ppstructure/table/README.md)、[arXiv 2210.05391](https://arxiv.org/abs/2210.05391) |
| 隐含"SLANet-plus 比 SLANet 快" | 官方 A100 + ONNX Runtime 的 E2E 延迟：SLANet **23.81 ms** vs SLANet_plus **23.42 ms**，差 0.39 ms，**在噪声内**。 | [table_structure_recognition.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/refs/heads/main/docs/version3.x/module_usage/table_structure_recognition.en.md) |
| "从 metadata 读取 character 列表"（§4.2） | metadata 是**换行分隔**字符串，不是 JSON（实测长度 582、split `\n` = 48 项）。`src/formula/model_info.rs` 的 `FormulaModelInfo::probe` 要求 `character` 是**合法 JSON**，否则返回 `Config` 错误 —— 直接复用会失败。 | 实测 `models/table/slanet-plus.onnx` metadata；对照 `src/formula/model_info.rs:34` |
| `max_len = 488` | **`max_len` 是重载的**：`ResizeTableImage.max_len = 488`（缩放目标）；`max_text_length = 500`（最大 token 数）。ONNX 里 Loop 的 trip 上限常量实测为 `500`。 | [SLANet_plus.yml](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/configs/table/SLANet_plus.yml)、[arXiv 2210.05391](https://arxiv.org/abs/2210.05391) |
| §5 的 SHA-256 `<待确认>` | **已核实一致**：`d57a942af6a2f57d6a4a0372573c696a2379bf5857c45e2ac69993f3b334514b`。可关闭该待确认项。 | 本机实测 |
| `models/configs/manifest.json` 无 table 条目 | 该文件**不是**模型注册表，而是模型集清单（`detector`/`recognizer`/`dictionary`/`classifier` 四键，id = `pp-ocrv6-medium-multilingual`）。表格模型本来就不属于它。**更正（E18 复核）**：`models/serve/manifest.json` **是存在的**（1,193 B，`id = pp-ocrv6-medium-plus-formula`，五 role：detector / recognizer / dictionary / formula_recognizer / formula_detector），本行的"不存在"是错的；另外 `models/configs/manifest.json` 是**没有任何代码读**的 legacy 死文件（`src/model_source.rs:322-328` 拒绝该形状），**已在 E18 按用户决定删除**（副本留在 `Temp/deleted-models-configs-manifest.json`）——"表格模型不在默认分发集合"的现行依据是 `assets/default_models.yaml` 没有 `table` 段。 | `models\configs\manifest.json` |
| 候选表把 UniTable 标为 ~500M PyTorch | 与官方 PubTabNet 口径不可比（UniTable 未在官方 PP-Structure 榜单中出现）；把它作为"对照"会引入不可比基准。 | 无官方对照来源 |

**对选型的影响**：官方资料里**没有**"SLANet-plus 优于 SLANet"的性能证据，两者延迟差 0.39 ms。选 SLANet-plus 的正当理由只能是"内部多语种/中文表格 Accuracy 更高"或"该 ONNX 是本地唯一可用的两输出制品"，**不能是速度**。

---

### R11 — 精度档（VLM）必须先证明边际收益，再谈性能

设计文档 §7 的"精度档（feature-gated VLM）"没有给出延迟预算。第一性事实：本机已有 `models/serve/pp_formulanet_plus_m.onnx` = **593,915,961 B（594 MB）**、`models/formula/detection/pix2text-mfd-1.5.onnx` = 80 MB。SLANet-plus 只有 7.8 MB。**同量级的 VLM 会把单页耗时推入秒级**（对照：官方 SLANeXt 标 351 MB、CPU 501.66 ms，来源 [table_recognition_v2.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleX/refs/heads/release/3.7/docs/pipeline_usage/tutorials/ocr_pipelines/table_recognition_v2.en.md)）。

建议：VLM 档必须自带 **CPU 延迟预算断言**（例如"启用后 p50 不得超过 X ms/页"），否则 feature-gate 一开就会把整页耗时从 1 s 量级推到不可接受区间。这不是优化建议，是防止性能回归的门禁。

---

## 3. 表格识别测试集从哪里来

### 3.1 结论（先说结论）

- **中文表格没有"许可无争议 + 全部维度齐备"的公开数据集**。最合适的组合是：**TabRecSet 作主力（17.7K 中文表格，且已提供 PubTabNet 格式标注，§3.2.1 有逐文件下载地址与 MD5）+ 少量自建补充集只覆盖它缺失的维度（§3.3 L3）**。
- **纯 TEDS 评测**原本只能用 PubTabNet / SynthTabNet / FinTabNet.c 这类英文集（中文覆盖为 0）；**TabRecSet 的 `pubtabnet_format.zip` 改变了这一点**——中文表格现在也能直接跑官方 TEDS 实现。
- **仍需自建的部分**：中文扫描歪斜、无线框表格、合并单元格（rowspan/colspan）嵌套组合、多行单元格、跨页表格——这些在公开集里覆盖不足（或整集无许可）。

### 3.2 公开数据集清单（名称 / 规模 / 官方指标 / 获取方式 / 许可）

| 数据集 | 规模 | 官方指标 | 获取方式 | 许可 |
| --- | --- | --- | --- | --- |
| **PubTabNet** | paper §3.1：**500,777 train / 9,115 val / 9,138 test**；repo 称 "568k+ images" | **TEDS / TEDS-Struct**（该数据集论文定义）；官方 Acc/TEDS | [github.com/ibm-aur-nlp/PubTabNet](https://github.com/ibm-aur-nlp/PubTabNet)；图像 [HF ajimeno/PubTabNet](https://huggingface.co/datasets/ajimeno/PubTabNet)（16.2 GB） | ⚠️ **冲突**：HF cardData 写 `cdla-sharing-1.0`，PaddleOCR 写需要 CDLA-Permissive（[table_datasets.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/docs/datasets/table_datasets.en.md)）。**落地前必须澄清** |
| **PubTables-1M** | **575,305 已标注页**（检测）/ **947,642 完整标注表格**（结构+功能） | GriTS（+ COCO AP50/AP75/AP/AR、Acc_Con） | [MSR Open Data](https://msropendata.com/datasets/505fcbe3-1383-42b1-913a-f651b8b712d3)（token+azcopy）/ [HF bsmock/pubtables-1m](https://huggingface.co/datasets/bsmock/pubtables-1m) | paper **CC-BY-4.0**（[arXiv 2110.00061](https://arxiv.org/abs/2110.00061)，CVPR 2022） |
| **FinTabNet** | **112,875 表格**，9,627 topologies，平均 11.94 行 / 4.36 列 / 1.01 合并格 | TEDS / GriTS | ❌ **原始 IBM 页面 `developer.ibm.com/data/fintabnet/` 已重定向失效**；未找到可用的一手下载 | CDLA-Permissive（按 HF card） |
| **FinTabNet.c** | **97,475 表格**，18,702 topologies，2.58 合并格 | TEDS / GriTS / DAR | [HF bsmock/FinTabNet.c](https://huggingface.co/datasets/bsmock/FinTabNet.c) | **cdla-permissive-2.0**（明确） |
| **TableBank** | 检测 417,234 表格 / 278,582 图；TSR 145,463 表格 / 145,463 图 | **TSR = 单参考 4-gram BLEU**；检测 = 面积重叠 P/R/F1（**不是 TEDS**） | [github.com/doc-analysis/TableBank](https://github.com/doc-analysis/TableBank)；[HF liminghao1630/TableBank](https://huggingface.co/datasets/liminghao1630/TableBank) | ⚠️ **冲突**：HF `apache-2.0` vs README "仅研究用途 / 请勿再分发" |
| **SciTSR** | **15,000** PDF 表格（12,000 train / 3,000 test）；SciTSR-COMP 2,885 / 716 | **关系 P/R/F1**（`scitsr/eval.py: eval_relations`） | [Google Drive](https://drive.google.com/file/d/1qXaJblBg9sbPN0xknWsYls1aGGtlp4ZN/view) | ❌ **未找到** |
| **ICDAR 2013 Table Competition** | 竞赛集 **158** 表（论文实测发现 2 例需拆分）；含练习集共 256 | **DAR**（有向邻接关系）；历史最佳 0.946 / Tensmeyer 0.953 / TATR 0.965 | [tamirhassan.com dataset](http://tamirhassan.com/html/dataset.html)（`eu-dataset-20130324.zip` 等） | ❌ **未找到**（原文仅称源文档"预期属公有领域"） |
| **cTDaR（ICDAR 2019）** | **1000 modern + 1000 archival** 图（区域检测）/ **80 文档**（表格识别） | Track A: IoU / P/R/F1 @0.6–0.9；Track B: 邻接关系法；排名 = WAvg.F1 | ❌ **官方 GitHub 下载链接已 404**；站点 [cndplab-founder.github.io/cTDaR2019](https://cndplab-founder.github.io/cTDaR2019/) | ❌ **未找到** |
| **WTW（Wired Table in the Wild）** | **14,581 图**；test 含 7 类困难子集（倾斜/弯曲/遮挡/极端长宽比/叠加/多色/不规则） | 论文报 TEDS；**官方评测代码 "To be updated"** | [Tianchi dataId=108587](https://tianchi.aliyun.com/dataset/dataDetail?dataId=108587)；[wangwen-whu/WTW-Dataset](https://github.com/wangwen-whu/WTW-Dataset) | ❌ **未找到** |
| **SynthTabNet** | **600k PNG**（4 × 150k 风格：fintabnet 10 GB / marketing 8 GB / pubtabnet 6 GB / sparse 3 GB） | TEDS / GriTS | `https://ds4sd-public-artifacts.s3.eu-de.cloud-object-storage.appdomain.cloud/datasets/synthtabnet_public/v2.0.0/*.zip`；[IBM/SynthTabNet](https://github.com/IBM/SynthTabNet) | ❌ **未找到** |
| **TabRecSet** | **38.1K 表格 = 20.4K 英文 + 17.7K 中文**（32,072 图 / 38,177 表；中文 16,530 图 / 17,762 表） | 端到端 TR 指标（paper） | ⬇️ **见 §3.2.1 逐文件下载表**；[DOI 10.6084/m9.figshare.20647788](https://doi.org/10.6084/m9.figshare.20647788)；[MaxKinny/TabRecSet](https://github.com/MaxKinny/TabRecSet) | ⚠️ **口径冲突**：GitHub README 写 **CC BY-SA 4.0**，figshare 记录的 license 字段写 **CC BY 4.0**（见 §3.2.1）。论文 [Sci Data 10, 110 (2023)](https://doi.org/10.1038/s41597-023-01985-8) |
| **TAL 表格识别竞赛集（好未来）** | **16,000 训练图**；验证集"不提供可训练标注" | 竞赛指标 | [ai.100tal.com/dataset](https://ai.100tal.com/dataset) | ❌ **未找到** |

**中文覆盖**：TabRecSet（17.7K）是唯一许可明确可用的；TAL（16K）与 WTW 中文场景集许可未知；PubTabNet / PubTables-1M / FinTabNet / TableBank / SciTSR **全为英文**（PubMed Central / arXiv LaTeX 来源）。PaddleOCR 自建的中文表格集（7,835 图）与 59.52/63.69 评测集**均不可下载**（来源：[table_recognition_v2.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleX/refs/heads/release/3.7/docs/pipeline_usage/tutorials/ocr_pipelines/table_recognition_v2.en.md)）。

#### 3.2.1 TabRecSet 逐文件下载清单（已核实，figshare API 元数据）

记录版本 **v9**（`published_date` 2025-07-21，`firstOnline` 2023-01-11），`download_disabled: false`，仓库总计 **5,673,915,294 B（5.28 GiB）**。
元数据端点：`https://api.figshare.com/v2/articles/20647788`（注意 figshare 的 `articleId` = 20647788，与 DOI 后缀一致）。

| 文件 | 大小 | MD5 | 下载地址 | 对本项目的用途 |
| --- | --- | --- | --- | --- |
| `pubtabnet_format.zip` | 9,769,549 B（9.3 MiB） | `602d54889fde4177066e4d1ddd29bbc4` | https://ndownloader.figshare.com/files/56418491 | ⭐ PubTabNet 格式标注，但**是 tokenized 训练格式**（见 §3.2.2 ①）→ TEDS 可比对结构 token 序列，但需一个 detokenize 适配层，**不能直接喂官方 `metric.py`** |
| `cropped_datasets_1Tab_per_img.zip` | 2,101,449,958 B（1.96 GiB） | `3ab78fbf0324ec376e02f7b54bf172d3` | https://ndownloader.figshare.com/files/56419934 | ⭐ **每图单表裁剪版**（LabelMe 4.5.9；`label` = `row-col-rowspan-colspan-text`，见 §3.2.2 ②）→ 匹配 SLANet 单表输入假设，**且自带结构 + 内容真值** |
| `TabRecSet (CurveTabSet).zip` | 3,412,457,623 B（3.18 GiB） | `048fd48511a4989c2b0da3627237fc38` | https://ndownloader.figshare.com/files/40332520 | 主数据集（含曲线/不规则表格，polygon 标注） |
| `tencent OCR.zip` | 20,018,363 B（19.1 MiB） | `959804228a93f46a779abd9220137095` | https://ndownloader.figshare.com/files/39445096 | 腾讯 OCR API 的 OCR 标注（可直接对照 rec 结果） |
| `README.md` | 2,725,577 B（2.6 MiB） | `59ebf9140d51a8bc652504b2e2e05aa0` | https://ndownloader.figshare.com/files/39439612 | 数据组织说明 |
| `TableMe-华工-黄双萍、杨帆 & 石家庄列米智能.exe` | 127,494,224 B（121.6 MiB） | `8ce957dfb5a73a4d3a65670a9d777598` | https://ndownloader.figshare.com/files/56060744 | 官方交互式标注工具（Windows EXE） |

下载（PowerShell，只取评测真正需要的两个包）：

```powershell
$d = "<repo>\datasets\table\tabrecset"
New-Item -ItemType Directory -Force -Path $d | Out-Null
# PubTabNet 格式标注（小，先拿这个验证 TEDS 通路）
Invoke-WebRequest "https://ndownloader.figshare.com/files/56418491" -OutFile "$d\pubtabnet_format.zip"
# 单表裁剪版（大）
Invoke-WebRequest "https://ndownloader.figshare.com/files/56419934" -OutFile "$d\cropped_datasets_1Tab_per_img.zip"
# 校验（应分别等于 602d54889fde4177066e4d1ddd29bbc4 / 3ab78fbf0324ec376e02f7b54bf172d3）
Get-FileHash "$d\pubtabnet_format.zip","$d\cropped_datasets_1Tab_per_img.zip" -Algorithm MD5
```

⚠️ **许可口径冲突（未解决）**：GitHub README 明确写 **CC BY-SA 4.0**（带 creativecommons.org 徽标与链接）；而 figshare 记录本身的 license 字段是 `{"name": "CC BY 4.0", "url": "https://creativecommons.org/licenses/by/4.0/"}`。二者差别在 **ShareAlike 传染性**——CC BY 4.0 允许以更宽松的条款再分发，CC BY-SA 4.0 要求衍生作品沿用同许可。既然实际下载发生在 figshare 上，figshare 字段更可能是"数据实际许可"，但**两个第一方来源互相矛盾**，落地为产品资产前必须澄清。仅作内部评测、不随产品分发时风险可控。

**为什么这条改变了 §3.3 的自建计划**：原本建议"自建集标注格式采用 PubTabNet jsonl 以便复跑官方 TEDS"，现在 TabRecSet 提供的是**需要转换的原料**，不是可直接吸收的成品标注（见 §3.2.2 ①②），所以正确顺序是**先写转换 / detokenize 适配层 → 用 val 的 7,548 张跑通 TEDS → 再决定自建多少页中文集**；自建部分只补 TabRecSet 覆盖不到的维度（见 §3.3 L3 的覆盖矩阵）。

#### 3.2.2 下载后实测（已核验，数据位于 `datasets/table/tabrecset/`）

两个包已下载并解压，实测结构：

| 目录 | 内容 | 规模 |
| --- | --- | --- |
| `pubtabnet_format/` | 12 个 `.json` + 2 个 `*_IMAGE_PATHS.txt`（`train_` / `val_` 各一套） | 14 个文件 |
| `cropped_datasets_1Tab_per_img/train/` | 30,633 个 `.json` + 30,633 个 `.jpg`（**配对差 0，无缺图**） | 2,116.6 MB |
| `cropped_datasets_1Tab_per_img/val/` | 7,548 个 `.json` + 7,548 个 `.jpg`（**配对差 0**） | 418.3 MB |

**① `pubtabnet_format/` 的实际 schema 不是"含 HTML 的 PubTabNet jsonl"，需要适配层。**
它是 **tokenized 训练格式**，8 类文件：`WORDMAP_STRUCTURE.json`（**53 项**，token 字符串 → id；`<pad>/<start>/<end>/<unk>` 之外是结构标签，例如 `rowspan="12"` → 5、`colspan="24"` → 27、`<td>` → 34、`<tr>` → 50、`</tr>` → 43）、`WORDMAP_CELL.json`、`CAPTIONS_STRUCTURE.json`、`CAPTIONS_CELL.json`、`CAPLENS_STRUCTURE.json`、`CAPLENS_CELL.json`、`NUMBER_CELLS_PER_IMAGE.json`、`IMAGE_PATHS.txt`。
→ TEDS 本身就是在**结构 token 序列**上做树编辑距离，所以这条路是通的；但官方 `metric.py` 解析的是 jsonl schema，**必须先写一个 detokenize 适配层**把 id 序列经 `WORDMAP_STRUCTURE` 还原成 token 串。**不要把"能算 TEDS"误读成"可直接喂官方脚本"**——这是本文档前一次修订说过头的一处，此处已更正。

**② `cropped_datasets_1Tab_per_img` 的标注比 README 描述的更适合直接做真值。**
实测（400 张 train 采样 / 10,516 个 cell）：格式为 **LabelMe 4.5.9**（`version: "4.5.9"`、`imagePath`、`imageWidth/Height`、`shapes[]`），`shapes[].shape_type` **100% 为 `polygon`**；`shapes[].label` 的编码是

```text
<row>-<col>-<rowspan>-<colspan>-<cell 文本>
```

字段顺序证据：① 400/400 张图的全部 label 均符合该形态（尾随 `-` 且文本为空 = 空单元格）；② 按「字段3 = rowspan」解释时逻辑网格铺满率 **331/400 = 82.8 %**，按「字段3 = colspan」只有 **301/400 = 75.3 %**；③ 语义上 `1-1-1-3-ACTIVITY BOOSTERS (KITCHEN)` 是跨 3 列标题远比跨 3 行合理，`2-3-1-4-二级菜单` 同理。

⚠️ 但铺满率**只有 82.8 %**，说明它并非严格的"网格完全分区"标注。→ 用它做真值前**必须定义未标注网格位置的处理规则**，否则 TEDS/GriTS 对标注完备性敏感、指标会抖动。这条是 L3 评测协议的前置条件。

**③ 关键精度约束：SLANet-plus 的词表装不下 TabRecSet 的 span。**
`WORDMAP_STRUCTURE.json` 中存在 `rowspan="41"`、`rowspan="23"`、`rowspan="22"`、`colspan="24"`、`colspan="21"`；而 `slanet-plus.onnx` 的 metadata 实测只到 `rowspan="20"` / `colspan="20"`（48 token + sos/eos = 50 类）。
→ **结构性不可达**：这类表在 SLANet-plus 上必然出错，属模型表达能力上限，不是实现缺陷。**TEDS 天花板 < 100 是构造性的**，评测时必须显式扣除这一项，否则会被误判成回归。

**官方指标实现位置**（避免自己重造）：

- **TEDS / TEDS-Struct**：[PubTabNet `src/metric.py`](https://raw.githubusercontent.com/ibm-aur-nlp/PubTabNet/master/src/metric.py)（`class TEDS`，`structure_only=True` → TEDS-Struct；基于 APTED 树编辑距离）。论文 [arXiv 1911.10683](https://arxiv.org/abs/1911.10683)（ECCV 2020）。
- **GriTS**：[table-transformer `src/grits.py`](https://raw.githubusercontent.com/microsoft/table-transformer/main/src/grits.py)；论文 [arXiv 2203.12555](https://arxiv.org/abs/2203.12555)（ICDAR 2023）。
- **cTDaR Track A/B**：[ctdar_measurement_tool](https://github.com/cndplab-founder/ctdar_measurement_tool)。
- **SciTSR 关系**：`scitsr/eval.py`（`json2Relations`、`eval_relations`）。
- ⚠️ **PaddleOCR 本身的表格指标不是 TEDS**：`ppocr/metrics/table_metric.py` 是**结构 token 串精确匹配 acc**（`pred_str == target_str`）；TEDS 只出现在 PP-Structure 的 `eval_table.py` 路径。`ppocr/metrics/` 目录下**没有** TEDS 实现。（来源：[table_metric.py](https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/main/ppocr/metrics/table_metric.py)）

### 3.3 推荐的自建测试集（项目内可行方案）

**分层设计**（测试金字塔，与 `AGENTS.md` §6 的"基线 → 修改 → 回归"一致）：

**L1 — 契约与形状单测（零外部资产，入 `tests/fixtures/`）**
- 仿 `tests/fixtures/formula-onnx/` 造 KB 级 2 输出最小 ONNX，钉住 `(N,L,8)` / `(N,L,50)` 契约与 48-token 换行分隔 metadata 的解析。
- 逻辑网格推导的纯函数单测：构造 rowspan/colspan/空单元格/对角表，断言 `occupied_cells` 网格与 HTML/Markdown 输出。参考 `decode_one_logic_points` 的语义。
- **已有可复用资产**：`refer\RapidTable-main\tests\test_files\table.jpg`（59,389 B）与 `table_without_txt.jpg`（34,907 B）。

**L2 — 真实页面回归（项目内已有，立即可用）**
- `datasets\text\04表格与键值对.png`（417,157 B）已在 `datasets\text\golden-manifest.json` 中，`text` 字段是整页真值、`annotation_scope = "source-dom-visible-text"`。可直接作为端到端回归样本（当前 `boxes` 全为 `[]`，所以只有 CER 没有 IoU）。
- 局限：**只有 1 页真实表格**，且无表格结构真值（无 cell bbox、无 HTML）。**不够**用于结构评测。

**L3 — 表格结构评测集（先接 TabRecSet，再按缺口自建，`datasets\table\`）**
**顺序很重要**：先下载 TabRecSet 的 `pubtabnet_format.zip`（9.3 MiB）把 **TEDS 评测通路跑通**（⚠️ 它是**结构 token 序列**，**不是**含 HTML 的 PubTabNet jsonl —— 必须先写一个 detokenize 适配层把 id 经 `WORDMAP_STRUCTURE` 还原成 token 串，**官方 `metric.py` 不能直接吃它**，见 §3.2.2 ①），确认指标口径无误后，再只对 TabRecSet 覆盖不到的维度自建补充集。这样自建量最小、且自建集与公开集共用同一套指标实现，结果可比。

1. **来源**：① TabRecSet 的 `cropped_datasets_1Tab_per_img.zip`（单表裁剪版，直接匹配 SLANet 单表输入）+ `pubtabnet_format.zip`（**结构 token 序列格式，需 detokenize 适配层**）—— 见 §3.2.1；② 项目自有文档页面截图；③ 其它公开可再分发的中文表格。⚠️ 许可口径冲突见 §3.2.1，落地为制品前需澄清。
2. **标注格式**：直接采用 **PubTabNet 的 jsonl schema**（`{filename, split, imgid, html:{structure:{tokens[]}, cell:[{tokens[], bbox[x0,y0,x1,y1]}]}}`），这样 TEDS 参考实现可以**直接跑**，不需要自己写指标；TabRecSet 已经是这个格式，自建集照抄即可对齐。
3. **规模建议**：**自建部分**首期 30–50 页（TabRecSet 已覆盖的维度不必重复），覆盖矩阵必须显式包含 TabRecSet 覆盖不足的项：中文扫描歪斜、无线框表格、rowspan ≥ 2 与 colspan ≥ 2 的嵌套组合、多行单元格、跨页表格。
4. **标注方式**：
   - **结构**：用 HTML 真值（人工校对），TEDS-Struct 作为结构指标。
   - **cell bbox**：允许半人工 —— 由现有 det 文本框 + 表格线检测半自动生成，再人工修正。
   - **cell 文本**：用整页 OCR 结果 + 人工校对，而不是逐格录入（成本降一个量级）。
5. **抽样必须稳定哈希**：复用 `src/evaluation/formula/sampling.rs` 的 `SampleStrategy::Hash`（数据集相对路径 + 真值稳定哈希，明确禁止"取前 N"），并把 `manifest_sha256` / `sample_set_sha256` / `content_sha256` 写进 manifest。这套机制已经在公式评测里落地，表格域应原样复用。
6. **指标挂载**：新增 `src/evaluation/table/`，与 `src/evaluation/{ocr,formula}.rs` 平级 —— `src/evaluation/mod.rs` 已明确"两类指标不得混合成一个默认汇总"，表格是第三个域，同样不得混入 ocr/formula 汇总。
7. **报告口径复用**：`src/bin/rapidstruct.rs:334` 的 `evaluation_report_value(&EvaluationSummary) -> Result<serde_json::Value,_>` 是报告序列化的唯一入口（CLI `rapidstruct evaluate` 与 `POST /api/evaluate` 共用），表格域应挂到同一出口。

**L4 — 性能基线（复用既有工具，不新建框架）**
- `pwsh -NoProfile -File tools/run_windows_baseline.ps1` → 写 `tests/baseline/`。
- 判据用 `src/evaluation/stats.rs` 的 `Stats`（p50/p95 的唯一口径）。
- provider 对比必须用 `tools/check_provider_claims.ps1 -Reference <cpu.json> -Reports <other.json>`，因为 `src/runtime/provider.rs` 的 `selected_ep` **只表示 EP 链头部**，不代表节点真的跑在该 EP 上。

---

## 4. 未确认 / 查不到的项登记（不编造）

**关于模型**
- SLANet_plus / SLANeXt 的**独立论文**：不存在。PaddleOCR 官方论文页明确写"未找到其独立论文，方法由上层报告覆盖"。SLANet 本身没有独立论文，只在 **PP-StructureV2**（[arXiv 2210.05391](https://arxiv.org/abs/2210.05391)）内提出。
- PP-StructureV2 的**同行评审会议/期刊**：未找到（官方目录仅列为 arXiv preprint）。
- SLANet_plus 在 **PubTabNet / cTDaR / WTW 上的 TEDS**：未找到（官方只有内部数据集 Accuracy 63.69）。
- PaddleOCR/PaddleX **ONNX 模型的 INT8 量化官方指引**：未找到。
- PaddleX **ONNX Runtime 后端的 FP16**：未找到（FP16 仅记录于 TensorRT 后端）。
- **官方发布的 SLANet ONNX**：不存在。HF 仓库文件列表只有 `.gitattributes / README.md / config.json / inference.json / inference.pdiparams / inference.yml`。ONNX 必须自行用 `paddle2onnx` 转换（官方默认 `--opset_version 7`，失败时自动升版；来源 [paddle2onnx.en.md](https://raw.githubusercontent.com/PaddlePaddle/PaddleX/release/3.7/docs/pipeline_deploy/paddle2onnx.en.md)）。本项目 `models/table/slanet-plus.onnx` 来自 RapidAI/RapidTable v2.0.0，是**第三方重打包**，不是官方制品 —— **其转换参数（opset、是否裁剪）无官方文档可查**。
- **PP-OCRv6 det 被官方特别推荐用于表格**：未找到。官方表格管线列的是 PP-OCRv5_server/mobile_det。
- 名称冲突：另有一个无关的 "SLANet"（Qasim/Mahmood et al., ICDAR 2019, "Rethinking Table Recognition using Graph Neural Networks"），一手 venue/DOI 未核实。
- **S-TEDS** 指标的官方定义/实现：未找到。

**关于数据集**
- FinTabNet 原始一手下载：未找到（IBM 页面已失效）。
- cTDaR 可用下载：未找到（官方 GitHub 404）。
- SciTSR / WTW / cTDaR / ICDAR-2013 GT / SynthTabNet / TAL 的**许可**：均未找到。
- WTW 官方评测代码：未找到（repo 标 "To be updated"）。
- PubTabNet 许可冲突（`cdla-sharing-1.0` vs "CDLA-Permissive"）与 TableBank 许可冲突（`apache-2.0` vs "请勿再分发"）：**官方两侧说法不一致，未解决**。
- PubTabNet 是否发布 test 集真值：未确认（部分资料称未发布 test GT）。

**关于本机/本项目**
- `rescale_cell_bboxes` 的坐标系语义：**未实测**（设计文档本身标为 `<待确认>`）。这是 R1 的前置阻塞项。
- DirectML 下 SLANet 的 `Loop` 控制流算子是否可执行：**未验证**（本机无 DirectML DLL）。
- 固定 H/W 后 SLANet 的实际加速比：**未实测**（Python 环境数字不可用）。
- 表格结构模型的最优线程数：**未定**（4 vs 8 线程的两个口径互相矛盾，落在噪声带内）。
- `input_other_ms = 79.86`（8.1 %）的构成：**未定位**。已定位的候选窗口是 `src/ocr/pipeline/rapid_structure.rs:156-162`（resize ≈4.7 ms/页）、`:189-195`（vertical padding 0.8–3.5）、`:210-212`（crop 1.5–2.5）、`src/ocr/rec/recognizer.rs:89-98,222-231`（批次装配/排序/bidi 0.2–2.1）、`rapid_structure.rs:106-132`（≈0.2），但这些加起来远小于 80 ms，**说明还有一个未识别的输入侧区段**。这一项建议先做定位再谈优化。

---

## 5. 建议的执行顺序

| 序 | 动作 | 前置 | 判据 |
| --- | --- | --- | --- |
| 1 | R9 把表格分量接入 `TimingLedger` | 无 | `shares()` 求和 = 1；`timing_split` 恒等式成立 |
| 2 | R3 `OrtSession` 支持多输出 + 契约测试 | 无 | 2 输出契约单测 + `slanet-plus.onnx` golden |
| 3 | 用 golden 钉死 `rescale_cell_bboxes` 的坐标系 | R3 | cell bbox 与人工标注一致 |
| 4 | R4 固定 H/W（不固定 L）+ 数值逐位一致断言 | R3 | 输出逐位相等；告警消失 |
| 5 | R1 回填改"整页 det 复用 + 几何匹配" | 3、4 | 回填净增 rec 成本 = 0；cell 文本非空率不降 |
| 6 | R2 表格路径独立 det 分辨率（用 L3 数据集定量） | L3 数据集 | CER 在容差内（mean CER ≤ 0.03）且 p50 下降 |
| 7 | R8 表格定位走文本分布启发式 | 3 | 区域召回达标；`ocr_total_ms` 无增量 |
| 8 | R5 表格 session 线程数（仅在超出噪声带时才改） | 1 | 线程矩阵 p50 超出 ±10–15 % |
| 9 | R7 DirectML per-EP 配置（仅在有 DML 设备时） | 6 | `check_provider_claims.ps1` 通过 |

**贯穿全程的硬约束**：每一步都必须有"改前基线 → 改动 → 新行为验证 → 既有行为验证 → 对比"的完整闭环（`AGENTS.md` §6/§9），并且**性能数字一律来自 Rust harness**，不得引用本文档中带 ⚠️ 的 Python 探针绝对值。
