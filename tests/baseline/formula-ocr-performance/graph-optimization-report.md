# Formula OCR Graph Optimization Audit

Date: 2026-10-17

## Question

Can serializing ONNX Runtime's `ORT_ENABLE_ALL` graph reduce the cost of the
PP-FormulaNet+ M decoder loop on the production Rust runtime?

The source model is `pp_formulanet_plus_m.onnx` (SHA-256
`71b6d389cf7b857e45252a4b98cfced1a3ffca7bf24d9497d02d052a41d9493b`). The
candidate was generated with Python ONNX Runtime 1.20.1 and tested by the
current Rust release binary, whose linked runtime reports 1.28.0. This is a
deliberately conservative cross-runtime experiment: a candidate is useful only
if it preserves decoded tokens and gives a repeatable wall-clock improvement.

## Graph result

The serialized graph shrinks from 824 to 228 top-level nodes and from 1,743 to
909 decoder-body nodes. ORT folds identities, batch-normalization scaffolding,
and other graph-local patterns. The decoder still contains one `Loop`; the
autoregressive recurrence is unchanged. The generated candidate is kept in
`target/` and is not a packaged model.

## 64-image regression

All runs used `manifest-val64.json`, CPUExecutionProvider, batch size 8, and
the current Rust release binary. Candidate output matched the source output for
all 64 samples through EOS. Exact match was 19/64, normalized match 19/64,
mean CER `0.0673319232`, and truncation/pipeline failures were zero for both.

| Pair | Baseline wall | Candidate wall | Candidate delta |
| ---: | ---: | ---: | ---: |
| 1 | 52.087 s | 52.793 s | +1.36% |
| 2 | 53.013 s | 52.176 s | -1.58% |
| 3 | 52.912 s | 52.099 s | -1.54% |
| Mean | 52.671 s | 52.356 s | -0.60% |

The sign changes across paired repetitions and the mean gain is smaller than
the observed run-to-run noise. It is not a repeatable performance improvement.

## Decision

**Reject and do not package the serialized graph.** The experiment proves that
ORT already exposes the expected graph simplifications, but saving that graph
does not provide a stable speed gain on the production runtime. The source
model remains the formula model. A previous experiment that called a cached
decoder once per token was 130.6% slower, so another split export is not a
candidate by itself. Future work should proceed only if token iteration can
stay inside one runtime call through a fused or device-side decoder; repeating
offline graph serialization would add an artifact without solving the measured
loop cost.

## Reproduction

```powershell
python tools/export_ort_optimized_formula.py `
  models/formula/recognition/onnx/pp_formulanet_plus_m.onnx `
  target/formula-ort-cpu-optimized.onnx

1..3 | ForEach-Object {
  $i = $_
  target/release/formula_eval.exe --model models/formula/recognition/onnx/pp_formulanet_plus_m.onnx `
    --dataset-root datasets/formula --dataset latexocr --split validate --limit 64 --batch-size 8 `
    --expect-manifest tests/baseline/formula-ocr-performance/manifest-val64.json `
    --output "target/formula-graph-baseline-$i.json"
  target/release/formula_eval.exe --model target/formula-ort-cpu-optimized.onnx `
    --dataset-root datasets/formula --dataset latexocr --split validate --limit 64 --batch-size 8 `
    --expect-manifest tests/baseline/formula-ocr-performance/manifest-val64.json `
    --output "target/formula-graph-optimized-$i.json"
}
```
