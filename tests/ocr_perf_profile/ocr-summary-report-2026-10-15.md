# OCR 性能测试总结报告

## 基线范围

本报告对应原始基线文件 ocr-baseline-cpu-2026-10-15.json，只覆盖 OCR 检测与识别通道，不覆盖公式识别和表格结构识别。

执行命令：

    cargo run --release --bin perf_profile -- --config models/configs/test-config.yaml --rounds 1 --warmup 0 --provider cpu --output tests/ocr_perf_profile/ocr-baseline-cpu-2026-10-15.json

测试集为 datasets/text 的 12 张图片。关键配置：

- detector：PP-OCRv6 medium，max_side_len=2000；
- recognizer：PP-OCRv6 medium；
- rec_batch_num=6，输入形状 [3,48,320]；
- provider：CPU；
- 线程计划：ORT intra=14、ORT inter=1、Rayon=3、sessions=2。

## 实测结果

| 指标 | 结果 |
|---|---:|
| OCR 总耗时 mean | 21,728.9 ms/img |
| OCR 总耗时 p50 | 22,694.6 ms/img |
| OCR 总耗时 p90 | 26,585.8 ms/img |
| 吞吐 | 0.05 img/s |
| detector inference mean | 7,040.7 ms/img |
| recognizer inference mean | 14,542.4 ms/img |
| recognizer postprocess mean | 44.4 ms/img |
| detector 输入 | 1984 x 1248 |
| region_count | min=13, mean=34.75, p50=36.5, p90=59.1, max=60 |
| recognizer padding ratio mean | 27.7% |
| recognizer padding ratio max | 53.7% |
| 进程累计峰值工作集 | 约 2.49 GiB |

Recognizer inference 占总耗时约 66.9%，detector inference 约 32.4%。其余 OCR 阶段均低于 0.3%，因此当前主要问题是 ONNX 推理成本，而不是 Rust 侧 resize、crop 或 CTC 解码。

## 每图观察

区域数较少时 recognizer 仍可能很慢，说明区域数不是唯一自变量。区域数、每批最大宽度、padding 后像素量和批次数必须联合分析。

| 图像 | 区域数 | detector ms | recognizer ms | 总 ms | padding |
|---|---:|---:|---:|---:|---:|
| 03旋转与倾斜.png | 13 | 7,197 | 5,975 | 13,284 | 28.6% |
| 09竖排文本.png | 14 | 7,084 | 9,203 | 16,421 | 19.8% |
| 06低对比度与深色背景.png | 21 | 7,339 | 8,511 | 15,958 | 25.6% |
| 02多语言与RTL混排.png | 22 | 7,063 | 11,193 | 18,398 | 29.9% |
| 07小字号与密集排版.png | 36 | 7,489 | 18,991 | 26,652 | 25.1% |
| 04表格与键值对.png | 60 | 6,880 | 18,962 | 25,988 | 53.7% |
| 12综合压力测试.png | 60 | 7,127 | 23,697 | 31,009 | 34.9% |

## 结论

1. 当前 CPU 基线没有硬件 EP 加速证据：detector 和 recognizer 均为 selected_ep=Cpu。
2. recognizer 是第一瓶颈，应优先验证 EP、模型大小/量化和批处理策略。
3. detector 输入接近 2K 长边，是第二瓶颈，应扫描分辨率并同时验证 OCR 召回率。
4. padding 有可观浪费，但不是第一优先级；必须以总 inference 时间验证 batch 改动是否有收益。
5. 内存峰值较高，但当前数据不足以证明分配器是根因。输入分辨率、模型大小和 batch tensor 应作为内存实验的主要变量。

## 证据边界

provider_resolution 证明的是会话级 provider 解析结果，不能证明 ONNX 图中每个节点都由该 EP 执行。GPU 实验必须配合 fail_if_provider_unavailable=true，并在必要时启用 ONNX Runtime profiling trace 检查节点级执行 provider。
