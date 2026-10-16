# Formula OCR Stage 2: M precision variants

采集日期：2026-10-16

本阶段评估当前 PP-FormulaNet+ M 单体 ONNX 图的 CPU 动态 INT8 与 FP16 变体。固定输入、预处理、batch、线程和 ORT 条件与 Stage 1 相同：`manifest-val64.json`、64 张验证图、batch 8、warmup 1、CPUExecutionProvider、ORT 1.20.1、intra-op 14、inter-op 1。

## 候选与加载结果

| 候选 | 文件大小 | 结果 |
| --- | ---: | --- |
| QInt8 dynamic Conv (`ConvInteger`) | 387,515,519 bytes | CPU ORT 无 `ConvInteger` 实现，session 创建失败 |
| QUInt8 dynamic Conv (`QLinearConv`) | 387,515,611 bytes | 可运行，但速度退化 |
| FP16，保留 FP32 IO | 297,981,874 bytes | 可运行，输出与 M 完全一致，但 CPU 极慢 |

候选 SHA-256：

- QInt8：`c3024d172510ff8e751b94e298b9de395e214e1365ec615a104698aeb6e14ce8`
- QUInt8：`c0a19c54c284ba0ff37e0c536263cacd3737861dbb2a6fd8a91cfdaf96de6aee`
- FP16：`5991a12e999987bcc2b47addb3b45c3ebcdc709ed4f6f2b4e7c6d6548ac06b6d`

## 结果

| 指标 | M FP32 | QUInt8 | FP16 |
| --- | ---: | ---: | ---: |
| 模型推理均值 | 598.3 ms/图 | 819.6 ms/图 | 2,341.4 ms/图 |
| 相对 M | 基线 | 慢 37.0% | 慢 291.3% |
| 与 M EOS 前 token 一致 | - | 27/64 | 64/64 |
| exact match（对验证标签） | 18/64 | 20/64 | 18/64 |
| normalized match | 18/64 | 20/64 | 18/64 |
| mean CER | 0.06716 | 0.06243 | 0.06716 |
| 截断 | 0 | 0 | 0 |

QUInt8 偶然改善了这个 64 样本子集的标签分数，但它改变了 37 条 M 输出且端到端模型推理更慢；这不是可接受的性能优化。QInt8 没有运行结果，因为运行时不支持生成的 `ConvInteger` 算子。FP16 的 token、LaTeX、EOS 和截断均保持一致，说明转换语义正确，但 CPU 没有 FP16 卷积收益，实际成本是 FP32/FP16 转换和未优化的 FP16 算子路径。

## 结论

**否决所有 precision 候选，不修改默认模型或 runtime。**

当前 M FP32 仍是 CPU 默认。没有把量化模型纳入模型注册表，也没有在 Rust 中增加精度分支；这样不会留下运行时无法加载的 QInt8 图，也不会以 4 倍 CPU 延迟换取相同输出的 FP16 图。

下一阶段回到方案中的增量解码：只有能由模型导出器提供正确的 encoder + cached decoder 图，才有减少整图 `Loop` 重算的依据。Rust 侧包一层循环调用当前 M 图不会改变计算量，不应实施。
