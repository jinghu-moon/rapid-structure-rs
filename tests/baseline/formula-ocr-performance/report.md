# Formula OCR Performance Report

采集日期：2026-10-16

模型：`pp_formulanet_plus_m.onnx`，593,915,961 bytes，SHA-256 `71b6d389cf7b857e45252a4b98cfced1a3ffca7bf24d9497d02d052a41d9493b`

硬件：Intel Core i5-13600KF（14 physical / 20 logical CPUs），NVIDIA RTX 4070 Ti SUPER

数据集：`latexocr_example/validate`；固定 64 样本 manifest SHA-256 `56c30466a6e1e291`（完整摘要见 `manifest-val64.json`）

运行条件：release build；provider 请求严格失败；CPU 默认 intra-op=14、inter-op=1；每项性能基准 warmup=1、rounds=5（线程矩阵 warmup=2、rounds=9）。

## 当前基线

CPU batch 矩阵使用固定的同一组 8 张验证图，报告见 `baseline-cpu-batch-1-2-4-8.json`。模型 `session.run` 是耗时主体，预处理与 tokenizer decode 占比很小。batch 扩大时单次请求总等待时间会增加，但并发处理多条公式的平均吞吐提高：

| Batch | 每图 E2E 均值 | 整批 E2E 均值 | Token 稳定 |
| ---: | ---: | ---: | --- |
| 1 | 689.0 ms | 689.0 ms | 是 |
| 2 | 590.2 ms | 1,180.4 ms | 是 |
| 4 | 551.5 ms | 2,205.8 ms | 是 |
| 8 | 456.0 ms | 3,647.6 ms | 是 |

上述每图分摊耗时是批处理吞吐口径，不代表单条公式的响应延迟。单图调用仍应看 batch=1 的延迟。历史 501 张验证集参考为 exact match 37.92%、mean CER 0.07967；本阶段固定 64 样本的当前结果为 exact match 29.69%、mean CER 0.06733。后者是确定抽样子集，不能与全量指标直接比较。

## 实验结论

| 候选 | 性能结果 | 正确性结果 | 结论 |
| --- | --- | --- | --- |
| CPU intra-op 线程 1/2/4/8/14 | 长公式单图 p50 分别为 3,010.5 / 1,906.7 / 1,461.2 / 1,334.7 / 1,400.5 ms。8 线程在单图微基准快于默认 14 线程约 4.7%，但固定 64 样本吞吐由 1.223 降至 1.162 img/s，耗时由 52.34 增至 55.10 s。 | EOS 前 token、LaTeX、EOS 与 truncated 对全部 64 条完全相同；exact、CER相同。 | 否决线程默认值变更；单图收益未转化为端到端吞吐收益。 |
| CUDA EP | 严格选中 CUDA 且无 CPU 回退。单图 p50 1,082.1 ms，对 CPU 689.0 ms；batch 8 每图分摊 699.9 ms，对 CPU 456.0 ms。 | 与单图基线 token 稳定。 | 否决 CUDA 作为此模型的性能默认值；硬件加速器实际执行不等于更快。 |
| CPU batch 16 | 16 图微基准约 593.0 ms/图，对 batch 8 的 706.5 ms/图改善约 16%；但独立 64 样本评测吞吐由 batch 8 的 1.223 降至 1.179 img/s，墙钟由 52.34 增至 54.26 s，峰值工作集由约 2.01 增至 2.95 GB。 | LaTeX、EOS、truncated 与 EOS 前 token 全部一致。EOS 后模型输出填充宽度随批内最长序列改变，不影响解码内容。 | 不更改默认 batch=8，不提升识别器现有最大 batch=16。大 batch 的吞吐收益依赖输入组合，内存代价明确。 |

本阶段没有找到足以采纳的默认配置优化，因此没有修改识别实现。当前公式识别 API 已支持最多 16 张一批，页面中的多公式可利用这一既有能力；单公式场景不要为了吞吐分摊指标改成等待一个大批次。

## 下一阶段优化方案

profile 表明成本集中在模型 `session.run`。继续调整 tokenizer 或图像预处理不会有实质收益；应针对自回归模型内部的逐 token `Loop` 推理成本寻找候选模型或图结构。

1. 先确认仓库中的 PP-FormulaNet+ S 推理权重能否在可复现环境中导出成 ONNX。与 M 模型使用同一 manifest 做单图和 batch 基准，并对 501 张验证集报告 exact match、normalized match、CER、截断和逐样本 token 差异。只有真实性能收益达到门槛且质量退化被明确接受时才替换。
2. 若模型结构允许，单独评估 FP16/INT8 ONNX。每次只改变一种精度；检查 CPU/CUDA provider 分区、权重大小、端到端延迟和 501 样本精度。量化后的少量 smoke test 不能作为采纳依据。
3. 只有模型导出或量化都无法改善后，再研究把整图 `Loop` 转为可逐步解码的模型接口。该改动需要模型侧支持缓存状态；单纯在 Rust 侧循环调用当前整图没有减少计算的依据。

每阶段都使用相同模型摘要与 manifest，分别记录单图响应延迟和批量吞吐；任何候选在固定质量门槛、边界样本或既有公式回归失败时都不采纳。

## 可复现命令

```powershell
cargo run --release --bin formula_bench -- --model models/formula/recognition/onnx/pp_formulanet_plus_m.onnx --image <fixed-image-list> --rounds 5 --warmup 1 --batch-sizes 1,2,4,8 --provider cpu --output <report.json>

cargo run --release --bin formula_eval -- --model models/formula/recognition/onnx/pp_formulanet_plus_m.onnx --dataset-root datasets/formula --dataset latexocr --split validate --limit 64 --batch-size 8 --manifest-output <manifest.json> --output <report.json>
```
