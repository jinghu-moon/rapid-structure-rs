# `table-golden` —— 表格结构模型的官方参考 golden

由 `tools/build_table_golden.py` 生成，绑定的模型是 `models/table/slanet-plus.onnx`。

**用途**：把"结构解码"的每一条契约钉死在**官方参考实现自己的输出**上，而不是钉在我们
对文档的解读上。参考实现 = 本仓库已检入的 `refer/RapidTable-main`，其中的
`rapid_table/table_structure/pp_structure/post_process.py`（PaddleOCR 的
`TableLabelDecode`）与 `rapid_table/table_matcher/main.py`（`TableMatch.decode_one_logic_points`）。
生成脚本用 `importlib` **逐字加载这两个文件本体**（不执行任何 `__init__.py`，只给
`rapid_table.utils.typings.ModelType` 打一个枚举桩），所以 golden 不是"照着重写一遍"，
而是"调用参考实现本身"。

Rust 侧的消费者是 `src/table/decode.rs` 里的 `#[cfg(test)] mod golden_tests`
（8 条测试），`cargo test --lib table::decode::golden_tests` 即可运行。

## 文件

| 文件 | 大小 | 内容 |
| --- | --- | --- |
| `table.jpg` | 59,389 B | 源图，`371×293`（宽 × 高）。sha256 `d754f91e263b0392708e48050971722f86a3a863c2497e73869802066b7dfc06` |
| `input_bgr_resized.bin` | 563,640 B | **`resize` 之后、`normalize` 之前**的 uint8 BGR，`488×385`（宽 × 高）。`385 × 488 × 3 = 563,640` |
| `structure.npy` | 23,328 B | `scale_1.tmp_0`（结构 logits），形状 `(1, 116, 50)`，`<f4`，C 序 |
| `boxes.npy` | 3,840 B | `scale_0.tmp_0`（单元格多边形），形状 `(1, 116, 8)`，`<f4`，C 序。`116 × 8 × 4 + 128 = 3,840` |
| `manifest.json` | 114,334 B | 全部元数据、逐阶段坐标、参考 token 流、逻辑网格 |

### 为什么存 `resize` 后的图而不是原图

`cv2.resize` 的插值结果会随 OpenCV 版本与 SIMD 后端变化，是整条链路上**唯一**不可
逐位复现的一步。把它**前置**烘焙进 `input_bgr_resized.bin` 之后，Rust 侧只剩
`normalize → pad → to_chw` 三步纯算术，才能做逐位比对。

这也解释了一个不直观的细节：`resize` 输出是 `488×385` 而不是 `488×488`。
`TablePreprocess` 的顺序是 **resize → normalize → pad → to_chw**，补零发生在
**归一化之后**，所以补进去的零是"归一化空间的 0"（即 (0 - mean) / std），
不是像素 0。

## 实测数字

```
L               = 116     （动态，不是 Loop 的 trip 上限 500）
token 步        = 114     （跳过 sos/eos、在 eos 处终止之后）
单元格          = 80      （16 行 × 5 列）
被丢弃的占位框  = 0
confidence      = 0.996027
词表            = 48 个片段 → 50 类，sos = 0，eos = 49
ratio           = 1.3153638814016173
w_ratio         = 1.0            （横表，与 doc 07 §12 R1 一致）
h_ratio         = 1.266211604095563
max(h, w)       = 371
grid            = 16 × 5
```

坐标范围（`after_rescale`）：x ∈ [1.1402, 368.7613]、y ∈ [0.7248, 285.8035]，
全部落在 `371×293` 之内。

### 两个被 golden 直接推翻的直觉

1. **`sos` 不在下标 0。** 真实模型第 0 步是 `<tbody>`；`eos` 在下标 114、
   `sos` 在下标 115（也就是 `sos` 被排在整条序列的最后）。参考实现的
   `if idx > 0 and char_idx == end_idx: break` 正是为这种排布准备的。
   `boxes.npy` / `structure.npy` 是**逐 step 对齐**的两路输出，因此取框必须用
   **原始 step 下标**（本仓库 `TokenStep::step_index`），不能用"过滤后的第 n 个"。

2. **坐标空间不是 488 画布。** 见下节。

## 坐标空间：`after_rescale` 是**原图（crop）像素**

参考实现分两步：

```text
_bbox_decode(bbox, shape_list)     # bbox[0::2] *= 原图 w；bbox[1::2] *= 原图 h
rescale_cell_bboxes(img, bboxes)   # [:, 0::2] *= w_ratio；[:, 1::2] *= h_ratio
                                   # w_ratio = 488/(w*ratio)，h_ratio = 488/(h*ratio)
```

两步复合后，x 与 y 的总因子都是 `488 / ratio = max(h, w)` —— 中间的 `_bbox_decode`
在代数上被完全抵消。所以 `after_rescale = raw_normalized × max(h, w)`，量纲是
**原图像素**，只有 `max(h, w) == 488` 时才恰好等于画布坐标。

`manifest.json` 对每个单元格存了三套坐标，Rust 侧三套都比：

| 键 | 含义 |
| --- | --- |
| `raw_normalized` | 模型 `scale_0.tmp_0` 的原始输出，未做任何变换 |
| `after_bbox_decode` | 只做 `_bbox_decode`（`× [原图 w, 原图 h]`） |
| `after_rescale` | 再做 `rescale_cell_bboxes`（`× [w_ratio, h_ratio]`） |

只比最终值无法区分"两步各自正确"与"两步的误差互相抵消"，所以三套都断言。

### 三种假设的目视对照（`Temp/e4-overlay-*.png`，用 `table.jpg` 渲染）

| 假设 | 渲染结果 |
| --- | --- |
| `after_rescale`（本结论） | 红框**贴合表格网格线** |
| `raw_normalized × 488`（画布假设） | **右侧列溢出画面**、纵向错位 —— 被证伪 |
| 只做 `_bbox_decode`（按原图 w/h，不做 rescale） | 横向对、**纵向被压缩** `371/293 = 1.266` 倍 —— 被证伪 |

第三行尤其关键：如果模型是按 `resize` 后的尺寸归一化，那么只做 `_bbox_decode` 就
应当正确，而实测它明显偏小。这与"模型输出归一化到补零后的 `488×488` 画布"完全吻合。

`golden_pins_the_coordinate_space_to_the_original_crop` 把这条结论写成了断言，
并额外断言"`×488` 假设必然跑出原图"——让反事实假设**可被证伪**，而不只是"我们
选了一个看起来对的"。

### 与本仓库路线的差异

`RapidTable` 把结构模型直接跑在**整张图**上（`refer/RapidTable-main/rapid_table/main.py:98-110`
不裁剪），所以它的 cell 坐标天然就是页面坐标。本仓库走"在页面 crop 里找小表"的路线，
**crop 偏移量必须另外加上**（见 doc 09 的 E9）。

## 容差

| 量 | 实测偏差 | 使用的容差 | 依据 |
| --- | --- | --- | --- |
| 逐阶段复现（`after_bbox_decode`、`after_rescale`） | **0.0**（逐位一致） | `1e-6` | 两侧都是"每坐标一次 `f32` 乘法"；371 量级上一个 f32 ulp ≈ 3.05e-5，故 `1e-6` 远小于一个 ulp |
| 代数不变量 `raw × max(h, w)` | **3.0517578125e-05** | `1e-4` | 与逐阶段复现**不是同一算式**（一次乘法 vs 两次相乘），差恰好一个 ulp |
| confidence / 逐 cell 分数 | 测试全绿 | `1e-5` | 参考实现按 `float64` 对**全部 114 个非跳过步**求均值 → `0.996027`；而 `cells[].score` 只是其中 **80 个单元格步**的分数（均值 `0.999756`）。两者分母不同，**不是同一个量**，不要互相代用 |

`UNIFORM_SCALE_TOLERANCE` 比 `COORD_TOLERANCE` 宽**不是**在放水：它是"代数等价、
算式不同"必然带来的一个 ulp 舍入差，两者被刻意分开，免得下一次有人为了调通不变量
而把逐阶段比对的精度一起放宽。

## 复现

```powershell
# 重新生成（会重跑真实模型，约需 Python + onnxruntime + opencv + pillow）
python tools/build_table_golden.py

# 只校验已落盘 golden 的自洽性，不重跑模型、不需要 onnxruntime
python tools/build_table_golden.py --verify

# Rust 侧
cargo test --lib table::decode::golden_tests
```

脚本会先核对模型身份（`7,758,305` 字节 /
`d57a942af6a2f57d6a4a0372573c696a2379bf5857c45e2ac69993f3b334514b`），
不符就退出码 2；随后把"逐阶段复现"与"参考实现自己的返回值"对拍，不符就退出码 3。

`--verify` 的**职责边界**：它只查 `manifest.json` 内部各处是否互相矛盾，以及各文件
的字节数 / SHA-256 是否与记载一致（模型、`table.jpg`、`input_bgr_resized.bin`），
加上两个 `.npy` 的**形状与 dtype**、词表自洽（`class_count == token_count + 2`、
`sos == 0`、`eos == class_count - 1`）、`cells` 与 `logic_points` 一一对应、
坐标不变量与"框不越出原图"、grid 与 logic_points 一致、以及 HTML 恰好是 token 流
外包三层标签。**它不核对 `.npy` 里的数值**：等尺寸的内容篡改不是它的职责，那是
Rust 侧 golden 测试的工作。损坏会逐条打印并返回 3；manifest 缺失/不可读返回 2 或 3。

**本机 Python `onnxruntime` 是 1.20.1，而 Rust 侧静态链接的是 1.28**，因此
`boxes.npy` / `structure.npy` 一旦重生成，数值可能有微小漂移；golden 是**冻结产物**，
不随环境变化，只有显式重跑才会变。
