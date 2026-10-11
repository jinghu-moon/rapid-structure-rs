#!/usr/bin/env python3
"""Build `tests/fixtures/table-golden/` —— SLANet-plus 真实模型 golden。

这个脚本**不重新实现**参考管线：`TablePreprocess` 与 `TableLabelDecode` 都通过
`importlib` 从 `refer/RapidTable-main/` 的**原始文件**逐字加载并调用。

唯一的例外是 `ModelType`：`rapid_table/utils/typings.py` 传递地依赖 `omegaconf`
（本机未安装），所以脚本注入一个只含 `ModelType` 枚举的桩模块。`ModelType` 在
`post_process.py` 里只被用作 `cfg["model_type"] == model_type.SLANETPLUS` 的比较，
桩与调用方共用同一个枚举成员，因此比较语义不受影响。

产物（全部入库，`tests/fixtures/table-golden/`）：

| 文件 | 内容 |
| --- | --- |
| `table.jpg` | 固定输入图的副本（原始字节） |
| `input_bgr_resized.bin` | **resize 之后、normalize 之前**的 uint8 BGR 像素（不带 letterbox） |
| `boxes.npy` | 模型第 1 路输出 `(L, 8)` float32，**原始未解码** |
| `structure.npy` | 模型第 2 路输出 `(L, 50)` float32，**原始未解码** |
| `manifest.json` | 身份（SHA-256）、IO 契约、词表、预处理配方、参考解码的**逐阶段**结果 |
| `README.md` | 手写的产物说明：文件表、实测数字、坐标空间结论、容差依据、复现命令 |

为什么存 **resize 之后** 而不是原图：`cv2.resize` 的插值结果在 Rust 侧无法逐位复现，
存原图会让"Rust 跑真实模型"这条验收项变成"比较两个 resize 实现"。存 resize 后的
uint8 把插值不确定性**前置消掉**，两侧只需各自做 `(x/255 - mean)/std` → 左上对齐补零（零在右侧/下方）→
CHW（三步都是逐位确定的）。

用法：

    python tools/build_table_golden.py
    python tools/build_table_golden.py --verify     # 只校验已落盘的 golden 自洽

退出码：0 正常；2 前置资产缺失；3 自洽校验失败。
"""

from __future__ import annotations

import argparse
import enum
import hashlib
import importlib.util
import json
import shutil
import sys
import types
from pathlib import Path

import numpy as np

REPO_ROOT = Path(__file__).resolve().parent.parent
REFERENCE_ROOT = REPO_ROOT / "refer" / "RapidTable-main" / "rapid_table"
DEFAULT_IMAGE = REPO_ROOT / "refer" / "RapidTable-main" / "tests" / "test_files" / "table.jpg"
DEFAULT_MODEL = REPO_ROOT / "models" / "table" / "slanet-plus.onnx"
DEFAULT_OUT = REPO_ROOT / "tests" / "fixtures" / "table-golden"

# 与 `docs/09-table-recognition-execution.md` §5.1 声明的一致；不一致就直接停，
# 避免 golden 与错误的模型绑定。
EXPECTED_MODEL_BYTES = 7_758_305
EXPECTED_MODEL_SHA256 = "d57a942af6a2f57d6a4a0372573c696a2379bf5857c45e2ac69993f3b334514b"

MAX_LEN = 488
# `TablePreprocess.__init__` 的三个常量，复制在这里只为了写进 manifest 供 Rust 侧比对。
MEAN = [0.485, 0.456, 0.406]
STD = [0.229, 0.224, 0.225]
SCALE = 1.0 / 255.0

# 代数不变量 `raw_normalized × max(h, w)` 与参考实现的两步相乘**不是同一算式**，
# 差恰好一个 f32 ulp（371 量级上 = 3.0517578125e-05）。与 Rust 侧
# `src/table/decode.rs` 的 `UNIFORM_SCALE_TOLERANCE` 保持同一个值。
UNIFORM_SCALE_TOLERANCE = 1e-4


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_module_from_file(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def install_reference_shim():
    """让 `post_process.py` 里的相对 import 能解析到真实文件。

    只造包壳（不执行任何 `__init__.py`），并按需加载两个**真实**模块文件。
    返回桩出来的 `ModelType`，供调用方构造 `cfg`。
    """
    for name, directory in (
        ("rapid_table", REFERENCE_ROOT),
        ("rapid_table.table_structure", REFERENCE_ROOT / "table_structure"),
        (
            "rapid_table.table_structure.pp_structure",
            REFERENCE_ROOT / "table_structure" / "pp_structure",
        ),
        ("rapid_table.utils", REFERENCE_ROOT / "utils"),
        ("rapid_table.table_matcher", REFERENCE_ROOT / "table_matcher"),
    ):
        shell = types.ModuleType(name)
        shell.__path__ = [str(directory)]
        sys.modules[name] = shell

    # 真实文件：`table_matcher/utils.py`（只 import copy/re，安全）
    load_module_from_file(
        "rapid_table.table_matcher.utils",
        REFERENCE_ROOT / "table_matcher" / "utils.py",
    )

    # 真实文件：`table_structure/utils.py`（无任何相对 import）
    load_module_from_file(
        "rapid_table.table_structure.utils",
        REFERENCE_ROOT / "table_structure" / "utils.py",
    )

    # 桩：`utils/typings.py` 传递依赖 omegaconf。只补 `ModelType`。
    class ModelType(enum.Enum):
        PPSTRUCTURE_EN = "ppstructure_en"
        PPSTRUCTURE_ZH = "ppstructure_zh"
        SLANETPLUS = "slanet_plus"
        UNITABLE = "unitable"

    typings = types.ModuleType("rapid_table.utils.typings")
    typings.ModelType = ModelType
    sys.modules["rapid_table.utils.typings"] = typings

    load_module_from_file(
        "rapid_table.table_structure.pp_structure.pre_process",
        REFERENCE_ROOT / "table_structure" / "pp_structure" / "pre_process.py",
    )
    load_module_from_file(
        "rapid_table.table_structure.pp_structure.post_process",
        REFERENCE_ROOT / "table_structure" / "pp_structure" / "post_process.py",
    )
    load_module_from_file(
        "rapid_table.table_matcher.main",
        REFERENCE_ROOT / "table_matcher" / "main.py",
    )
    return ModelType


def load_image_bgr(path: Path) -> np.ndarray:
    """复刻 `rapid_table/utils/load_image.py:LoadImage` 的路径分支。

    `Image.open` 给 RGB，`convert_img` 对 str/Path 走 `cvtColor(RGB2BGR)` —— 所以
    送进网络的**是 BGR**，而 mean/std 仍是 RGB 序（PaddleOCR 的历史行为）。
    """
    import cv2
    from PIL import Image

    rgb = np.array(Image.open(path))
    if rgb.ndim == 2:
        return cv2.cvtColor(rgb, cv2.COLOR_GRAY2BGR)
    if rgb.shape[2] == 4:
        rgb = cv2.cvtColor(rgb, cv2.COLOR_RGBA2RGB)
    return cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR)


def verify_existing(args: argparse.Namespace) -> int:
    """只读已落盘的 golden，校验它自洽 —— 不重跑模型、不需要 onnxruntime。

    与生成路径的区别：生成路径把 golden 和**参考实现自己的返回值**对拍；这里没有参考
    实现在场，所以只能查"golden 内部各处是否互相矛盾"。它抓的是产物被手改、被截断、
    或 `manifest.json` 与文件字节脱钩这一类损坏；
    **精度内容（`.npy` 里的数值本身）由 `cargo test --lib table::decode::golden_tests`
    负责**，这里只核对形状与 dtype —— 同尺寸的内容篡改不是这个开关的职责。
    """
    out = args.out
    manifest_path = out / "manifest.json"
    if not manifest_path.exists():
        print(f"missing golden manifest: {manifest_path}", file=sys.stderr)
        return 2
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        print(f"unreadable golden manifest {manifest_path}: {error}", file=sys.stderr)
        return 3

    missing_keys = [
        key
        for key in ("model", "source_image", "input", "outputs", "vocabulary", "geometry", "reference")
        if key not in manifest
    ]
    if missing_keys:
        print(f"malformed golden manifest {manifest_path}: missing {missing_keys}", file=sys.stderr)
        return 3

    failures: list[str] = []

    def check(condition: bool, message: str) -> None:
        if not condition:
            failures.append(message)

    def check_file(record: dict, key: str = "file") -> Path | None:
        """核对一个记录在案的文件：存在、字节数、SHA-256。"""
        path = out / record[key]
        if not path.exists():
            failures.append(f"missing file: {path}")
            return None
        size = record.get("bytes")
        if size is not None:
            check(path.stat().st_size == size, f"{path.name}: {path.stat().st_size} B, manifest says {size} B")
        sha = record.get("sha256")
        if sha is not None:
            check(sha256_file(path) == sha, f"{path.name}: sha256 does not match the manifest")
        return path

    # ---- 身份：仓库里的模型本身，以及 manifest 对它的记载 -------------------
    if not args.model.exists():
        failures.append(f"missing model: {args.model}")
    else:
        model_bytes = args.model.stat().st_size
        model_sha = sha256_file(args.model)
        check(model_bytes == EXPECTED_MODEL_BYTES, f"model: {model_bytes} B, expected {EXPECTED_MODEL_BYTES} B")
        check(model_sha == EXPECTED_MODEL_SHA256, f"model: sha256 {model_sha} != {EXPECTED_MODEL_SHA256}")
    check(
        manifest["model"]["bytes"] == EXPECTED_MODEL_BYTES,
        "manifest.model.bytes disagrees with EXPECTED_MODEL_BYTES",
    )
    check(
        manifest["model"]["sha256"] == EXPECTED_MODEL_SHA256,
        "manifest.model.sha256 disagrees with EXPECTED_MODEL_SHA256",
    )

    check_file(manifest["source_image"])
    input_path = check_file(manifest["input"])

    # ---- 输入：字节数必须等于 h × w × 3（uint8 BGR） ------------------------
    if input_path is not None:
        expected = manifest["input"]["height"] * manifest["input"]["width"] * 3
        check(
            input_path.stat().st_size == expected,
            f"input_bgr_resized.bin: {input_path.stat().st_size} B != h*w*3 = {expected} B",
        )
    check(
        manifest["input"]["channel_order"] == "BGR",
        f"input.channel_order is {manifest['input']['channel_order']!r}, expected 'BGR'",
    )

    # ---- 两路输出：形状、dtype、以及步数与词表宽度互相印证 ------------------
    arrays: dict[str, np.ndarray] = {}
    for output in manifest["outputs"]:
        path = out / output["file"]
        if not path.exists():
            failures.append(f"missing file: {path}")
            continue
        try:
            values = np.load(path)
        except (OSError, ValueError) as error:
            failures.append(f"{output['file']}: unreadable ({error})")
            continue
        arrays[output["file"]] = values
        check(
            list(values.shape) == output["shape"],
            f"{output['file']}: shape {list(values.shape)} != manifest {output['shape']}",
        )
        check(
            str(values.dtype) == output["dtype"],
            f"{output['file']}: dtype {values.dtype} != manifest {output['dtype']}",
        )

    vocabulary = manifest["vocabulary"]
    check(
        vocabulary["class_count"] == vocabulary["token_count"] + 2,
        "class_count != token_count + 2 (sos/eos)",
    )
    check(vocabulary["token_count"] == len(vocabulary["tokens"]), "token_count != len(tokens)")
    check(vocabulary["sos_class_index"] == 0, "sos_class_index != 0")
    check(
        vocabulary["eos_class_index"] == vocabulary["class_count"] - 1,
        "eos_class_index != class_count - 1",
    )

    for output in manifest["outputs"]:
        values = arrays.get(output["file"])
        if values is None:
            continue
        if output["file"].endswith("structure.npy"):
            check(
                values.shape[2] == vocabulary["class_count"],
                f"structure.npy width {values.shape[2]} != class_count {vocabulary['class_count']}",
            )
        if output["file"].endswith("boxes.npy"):
            check(values.shape[2] == 8, f"boxes.npy width {values.shape[2]} != 8")

    steps = {output["file"]: output["shape"][1] for output in manifest["outputs"]}
    check(len(set(steps.values())) == 1, f"the two outputs disagree on step count: {steps}")

    # ---- 参考解码结果：内部一致性 -------------------------------------------
    reference = manifest["reference"]
    cells = reference["cells"]
    logic_points = reference["logic_points"]

    check(
        len(cells) == len(logic_points),
        f"len(cells) = {len(cells)} != len(logic_points) = {len(logic_points)}",
    )
    check(
        len(reference["kept_bboxes"]) == len(cells) - reference["dropped_blank_count"],
        "len(kept_bboxes) != len(cells) - dropped_blank_count",
    )
    check(
        len(cells) == reference["dropped_blank_count"]
        + sum(1 for cell in cells if not cell["blank"]),
        "cells' blank flags disagree with dropped_blank_count",
    )
    blank_mismatch = [
        cell["step"]
        for cell in cells
        if cell["blank"] != bool(np.all(np.asarray(cell["after_rescale"], dtype=np.float32) == 0.0))
    ]
    check(not blank_mismatch, f"blank flag contradicts all-zero after_rescale at steps {blank_mismatch}")
    check(
        [cell["step"] for cell in cells] == sorted(cell["step"] for cell in cells),
        "cells are not ordered by step index",
    )
    check(len(set(cell["step"] for cell in cells)) == len(cells), "duplicate step index among cells")

    # 坐标链路：`after_rescale == raw_normalized × max(h, w)`，且必须落在原图内。
    uniform = float(manifest["geometry"]["uniform_scale_max_h_w"])
    check(
        uniform == max(manifest["source_image"]["height"], manifest["source_image"]["width"]),
        "uniform_scale_max_h_w != max(image height, image width)",
    )
    for record in ("raw_normalized", "after_bbox_decode", "after_rescale"):
        bad = [cell["step"] for cell in cells if len(cell[record]) != 8]
        check(not bad, f"{record} is not 8 numbers long at steps {bad}")
    worst = 0.0
    for cell in cells:
        raw = np.asarray(cell["raw_normalized"], dtype=np.float32)
        rescaled = np.asarray(cell["after_rescale"], dtype=np.float32)
        worst = max(worst, float(np.abs(raw * np.float32(uniform) - rescaled).max(initial=0.0)))
    check(worst <= UNIFORM_SCALE_TOLERANCE, f"uniform-scale invariant drifted by {worst} (limit {UNIFORM_SCALE_TOLERANCE})")
    height = manifest["source_image"]["height"]
    width = manifest["source_image"]["width"]
    outside = [
        cell["step"]
        for cell in cells
        if not cell["blank"]
        and (
            min(cell["after_rescale"][0::2]) < 0.0
            or max(cell["after_rescale"][0::2]) > width
            or min(cell["after_rescale"][1::2]) < 0.0
            or max(cell["after_rescale"][1::2]) > height
        )
    ]
    check(not outside, f"rescaled cell boxes fall outside the {width}x{height} source image at steps {outside}")

    # 逻辑网格必须能从 logic_points 复算出来（与 generate 路径同一算式）。
    rows = max((point[1] + 1 for point in logic_points), default=0)
    cols = max((point[3] + 1 for point in logic_points), default=0)
    check(
        [rows, cols] == [reference["grid"]["rows"], reference["grid"]["cols"]],
        f"logic_points imply {rows}x{cols}, manifest says {reference['grid']['rows']}x{reference['grid']['cols']}",
    )

    # HTML 只是 token 流外面套了 `<html><body><table>` / `</table></body></html>`。
    check(
        len(reference["html"]) == len(reference["tokens"]) + 6,
        f"len(html) = {len(reference['html'])} != len(tokens) + 6 = {len(reference['tokens']) + 6}",
    )
    check(reference["html"][3:-3] == reference["tokens"], "html[3:-3] != tokens")
    check(
        reference["html"][:3] == ["<html>", "<body>", "<table>"]
        and reference["html"][-3:] == ["</table>", "</body>", "</html>"],
        "html is not wrapped in <html><body><table> ... </table></body></html>",
    )

    if failures:
        print(f"golden at {out} is NOT self-consistent:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 3

    print(
        f"golden at {out} is self-consistent: {len(cells)} cells, "
        f"{len(logic_points)} logic points, grid {rows}x{cols}, "
        f"uniform-scale deviation {worst:g}"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--image", type=Path, default=DEFAULT_IMAGE)
    parser.add_argument("--model", type=Path, default=DEFAULT_MODEL)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument(
        "--verify",
        action="store_true",
        help="不重跑模型，只校验已落盘 golden 的自洽性（尺寸/SHA/参考链一致）",
    )
    args = parser.parse_args()

    if not args.image.exists():
        print(f"missing source image: {args.image}", file=sys.stderr)
        return 2
    if not args.model.exists():
        print(f"missing model: {args.model}", file=sys.stderr)
        return 2

    if args.verify:
        return verify_existing(args)

    model_type = install_reference_shim()
    from rapid_table.table_structure.pp_structure.post_process import TableLabelDecode
    from rapid_table.table_structure.pp_structure.pre_process import TablePreprocess

    model_sha = sha256_file(args.model)
    model_bytes = args.model.stat().st_size
    if model_bytes != EXPECTED_MODEL_BYTES or model_sha != EXPECTED_MODEL_SHA256:
        print(
            "model identity mismatch:\n"
            f"  expected {EXPECTED_MODEL_BYTES} B / {EXPECTED_MODEL_SHA256}\n"
            f"  actual   {model_bytes} B / {model_sha}",
            file=sys.stderr,
        )
        return 2

    ori_bgr = load_image_bgr(args.image)
    ori_height, ori_width = ori_bgr.shape[:2]

    preprocess = TablePreprocess()
    resized_bgr, shape_list = preprocess.resize_image(ori_bgr)
    normalized = preprocess.normalize(resized_bgr)
    padded, shape_list = preprocess.pad_img(normalized, shape_list)
    chw = preprocess.to_chw(padded)
    model_input = np.expand_dims(chw, 0).astype(np.float32)

    import onnxruntime as ort

    options = ort.SessionOptions()
    options.log_severity_level = 4
    # 与参考实现 `OrtInferSession._init_sess_opts` 对齐：ORT_ENABLE_ALL。
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    session = ort.InferenceSession(
        str(args.model), sess_options=options, providers=["CPUExecutionProvider"]
    )
    input_names = [value.name for value in session.get_inputs()]
    output_names = [value.name for value in session.get_outputs()]
    if len(input_names) != 1 or len(output_names) != 2:
        print(
            f"unexpected IO: {input_names=} {output_names=}", file=sys.stderr
        )
        return 3

    raw_outputs = session.run(output_names, {input_names[0]: model_input})
    boxes = np.asarray(raw_outputs[0], dtype=np.float32)
    structure = np.asarray(raw_outputs[1], dtype=np.float32)
    if boxes.ndim != 3 or boxes.shape[2] != 8:
        print(f"unexpected box output shape {boxes.shape}", file=sys.stderr)
        return 3
    if structure.ndim != 3 or structure.shape[2] != 50:
        print(f"unexpected structure output shape {structure.shape}", file=sys.stderr)
        return 3

    steps = int(structure.shape[1])
    character = session.get_modelmeta().custom_metadata_map["character"].splitlines()

    decode = TableLabelDecode(list(character), {"model_type": model_type.SLANETPLUS})

    # ---- 逐阶段复现参考解码，并与参考实现自身的返回值对拍 -------------------
    structure_idx = structure[0].argmax(axis=1)
    structure_prob = structure[0].max(axis=1)
    ignored = [decode.char_to_index[decode.beg_str], decode.char_to_index[decode.end_str]]
    end_idx = decode.char_to_index[decode.end_str]

    tokens: list[str] = []
    cells: list[dict] = []
    scores: list[float] = []
    for step in range(steps):
        class_index = int(structure_idx[step])
        if step > 0 and class_index == end_idx:
            break
        if class_index in ignored:
            continue
        text = decode.character[class_index]
        if text in decode.td_token:
            # 全程保持 float32：参考实现就是 float32 in-place 乘 Python 标量，
            # 用 float64 复算会在 1e-9 量级对不上（那是 dtype 差异，不是逻辑差异）。
            raw_row = boxes[0, step].copy()
            after_bbox_decode = decode._bbox_decode(raw_row.copy(), shape_list)
            cell = {
                "step": step,
                "class_index": class_index,
                "tag": text,
                "raw_normalized": [float(v) for v in raw_row],
                "after_bbox_decode": [float(v) for v in after_bbox_decode],
                "score": float(structure_prob[step]),
            }
            cells.append(cell)
        tokens.append(text)
        scores.append(float(structure_prob[step]))

    # 参考实现会在原地修改 bbox_preds，所以用副本走一遍它自己的路径
    reference_structs, reference_cells = decode(
        boxes.copy(),
        structure.copy(),
        np.asarray([shape_list]),
        np.asarray([ori_bgr]),
    )
    reference_html, reference_confidence = reference_structs[0]
    reference_cells = np.asarray(reference_cells[0])

    # 把 `after_rescale` 补进每条 cell 记录，并标出哪些被 `filter_blank_bbox` 丢掉
    ratio_stage = min(MAX_LEN / ori_height, MAX_LEN / ori_width)
    w_ratio = MAX_LEN / (ori_width * ratio_stage)
    h_ratio = MAX_LEN / (ori_height * ratio_stage)
    for cell in cells:
        values = np.asarray(cell["after_bbox_decode"], dtype=np.float32).copy()
        values[0::2] *= w_ratio
        values[1::2] *= h_ratio
        cell["after_rescale"] = [float(v) for v in values]
        cell["blank"] = bool(np.all(values == 0))

    kept = [cell for cell in cells if not cell["blank"]]
    if kept:
        expected = np.asarray([cell["after_rescale"] for cell in kept], dtype=np.float64)
        if expected.shape != reference_cells.shape or not np.allclose(
            expected, reference_cells, rtol=1e-9, atol=1e-9
        ):
            print(
                "stage-by-stage replay disagrees with the reference decoder:\n"
                f"  replay    {expected.shape}\n  reference {reference_cells.shape}",
                file=sys.stderr,
            )
            return 3

    if tokens != [item for item in reference_html[3:-3]]:
        print("token stream disagrees with the reference decoder", file=sys.stderr)
        return 3

    # 逻辑网格也走参考实现自己那份 `decode_one_logic_points`（它不使用 `self`，
    # 所以直接以 `None` 作 self 调用）。`max_rows = max(max_rows, r_end + 1)`
    # 是它的收尾口径，Rust 侧的 `GridSize` 必须一致。
    from rapid_table.table_matcher.main import TableMatch

    logic_points = TableMatch.decode_one_logic_points(None, reference_html)
    # `decode_one_logic_points` 只返回逐单元格的 [r_start, r_end, col_start, col_end]，
    # 不返回 max_rows/max_cols —— 这里按它的口径自己收一次。
    grid_rows = max((point[1] + 1 for point in logic_points), default=0)
    grid_cols = max((point[3] + 1 for point in logic_points), default=0)

    if len(logic_points) != len(cells):
        print(
            f"logic points ({len(logic_points)}) disagree with cell boxes ({len(cells)})",
            file=sys.stderr,
        )
        return 3

    args.out.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(args.image, args.out / "table.jpg")
    np.save(args.out / "boxes.npy", boxes)
    np.save(args.out / "structure.npy", structure)
    (args.out / "input_bgr_resized.bin").write_bytes(resized_bgr.tobytes())

    manifest = {
        "note": (
            "由 tools/build_table_golden.py 生成。boxes/structure 是模型原始输出，"
            "输入是 input_bgr_resized.bin（resize 后、normalize 前的 uint8 BGR）。"
        ),
        "command": (
            "python tools/build_table_golden.py "
            f"--image {args.image.relative_to(REPO_ROOT).as_posix()} "
            f"--model {args.model.relative_to(REPO_ROOT).as_posix()}"
        ),
        "source_image": {
            "file": "table.jpg",
            "sha256": sha256_file(args.image),
            "bytes": args.image.stat().st_size,
            "height": ori_height,
            "width": ori_width,
        },
        "model": {
            "file": args.model.relative_to(REPO_ROOT).as_posix(),
            "sha256": model_sha,
            "bytes": model_bytes,
        },
        "input": {
            "name": input_names[0],
            "file": "input_bgr_resized.bin",
            "dtype": "uint8",
            "channel_order": "BGR",
            "height": int(resized_bgr.shape[0]),
            "width": int(resized_bgr.shape[1]),
            "sha256": sha256_file(args.out / "input_bgr_resized.bin"),
        },
        "outputs": [
            {
                "name": output_names[0],
                "file": "boxes.npy",
                "shape": list(boxes.shape),
                "dtype": "float32",
            },
            {
                "name": output_names[1],
                "file": "structure.npy",
                "shape": list(structure.shape),
                "dtype": "float32",
            },
        ],
        "vocabulary": {
            "token_count": len(character),
            "class_count": len(decode.character),
            "sos_class_index": decode.char_to_index[decode.beg_str],
            "eos_class_index": decode.char_to_index[decode.end_str],
            "tokens": character,
        },
        "preprocess": {
            "max_len": MAX_LEN,
            "mean": MEAN,
            "std": STD,
            "scale": SCALE,
            "order": "resize -> normalize -> pad(top-left, zeros) -> to_chw",
            "resize": "cv2.resize default (INTER_LINEAR), ratio = max_len / max(h, w)",
            "shape_list": [float(v) for v in shape_list],
        },
        "geometry": {
            "ratio": float(ratio_stage),
            "w_ratio": float(w_ratio),
            "h_ratio": float(h_ratio),
            "uniform_scale_max_h_w": float(max(ori_height, ori_width)),
            "statement": (
                "参考实现两步复合后 x 与 y 的总因子都是 max(h, w) = 488 / ratio"
            ),
        },
        "reference": {
            "confidence": reference_confidence,
            "tokens": tokens,
            "html": reference_html,
            "cells": cells,
            "kept_bboxes": [[float(v) for v in row] for row in reference_cells],
            "dropped_blank_count": len(cells) - len(kept),
            "logic_points": [[int(v) for v in point] for point in logic_points],
            "grid": {"rows": int(grid_rows), "cols": int(grid_cols)},
        },
    }
    (args.out / "manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )

    print(f"wrote golden to {args.out}")
    print(f"  L = {steps}, cells = {len(cells)} (kept {len(kept)})")
    print(f"  confidence = {reference_confidence:.6f}")
    print(f"  tokens = {len(tokens)}")
    print(f"  ratio = {ratio_stage:.6f}, w_ratio = {w_ratio:.6f}, h_ratio = {h_ratio:.6f}")
    print(f"  max(h, w) = {max(ori_height, ori_width)}")
    print(f"  grid = {grid_rows} x {grid_cols} (logic points {len(logic_points)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
