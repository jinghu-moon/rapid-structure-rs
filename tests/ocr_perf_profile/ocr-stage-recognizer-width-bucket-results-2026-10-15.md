# OCR 优化阶段结果：Recognizer width bucket

## 阶段目标

在 medium + DirectML、`rec_batch_num=6`、同一 12 图数据集下，试验按 crop 宽高比限制 batch 内跨度，观察是否能降低 padding 而不增加推理调用成本。候选阈值为 1.5 和 2.0；默认固定批行为作为对照。

## 结果

| 策略 | OCR mean ms | p50 ms | p90 ms | recognizer infer ms | 平均批数/图 | mean CER | 峰值累计 KB |
|---|---:|---:|---:|---:|---:|---:|---:|
| 固定 batch 6（对照） | 379.7 | 376.3 | 497.4 | 210.628 | 6.2 | 0.4320880473 | 1,177,780 |
| width bucket 1.5 | 478.7 | 509.1 | 642.7 | 306.385 | 8.1 | 未重复评测 | 847,660 |
| width bucket 2.0 | 470.9 | 511.6 | 618.0 | 298.886 | 8.0 | 0.4314555228 | 835,736 |

1.5 阈值把 padding 降低到约 17.5%，但平均批数增加，recognizer inference 比对照慢约 45%，总延迟慢约 26%。2.0 阈值仍使 recognizer inference 慢约 42%、总延迟慢约 24%；CER 的微小改善不足以抵消性能退化。其 padding 下降来自更多批次，而非更高的单批吞吐。

## 决策

**否决 width bucket，回退本阶段生产代码。** 实验新增的配置字段、范围生成逻辑和单元测试均已移除；默认 recognizer 仍按宽高比排序后以 `rec_batch_num=6` 固定切批。候选 profiler/CER JSON 保留为否决证据。

后续不继续增加阈值实体。更有价值的方向是分析 DirectML 对动态 batch shape 的调用开销，或在确认 ORT 能复用合适形状的前提下设计有限形状集合；这需要新的测量依据后再实施。

## 证据文件

- `ocr-provider-directml-widthbucket-1p5.json`
- `ocr-provider-directml-widthbucket-2p0.json`
- `ocr-eval-directml-widthbucket-2p0.json`
- 对照：`ocr-provider-directml-batch-6.json`、`ocr-eval-directml-batch-6.json`
