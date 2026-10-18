# OCR 优化阶段结果：Provider / EP 验证

## 阶段目标

在同一套 PP-OCRv6 medium OCR 模型、max_side_len=2000、12 张文本测试图和自动线程计划下，验证 CPU、DirectML、CUDA 是否产生真实的 OCR 推理加速。

provider 候选均使用严格配置，fail_if_provider_unavailable=true。这样不可用 provider 会直接失败，不能静默回退 CPU。

## 性能结果

| provider | selected EP | fallback | OCR mean ms | p50 ms | p90 ms | detector ms | recognizer ms |
|---|---|---:|---:|---:|---:|---:|---:|
| CPU 基线 | Cpu | false | 21,728.9 | 22,694.6 | 26,585.8 | 7,040.7 | 14,542.4 |
| DirectML | DirectMl | false | 669.4 | 416.0 | 543.7 | 283.0 | 269.0 |
| CUDA | Cuda | false | 20,951.2 | 20,445.3 | 26,315.3 | 6,810.0 | 13,995.7 |

DirectML 相对 CPU 基线：

- OCR mean 延迟下降约 96.9%；
- detector inference 下降约 96.0%；
- recognizer inference 下降约 98.2%；
- 吞吐从约 0.05 img/s 提升到约 1.49 img/s。

CUDA 仅比 CPU mean 快约 3.6%，在测量抖动范围内，不能据此认定 CUDA 真正承担了模型推理。

## 正确性结果

| provider | mean CER |
|---|---:|
| CPU 基线 | 0.4320880473 |
| DirectML | 0.4320880473 |
| CUDA | 0.4320880473 |

三者 12 个 case 的 CER 均逐值一致。DirectML 没有引入 OCR 正确性回归。

## 采纳决策

**采纳 DirectML 作为显式 OCR 加速配置。**

当时仅采纳显式 DirectML 配置，保留 CPU 全局默认。该默认决策已由后续 medium OCR 重复基准（DirectML 约 669.4 ms，CPU 约 21,728.9 ms，识别 CER 一致）取代：项目现在默认请求 DirectML，并对未编译、运行库不可用、会话初始化失败或首次推理失败回退 CPU。此处的数据仍是当时的实测记录，不代表公式识别也已证明 DirectML 更快。

**不采纳 CUDA。**

CUDA 的会话级 selected_ep=Cuda 和 fallback_to_cpu=false 不能证明节点级 GPU 执行。实测速度与 CPU 接近，因此只记录为“未验证加速”，后续需要匹配的 cuDNN 和 ORT profiling trace。

## 文件

- ocr-provider-directml.yaml
- ocr-provider-directml-2026-10-15.json
- ocr-provider-cuda-2026-10-15.json
- ocr-eval-directml-2026-10-15.json
- ocr-eval-cuda-2026-10-15.json

## 运行方式

DirectML 需要构建 directml-provider feature：

    cargo run --release --features directml-provider --bin perf_profile -- --config tests/ocr_perf_profile/ocr-provider-directml.yaml --rounds 3 --warmup 1 --output tests/ocr_perf_profile/ocr-provider-directml-repeat.json

CUDA 同理需要 cuda-provider feature；当前实测不应把它作为 OCR 加速结论。
