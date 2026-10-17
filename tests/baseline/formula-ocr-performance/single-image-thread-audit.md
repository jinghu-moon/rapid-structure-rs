# Formula OCR Single-Image Thread Audit

Date: 2026-10-17

## Question

Does lowering ONNX Runtime intra-op threads improve the latency a user sees
when one formula is recognized at a time?

Both candidates used the current Rust release binary, the PP-FormulaNet+ M
model (SHA-256
`71b6d389cf7b857e45252a4b98cfced1a3ffca7bf24d9497d02d052a41d9493b`),
CPUExecutionProvider, batch size 1, and the pinned 64-image manifest. Model
load is excluded from the per-image timings. The automatic setting resolves to
intra-op 14 and inter-op 1 on this machine.

## Results

| Setting | Mean | P50 | P95 | Max | Wall |
| --- | ---: | ---: | ---: | ---: | ---: |
| Automatic (14/1) | 1,124.0 ms | 895.7 ms | 2,615.2 ms | 4,824.9 ms | 71.96 s |
| Explicit intra 8 | 1,158.7 ms | 1,014.9 ms | 2,590.3 ms | 4,641.0 ms | 74.18 s |

Explicit intra 8 is 3.1% slower on mean latency and 13.3% slower at P50.
Its P95 is only 1.0% lower in this single run and does not offset the worse
mean and median. This is consistent with the previous
batch-throughput test, where intra 8 also reduced throughput.

The candidate preserved all 64 EOS-truncated token sequences and LaTeX strings
against the current Rust baseline. Both runs scored 19/64 exact and normalized
matches, mean CER `0.0673319232`, zero truncations, and zero pipeline failures.

## Decision

**Reject explicit intra-op 8 for single-image recognition.** Keep the automatic
14/1 plan. The measured cost remains in model `session.run`; changing thread
counts does not remove the decoder Loop and does not improve user-visible
latency.

The next performance candidate must reduce work inside one runtime call, such
as a fused or device-side decoder export. A Rust loop that calls a cached
decoder once per token has already been measured as 130.6% slower and is not a
valid follow-up by itself.

## Reproduction

```powershell
target/release/formula_eval.exe --model models/formula/recognition/onnx/pp_formulanet_plus_m.onnx `
  --dataset-root datasets/formula --dataset latexocr --split validate --limit 64 --batch-size 1 `
  --threads 8 --expect-manifest tests/baseline/formula-ocr-performance/manifest-val64.json `
  --output target/formula-single-threads8.json

target/release/formula_eval.exe --model models/formula/recognition/onnx/pp_formulanet_plus_m.onnx `
  --dataset-root datasets/formula --dataset latexocr --split validate --limit 64 --batch-size 1 `
  --expect-manifest tests/baseline/formula-ocr-performance/manifest-val64.json `
  --output target/formula-single-auto14.json
```
