# Formula OCR Optimization Plan

## Evidence-based target

The current PP-FormulaNet+ M profile places nearly all warm latency in ONNX `session.run`. CPU thread changes did not improve the fixed-set result, and CUDA was slower despite selecting the CUDA EP. CPU batch 16 also failed to improve end-to-end throughput on the fixed 64-sample set and used about 0.94 GB more peak working set than batch 8. Keep the current CPU default, batch evaluation default 8, and recognizer maximum batch 16 until a candidate passes the same quality and latency gates.

## Ordered experiments

1. **Smaller exported model (completed and rejected).** The docparser split S graph has a valid encoder/prep/decoder-step contract and is about 65% faster in a fixed batch-8 model-only microbenchmark, but it loses 5/64 exact and normalized matches, raises mean CER from 0.06716 to 0.09682, and truncates one sample. Evidence is recorded in `stage-s-model-report.md`; no production session code was added.
2. **Reduced precision (completed and rejected).** QInt8 `ConvInteger` cannot load on the CPU ORT build; runnable QUInt8 is 37% slower than M and changes 37/64 outputs; FP16 preserves outputs exactly but is 291% slower on CPU. Evidence is recorded in `stage-precision-report.md`; no precision branch was added.
3. **Incremental decoding (completed and rejected as a performance change).** The M encoder/prep/one-token decoder preserves all 64 outputs exactly, but batch-8 CPU inference is 1401.4 ms/image versus 607.8 ms/image for the M monolith. Per-token ORT dispatch dominates the saved Loop work. Evidence is recorded in `stage-incremental-decoding-report.md`; no Rust split-session path was added.

## Acceptance gates

- Every run uses the same image manifest and records model SHA-256, ORT version, provider actually selected, thread counts, and batch size.
- Report single-formula latency separately from multi-formula throughput. Include cold load separately from warm inference.
- Use the pinned 64-sample manifest as the screening gate. A candidate that fails its latency or output-equivalence gate is rejected at screening; run the complete 501-sample set only for candidates that pass screening and are being considered for adoption.
- Compare exact match, normalized match, mean CER, truncation, failures, LaTeX, EOS position, and token IDs through EOS against the pinned baseline.
- Adopt only when measured end-to-end improvement is repeatable, output quality remains within the agreed gate, and memory use remains acceptable for the target workload. Otherwise remove the candidate and record the rejected evidence.
