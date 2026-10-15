# OCR 优化阶段结果：Recognizer batch

## 阶段目标

固定 PP-OCRv6 medium、DirectML、`max_side_len=2000`、自动线程计划和 12 张 OCR 测试图，只扫描 `rec_batch_num = 1, 2, 4, 6, 8, 12, 16`。每个候选使用 1 轮 warmup 和 2 轮正式测量，并记录 recognizer 批详情、padding、延迟、峰值工作集和 CER。

所有 profiler 报告均确认 detector/recognizer 的 `selected_ep=DirectMl`、`fallback_to_cpu=false`。CER 对照使用相同的 DirectML 严格配置：batch 6 是当前基线，batch 12 是 recognizer inference 最低的候选。

## 性能结果

| rec_batch_num | OCR mean ms | p50 ms | p90 ms | recognizer infer ms | 平均批数/图 | padding ratio | profiler peak cumulative KB |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 791.5 | 712.4 | 1155.0 | 621.981 | 35.2 | 10.25% | 574,980 |
| 2 | 519.0 | 484.7 | 711.7 | 353.487 | 17.8 | 13.51% | 655,732 |
| 4 | 401.2 | 384.8 | 531.5 | 237.514 | 9.2 | 20.99% | 848,128 |
| 6（当前） | 379.7 | 376.3 | 497.4 | 210.628 | 6.2 | 27.71% | 1,177,780 |
| 8 | 384.8 | 432.3 | 474.4 | 216.749 | 4.8 | 31.89% | 842,264 |
| 12 | 381.9 | 380.3 | 496.0 | 204.860 | 3.3 | 43.19% | 1,410,388 |
| 16 | 385.5 | 389.8 | 488.6 | 202.485 | 2.7 | 43.87% | 1,328,416 |

batch 12/16 只把 recognizer inference 降低约 2.7%/3.9%，但 OCR 总均值比 batch 6 高约 0.6%/1.5%，padding 增加到约 43%，且 batch 12 的峰值累计工作集比 batch 6 高约 20%。batch 1/2/4 的 padding 较低或适中，却因调用次数增加而明显变慢。

## 正确性结果

| 配置 | mean CER | peak working set（评测进程） |
|---|---:|---:|
| batch 6（对照） | 0.4320880473 | 1,205,727,232 bytes |
| batch 12 | 0.4332090318 | 1,455,415,296 bytes |

batch 12 的 CER 略高于 batch 6，且峰值工作集更高。两者逐 case 的主要文本结果相同，但没有可量化的准确率收益。

## 决策

**否决本阶段的 batch 默认值改动，保留 `rec_batch_num=6`。** 低 padding 不是优化目标本身；实际 inference 和端到端总延迟证明更小批次的调度成本更高，更大批次的 padding 和内存成本又抵消了 inference 收益。没有生产配置需要回退。

下一步应转向宽度分桶：保持模型和 batch 上限 6，只按目标宽度相近的 crop 组成 batch，验证在不增加调用次数的条件下能否降低 padding；仍须同时通过 CER 和端到端延迟门槛。

## 证据文件

- `ocr-provider-directml-batch-1.json`
- `ocr-provider-directml-batch-2.json`
- `ocr-provider-directml-batch-4.json`
- `ocr-provider-directml-batch-6.json`
- `ocr-provider-directml-batch-8.json`
- `ocr-provider-directml-batch-12.json`
- `ocr-provider-directml-batch-16.json`
- `ocr-eval-directml-batch-6.json`
- `ocr-eval-directml-batch-12.json`
