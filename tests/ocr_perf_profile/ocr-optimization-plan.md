# OCR 性能优化方案

## 目标

在保持 OCR 正确率和既有输出契约的前提下，降低单图延迟和峰值内存。所有优化必须以 ocr-baseline-cpu-2026-10-15.json 为基线，并同时记录速度、准确率和资源变化。

## 实验顺序

### 已完成阶段：Provider/EP 真实性验证

DirectML 在严格模式下通过 OCR CER 回归，并将 mean 延迟从 21,728.9 ms 降至 669.4 ms，已作为显式加速配置采纳。CUDA 的 selected EP 虽为 Cuda，但 mean 延迟仍为 20,951.2 ms，暂不采纳，详见 ocr-stage-provider-results-2026-10-15.md。

### 1. Provider/EP 实验

先验证加速后端是否真实工作：cpu、directml、cuda。

每个实验记录 requested provider、selected EP、fallback_to_cpu、detector inference、recognizer inference、OCR 总耗时、working set 和识别结果一致性。

GPU 配置应设置 fail_if_provider_unavailable=true，防止不可用时静默回退 CPU。即使 selected EP 为 GPU，仍需使用 ORT profiling trace 检查节点级 fallback。

### 2. Recognizer 模型实验

Recognizer 当前平均耗时约 14.5 秒，是最高收益点。比较 medium、small、tiny、FP32 与可验证量化模型，以及相同模型在 CPU/GPU EP 下的结果。

验收条件：现有 OCR 准确率测试通过；中英文、RTL、竖排、数字公式、密集文本样例无回归；记录 p50/p90；输出文本和区域顺序保持正确。

本阶段已在 DirectML 上完成 medium/small/tiny 对比。small 的 OCR mean 为 339.1 ms、mean CER 为 0.4476513565；tiny 为 224.0 ms、mean CER 为 0.4703094065；medium 对照为 375.4 ms、mean CER 为 0.4320880473。small/tiny 虽然更快且峰值工作集更低，但 CER 均回归，特别是旋转、RTL、竖排和密集文本样例，因此两者均否决，不修改默认模型。完整证据见 `ocr-stage-recognizer-model-results-2026-10-15.md`。

### 3. Detector 分辨率实验

扫描 max_side_len = 736、960、1280、1600、2000。优先关注 1280 和 1600。

每个点记录 detector 实际输入尺寸、detector inference、recognizer inference、region_count、总耗时、OCR 召回率和 CER。降低 detector 分辨率可能同时减少区域数，因此必须验证完整 OCR 结果。

### 4. Recognizer batch 实验

比较 rec_batch_num = 1、2、4、6、8、12、16，同时比较当前固定分批、更细粒度宽度分桶、按目标宽度阈值动态成批。

重点指标：recognizer inference、total_padded_pixels、total_useful_pixels、padding_ratio、batch 数、峰值工作集和 OCR 结果一致性。不能因为 padding ratio 降低就直接判定优化成功。

已完成固定批大小扫描。batch 1/2/4 虽降低 padding 或批大小，但端到端 OCR mean 分别为 791.5/519.0/401.2 ms；batch 8/12/16 分别为 384.8/381.9/385.5 ms，均未优于 batch 6 的 379.7 ms。batch 12/16 的 recognizer inference 略低，却产生约 43% padding，batch 12 峰值累计工作集升至 1,410,388 KB，且 CER 从 0.4320880473 变为 0.4332090318。因此保留 `rec_batch_num=6`，否决固定批大小改动。下一步只验证宽度分桶是否能在不增加调用次数的条件下降低 padding，详见 `ocr-stage-recognizer-batch-results-2026-10-15.md`。

### 5. CPU 线程实验

固定 CPU provider，扫描 ORT intra = 1、2、4、8、14，ORT inter=1。同时记录 Rayon 线程数、总耗时、detector inference、recognizer inference 和内存。

## 实验报告格式

每个实验目录至少包含：

    ocr-<experiment>-<variant>.json
    ocr-<experiment>-summary.md

JSON 必须包含 meta.provider_resolution、meta.thread_plan、meta.models、images[].rounds[].det_input、images[].rounds[].region_count、images[].rounds[].rec_batches、summary.stage_timing 和 summary.stage_memory。

## 选择规则

候选方案按以下顺序筛选：

1. OCR 正确性不回归；
2. recognizer 和 detector 的真实 inference 时间下降；
3. 总延迟 p50/p90 下降；
4. 峰值工作集不出现不可接受增长；
5. 方案不引入重复管线或临时兼容层。

## 推荐路线

    EP 真实性验证
      -> recognizer 模型/量化实验
      -> detector 分辨率扫描
      -> recognizer batch 分桶实验
      -> CPU 线程扫描

当前不建议优先优化 resize、crop 或 CTC 后处理；它们在基线中不是主要耗时来源。
