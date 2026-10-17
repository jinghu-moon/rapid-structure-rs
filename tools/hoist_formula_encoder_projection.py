#!/usr/bin/env python3
"""Hoist the invariant encoder projection out of PP-FormulaNet+ M's Loop.

The exported M graph keeps the encoder representation constant while the
decoder Loop runs.  Its first body operations project that representation with
``linear_292`` on every token.  This tool performs the semantics-preserving
graph rewrite used by the formula performance experiment.

The input and output are deliberately explicit.  The repository does not
silently replace a downloaded model: the transformed model must be generated,
validated, and evaluated against the source model before it is selected.
"""

from __future__ import annotations

import argparse
import copy
from pathlib import Path

import onnx
from onnx import helper


WEIGHT_NAME = "linear_292.w_0"
BIAS_NAME = "linear_292.b_0"
HOISTED_WEIGHT_NAME = "formula_hoisted_linear_292_weight"
HOISTED_BIAS_NAME = "formula_hoisted_linear_292_bias"
MATMUL_NAME = "MatMul.0"
BIAS_ADD_NAME = "Add.20"
TRANSPOSE_OUTPUT = "p2o.pd_op.transpose.0.0"
PROJECTION_OUTPUT = "p2o.sub_block.pd_op.matmul.0.0"
BIAS_OUTPUT = "Add.21"
HOISTED_PROJECTION_OUTPUT = "formula_hoisted_encoder_projection"
HOISTED_BIAS_OUTPUT = "formula_hoisted_encoder_projection_bias"


def _loop_body(model: onnx.ModelProto) -> onnx.GraphProto:
    loops = [node for node in model.graph.node if node.op_type == "Loop"]
    if len(loops) != 1:
        raise ValueError(f"expected exactly one top-level Loop, found {len(loops)}")
    bodies = [attr.g for attr in loops[0].attribute if attr.name == "body"]
    if len(bodies) != 1:
        raise ValueError("the decoder Loop must have exactly one body graph")
    return bodies[0]


def _find_named(nodes, op_type: str, name: str):
    matches = [node for node in nodes if node.op_type == op_type and node.name == name]
    if len(matches) != 1:
        raise ValueError(f"expected one {op_type} named {name!r}, found {len(matches)}")
    return matches[0]


def transform(input_path: Path, output_path: Path) -> None:
    model = onnx.load(str(input_path), load_external_data=True)
    body = _loop_body(model)
    matmul = _find_named(body.node, "MatMul", MATMUL_NAME)
    add = _find_named(body.node, "Add", BIAS_ADD_NAME)

    if list(matmul.input) != [TRANSPOSE_OUTPUT, WEIGHT_NAME]:
        raise ValueError(
            f"unexpected {MATMUL_NAME} inputs: {list(matmul.input)!r}; "
            f"expected [{TRANSPOSE_OUTPUT!r}, {WEIGHT_NAME!r}]"
        )
    if list(add.input) != [PROJECTION_OUTPUT, BIAS_NAME]:
        raise ValueError(
            f"unexpected {BIAS_ADD_NAME} inputs: {list(add.input)!r}; "
            f"expected [{PROJECTION_OUTPUT!r}, {BIAS_NAME!r}]"
        )

    initializers = {tensor.name: tensor for tensor in body.initializer}
    missing = [name for name in (WEIGHT_NAME, BIAS_NAME) if name not in initializers]
    if missing:
        raise ValueError(f"projection initializers are missing from Loop body: {missing}")

    existing_top = {tensor.name for tensor in model.graph.initializer}
    if HOISTED_WEIGHT_NAME in existing_top or HOISTED_BIAS_NAME in existing_top:
        raise ValueError("output model already contains hoisted projection initializers")

    weight = copy.deepcopy(initializers[WEIGHT_NAME])
    weight.name = HOISTED_WEIGHT_NAME
    bias = copy.deepcopy(initializers[BIAS_NAME])
    bias.name = HOISTED_BIAS_NAME
    model.graph.initializer.extend([weight, bias])

    top_transpose = _find_named(model.graph.node, "Transpose", "Transpose.0")
    if list(top_transpose.output) != [TRANSPOSE_OUTPUT]:
        raise ValueError(
            f"unexpected encoder transpose output: {list(top_transpose.output)!r}"
        )
    transpose_index = list(model.graph.node).index(top_transpose)
    hoisted_matmul = helper.make_node(
        "MatMul",
        [TRANSPOSE_OUTPUT, HOISTED_WEIGHT_NAME],
        [HOISTED_PROJECTION_OUTPUT],
        name="FormulaHoistedEncoderProjectionMatMul",
    )
    hoisted_add = helper.make_node(
        "Add",
        [HOISTED_PROJECTION_OUTPUT, HOISTED_BIAS_NAME],
        [HOISTED_BIAS_OUTPUT],
        name="FormulaHoistedEncoderProjectionBias",
    )
    nodes = list(model.graph.node)
    nodes[transpose_index + 1 : transpose_index + 1] = [hoisted_matmul, hoisted_add]

    # The Identity immediately following Add.20 already carries Add.21 into
    # the decoder body's existing dataflow. Removing only the two invariant
    # nodes preserves the body output names and all Loop state contracts.
    body_nodes = list(body.node)
    matmul_index = body_nodes.index(matmul)
    if matmul_index + 1 >= len(body_nodes) or body_nodes[matmul_index + 1] is not add:
        raise ValueError("MatMul.0 and Add.20 must be adjacent in the Loop body")
    del body_nodes[matmul_index : matmul_index + 2]
    # The existing Identity keeps the body-local output name and captures the
    # invariant top-level value from the enclosing graph.
    body_identity = next(
        (node for node in body_nodes if BIAS_OUTPUT in node.input), None
    )
    if body_identity is None:
        raise ValueError(f"could not find the body consumer of {BIAS_OUTPUT!r}")
    for input_index, input_name in enumerate(body_identity.input):
        if input_name == BIAS_OUTPUT:
            body_identity.input[input_index] = HOISTED_BIAS_OUTPUT
            break

    # The projection nodes have been removed from the body, so their local
    # initializers are dead. Removing them keeps the packaged model from
    # carrying a second copy of the same weights.
    body_initializers = [
        initializer
        for initializer in body.initializer
        if initializer.name not in {WEIGHT_NAME, BIAS_NAME}
    ]
    body.ClearField("initializer")
    body.initializer.extend(body_initializers)
    body.ClearField("node")
    body.node.extend(body_nodes)

    # Rebuild the parent node list last. Clearing it first would invalidate
    # the protobuf object that owns the Loop body we just edited.
    model.graph.ClearField("node")
    model.graph.node.extend(nodes)

    onnx.checker.check_model(model)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    onnx.save(model, str(output_path))


def verify(path: Path) -> None:
    model = onnx.load(str(path), load_external_data=True)
    body = _loop_body(model)
    if any(node.name in {MATMUL_NAME, BIAS_ADD_NAME} for node in body.node):
        raise ValueError("Loop body still contains the invariant projection")
    body_initializers = {tensor.name for tensor in body.initializer}
    duplicated = body_initializers.intersection({WEIGHT_NAME, BIAS_NAME})
    if duplicated:
        raise ValueError(f"Loop body retains unused projection weights: {sorted(duplicated)}")
    _find_named(model.graph.node, "MatMul", "FormulaHoistedEncoderProjectionMatMul")
    _find_named(model.graph.node, "Add", "FormulaHoistedEncoderProjectionBias")
    if not any(
        HOISTED_BIAS_OUTPUT in node.output for node in model.graph.node
    ):
        raise ValueError(f"missing top-level output {HOISTED_BIAS_OUTPUT!r}")
    top_initializers = {tensor.name for tensor in model.graph.initializer}
    for name in (HOISTED_WEIGHT_NAME, HOISTED_BIAS_NAME):
        if name not in top_initializers:
            raise ValueError(f"missing top-level initializer {name!r}")
    onnx.checker.check_model(model)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="source PP-FormulaNet+ M ONNX model")
    parser.add_argument("output", type=Path, nargs="?", help="rewritten ONNX model")
    parser.add_argument(
        "--verify-only", action="store_true", help="verify an already rewritten model"
    )
    args = parser.parse_args()
    if args.verify_only:
        verify(args.input)
    else:
        if args.output is None:
            parser.error("output is required unless --verify-only is used")
        transform(args.input, args.output)
        verify(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
