# Formula OCR Optimization Plan

## Evidence-based target

The current PP-FormulaNet+ M profile places nearly all warm latency in ONNX `session.run`. CPU thread changes did not improve the fixed-set result, and CUDA was slower despite selecting the CUDA EP. CPU batch 16 also failed to improve end-to-end throughput on the fixed 64-sample set and used about 0.94 GB more peak working set than batch 8. Keep the current CPU default, batch evaluation default 8, and recognizer maximum batch 16 until a candidate passes the same quality and latency gates.

## Ordered experiments

1. **Smaller exported model.** Determine whether the existing PP-FormulaNet+ S inference weights can be exported to ONNX with the current tokenizer contract. Verify the graph contract and output behavior before timing it. Compare it against M using the pinned manifest, CPU single-image p50/p95, batch throughput, peak working set, and full 501-sample exact/normalized match, CER, truncation, and per-record outputs.
2. **Reduced precision.** If model export is not a practical candidate, create one FP16 or INT8 variant at a time. Confirm provider placement and runtime support, then apply the same benchmark and quality suite. Do not accept a faster artifact if it changes predictions beyond an explicitly approved quality budget.
3. **Incremental decoding.** If the model graph remains the bottleneck, investigate exporting an encoder plus cached decoder state so each generated token does not rerun the complete ONNX `Loop`. This requires model/exporter support; wrapping the current graph in a Rust token loop is not an optimization.

## Acceptance gates

- Every run uses the same image manifest and records model SHA-256, ORT version, provider actually selected, thread counts, and batch size.
- Report single-formula latency separately from multi-formula throughput. Include cold load separately from warm inference.
- Run the complete 501-sample validation set for any model, graph, precision, or decoding change.
- Compare exact match, normalized match, mean CER, truncation, failures, LaTeX, EOS position, and token IDs through EOS against the pinned baseline.
- Adopt only when measured end-to-end improvement is repeatable, output quality remains within the agreed gate, and memory use remains acceptable for the target workload. Otherwise remove the candidate and record the rejected evidence.
