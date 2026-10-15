# OCR 优化阶段结果：Detector 最大边长扫描

## 阶段目标

验证降低 global.max_side_len 是否能在当前 PP-OCRv6 medium CPU 配置下减少 OCR 延迟，同时保持现有 OCR 正确率。

本阶段只改变 detector 输入分辨率，未改变模型、provider、线程计划或 recognizer 批处理逻辑。

## 测试方法

测试集：datasets/text 12 张图片。

性能命令：

    cargo run --release --bin perf_profile -- --config models/configs/test-config.yaml --max-side-len <1280|1600> --rounds 1 --warmup 0 --provider cpu

正确率命令：

    target\release\rapidstruct.exe evaluate --manifest datasets/text/golden-manifest.json --config <config>

基线配置为 max_side_len=2000。候选配置只把该值改为 1280 或 1600。

## 性能对比

| 配置 | OCR mean ms | p50 ms | p90 ms | detector mean ms | recognizer mean ms | detector 输入 | 峰值工作集 |
|---|---:|---:|---:|---:|---:|---|---:|
| 2000 基线 | 21,728.9 | 22,694.6 | 26,585.8 | 7,040.7 | 14,542.4 | 1984x1248 | 约 2.49 GiB |
| 1600 候选 | 13,714.0 | 14,891.1 | 17,433.4 | 3,284.2 | 10,316.0 | 1600x1024 | 约 1.74 GiB |
| 1280 候选 | 10,969.7 | 11,519.4 | 13,811.9 | 1,888.3 | 8,975.9 | 1280x800 | 约 1.32 GiB |

速度收益明确：1600 的 mean 延迟下降约 36.9%，1280 下降约 49.5%。内存也同步下降。

## OCR 正确率对比

| 配置 | mean CER | 相对基线 |
|---|---:|---:|
| 2000 基线 | 0.4320880473 | 0 |
| 1600 候选 | 0.4394412041 | +0.0073531568 |
| 1280 候选 | 0.4333707392 | +0.0012826920 |

关键回归样例 09竖排文本.png：

| 配置 | CER |
|---|---:|
| 2000 基线 | 0.1129032224 |
| 1600 候选 | 0.1161290333 |
| 1280 候选 | 0.1806451678 |

## 决策

**拒绝 1280 和 1600，不修改生产配置。**

原因是两者都使 mean CER 变差；1280 虽然整体变化很小，但竖排文本 CER 明显恶化，不能用平均延迟收益掩盖特定版式回归。1600 的 mean CER 退化更明显。

因此本阶段没有生产代码优化提交，保留三份完整 JSON 作为可复现的否决证据。下一阶段应优先验证 provider/EP 真实性、recognizer 模型或量化方案，以及在保持 detector=2000 的条件下进行 recognizer batch 分桶实验。

## 文件

- ocr-baseline-cpu-2026-10-15.json
- ocr-candidate-max1280-cpu-2026-10-15.json
- ocr-candidate-max1600-cpu-2026-10-15.json
- ocr-eval-medium-2000.json、ocr-eval-medium-1280.json、ocr-eval-medium-1600.json：对应三档配置的 OCR CER 结果。
