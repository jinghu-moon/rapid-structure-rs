# Formula OCR Stage 3: incremental decoding

采集日期：2026-10-16

本阶段评估 docparser-rs 发布的 PP-FormulaNet+ M 拆分图：encoder、cross-attention prep、单 token `decoder_step`。目标是确认 KV cache 是否能替代单体 M 图内部的 `Loop`，并在不改变输出的前提下减少推理时间。

## 测试条件

- 固定输入：`manifest-val64.json`，64 张 `latexocr_example/validate` 图片。
- CPU：Intel Core i5-13600KF，ORT intra-op 14、inter-op 1，ONNX Runtime 1.20.1，CPUExecutionProvider。
- batch：8；warmup 1；预处理和 tokenizer 解码方式与 Rust `FormulaPreprocessor` / `FormulaTokenizer` 对齐。
- M 单体基线 SHA-256：`71b6d389cf7b857e45252a4b98cfced1a3ffca7bf24d9497d02d052a41d9493b`。
- 拆分图 SHA-256：
  - encoder `e15dc1f333c680ba2b9eef5a8be713391ad9b587c074ff5120254063ea7a1244`
  - prep `6fd37a4dc090b28c1a2a62516ec863786f8033dca14638400c0f7e3ce71d061f`
  - decoder `bb4a0d6f868bf60e2cf551203332db6c3f3ba1b838110ba03f669a5fc025787c`
  - tokenizer `2811d82701ec97c192fa256aa2b4516929373870ae660326cc5b1dc879b95ff2`

M 拆分图的实际契约为 `tokens [B,1]`、`pos [B]`、`kb/vb [6,B,16,KV,32]`、cross KV `[6,B,16,144,32]`；初始 token 为 BOS `0`、位置为 `0`、KV 长度为 `0`，每步追加一个 token 和一个 KV 位置。该调用顺序由真实图 probe 验证。

## 结果

| 指标 | M 单体 | M 拆分 + KV | 差异 |
| --- | ---: | ---: | ---: |
| batch-8 模型推理均值 | 607.8 ms/图 | 1401.4 ms/图 | 慢 130.6% |
| EOS 前 token 完全一致 | - | 64/64 | 保持 |
| LaTeX 完全一致 | - | 64/64 | 保持 |
| exact match（64） | 18/64 | 18/64 | 保持 |
| normalized match（64） | 18/64 | 18/64 | 保持 |
| mean CER | 0.06716 | 0.06716 | 保持 |
| 截断 | 0 | 0 | 保持 |

拆分图的输出质量证明 encoder/prep/decoder 导出和 KV 更新语义正确；性能结果证明在当前 CPU ORT API 下，逐 token `Session::run` 调度成本超过了单体图内部 Loop 的收益。把这套循环搬到 Rust 中不会改变该事实。

本候选在固定 64 样本筛选阶段已慢约 2.3 倍，因此按更新后的分阶段验收规则，不再运行完整 501 样本；完整集只用于通过固定集延迟与输出等价筛选、实际可能被采纳的候选。

## 结论

**否决拆分 M 作为默认实现，未修改 Rust 生产路径。**

这是一个质量等价但性能退化的候选，不能因为支持取消或流式解码就当作 OCR 性能优化。只有在未来有真正的 fused decoder、设备端图循环或能显著降低跨 `Session::run` 调度成本的 runtime API 时，才应重新评估。

三阶段实验均未找到可采纳的默认模型优化；当前 M 单体 FP32 CPU 路径保持不变。
