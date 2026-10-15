# OCR 优化阶段结果：Recognizer 模型

## 阶段目标

固定 DirectML、`max_side_len=2000`、自动线程计划和同一份 12 图 golden manifest，只替换 PP-OCRv6 的 medium、small、tiny 模型，判断更小的 recognizer 是否在保持 OCR 正确率的同时降低延迟和内存。

所有 profiler 运行均使用 `fail_if_provider_unavailable=true`，并确认 detector/recognizer 的 `selected_ep=DirectMl`、`fallback_to_cpu=false`。medium 对照为同参数 3 轮、1 轮 warmup 的重复测量；CER 使用同一 manifest 和 DirectML 严格配置。

## 结果

| 模型 | OCR mean ms | p50 ms | p90 ms | recognizer infer ms | detector infer ms | profiler peak cumulative KB | mean CER |
|---|---:|---:|---:|---:|---:|---:|---:|
| medium（对照） | 375.4 | 376.9 | 484.4 | 206.307 | 53.419 | 1,172,344 | 0.4320880473 |
| small | 339.1 | 340.4 | 448.6 | 189.815 | 37.693 | 775,728 | 0.4476513565 |
| tiny | 224.0 | 205.5 | 285.6 | 89.693 | 29.254 | 597,288 | 0.4703094065 |

相对 medium，small 的 OCR mean 降低约 9.7%，tiny 降低约 40.3%；两者的 profiler 累计峰值也更低。但 CER 分别上升 0.0155633092（约 3.6%）和 0.0382213593（约 8.8%），因此速度收益不能抵消 OCR 正确率回归。

关键样例也出现退化：small 的 `03旋转与倾斜.png` CER 从 0.374502 上升到 0.470120，`08数字公式与符号.png` 从 0.345133 上升到 0.390645；tiny 的 `02多语言与RTL混排.png` CER 为 0.673152，`09竖排文本.png` 从 0.112903 上升到 0.254839。

## 决策

**否决 small 和 tiny。** 本阶段没有生产配置或模型默认值变更，也没有需要回退的代码改动。更小模型的速度和内存数据作为后续可选的低准确率档位证据保留，但不能作为当前 OCR 默认优化。

下一阶段按计划进入 recognizer batch 实验，先固定 medium + DirectML，比较 `rec_batch_num` 和宽度分桶对 padding、recognizer inference、总延迟及 CER 的影响。

## 证据文件

- `ocr-provider-directml-medium-repeat.json`
- `ocr-provider-directml-small.json`
- `ocr-provider-directml-tiny.json`
- `ocr-eval-small-directml.json`
- `ocr-eval-tiny-directml.json`
- medium CER 对照：`ocr-eval-directml-2026-10-15.json`

## 复现实验

```text
cargo run --release --features directml-provider --bin perf_profile -- --config Temp/ocr-small-directml-strict.yaml --provider directml --rounds 3 --warmup 1 --output tests/ocr_perf_profile/ocr-provider-directml-small.json
cargo run --release --features directml-provider --bin perf_profile -- --config Temp/ocr-tiny-directml-strict.yaml --provider directml --rounds 3 --warmup 1 --output tests/ocr_perf_profile/ocr-provider-directml-tiny.json
cargo run --release --features directml-provider --bin rapidstruct -- evaluate --manifest datasets/text/golden-manifest.json --config Temp/ocr-small-directml-eval.yaml --output tests/ocr_perf_profile/ocr-eval-small-directml.json
cargo run --release --features directml-provider --bin rapidstruct -- evaluate --manifest datasets/text/golden-manifest.json --config Temp/ocr-tiny-directml-eval.yaml --output tests/ocr_perf_profile/ocr-eval-tiny-directml.json
```
