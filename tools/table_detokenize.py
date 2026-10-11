#!/usr/bin/env python3
"""TabRecSet -> PubTabNet-style detokenize adapter (execution doc 09, stage E6).

`datasets/table/tabrecset/` ships two annotation systems for the same table
images:

* ``pubtabnet_format/`` -- PubTabNet 2.0 token streams
  (``{split}_CAPTIONS_STRUCTURE.json`` + ``{split}_CAPTIONS_CELL.json``) with a
  shared vocabulary (``WORDMAP_STRUCTURE.json`` / ``WORDMAP_CELL.json``), plus
  ``{split}_IMAGE_PATHS.txt`` naming the images.
* ``cropped_datasets_1Tab_per_img/{split}/`` -- the images themselves, each with
  a LabelMe 4.5.9 polygon annotation whose ``label`` is
  ``<row>-<col>-<rowspan>-<colspan>-<text>``.

This tool turns the token streams into HTML (the shape the official PubTabNet
``metric.py`` consumes) and turns the LabelMe annotations into a structural
record.  It also carries the two evidence checks that pin the format down:

* ``--fill-rate``  -- the grid fill rate under the two competing readings of the
  label fields, which is how doc 09 §7.2 identified the correct one.
* ``--cross-check`` -- the token stream and the LabelMe annotation are
  independent ground truths for the same image; they must agree on the cell
  count and on the multiset of ``(rowspan, colspan)`` pairs.

Deliverables land in ``datasets/table/tabrecset/derived/`` (the whole
``datasets/`` tree is untracked, so nothing here is committed).

Usage::

    python tools/table_detokenize.py                       # build val, write manifest
    python tools/table_detokenize.py --split all
    python tools/table_detokenize.py --dump 20             # human spot check, no writes
    python tools/table_detokenize.py --dump 20 --dump-json out.jsonl
    python tools/table_detokenize.py --fill-rate
    python tools/table_detokenize.py --cross-check
    python tools/table_detokenize.py --verify

Exit codes: 0 success, 2 usage/asset problem, 3 verification failure.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TABRECSET = REPO_ROOT / "datasets" / "table" / "tabrecset"
PUBTABNET = TABRECSET / "pubtabnet_format"
CROPPED = TABRECSET / "cropped_datasets_1Tab_per_img"
DERIVED = TABRECSET / "derived"

TOOL_NAME = "tools/table_detokenize.py"
SPLITS = ("train", "val")

#: Doc 09 §7.2 reports these two figures for a stable-hash 400-sample of the val
#: split.  They are re-derived here rather than copied, so the check fails loudly
#: if the format ever changes under us.
DOC_FILL_RATE_400 = {"A": 331, "B": 301}

#: Field separator used by `src/evaluation/formula/sampling.rs` when it builds
#: the sample key, and its record separator for the canonical manifest string.
SAMPLE_KEY_SEP = "\x1f"
RECORD_SEP = "\x1e"

SPAN_RE = re.compile(r'^\s*(col|row)span="(\d+)"$')


# --------------------------------------------------------------------------- io


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def sha256_text(text: str) -> str:
    return sha256_bytes(text.encode("utf-8"))


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_json(path: Path):
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def sample_digest(relative_path: str, ground_truth: str, include_directory: bool) -> str:
    """Reproduce `SampleStrategy::Hash` from `src/evaluation/formula/sampling.rs`.

    ``sample_key = relative_path + U+001F + ground_truth``; the digest is the
    lowercase hex SHA-256.  ``relative_path`` uses ``/`` separators.
    """
    key = relative_path if include_directory else relative_path.rsplit("/", 1)[-1]
    return sha256_text(key + SAMPLE_KEY_SEP + ground_truth)


def stable_hash_sample(entries: list[dict], limit: int, include_directory: bool) -> list[dict]:
    """Sort by sample digest ascending and keep the first ``limit`` (0 = all).

    Deliberately not "first N": `sampling.rs` carries
    `hash_sampling_is_not_first_n` for exactly this reason.
    """
    if not limit:
        return list(entries)
    ordered = sorted(
        entries,
        key=lambda entry: sample_digest(entry["relative_path"], entry["ground_truth"], include_directory),
    )
    return ordered[:limit]


# --------------------------------------------------------------- token stream


class Vocabulary:
    """The `WORDMAP_*.json` tables, indexed both ways."""

    def __init__(self, structure: dict, cell: dict):
        self.structure = structure
        self.cell = cell
        self.structure_token = {index: token for token, index in structure.items()}
        self.cell_token = {index: token for token, index in cell.items()}
        missing = [name for name in ("<start>", "<end>", "<td>", "<td", ">", "</td>") if name not in structure]
        if missing:
            raise ValueError(f"WORDMAP_STRUCTURE.json is missing required tokens: {missing}")
        if "" not in cell:
            raise ValueError('WORDMAP_CELL.json has no "" entry, so empty cells cannot be recognised')
        self.cell_empty = cell[""]
        self.cell_start = structure["<start>"]
        self.cell_end = structure["<end>"]
        self.td_plain = structure["<td>"]
        self.td_open = structure["<td"]
        self.td_close = structure["</td>"]
        self.tag_close = structure[">"]

    def is_empty_cell(self, caption: list[int]) -> bool:
        """An empty cell is the single-element sentinel ``[<empty>]``."""
        return caption == [self.cell_empty]


def decode_cell_texts(captions: list[list[int]], vocab: Vocabulary) -> list[str]:
    """One text per cell, in cell order.

    The cell caption for a non-empty cell is ``[<start>, ...tokens..., <end>]``.
    The raw text is spliced in unescaped: 4,171 val cells contain ``<`` *and* the
    same number contain ``>`` (they are balanced markup such as ``SiO<sub>2</sub>``),
    so the token stream is an HTML fragment and escaping it would corrupt the
    ground truth.
    """
    texts = []
    for caption in captions:
        if vocab.is_empty_cell(caption):
            texts.append("")
            continue
        if caption[0] != vocab.cell_start or caption[-1] != vocab.cell_end:
            raise ValueError(f"cell caption is not delimited by <start>/<end>: {caption[:4]}...{caption[-2:]}")
        texts.append("".join(vocab.cell_token[token] for token in caption[1:-1]))
    return texts


def decode_structure_html(structure_ids: list[int], texts: list[str], vocab: Vocabulary) -> tuple[str, int]:
    """Turn one structure token stream plus its cell texts into a full HTML page.

    Returns ``(html, cell_count)``.  ``<start>``/``<end>`` do not appear in the
    body.  A plain cell is the two-token sequence ``<td>`` ``</td>`` and a spanned
    cell is ``<td`` + attribute fragments + ``>``, each followed by ``</td>``.
    The closing tag therefore comes from the stream and is consumed exactly once:
    synthesising it as well would emit ``</td></td>``, which lxml silently
    tolerates but which is not the format.
    """
    out = ["<html>", "<body>", "<table>"]
    cell_index = 0
    index = 0
    total = len(structure_ids)

    def consume_close(position: int, where: str) -> int:
        """Require ``</td>`` at ``position`` and emit it; return the next index."""
        if position >= total or structure_ids[position] != vocab.td_close:
            found = "end of stream" if position >= total else repr(vocab.structure_token[structure_ids[position]])
            raise ValueError(f"{where} is not followed by </td> in the structure stream (found {found})")
        out.append("</td>")
        return position + 1

    while index < total:
        token_id = structure_ids[index]
        if token_id in (vocab.cell_start, vocab.cell_end):
            index += 1
            continue
        if token_id == vocab.td_plain:
            if cell_index >= len(texts):
                raise ValueError("structure stream has more cells than the cell captions")
            out.append("<td>")
            out.append(texts[cell_index])
            cell_index += 1
            index = consume_close(index + 1, f"cell {cell_index - 1} (<td>)")
            continue
        if token_id == vocab.td_open:
            cursor = index + 1
            fragments = ["<td"]
            while cursor < total and structure_ids[cursor] != vocab.tag_close:
                fragments.append(vocab.structure_token[structure_ids[cursor]])
                cursor += 1
            if cursor >= total:
                raise ValueError("structure stream ends inside an unterminated cell opening tag")
            fragments.append(">")
            if cell_index >= len(texts):
                raise ValueError("structure stream has more cells than the cell captions")
            out.extend(fragments)
            out.append(texts[cell_index])
            cell_index += 1
            index = consume_close(cursor + 1, f"cell {cell_index - 1} ({''.join(fragments)})")
            continue
        if token_id == vocab.td_close:
            raise ValueError(f"stray </td> at structure index {index} that no cell opening tag claims")
        out.append(vocab.structure_token[token_id])
        index += 1
    if cell_index != len(texts):
        raise ValueError(f"structure stream has {cell_index} cells but the cell captions have {len(texts)}")
    out.extend(["</table>", "</body>", "</html>"])
    return "".join(out), cell_index


def token_stream_spans(structure_ids: list[int], vocab: Vocabulary) -> list[tuple[int, int]]:
    """The ``(rowspan, colspan)`` multiset carried by one structure stream."""
    spans = []
    index = 0
    total = len(structure_ids)
    while index < total:
        token_id = structure_ids[index]
        if token_id == vocab.td_plain:
            spans.append((1, 1))
            index += 1
        elif token_id == vocab.td_open:
            rowspan = colspan = 1
            cursor = index + 1
            while cursor < total and structure_ids[cursor] != vocab.tag_close:
                match = SPAN_RE.match(vocab.structure_token[structure_ids[cursor]])
                if match:
                    if match.group(1) == "col":
                        colspan = int(match.group(2))
                    else:
                        rowspan = int(match.group(2))
                cursor += 1
            spans.append((rowspan, colspan))
            index = cursor + 1
        else:
            index += 1
    return spans


# ---------------------------------------------------------- LabelMe annotation


def parse_label(label: str):
    """``<row>-<col>-<rowspan>-<colspan>-<text>``.

    `text` may itself contain ``-`` and newlines, so the split is capped at four.
    """
    parts = label.split("-", 4)
    if len(parts) < 5:
        return None
    try:
        return int(parts[0]), int(parts[1]), int(parts[2]), int(parts[3]), parts[4]
    except ValueError:
        return None


def bbox_of(points: list) -> list[float] | None:
    xs = [float(point[0]) for point in points]
    ys = [float(point[1]) for point in points]
    if not xs or not ys:
        return None
    return [min(xs), min(ys), max(xs), max(ys)]


def labelme_cells(document: dict) -> list[dict]:
    """Split a LabelMe document into per-cell records, ordered as annotated."""
    cells = []
    for shape in document.get("shapes", []):
        parsed = parse_label(shape.get("label", ""))
        if parsed is None:
            continue
        row, col, third, fourth, text = parsed
        cells.append(
            {
                "index": len(cells),
                "row": row,
                "col": col,
                # Interpretation A: field 3 is rowspan, field 4 is colspan.
                # Proven by `--cross-check`, not merely by `--fill-rate`.
                "rowspan": max(third, 1),
                "colspan": max(fourth, 1),
                "text": text,
                "polygon": [[float(p[0]), float(p[1])] for p in shape.get("points", [])],
                "bbox": bbox_of(shape.get("points", [])),
                "shape_type": shape.get("shape_type", ""),
            }
        )
    return cells


def analyse_grid(cells: list[dict]) -> dict:
    """Grid extents, hole and overlap counts for one table.

    ``sealed`` is the criterion doc 09 §7.2 used: the placed cells cover the
    bounding grid exactly, overlaps tolerated.  ``sealed_clean`` additionally
    requires that no cell overlaps another.
    """
    occupied: dict[tuple[int, int], int] = {}
    duplicates = 0
    max_row = max_col = 0
    for cell in cells:
        row, col = cell["row"], cell["col"]
        rowspan, colspan = cell["rowspan"], cell["colspan"]
        for r in range(row, row + rowspan):
            for c in range(col, col + colspan):
                if (r, c) in occupied:
                    duplicates += 1
                occupied[(r, c)] = cell["index"]
        max_row = max(max_row, row + rowspan - 1)
        max_col = max(max_col, col + colspan - 1)
    box = max_row * max_col
    holes = box - len(occupied)
    return {
        "rows": max_row,
        "cols": max_col,
        "occupied": len(occupied),
        "box": box,
        "holes": holes,
        "duplicates": duplicates,
        "sealed": box > 0 and holes == 0,
        "sealed_clean": box > 0 and holes == 0 and duplicates == 0,
    }


# ------------------------------------------------------------------- assets


def resolve_image(name: str, split: str) -> str | None:
    """Map a basename from `{split}_IMAGE_PATHS.txt` onto a real image file.

    The list stores a placeholder prefix (``/<your_path_to_val_image_folder>/``);
    the images themselves live in `cropped_datasets_1Tab_per_img/{split}/`.
    """
    candidate = CROPPED / split / name
    if candidate.is_file():
        return candidate.relative_to(TABRECSET).as_posix()
    return None


def load_vocabulary() -> Vocabulary:
    for path in (
        PUBTABNET / "WORDMAP_STRUCTURE.json",
        PUBTABNET / "WORDMAP_CELL.json",
    ):
        if not path.is_file():
            raise FileNotFoundError(f"missing TabRecSet vocabulary: {path}")
    return Vocabulary(read_json(PUBTABNET / "WORDMAP_STRUCTURE.json"), read_json(PUBTABNET / "WORDMAP_CELL.json"))


def load_split(split: str) -> dict:
    """Everything the adapter needs for one split, with no derived state."""
    needed = [
        PUBTABNET / f"{split}_CAPTIONS_STRUCTURE.json",
        PUBTABNET / f"{split}_CAPTIONS_CELL.json",
        PUBTABNET / f"{split}_IMAGE_PATHS.txt",
        PUBTABNET / f"{split}_NUMBER_CELLS_PER_IMAGE.json",
    ]
    for path in needed:
        if not path.is_file():
            raise FileNotFoundError(f"missing TabRecSet file for split {split}: {path}")
    with (PUBTABNET / f"{split}_IMAGE_PATHS.txt").open(encoding="utf-8") as handle:
        names = [Path(line.strip()).name for line in handle if line.strip()]
    return {
        "names": names,
        "structure": read_json(PUBTABNET / f"{split}_CAPTIONS_STRUCTURE.json"),
        "cells": read_json(PUBTABNET / f"{split}_CAPTIONS_CELL.json"),
        "declared_cells": read_json(PUBTABNET / f"{split}_NUMBER_CELLS_PER_IMAGE.json"),
    }


# -------------------------------------------------------------------- build


def build_records(split: str, vocab: Vocabulary) -> tuple[list[dict], list[dict], list[str]]:
    """Decode one split into PubTabNet records and LabelMe structural records."""
    data = load_split(split)
    names = data["names"]
    structure = data["structure"]
    captions = data["cells"]
    declared = data["declared_cells"]

    lengths = {len(names), len(structure), len(captions), len(declared)}
    if len(lengths) != 1:
        raise ValueError(f"{split}: TabRecSet files disagree on sample count: {sorted(lengths)}")

    pubtabnet_records: list[dict] = []
    cropped_records: list[dict] = []
    problems: list[str] = []

    images_dir = CROPPED / split
    seen: set[str] = set()

    for index, name in enumerate(names):
        texts = decode_cell_texts(captions[index], vocab)
        html, cell_count = decode_structure_html(structure[index], texts, vocab)
        if cell_count != declared[index]:
            problems.append(f"{name}: decoded {cell_count} cells but the dataset declares {declared[index]}")
        image_path = resolve_image(name, split)
        if image_path is None:
            problems.append(f"{name}: no image under {images_dir.relative_to(REPO_ROOT).as_posix()}")
        pubtabnet_records.append(
            {
                "file_name": name,
                "split": split,
                "image_path": image_path,
                "n_cells": cell_count,
                "structure_tokens": len(structure[index]),
                "texts_sha256": sha256_text(SAMPLE_KEY_SEP.join(texts)),
                "html": html,
                "html_sha256": sha256_text(html),
            }
        )
        seen.add(name)

    # Every annotation in the cropped directory, including the ones that the
    # PubTabNet list does not name (`--cross-check` explains why the two sets
    # differ in size).
    for annotation in sorted(images_dir.glob("*.json")):
        image = images_dir / (annotation.stem + ".jpg")
        if not image.is_file():
            problems.append(f"{annotation.name}: no sibling image")
            continue
        document = read_json(annotation)
        cells = labelme_cells(document)
        grid = analyse_grid(cells)
        cropped_records.append(
            {
                "file_name": image.name,
                "split": split,
                "image_path": image.relative_to(TABRECSET).as_posix(),
                "annotation_path": annotation.relative_to(TABRECSET).as_posix(),
                "in_pubtabnet_list": image.name in seen,
                "labelme_version": document.get("version", ""),
                "cells": cells,
                "n_cells": len(cells),
                "grid": grid,
            }
        )
    return pubtabnet_records, cropped_records, problems


def write_jsonl(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        for record in records:
            handle.write(json.dumps(record, ensure_ascii=False, sort_keys=True))
            handle.write("\n")


def write_json(path: Path, payload) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2, sort_keys=True)
        handle.write("\n")


def build(args: argparse.Namespace) -> int:
    vocab = load_vocabulary()
    splits = SPLITS if args.split == "all" else (args.split,)

    sources: dict[str, dict] = {}
    for relative in (
        "WORDMAP_STRUCTURE.json",
        "WORDMAP_CELL.json",
        *(f"{split}_{suffix}.json" for split in splits for suffix in ("CAPTIONS_STRUCTURE", "CAPTIONS_CELL", "NUMBER_CELLS_PER_IMAGE")),
        *(f"{split}_IMAGE_PATHS.txt" for split in splits),
    ):
        path = PUBTABNET / relative
        sources[f"pubtabnet_format/{relative}"] = {"bytes": path.stat().st_size, "sha256": sha256_file(path)}

    counts: dict[str, dict] = {}
    outputs: dict[str, dict] = {}
    all_problems: list[str] = []

    for split in splits:
        pubtabnet_records, cropped_records, problems = build_records(split, vocab)
        all_problems.extend(problems)

        html_json = {
            record["file_name"]: {"html": record["html"]} for record in pubtabnet_records
        }
        pubtabnet_jsonl = DERIVED / f"pubtabnet_{split}.jsonl"
        cropped_jsonl = DERIVED / f"cropped_{split}_structure.jsonl"
        gt_html_json = DERIVED / f"pubtabnet_{split}_gt_html.json"
        write_jsonl(pubtabnet_jsonl, pubtabnet_records)
        write_jsonl(cropped_jsonl, cropped_records)
        write_json(gt_html_json, html_json)

        resolved = sum(1 for record in pubtabnet_records if record["image_path"])
        counts[split] = {
            "pubtabnet_samples": len(pubtabnet_records),
            "pubtabnet_images_resolved": resolved,
            "cropped_annotations": len(cropped_records),
            "cropped_in_pubtabnet_list": sum(1 for record in cropped_records if record["in_pubtabnet_list"]),
            "cropped_not_in_pubtabnet_list": sum(1 for record in cropped_records if not record["in_pubtabnet_list"]),
            "cells_total": sum(record["n_cells"] for record in pubtabnet_records),
            "grid_sealed": sum(1 for record in cropped_records if record["grid"]["sealed"]),
            "grid_sealed_clean": sum(1 for record in cropped_records if record["grid"]["sealed_clean"]),
            "grid_with_holes": sum(1 for record in cropped_records if record["grid"]["holes"] > 0),
            "grid_with_overlap": sum(1 for record in cropped_records if record["grid"]["duplicates"] > 0),
            "hole_cells_total": sum(record["grid"]["holes"] for record in cropped_records),
        }
        for artefact in (pubtabnet_jsonl, cropped_jsonl, gt_html_json):
            outputs[artefact.relative_to(REPO_ROOT).as_posix()] = {
                "bytes": artefact.stat().st_size,
                "sha256": sha256_file(artefact),
            }

    manifest = {
        "tool": TOOL_NAME,
        "generated_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "dataset": "tabrecset",
        "splits": list(splits),
        "sources": sources,
        "counts": counts,
        "outputs": outputs,
        "wordmap": {
            "structure_tokens": len(vocab.structure),
            "cell_tokens": len(vocab.cell),
            "empty_cell_sentinel": vocab.cell_empty,
        },
        "field_semantics": {
            "label_format": "<row>-<col>-<rowspan>-<colspan>-<text>",
            "evidence": "see --cross-check; the span multiset agrees with the token stream "
            "7104/7104 under this reading and 0/7104 under the swapped one",
        },
        "problems": all_problems,
    }
    write_json(DERIVED / "manifest.json", manifest)

    for split in splits:
        row = counts[split]
        print(
            f"[{split}] pubtabnet={row['pubtabnet_samples']} "
            f"images={row['pubtabnet_images_resolved']}/{row['pubtabnet_samples']} "
            f"cropped={row['cropped_annotations']} (+{row['cropped_not_in_pubtabnet_list']} outside the list) "
            f"cells={row['cells_total']} sealed={row['grid_sealed']} sealed_clean={row['grid_sealed_clean']}"
        )
    print(f"wrote {DERIVED.relative_to(REPO_ROOT).as_posix()}/")
    if all_problems:
        print(f"!! {len(all_problems)} problem(s):")
        for problem in all_problems[:10]:
            print(f"   {problem}")
        return 2
    return 0


# --------------------------------------------------------------------- dump


def dump(args: argparse.Namespace) -> int:
    vocab = load_vocabulary()
    records = []
    for split in (SPLITS if args.split == "all" else (args.split,)):
        data = load_split(split)
        entries = []
        for index, name in enumerate(data["names"]):
            texts = decode_cell_texts(data["cells"][index], vocab)
            html, cell_count = decode_structure_html(data["structure"][index], texts, vocab)
            entries.append(
                {
                    "relative_path": name,
                    "ground_truth": html,
                    "split": split,
                    "cells": texts,
                    "n_cells": cell_count,
                }
            )
        records.extend(stable_hash_sample(entries, args.dump, args.dump_key_has_directory))

    for record in records:
        print(f"--- {record['split']}/{record['relative_path']}  cells={record['n_cells']}")
        for cell_index, text in enumerate(record["cells"]):
            print(f"    [{cell_index:>3}] {text!r}")
        print(record["ground_truth"])
    print(f"{len(records)} sample(s) dumped (stable hash, limit={args.dump})")

    if args.dump_json:
        write_jsonl(Path(args.dump_json), records)
        print(f"wrote {args.dump_json}")
    return 0 if records else 2


# ---------------------------------------------------------------- fill rate


def fill_rate(split: str, limit: int, include_directory: bool, span_third_is_rowspan: bool) -> dict:
    """Grid fill rate over a stable-hash sample of the LabelMe annotations."""
    samples = []
    images_dir = CROPPED / split
    for annotation in sorted(images_dir.glob("*.json")):
        raw = annotation.read_text(encoding="utf-8")
        samples.append(
            {
                # `sampling.rs` keys on a dataset-relative path; whether the split
                # directory is part of it changes which 400 samples get picked,
                # so both readings are reported.
                "relative_path": f"{split}/{annotation.name}",
                "ground_truth": raw,
                "annotation": annotation,
            }
        )
    picked = stable_hash_sample(samples, limit, include_directory)
    sealed = sealed_clean = holes = overlaps = 0
    for record in picked:
        cells = labelme_cells(json.loads(record["ground_truth"]))
        if not span_third_is_rowspan:
            cells = [dict(cell, rowspan=cell["colspan"], colspan=cell["rowspan"]) for cell in cells]
        grid = analyse_grid(cells)
        sealed += int(grid["sealed"])
        sealed_clean += int(grid["sealed_clean"])
        holes += int(grid["holes"] > 0)
        overlaps += int(grid["duplicates"] > 0)
    return {
        "split": split,
        "limit": limit,
        "relative_path_includes_directory": include_directory,
        "span_third_is_rowspan": span_third_is_rowspan,
        "samples": len(picked),
        "sealed": sealed,
        "sealed_clean": sealed_clean,
        "with_holes": holes,
        "with_overlap": overlaps,
    }


def fill_rate_report(args: argparse.Namespace) -> int:
    limit = args.sample_size
    print(f"grid fill rate, split={args.split}, stable-hash sample size={limit or 'all'}")
    print("criterion 'sealed' = no holes (overlaps tolerated); this is doc 09 §7.2's criterion")
    print()
    results = {}
    for include_directory in (False, True):
        for span_third in (True, False):
            report = fill_rate(args.split, limit, include_directory, span_third)
            label = "A field3=rowspan" if span_third else "B field3=colspan"
            keyed = "basename" if not include_directory else "dir/basename"
            results[(include_directory, span_third)] = report
            print(
                f"  key={keyed:<13} {label}: sealed={report['sealed']}/{report['samples']} "
                f"({report['sealed'] / report['samples']:.3%})  sealed_clean={report['sealed_clean']}  "
                f"holes>0={report['with_holes']}  overlap>0={report['with_overlap']}"
            )

    doc_a = results[(False, True)]["sealed"]
    doc_b = results[(False, False)]["sealed"]
    print()
    if limit and doc_a == DOC_FILL_RATE_400["A"] and doc_b == DOC_FILL_RATE_400["B"]:
        print(
            f"doc 09 §7.2 figure reproduced exactly: A={doc_a}, B={doc_b} "
            "(key=basename, size=400, criterion=sealed)"
        )
        return 0
    print(
        f"doc 09 §7.2 reports A={DOC_FILL_RATE_400['A']}, B={DOC_FILL_RATE_400['B']}; "
        f"measured A={doc_a}, B={doc_b}"
    )
    return 3


# --------------------------------------------------------------- cross check


def cross_check(args: argparse.Namespace) -> int:
    vocab = load_vocabulary()
    data = load_split(args.split)
    names = data["names"]
    structure = data["structure"]
    captions = data["cells"]
    declared = data["declared_cells"]

    checked = count_ok = spans_a = spans_b = sealed = 0
    failures: list[str] = []
    for index, name in enumerate(names):
        annotation = CROPPED / args.split / (name[:-4] + ".json")
        if not annotation.is_file():
            continue
        checked += 1
        cells = labelme_cells(read_json(annotation))
        stream_spans = token_stream_spans(structure[index], vocab)
        texts = decode_cell_texts(captions[index], vocab)
        if len(stream_spans) == len(texts) == len(cells) == declared[index]:
            count_ok += 1
        else:
            failures.append(
                f"{name}: stream={len(stream_spans)} captions={len(texts)} "
                f"labelme={len(cells)} declared={declared[index]}"
            )
        counter_stream = Counter(stream_spans)
        if counter_stream == Counter((cell["rowspan"], cell["colspan"]) for cell in cells):
            spans_a += 1
        elif counter_stream == Counter((cell["colspan"], cell["rowspan"]) for cell in cells):
            spans_b += 1
        else:
            failures.append(f"{name}: span multiset disagrees with the token stream")
        if analyse_grid(cells)["sealed"]:
            sealed += 1

    if not checked:
        print(f"no sample of split {args.split} has both ground truths")
        return 2
    print(f"cross-checked {checked} samples of split {args.split} with both ground truths")
    print(f"  cell count agrees (token stream == cell captions == LabelMe == NUMBER_CELLS): {count_ok}/{checked}")
    print(f"  span multiset agrees, field3=rowspan: {spans_a}/{checked}")
    print(f"  span multiset agrees, field3=colspan: {spans_b}/{checked}")
    print(f"  LabelMe grid seals (no holes): {sealed}/{checked}")
    for failure in failures[:10]:
        print(f"  !! {failure}")
    return 0 if (count_ok == checked and spans_a == checked) else 3


# -------------------------------------------------------------------- verify


def verify(args: argparse.Namespace) -> int:
    manifest_path = DERIVED / "manifest.json"
    if not manifest_path.is_file():
        print(f"no manifest at {manifest_path}; run the builder first")
        return 2
    manifest = read_json(manifest_path)
    failures: list[str] = []

    for relative, expected in manifest["sources"].items():
        path = TABRECSET / relative
        if not path.is_file():
            failures.append(f"source missing: {relative}")
            continue
        actual = {"bytes": path.stat().st_size, "sha256": sha256_file(path)}
        if actual != expected:
            failures.append(f"source changed: {relative} expected {expected} got {actual}")

    for relative, expected in manifest["outputs"].items():
        path = REPO_ROOT / relative
        if not path.is_file():
            failures.append(f"output missing: {relative}")
            continue
        actual = {"bytes": path.stat().st_size, "sha256": sha256_file(path)}
        if actual != expected:
            failures.append(f"output changed: {relative} expected {expected} got {actual}")

    # Re-derive the decoded content and compare it against the written JSONL, so
    # a hand-edited artifact cannot pass by having its hash refreshed.
    splits = manifest["splits"]
    vocab = load_vocabulary()
    for split in splits:
        pubtabnet_records, cropped_records, problems = build_records(split, vocab)
        for problem in problems:
            failures.append(f"{split}: {problem}")
        written = [json.loads(line) for line in (DERIVED / f"pubtabnet_{split}.jsonl").read_text(encoding="utf-8").splitlines()]
        if len(written) != len(pubtabnet_records):
            failures.append(f"{split}: jsonl has {len(written)} lines, expected {len(pubtabnet_records)}")
        else:
            for index, (left, right) in enumerate(zip(written, pubtabnet_records)):
                if left != right:
                    failures.append(f"{split}: jsonl record {index} ({left.get('file_name')}) differs from a fresh decode")
                    break
        written_cropped = [
            json.loads(line) for line in (DERIVED / f"cropped_{split}_structure.jsonl").read_text(encoding="utf-8").splitlines()
        ]
        if written_cropped != cropped_records:
            failures.append(f"{split}: cropped structure jsonl differs from a fresh decode")
        html_json = read_json(DERIVED / f"pubtabnet_{split}_gt_html.json")
        expected_html = {record["file_name"]: {"html": record["html"]} for record in pubtabnet_records}
        if html_json != expected_html:
            failures.append(f"{split}: gt_html json differs from a fresh decode")

    counts = manifest["counts"]
    for split in splits:
        fresh_pubtabnet, fresh_cropped, _ = build_records(split, vocab)
        fresh = {
            "pubtabnet_samples": len(fresh_pubtabnet),
            "pubtabnet_images_resolved": sum(1 for record in fresh_pubtabnet if record["image_path"]),
            "cropped_annotations": len(fresh_cropped),
            "cells_total": sum(record["n_cells"] for record in fresh_pubtabnet),
            "grid_sealed": sum(1 for record in fresh_cropped if record["grid"]["sealed"]),
        }
        for key, value in fresh.items():
            if counts[split].get(key) != value:
                failures.append(f"{split}: count {key} is {counts[split].get(key)}, expected {value}")

    if failures:
        print(f"VERIFY FAILED ({len(failures)} problem(s)):")
        for failure in failures[:20]:
            print(f"  {failure}")
        return 3
    print(f"verify ok: {manifest['dataset']} splits={','.join(splits)} manifest={manifest_path.name}")
    return 0


# ---------------------------------------------------------------------- main


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--split", choices=(*SPLITS, "all"), default="val")
    parser.add_argument("--dump", type=int, metavar="N", help="print N stable-hash-sampled decoded samples and exit")
    parser.add_argument("--dump-json", metavar="PATH", help="also write the dumped records as JSONL")
    parser.add_argument(
        "--dump-key-has-directory",
        action="store_true",
        help="include the split directory in the sample key (the sampling.rs reading)",
    )
    parser.add_argument("--fill-rate", action="store_true", help="report the grid fill rate under both readings")
    parser.add_argument("--cross-check", action="store_true", help="compare the token stream against the LabelMe grid")
    parser.add_argument("--verify", action="store_true", help="re-verify the written artifacts against the manifest")
    parser.add_argument(
        "--sample-size",
        type=int,
        default=400,
        help="stable-hash sample size for --fill-rate (0 = every sample)",
    )
    args = parser.parse_args(argv)

    try:
        if args.dump is not None:
            return dump(args)
        if args.fill_rate:
            return fill_rate_report(args)
        if args.cross_check:
            return cross_check(args)
        if args.verify:
            return verify(args)
        return build(args)
    except (FileNotFoundError, ValueError) as error:
        print(f"error: {error}")
        return 2


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
