#!/usr/bin/env python3
"""Serialize ONNX Runtime's fully optimized graph for a formula model.

This is an experiment tool.  The output must be benchmarked against the source
model on the same runtime and manifest before it can be considered for packaging.
"""

from __future__ import annotations

import argparse
from pathlib import Path

import onnx
import onnxruntime as ort


def export(source: Path, output: Path, intra_threads: int, inter_threads: int) -> None:
    options = ort.SessionOptions()
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    options.intra_op_num_threads = intra_threads
    options.inter_op_num_threads = inter_threads
    options.optimized_model_filepath = str(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    ort.InferenceSession(
        str(source),
        sess_options=options,
        providers=["CPUExecutionProvider"],
    )
    onnx.checker.check_model(onnx.load(str(output), load_external_data=True))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="source formula ONNX model")
    parser.add_argument("output", type=Path, help="optimized ONNX output")
    parser.add_argument("--intra-threads", type=int, default=14)
    parser.add_argument("--inter-threads", type=int, default=1)
    args = parser.parse_args()
    if args.intra_threads <= 0 or args.inter_threads <= 0:
        parser.error("thread counts must be greater than zero")
    export(args.source, args.output, args.intra_threads, args.inter_threads)
    print(f"wrote {args.output} ({args.output.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
