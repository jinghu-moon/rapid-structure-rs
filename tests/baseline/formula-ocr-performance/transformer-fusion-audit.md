# Formula OCR Transformer Fusion Audit

Date: 2026-10-17

## Question

Can ONNX Runtime's Python BART transformer fusions expose a fused attention
operator inside the PP-FormulaNet+ M decoder Loop?

## Viability gate

The source PP-FormulaNet+ M graph was first passed to
`onnxruntime.transformers.optimizer.optimize_model` with `model_type=bart`,
`num_heads=8`, `hidden_size=512`, and `opt_level=0`. The optimizer failed before
writing a model because its topological sorter reported:

```text
RuntimeError: Graph is not a DAG: len(sorted_node_set)=538,
len(graph.node)=544, failed at node Gelu_5
```

This is a limitation of the fusion tool's graph handling. It does not indicate
that the source ONNX model is invalid; the source model passes ONNX checker and
the production ORT session loads it.

As a second gate, the same optimizer was run on the already serialized ORT
optimized graph (`target/formula-ort-cpu-optimized.onnx`). It completed, but
produced no `Attention` or `MultiHeadAttention` node and left 62 `MatMul` nodes
inside the decoder Loop. The saved candidate then failed production ORT session
creation:

```text
Graph output (p2o.sub_block.pd_op.scale.36.0) does not exist in the graph.
```

The candidate therefore cannot be evaluated for quality or speed.

## Decision

**Reject the generic transformer fusion path.** It neither recognizes the
decoder pattern nor preserves this exported graph's output contract. No
runtime or model registry changes were made.

The remaining viable performance direction is a model-side export that keeps
decoder iteration inside one valid runtime graph while exposing explicit cached
state or a custom fused decoder. A generic BART graph rewrite is not a safe
substitute for that export.

## Reproduction

```powershell
@'
from onnxruntime.transformers import optimizer
optimizer.optimize_model(
    "models/formula/recognition/onnx/pp_formulanet_plus_m.onnx",
    model_type="bart", num_heads=8, hidden_size=512,
    opt_level=0, use_gpu=False, only_onnxruntime=False,
)
'@ | python

@'
from onnxruntime.transformers import optimizer
m = optimizer.optimize_model(
    "target/formula-ort-cpu-optimized.onnx",
    model_type="bart", num_heads=8, hidden_size=512,
    opt_level=0, use_gpu=False, only_onnxruntime=False,
)
m.save_model_to_file("target/formula-bart-fused.onnx", use_external_data_format=True)
'@ | python
```
