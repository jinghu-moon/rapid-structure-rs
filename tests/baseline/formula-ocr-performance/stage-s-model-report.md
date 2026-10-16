# Formula OCR Stage 1: PP-FormulaNet+ S

采集日期：2026-10-16

本阶段评估 `PP-FormulaNet_plus-S` 的拆分 ONNX 图，目标是验证更小的 encoder + cross-attention prep + cached decoder 是否能替换当前 PP-FormulaNet+ M 单体图。

## 测试条件

- 固定输入：`manifest-val64.json`，64 张 `latexocr_example/validate` 图片。
- CPU：Intel Core i5-13600KF，ORT intra-op 14、inter-op 1，ONNX Runtime 1.20.1，CPUExecutionProvider。
- batch：8；预热 1 次；端到端模型推理计时不含模型加载和 tokenizer 解码。
- 图像预处理按 Rust `FormulaPreprocessor` 语义复现：裁剪、Pillow 双线性缩放、居中填充、白色 padding、BGR 灰度归一化。
- S 文件 SHA-256：
  - encoder `a021388d62aa43953b1dc7a3e0ea919b076aa8fdb98d8491830c6dbf2e5e4706`
  - prep `c18cf4eb94c3d16cf88faad285333a5b4b30b6e1718c2b92b4564baf984fbd7e`
  - decoder `82c80534beccd1a2194dc1aaa4c1239dff8f00f22e64fac5b7250be2e994ef5f`
  - tokenizer `2811d82701ec97c192fa256aa2b4516929373870ae660326cc5b1dc879b95ff2`
- M 基线模型 SHA-256：`71b6d389cf7b857e45252a4b98cfced1a3ffca7bf24d9497d02d052a41d9493b`。

S decoder 的真实调用契约已验证：初始 `tokens=[0,0,0]`，`pos=0`，空 KV 的形状为 `[2,B,16,0,24]`；每次返回连续 3 个 token 和 3 个 KV 位置，下一步 `pos += 3` 并沿 KV 轴追加。S 的第一段输出与 M 单体图逐 token 对齐，证明拆分图调用顺序正确。

## 性能与质量

| 指标 | M 单体 | S 拆分 | 差异 |
| --- | ---: | ---: | ---: |
| 运行 1 模型推理均值 | 598.3 ms/图 | 206.9 ms/图 | S 快 65.4% |
| 运行 2 模型推理均值 | 591.2 ms/图 | 197.0 ms/图 | S 快 66.7% |
| exact match（64） | 18 | 13 | -5 |
| normalized match（64） | 18 | 13 | -5 |
| mean CER | 0.06716 | 0.09682 | +0.02966 |
| EOS 截断 | 0 | 1 | 退化 |
| EOS 前 token 完全一致 | - | 31/64 | 质量不稳定 |

这里的速度是三个 S 图和一个 M 图的 Python ORT 模型推理时间，不能直接替代已有 Rust `formula_eval` 的完整墙钟指标；它足以作为候选筛选，因为两者使用同一进程、同一批次、同一 ORT provider 和同一输入张量。S 的质量已经在固定集上低于 M，速度收益不足以改变这个结论。

## 结论

**否决 S 作为默认模型，未修改 Rust 生产路径。**

否决依据是质量门槛，而不是速度：S 在固定 64 样本上 exact、normalized match 各少 5 条，CER 增加约 44%，并产生截断。按照优化方案，固定集质量失败后不再把候选扩展到完整 501 样本，也不引入三套新会话和模型注册逻辑。当前 M 实现保持不变。

## 后续实验

1. 先确认是否存在与当前 M 训练/词表完全一致、并能保持 M 输出的 S 导出版本；当前拆分 S 图不能作为等价替代。
2. 若没有等价小模型，评估单独的 FP16/INT8 M 图，并保持同一 64 样本质量门槛；CPU 上 FP16 只有在 ORT 有实际硬件路径时才有依据。
3. 只有精度候选通过后，才将拆分会话接口重构进共享 runtime；不为被否决的 S 候选保留生产代码。
