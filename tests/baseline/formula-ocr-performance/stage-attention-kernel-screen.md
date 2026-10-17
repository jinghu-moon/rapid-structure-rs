# Formula OCR Attention Kernel Screen

Date: 2026-10-17

## Question

Can ONNX Runtime's `com.microsoft::MultiHeadAttention` CPU kernel reduce the
cost of the repeated attention subgraphs inside PP-FormulaNet+ M's decoder
Loop?

## Model facts

The Paddle `MBartAttention` implementation has distinct Q/K/V projections.
The exported ONNX body has six decoder layers, each with self-attention,
cross-attention, and a feed-forward block. The existing Loop carries decoder
state and per-layer key/value state. Cross-attention cache selection is inside
the Loop, so the encoder-side K/V path is not an independent projection that
can simply be moved outside it.

The attention math is represented as projection and reshape nodes around
`MatMul -> mask Add -> Softmax -> MatMul`. The generic BART transformer fuser
did not recognize these subgraphs and its saved candidate did not load in the
production runtime; see `transformer-fusion-audit.md`.

## Runtime operator screen

ONNX Runtime 1.20.1 CPUExecutionProvider loaded a minimal
`com.microsoft::MultiHeadAttention` graph with Q/K/V inputs and an additive
attention bias. The operator's documented contract supports Q/K/V in
`[batch, sequence, hidden]` form and an attention bias in
`[batch, heads, query_sequence, key_sequence]` form. An isolated test against
the equivalent standard ONNX attention graph, with the latter half of key
positions masked by `-10000`, produced a maximum absolute output difference of
`2.83e-7`.

The microbenchmark used one query token, 128 key/value tokens, hidden size 512,
8 heads of size 64, FP32, CPUExecutionProvider, intra-op 14/inter-op 1, and
graph optimizations disabled for both graphs. After 50 warm-up calls, five
rounds of 1,000 `Session::Run` calls gave these median latencies:

| Graph | Median per call |
| --- | ---: |
| Standard ONNX reshape / transpose / MatMul / Softmax attention | 0.05942 ms |
| `com.microsoft::MultiHeadAttention` | 0.04856 ms |

The isolated fused graph was 18.3% faster in this setup. This is only an
operator-level screen: the benchmark did not execute the formula model, its
Loop, its cache branches, the model's actual masks, the production Rust-linked ORT
1.28.0, or end-to-end preprocessing and token decoding. It is not evidence of
an OCR latency improvement and does not justify replacing the model asset.

## Decision

**Keep MHA as the next graph-rewrite candidate; do not adopt the synthetic
result as an optimization.** A real candidate must be generated from the
formula model's attention subgraphs and pass all of these gates:

1. ONNX checker and session creation on the production Rust-linked ORT 1.28.0.
2. Exact equality of EOS-truncated token IDs for all 64 pinned samples against
   the current adopted model, including batch=1 and the standard batch=8 run.
3. No truncation or pipeline failures and unchanged decoded LaTeX/EOS indices.
4. Repeatable end-to-end improvement over the same model, manifest, provider,
   batch, and thread plan. Otherwise reject the graph rewrite.

The graph rewrite must preserve the Loop's existing cache recurrence and
attention mask semantics. If matching those contracts requires introducing
per-token Rust dispatch or changing the model recurrence without an exact
equivalence proof, stop the candidate rather than adding a runtime workaround.

No production code or model asset was changed in this screen.
