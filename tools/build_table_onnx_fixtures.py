"""重新生成 `tests/fixtures/table-onnx/*.onnx` 契约 fixture。

这些 fixture 随仓库提交，必须能在干净 clone / CI 中复现
`src/table/structure.rs` 与 `src/runtime/contracts.rs` 的契约测试；
脚本**不依赖任何外部模型**、不依赖开发机绝对路径。

用法：

```powershell
python tools/build_table_onnx_fixtures.py
```

契约事实（由 `models/table/slanet-plus.onnx` 实测得到，见 `docs/07-table-recognition.md` §5.2）：

- 输入：单输入 `FLOAT [N, 3, H, W]`，**四个维度全是动态的**；
- 输出：**两路** FLOAT rank-3 ——
  `save_infer_model/scale_0.tmp_0` `[N, L, 8]`（单元格框）与
  `save_infer_model/scale_1.tmp_0` `[N, L, 50]`（结构 token 概率）；
- metadata `character`：**换行分隔**的 48 个 token（长度 582），
  **不是**公式域那种 JSON。

与真实模型的差异（刻意保留）：这里把 `L` 固定成 4，真实模型是动态的。
契约只约束 rank / dtype / **末维**，不约束 `L`。
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

FIXTURE_DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "table-onnx"

# 真实 SLANet_plus 导出器的输出名。契约按名字精确匹配，改名的重新导出会**响亮地**失败，
# 而不是静默丢掉一路输出。
BOX_OUTPUT = "save_infer_model/scale_0.tmp_0"
STRUCTURE_OUTPUT = "save_infer_model/scale_1.tmp_0"

BOX_LAST_DIM = 8
STRUCTURE_LAST_DIM = 50

# fixture 里固定的解码长度（真实模型动态）。
FIXTURE_L = 4

# `character` metadata：**逐字节照抄**真实 `models/table/slanet-plus.onnx` 的 48 个 token
# （长度 582，换行分隔）。
#
# 这不是随便捏的列表 —— 它揭示了一件容易看反的事：SLANet 的结构 token **不是整标签**，
# 而是**片段**：
#
#   `<td>`               = `<td` + `>`
#   `<td colspan="2">`   = `<td` + ` colspan="2"` + `>`
#   空单元格              = `<td></td>`（单 token，位于末尾）
#
# 所以词表里**同时**有 `<td`、`>`、`</td>`、`<td></td>`；「不含 `<td>`」只对完整的
# `<td>` 成立。解码器（`src/table/decode.rs`）必须**先拼接片段再解析 HTML**，
# 不能按 token 做字符串相等匹配。
#
# 另：`rowspan` / `colspan` 都从 **2** 起（1 隐含，不需要 token），到 20 止。
REAL_CHARACTER_TOKENS = [
    "<thead>",
    "</thead>",
    "<tbody>",
    "</tbody>",
    "<tr>",
    "</tr>",
    "<td",
    ">",
    "</td>",
    *[f' colspan="{n}"' for n in range(2, 21)],
    *[f' rowspan="{n}"' for n in range(2, 21)],
    "<td></td>",
]

SYNTHETIC_CHARACTER_METADATA = "\n".join(REAL_CHARACTER_TOKENS)

assert len(REAL_CHARACTER_TOKENS) == 48, len(REAL_CHARACTER_TOKENS)
assert len(SYNTHETIC_CHARACTER_METADATA) == 582, len(SYNTHETIC_CHARACTER_METADATA)

# 公式域风格的 JSON metadata：表格域**必须**拒绝它（证明没有复用公式域的解析）。
JSON_CHARACTER_METADATA = '{"fast_tokenizer_file":{"model":{"type":"WordLevel"}}}'


def float_input(name: str = "x", dims=None) -> onnx.ValueInfoProto:
    dims = dims if dims is not None else ["N", 3, "H", "W"]
    return helper.make_tensor_value_info(name, TensorProto.FLOAT, dims)


def int64_input(name: str = "x", dims=None) -> onnx.ValueInfoProto:
    dims = dims if dims is not None else ["N", 3, "H", "W"]
    return helper.make_tensor_value_info(name, TensorProto.INT64, dims)


def two_output_nodes(
    *,
    box_output: str = BOX_OUTPUT,
    structure_output: str = STRUCTURE_OUTPUT,
) -> tuple[list, list]:
    """`[N, 3, H, W]` -> FLOAT `[N, 4, 8]` + FLOAT `[N, 4, 50]`。

    两路输出都由输入的第 0 维（batch）导出，所以 fixture 是**真的可运行**的，
    并且 batch 维会随输入传播 —— 契约测试因此能断言 `(N, L, 8)` / `(N, L, 50)`。
    """
    initializers = [
        numpy_helper.from_array(np.array([0], dtype=np.int64), "batch_starts"),
        numpy_helper.from_array(np.array([1], dtype=np.int64), "batch_ends"),
        numpy_helper.from_array(np.array([0], dtype=np.int64), "batch_axes"),
        numpy_helper.from_array(np.array([FIXTURE_L, BOX_LAST_DIM], dtype=np.int64), "box_tail"),
        numpy_helper.from_array(
            np.array([FIXTURE_L, STRUCTURE_LAST_DIM], dtype=np.int64), "structure_tail"
        ),
        numpy_helper.from_array(
            np.ones((1, FIXTURE_L, BOX_LAST_DIM), dtype=np.float32), "box_seed"
        ),
        numpy_helper.from_array(
            np.ones((1, FIXTURE_L, STRUCTURE_LAST_DIM), dtype=np.float32), "structure_seed"
        ),
    ]
    nodes = [
        helper.make_node("Shape", ["x"], ["x_shape"]),
        helper.make_node(
            "Slice", ["x_shape", "batch_starts", "batch_ends", "batch_axes"], ["batch_dim"]
        ),
        helper.make_node("Concat", ["batch_dim", "box_tail"], ["box_shape"], axis=0),
        helper.make_node("Expand", ["box_seed", "box_shape"], [box_output]),
        helper.make_node("Concat", ["batch_dim", "structure_tail"], ["structure_shape"], axis=0),
        helper.make_node("Expand", ["structure_seed", "structure_shape"], [structure_output]),
    ]
    outputs = [
        helper.make_tensor_value_info(
            box_output, TensorProto.FLOAT, ["N", FIXTURE_L, BOX_LAST_DIM]
        ),
        helper.make_tensor_value_info(
            structure_output, TensorProto.FLOAT, ["N", FIXTURE_L, STRUCTURE_LAST_DIM]
        ),
    ]
    return nodes, initializers, outputs


def build_two_output_model(
    *,
    input_type: str = "float",
    input_dims=None,
    extra_input_names: tuple[str, ...] = (),
    metadata: str | None = None,
    box_output: str = BOX_OUTPUT,
    structure_output: str = STRUCTURE_OUTPUT,
) -> onnx.ModelProto:
    dims = input_dims if input_dims is not None else ["N", 3, "H", "W"]
    inputs = [float_input("x", dims) if input_type == "float" else int64_input("x", dims)]
    for name in extra_input_names:
        inputs.append(float_input(name, dims))

    nodes, initializers, outputs = two_output_nodes(
        box_output=box_output, structure_output=structure_output
    )
    graph = helper.make_graph(
        nodes, "table_structure_contract_fixture", inputs, outputs, initializer=initializers
    )
    model = helper.make_model(graph, producer_name="rapid-structure-rs-fixtures")
    model.ir_version = 8
    model.opset_import[0].version = 15
    if metadata is not None:
        entry = model.metadata_props.add()
        entry.key = "character"
        entry.value = metadata
    return model


def build_single_output_model() -> onnx.ModelProto:
    """只有框那一路 —— `require_outputs` 必须拒绝它。"""
    nodes, initializers, outputs = two_output_nodes()
    graph = helper.make_graph(
        nodes,
        "table_single_output_fixture",
        [float_input()],
        outputs[:1],
        initializer=initializers,
    )
    model = helper.make_model(graph, producer_name="rapid-structure-rs-fixtures")
    model.ir_version = 8
    model.opset_import[0].version = 15
    entry = model.metadata_props.add()
    entry.key = "character"
    entry.value = SYNTHETIC_CHARACTER_METADATA
    return model


def build_last_dim_model() -> onnx.ModelProto:
    """结构概率那一路的末维是 51（不是 50）—— 末维校验必须拒绝它。"""
    initializers = [
        numpy_helper.from_array(np.array([0], dtype=np.int64), "batch_starts"),
        numpy_helper.from_array(np.array([1], dtype=np.int64), "batch_ends"),
        numpy_helper.from_array(np.array([0], dtype=np.int64), "batch_axes"),
        numpy_helper.from_array(np.array([FIXTURE_L, BOX_LAST_DIM], dtype=np.int64), "box_tail"),
        numpy_helper.from_array(np.array([FIXTURE_L, 51], dtype=np.int64), "structure_tail"),
        numpy_helper.from_array(
            np.ones((1, FIXTURE_L, BOX_LAST_DIM), dtype=np.float32), "box_seed"
        ),
        numpy_helper.from_array(np.ones((1, FIXTURE_L, 51), dtype=np.float32), "structure_seed"),
    ]
    nodes = [
        helper.make_node("Shape", ["x"], ["x_shape"]),
        helper.make_node(
            "Slice", ["x_shape", "batch_starts", "batch_ends", "batch_axes"], ["batch_dim"]
        ),
        helper.make_node("Concat", ["batch_dim", "box_tail"], ["box_shape"], axis=0),
        helper.make_node("Expand", ["box_seed", "box_shape"], [BOX_OUTPUT]),
        helper.make_node("Concat", ["batch_dim", "structure_tail"], ["structure_shape"], axis=0),
        helper.make_node("Expand", ["structure_seed", "structure_shape"], [STRUCTURE_OUTPUT]),
    ]
    outputs = [
        helper.make_tensor_value_info(BOX_OUTPUT, TensorProto.FLOAT, ["N", FIXTURE_L, BOX_LAST_DIM]),
        helper.make_tensor_value_info(STRUCTURE_OUTPUT, TensorProto.FLOAT, ["N", FIXTURE_L, 51]),
    ]
    graph = helper.make_graph(
        nodes, "table_last_dim_fixture", [float_input()], outputs, initializer=initializers
    )
    model = helper.make_model(graph, producer_name="rapid-structure-rs-fixtures")
    model.ir_version = 8
    model.opset_import[0].version = 15
    return model


def build_dtype_model() -> onnx.ModelProto:
    """结构概率那一路是 INT64 —— dtype 校验必须拒绝它。"""
    initializers = [
        numpy_helper.from_array(np.array([0], dtype=np.int64), "batch_starts"),
        numpy_helper.from_array(np.array([1], dtype=np.int64), "batch_ends"),
        numpy_helper.from_array(np.array([0], dtype=np.int64), "batch_axes"),
        numpy_helper.from_array(np.array([FIXTURE_L, BOX_LAST_DIM], dtype=np.int64), "box_tail"),
        numpy_helper.from_array(
            np.array([FIXTURE_L, STRUCTURE_LAST_DIM], dtype=np.int64), "structure_tail"
        ),
        numpy_helper.from_array(
            np.ones((1, FIXTURE_L, BOX_LAST_DIM), dtype=np.float32), "box_seed"
        ),
        numpy_helper.from_array(
            np.zeros((1, FIXTURE_L, STRUCTURE_LAST_DIM), dtype=np.int64), "structure_seed"
        ),
    ]
    nodes = [
        helper.make_node("Shape", ["x"], ["x_shape"]),
        helper.make_node(
            "Slice", ["x_shape", "batch_starts", "batch_ends", "batch_axes"], ["batch_dim"]
        ),
        helper.make_node("Concat", ["batch_dim", "box_tail"], ["box_shape"], axis=0),
        helper.make_node("Expand", ["box_seed", "box_shape"], [BOX_OUTPUT]),
        helper.make_node("Concat", ["batch_dim", "structure_tail"], ["structure_shape"], axis=0),
        helper.make_node("Expand", ["structure_seed", "structure_shape"], [STRUCTURE_OUTPUT]),
    ]
    outputs = [
        helper.make_tensor_value_info(BOX_OUTPUT, TensorProto.FLOAT, ["N", FIXTURE_L, BOX_LAST_DIM]),
        helper.make_tensor_value_info(
            STRUCTURE_OUTPUT, TensorProto.INT64, ["N", FIXTURE_L, STRUCTURE_LAST_DIM]
        ),
    ]
    graph = helper.make_graph(
        nodes, "table_output_dtype_fixture", [float_input()], outputs, initializer=initializers
    )
    model = helper.make_model(graph, producer_name="rapid-structure-rs-fixtures")
    model.ir_version = 8
    model.opset_import[0].version = 15
    return model


def build_renamed_output_model() -> onnx.ModelProto:
    """两路输出的名字都不是 SLANet 的名字 —— 精确名字匹配必须拒绝它。

    这条 fixture 存在，是为了证明「按名字精确匹配」不是空话：
    否则一个只检查输出个数、不检查名字的实现会静默通过。
    """
    return build_two_output_model(
        metadata=SYNTHETIC_CHARACTER_METADATA,
        box_output="boxes",
        structure_output="structures",
    )


def build_extra_output_model() -> onnx.ModelProto:
    """两路契约输出**都在**，另外多声明一路 `extra`。

    这条 fixture 走的是 `require_outputs` 的"未声明的多余输出"分支 ——
    与 `table_renamed_output.onnx`（走"缺项"分支）是**两条不同的错误路径**，
    缺少它，"恰好这一组"里的"恰好"就没有测试覆盖。
    """
    nodes, initializers, outputs = two_output_nodes()
    initializers.append(
        numpy_helper.from_array(np.ones((1, FIXTURE_L, BOX_LAST_DIM), dtype=np.float32), "extra_seed")
    )
    initializers.append(
        numpy_helper.from_array(np.array([FIXTURE_L, BOX_LAST_DIM], dtype=np.int64), "extra_tail")
    )
    nodes.append(helper.make_node("Concat", ["batch_dim", "extra_tail"], ["extra_shape"], axis=0))
    nodes.append(helper.make_node("Expand", ["extra_seed", "extra_shape"], ["extra"]))
    outputs.append(
        helper.make_tensor_value_info("extra", TensorProto.FLOAT, ["N", FIXTURE_L, BOX_LAST_DIM])
    )
    graph = helper.make_graph(
        nodes, "table_extra_output_fixture", [float_input()], outputs, initializer=initializers
    )
    model = helper.make_model(graph, producer_name="rapid-structure-rs-fixtures")
    model.ir_version = 8
    model.opset_import[0].version = 15
    return model


def fixture_models() -> dict[str, onnx.ModelProto]:
    return {
        # 正例
        "table_ok.onnx": build_two_output_model(metadata=SYNTHETIC_CHARACTER_METADATA),
        # 输入契约负例
        "table_multi_input.onnx": build_two_output_model(
            metadata=SYNTHETIC_CHARACTER_METADATA, extra_input_names=("x1",)
        ),
        "table_input_int64.onnx": build_two_output_model(
            input_type="int64", metadata=SYNTHETIC_CHARACTER_METADATA
        ),
        "table_input_rank3.onnx": build_two_output_model(
            input_dims=["N", 3, "W"], metadata=SYNTHETIC_CHARACTER_METADATA
        ),
        # 输出契约负例
        "table_single_output.onnx": build_single_output_model(),
        "table_output_dtype.onnx": build_dtype_model(),
        "table_output_last_dim.onnx": build_last_dim_model(),
        "table_renamed_output.onnx": build_renamed_output_model(),
        "table_extra_output.onnx": build_extra_output_model(),
        # metadata 负例
        "table_metadata_json.onnx": build_two_output_model(metadata=JSON_CHARACTER_METADATA),
        "table_metadata_absent.onnx": build_two_output_model(),
    }


def check(folder: Path, models: dict[str, onnx.ModelProto]) -> None:
    import onnxruntime as ort

    ort.set_default_logger_severity(3)
    runnable = {"table_ok", "table_renamed_output"}
    for name, model in models.items():
        onnx.checker.check_model(model)
        path = folder / name
        onnx.save(model, str(path))
        session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
        stem = Path(name).stem
        feed = {session.get_inputs()[0].name: np.zeros((2, 3, 488, 488), dtype=np.float32)}
        try:
            outputs = session.run(None, feed)
        except Exception as error:  # noqa: BLE001 - fixtures intentionally include bad types
            if stem in runnable:
                raise
            print(f"  {name}: load ok, run rejected as designed ({type(error).__name__})")
            continue
        described = ", ".join(f"{o.shape}:{o.dtype}" for o in outputs)
        print(f"  {name}: run ok, {len(outputs)} output(s) [{described}]")


def main() -> int:
    FIXTURE_DIR.mkdir(parents=True, exist_ok=True)
    models = fixture_models()
    check(FIXTURE_DIR, models)
    print(f"wrote {len(models)} fixtures into {FIXTURE_DIR}")
    print(
        "`character` metadata: "
        f"{len(REAL_CHARACTER_TOKENS)} tokens, "
        f"{len(SYNTHETIC_CHARACTER_METADATA)} chars (byte-identical to the real model)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
