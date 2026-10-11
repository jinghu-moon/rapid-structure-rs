#!/usr/bin/env python3
"""Assemble ``tests/baseline/windows-baseline/table-teds-cpu.json`` (E7, doc 09 §8).

The baseline is *derived*, never hand-typed: this script reads

* the per-run reports written by ``target/release/table_eval.exe`` (at least two
  runs at the same configuration, so that byte-identity across runs is a real
  claim rather than a vacuous one),
* the per-sample TEDS file written by ``tools/table_teds.py``,

and folds them into one artifact that states exactly which metric was reported,
on which sample set, with which model and configuration.

Only **TEDS-Struct** is reported.  Full TEDS compares cell *text*, and the text of
a predicted cell comes from page-level OCR matched onto the predicted cell
polygons -- that mechanism (R1 / E9) does not exist yet.  Inventing a provisional
matching rule here would produce a number that measures this script rather than
the product, so the gap is registered instead of filled.

Run:

    python tools/build_table_teds_baseline.py \
        --report Temp/e7-run1-report.json --report Temp/e7-run2-report.json \
        --report Temp/e7-run3-report.json --teds Temp/e7-run1-teds.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import statistics
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT = REPO_ROOT / "tests" / "baseline" / "windows-baseline" / "table-teds-cpu.json"

# The official implementation, pinned by content in tools/table_teds.py.  Repeated
# here so the baseline records which metric definition produced the number.
METRIC_SOURCE = "https://raw.githubusercontent.com/ibm-aur-nlp/PubTabNet/master/src/metric.py"
METRIC_SHA256 = "a9aedd313678b42e55b06c4b0a365d658ef495aaf3abb7fd6012a6e9f334f9f9"

NOT_COMPARABLE = (
    "TEDS-Struct on TabRecSet val. NOT comparable with the SLANet TEDS 95.89 in the "
    "PubTabNet paper: different dataset (Chinese/mixed TabRecSet val vs English "
    "PubTabNet test), different split, and structure-only rather than full TEDS. "
    "SLANet_plus 63.69 is a PaddleX internal-set Accuracy, not a TEDS, and is not "
    "comparable either."
)

# The full-TEDS wording differs in kind, not just in the metric name: the cell text comes
# from this project's own page-level OCR, which is a second, independent reason the number
# is not comparable.  Keeping one shared string would have forced one of the two cases to
# state something untrue about itself.
NOT_COMPARABLE_FULL = (
    "TEDS on TabRecSet val, with cell text produced by this project's page-level OCR "
    "matched onto the predicted cell polygons. NOT comparable with the SLANet TEDS 95.89 "
    "in the PubTabNet paper: different dataset (Chinese/mixed TabRecSet val vs English "
    "PubTabNet test), a different split, and PubTabNet scores TEDS against ground-truth "
    "cell text rather than recognised text. SLANet_plus 63.69 is a PaddleX internal-set "
    "Accuracy, not a TEDS, and is not comparable either."
)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load(path: Path) -> dict:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def histogram(values: list[float]) -> list[dict]:
    buckets = Counter(round(value, 1) for value in values)
    total = len(values)
    return [
        {"score": bucket, "count": buckets[bucket], "fraction": buckets[bucket] / total}
        for bucket in sorted(buckets)
    ]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--report", action="append", required=True, metavar="PATH", help="table_eval report (repeat per run)")
    parser.add_argument("--teds", required=True, metavar="PATH", help="per-sample TEDS file from tools/table_teds.py")
    parser.add_argument("--ablation", metavar="PATH", help="probe file from `tools/table_teds.py --ablate`")
    parser.add_argument("--predictions", metavar="PATH", help="prediction json, to measure cell-count agreement")
    parser.add_argument("--gt-html", metavar="PATH", help="ground-truth html json, for the same measurement")
    parser.add_argument("--output", metavar="PATH", default=str(DEFAULT_OUTPUT))
    parser.add_argument(
        "--stage",
        metavar="TEXT",
        help="override the `stage` field (defaults to the stage that introduced the metric: "
        "E7 for TEDS-Struct, E9 for full TEDS). Pass the stage that rebuilt this artifact when "
        "it is regenerated later, so the file names the run that produced it rather than the "
        "run that first defined the metric.",
    )
    args = parser.parse_args(argv)

    reports = [load(Path(path)) for path in args.report]
    teds = load(Path(args.teds))
    scores: dict[str, float] = teds["scores"]

    # The metric *definition* is read out of the per-sample TEDS file, never assumed here.
    # Hardcoding `structure_only: True` made this artifact lie the moment it was fed a
    # full-TEDS run, and the lie would be invisible -- in the file whose whole job is to
    # state which metric produced the number.
    metric_structure_only = teds.get("structure_only")
    if metric_structure_only is None:
        print(
            "error: the per-sample TEDS file has no `structure_only` field, so this baseline "
            "cannot state which metric definition produced the number; re-run "
            "tools/table_teds.py with --out"
        )
        return 2

    primary = reports[0]
    failed = {entry["file_name"] for entry in primary["failures"]}
    values = list(scores.values())
    decodable = [scores[name] for name in scores if name not in failed]

    # Determinism evidence: the prediction artifact is compared by *bytes*, which is
    # a stronger statement than "three means happened to agree".
    digests = [report["outputs"]["predictions"]["sha256"] for report in reports]
    sizes = [report["outputs"]["predictions"]["bytes"] for report in reports]
    deterministic = len(set(digests)) == 1

    wall_ms = [report["timing"]["loop_wall_ms"] for report in reports]
    mean_ms = [report["timing"]["total_ms"]["mean"] for report in reports]
    p50_ms = [report["timing"]["total_ms"]["p50"] for report in reports]

    # The protocol section is *read out of the report*, never restated here.  A baseline
    # that hardcodes its own epsilon can silently disagree with the runs it summarises,
    # and then the disagreement is invisible in the artifact that is supposed to settle it.
    protocol = primary.get("protocol")
    if not protocol:
        print(
            "error: the report has no `protocol` section, so this baseline cannot state which "
            "epsilon it was built under; re-run with a report produced by the E8 harness"
        )
        return 2
    epsilon = protocol["repeat_epsilon"]

    def agrees(left: float, right: float) -> bool:
        # Mirrors `within_repeat_epsilon`: epsilon 0 means bit-identical, not "close".
        if epsilon == 0.0:
            return left == right and math.copysign(1.0, left) == math.copysign(1.0, right)
        return abs(left - right) <= epsilon

    run_means = [report["teds_struct_mean"] for report in reports if "teds_struct_mean" in report]
    if run_means:
        within_epsilon = all(agrees(value, run_means[0]) for value in run_means)
        epsilon_evidence = "per-run TEDS means compared with the report's own repeat_epsilon"
    else:
        # Scores come from `tools/table_teds.py` for one prediction file, so the runs do
        # not each carry a score.  What the reports *do* carry is the prediction
        # artifact's sha256, and byte-identical predictions imply an identical score --
        # a stronger statement than agreeing means.
        within_epsilon = deterministic
        epsilon_evidence = (
            "prediction artifacts compared byte for byte; byte-identical predictions imply an "
            "identical score, which is stronger than comparing means"
        )

    # Why the model's number is where it is: cell-count agreement separates "wrong
    # number of cells" from "right cells, wrong attributes".  `metric.py` charges
    # both, but only the first is a modelling failure worth chasing.
    cell_counts = None
    if args.predictions and args.gt_html:
        predicted = load(Path(args.predictions))
        truth = load(Path(args.gt_html))
        agreement = Counter()
        comparable = 0
        for name, source in predicted.items():
            if not source:
                continue
            comparable += 1
            agreement[source.count("<td") - truth[name]["html"].count("<td")] += 1
        exact = agreement[0]
        under = sum(count for delta, count in agreement.items() if delta < 0)
        over = sum(count for delta, count in agreement.items() if delta > 0)
        cell_counts = {
            "comparable_samples": comparable,
            "exact": exact,
            "exact_fraction": exact / comparable,
            "under_counted": under,
            "over_counted": over,
            "difference_histogram": {str(delta): count for delta, count in sorted(agreement.items())},
            "note": (
                "predicted cell count minus true cell count, counted as occurrences of `<td` in each "
                "html string. Under-counting dominates, i.e. the model merges or drops cells far more "
                "often than it invents them."
            ),
        }

    baseline = {
        "tool": "tools/build_table_teds_baseline.py",
        "generated_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "stage": args.stage
        or (
            "E7 (docs/09-table-recognition-execution.md section 8)"
            if metric_structure_only
            else "E9 (docs/09-table-recognition-execution.md section 10)"
        ),
        "metric": {
            "name": "TEDS-Struct" if metric_structure_only else "TEDS",
            "structure_only": metric_structure_only,
            "definition": "1 - APTED tree edit distance / max(node_count_pred, node_count_true)",
            "implementation": METRIC_SOURCE,
            "implementation_sha256": METRIC_SHA256,
            "not_reported": ["TEDS"] if metric_structure_only else [],
            "not_reported_reason": (
                "full TEDS compares cell text, and predicted cell text requires page-level OCR "
                "matched onto the predicted cell polygons (R1 / E9). No such mechanism exists yet, "
                "so the number would measure a provisional matching rule instead of the product."
            )
            if metric_structure_only
            else None,
            "comparability": NOT_COMPARABLE if metric_structure_only else NOT_COMPARABLE_FULL,
            "note": (
                "the official implementation is not bounded below by 0: when the predicted tree is "
                "much smaller than the true tree the edit distance can exceed the normaliser, so "
                "negative scores occur and are reported as produced."
            ),
        },
        "model": primary["model"],
        "config": primary["config"],
        "dataset": {
            "root": primary["dataset"]["root"],
            "manifest": primary["dataset"]["manifest"],
            "manifest_bytes": primary["dataset"]["manifest_sha256"][0],
            "manifest_sha256": primary["dataset"]["manifest_sha256"][1],
            "sample_set_sha256": primary["dataset"].get("sample_set_sha256"),
            "content_sha256": primary["dataset"].get("content_sha256"),
            "samples_loaded": primary["dataset"]["samples_loaded"],
            "selected": primary["dataset"]["selected"],
            "decodable": primary["dataset"]["decodable"],
            "failed": primary["dataset"]["failed"],
            "html_hashes_verified": primary["dataset"]["html_hashes_verified"],
            "selection": "hash-based (sha256 of relative_path + \\x1f + ground truth, ascending), limit 0 = all",
            "failure_scoring": primary["dataset"]["failure_scoring"],
        },
        "result": {
            "mean": teds["mean"],
            "samples": len(values),
            "mean_decodable_only": sum(decodable) / len(decodable),
            "decodable_samples": len(decodable),
            "stddev": statistics.pstdev(values),
            "min": min(values),
            "max": max(values),
            "exact_one": sum(1 for value in values if value >= 1.0 - 1e-9),
            "exact_zero": sum(1 for value in values if value == 0.0),
            "histogram": histogram(values),
        },
        "determinism": {
            "runs": len(reports),
            # A single run cannot establish byte-identity: one digest trivially forms a
            # singleton set.  Saying `true` there would be a vacuous claim dressed as
            # evidence, so it is reported as unknown instead.
            "identical_prediction_digests": deterministic if len(reports) >= 2 else None,
            "prediction_sha256": digests,
            "prediction_bytes": sizes,
            "teds_stddev_across_runs": 0.0 if (deterministic and len(reports) >= 2) else None,
            "method": (
                "same configuration, full val set, %d run(s); the prediction artifacts are compared "
                "byte for byte. Byte-identical predictions imply an identical TEDS, which is a stronger "
                "statement than agreement of the means and is what E8 needs when fixing epsilon."
                % len(reports)
            )
            if len(reports) >= 2
            else (
                "only one run was supplied, so byte-identity across runs was NOT established for "
                "this artifact; `identical_prediction_digests` is null on purpose rather than a "
                "vacuous true. See docs/10-table-evaluation-protocol.md section 1 for the "
                "determinism argument and the run count that established it."
            ),
            "timing_across_runs": {
                "loop_wall_ms": wall_ms,
                "total_ms_mean": mean_ms,
                "total_ms_p50": p50_ms,
            },
        },
        "protocol": {
            **protocol,
            "source": "read from the report's `protocol` section; never restated in this script",
            # With a single run there is nothing to agree *with*: `within_epsilon` is
            # computed over one mean and would be trivially true.  Report it as unknown.
            "runs_agree_within_epsilon": within_epsilon if len(reports) >= 2 else None,
            "epsilon_evidence": epsilon_evidence
            if len(reports) >= 2
            else (
                "only one run was supplied, so agreement across runs is unknown for this "
                "artifact. See docs/10-table-evaluation-protocol.md section 1."
            ),
        },
        "span_exceedance": primary["span_exceedance"],
        "failure_kinds": primary["failure_kinds"],
        "failures": primary["failures"],
        "timing": primary["timing"],
    }
    if not metric_structure_only:
        # A full-TEDS baseline reports TEDS, so "TEDS is not reported" is not a statement
        # this artifact gets to make.  Dropping the key is honest; `null` would read as
        # "the reason is unknown".
        baseline["metric"].pop("not_reported_reason", None)
        # Which route produced the cell text is part of the metric's definition here, and
        # `recognizer_calls` is the G2 evidence for the page-reuse route.  Copying them
        # from the report keeps the artifact self-describing instead of forcing a reader
        # to go find the run that made it.
        baseline["content_source"] = primary.get("content_source")
        baseline["backfill"] = primary.get("backfill")
        baseline["page_ocr"] = primary.get("page_ocr")
        # Which scale the page OCR engine ran at is part of the metric's definition for the
        # same reason `content_source` is: the same YAML is overwritten to a different
        # resolution by `TableOcrResolution::apply`, so the YAML path alone does not pin
        # the number.  `live_knob` inside it names the one field that actually took effect.
        baseline["ocr_resolution"] = primary.get("ocr_resolution")
        # How many samples the model could not express is part of the metric's definition
        # for the same reason: a low-confidence sample is still scored, so a reader who
        # sees only `result.mean` cannot tell a 0.58 made of well-formed tables from a
        # 0.58 that hides a degradation tail.  Carried verbatim so the artifact states
        # its own coverage instead of relying on prose in the docs.
        baseline["structure_confidence"] = primary.get("structure_confidence")
    if args.ablation:
        baseline["scale_reference"] = load(Path(args.ablation))
    if cell_counts is not None:
        baseline["cell_count_agreement"] = cell_counts

    # `--output` may be relative to the current directory (the documented usage) or
    # absolute; resolve first so the "wrote ..." line can always be printed relative to
    # the repo root instead of blowing up when the path is relative.
    output = Path(args.output)
    if not output.is_absolute():
        output = (Path.cwd() / output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(baseline, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    try:
        shown = output.relative_to(REPO_ROOT).as_posix()
    except ValueError:
        shown = output.as_posix()
    print(f"wrote {shown}")
    print(f"  {baseline['metric']['name']} mean {teds['mean']:.6f} over {len(values)} sample(s)")
    print(f"  decodable-only  {baseline['result']['mean_decodable_only']:.6f} over {len(decodable)}")
    print(f"  deterministic   {baseline['determinism']['identical_prediction_digests']} "
          f"({len(set(digests))} distinct prediction digest(s) over {len(reports)} run(s))")
    if cell_counts is not None:
        print(f"  cell count exact {cell_counts['exact']}/{cell_counts['comparable_samples']} "
              f"({cell_counts['exact_fraction']:.2%}), under {cell_counts['under_counted']}, over {cell_counts['over_counted']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
