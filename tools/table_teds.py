#!/usr/bin/env python3
"""Run the official PubTabNet TEDS metric (execution doc 09, stages E6/E7).

The metric is `src/metric.py` from `ibm-aur-nlp/PubTabNet`.  That repository ships
no ``LICENSE`` file, so this tool does **not** vendor a copy into the tree; it
loads one from ``refer/PubTabNet-metric/metric.py`` (the ``refer/`` tree is
untracked, exactly like ``refer/RapidTable-main``) and refuses to run unless the
file matches a pinned byte count and SHA-256.  TEDS numbers are only comparable
between runs when the implementation is identical, so the pin is not optional.

Original header of the loaded file::

    # Copyright 2020 IBM
    # Author: peter.zhong@au1.ibm.com
    #
    # ... Apache 2.0 License.

Two things need care:

* ``metric.py`` does ``from parallel import parallel_process`` at module scope.
  The PyPI ``parallel`` package cannot be installed under Python 3 (its source
  still uses Python 2 ``raise X, y`` syntax), so an in-memory shim that runs the
  work sequentially is injected instead.  ``batch_evaluate`` only reaches it when
  ``n_jobs > 1``; this tool always asserts ``n_jobs == 1``, and the shim warns if
  it is ever entered.
* ``TEDS.evaluate`` silently returns ``0.0`` when either HTML string is empty or
  lacks a ``body/table`` element.  ``--self-check`` asserts that condition away
  rather than letting a formatting bug masquerade as a bad score.

Usage::

    python tools/table_teds.py --fetch           # download into refer/, print the hash
    python tools/table_teds.py --self-check      # E6 acceptance: GT scores 1.0, a perturbation does not
    python tools/table_teds.py --true GT.json --pred PRED.json [--out scores.json]
    python tools/table_teds.py --true GT.json --pred PRED.json --structure-only

Exit codes: 0 success, 2 asset problem, 3 check failure.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
import types
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
METRIC_PATH = REPO_ROOT / "refer" / "PubTabNet-metric" / "metric.py"
METRIC_URL = "https://raw.githubusercontent.com/ibm-aur-nlp/PubTabNet/master/src/metric.py"

#: Pinned so a silent upstream edit cannot move the numbers.  Refreshing the pin
#: is a deliberate act: re-download, re-run `--self-check`, update both values.
METRIC_BYTES = 6181
METRIC_SHA256 = "a9aedd313678b42e55b06c4b0a365d658ef495aaf3abb7fd6012a6e9f334f9f9"

DERIVED = REPO_ROOT / "datasets" / "table" / "tabrecset" / "derived"


def sha256_file(path: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fetch(force: bool = False) -> int:
    if METRIC_PATH.is_file() and not force:
        print(f"already present: {METRIC_PATH.relative_to(REPO_ROOT).as_posix()}")
    else:
        import urllib.request

        METRIC_PATH.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(METRIC_URL, timeout=60) as response:
            payload = response.read()
        METRIC_PATH.write_bytes(payload)
        print(f"downloaded {METRIC_URL}")
    actual_bytes = METRIC_PATH.stat().st_size
    actual_sha = sha256_file(METRIC_PATH)
    print(f"  path   : {METRIC_PATH.relative_to(REPO_ROOT).as_posix()}")
    print(f"  bytes  : {actual_bytes} (pinned {METRIC_BYTES})")
    print(f"  sha256 : {actual_sha} (pinned {METRIC_SHA256})")
    if actual_bytes != METRIC_BYTES or actual_sha != METRIC_SHA256:
        print("!! the loaded metric does not match the pin; update METRIC_BYTES/METRIC_SHA256 deliberately")
        return 3
    return 0


def install_parallel_shim() -> None:
    """Satisfy ``from parallel import parallel_process`` without PyPI's package.

    The real package is Python-2-only.  ``metric.py`` imports it unconditionally
    but only calls it when ``n_jobs > 1``; the shim preserves the call contract
    (serial front, then the rest, order preserved) while running sequentially.
    """
    if "parallel" in sys.modules:
        return
    shim = types.ModuleType("parallel")
    shim.__doc__ = "sequential stand-in for the Python-2-only `parallel` package (see tools/table_teds.py)"

    def parallel_process(dict_list, function, n_jobs=4, use_kwargs=False, front_num=0, **kwargs):
        print(f"!! parallel_process shim entered with n_jobs={n_jobs}; running sequentially", file=sys.stderr)
        items = list(dict_list)
        results = []
        for item in items[:front_num]:
            results.append(function(**item) if use_kwargs else function(item))
        for item in items[front_num:]:
            results.append(function(**item) if use_kwargs else function(item))
        return results

    shim.parallel_process = parallel_process
    sys.modules["parallel"] = shim


def require_dependencies() -> str | None:
    """Return a remedy message when a hard dependency is missing."""
    missing = []
    for module in ("lxml", "tqdm", "distance", "apted"):
        try:
            importlib.import_module(module)
        except ImportError:
            missing.append(module)
    if missing:
        return f"missing Python dependency: {', '.join(missing)}; install with `pip install lxml tqdm apted distance`"
    return None


def silence_tqdm() -> None:
    """`metric.py` does `from tqdm import tqdm`, so patch the module before it loads.

    A 7,104-sample run otherwise prints one progress block per second to stderr,
    which buries the actual result.
    """
    import tqdm as tqdm_module

    tqdm_module.tqdm = lambda iterable=None, **kwargs: iterable if iterable is not None else []


def load_metric():
    """Import the pinned ``metric.py`` and return its module."""
    if not METRIC_PATH.is_file():
        raise FileNotFoundError(
            f"{METRIC_PATH.relative_to(REPO_ROOT).as_posix()} is absent; "
            "run `python tools/table_teds.py --fetch` first"
        )
    actual_bytes = METRIC_PATH.stat().st_size
    actual_sha = sha256_file(METRIC_PATH)
    if actual_bytes != METRIC_BYTES or actual_sha != METRIC_SHA256:
        raise ValueError(
            f"{METRIC_PATH.relative_to(REPO_ROOT).as_posix()} does not match the pin "
            f"(bytes {actual_bytes} vs {METRIC_BYTES}, sha256 {actual_sha} vs {METRIC_SHA256})"
        )
    remedy = require_dependencies()
    if remedy:
        raise ValueError(remedy)
    install_parallel_shim()
    silence_tqdm()
    spec = importlib.util.spec_from_file_location("pubtabnet_metric", METRIC_PATH)
    if spec is None or spec.loader is None:
        raise ValueError(f"cannot load {METRIC_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["pubtabnet_metric"] = module
    spec.loader.exec_module(module)
    return module


def score(pred_json: dict, true_json: dict, structure_only: bool = False) -> dict:
    """TEDS per file name, using the official implementation with ``n_jobs = 1``."""
    metric = load_metric()
    teds = metric.TEDS(structure_only=structure_only, n_jobs=1)
    return teds.batch_evaluate(pred_json, true_json)


def assert_parseable(true_json: dict) -> list[str]:
    """`evaluate` returns 0.0 -- not an error -- for HTML it cannot parse."""
    from lxml import html as lxml_html

    problems = []
    parser = lxml_html.HTMLParser(remove_comments=True, encoding="utf-8")
    for name, payload in true_json.items():
        source = payload["html"] if isinstance(payload, dict) else payload
        if not source:
            problems.append(f"{name}: empty html")
            continue
        try:
            tree = lxml_html.fromstring(source, parser=parser)
        except Exception as error:  # noqa: BLE001 - report, do not raise
            problems.append(f"{name}: lxml refused the html: {error}")
            continue
        tables = tree.xpath("body/table")
        if len(tables) != 1:
            problems.append(f"{name}: expected exactly one body/table, found {len(tables)}")
    return problems


def self_check(limit: int, structure_only: bool, perturb_samples: int) -> int:
    gt_path = DERIVED / "pubtabnet_val_gt_html.json"
    if not gt_path.is_file():
        print(f"no ground truth at {gt_path.relative_to(REPO_ROOT).as_posix()}; run tools/table_detokenize.py")
        return 2
    with gt_path.open(encoding="utf-8") as handle:
        all_true = json.load(handle)
    names = sorted(all_true)
    if limit:
        names = names[:limit]
    true_json = {name: all_true[name] for name in names}
    print(f"self-check over {len(true_json)} sample(s) of {gt_path.name}")

    problems = assert_parseable(true_json)
    if problems:
        print(f"!! {len(problems)} sample(s) are not parseable as a table:")
        for problem in problems[:10]:
            print(f"   {problem}")
        return 3
    print(f"  every sample parses to exactly one body/table: {len(true_json)}/{len(true_json)}")

    metric = load_metric()
    teds = metric.TEDS(structure_only=structure_only, n_jobs=1)

    identical = teds.batch_evaluate({name: true_json[name]["html"] for name in true_json}, true_json)
    bad_identity = [name for name, value in identical.items() if value != 1.0]
    print(f"  ground truth against itself scores exactly 1.0: {len(true_json) - len(bad_identity)}/{len(true_json)}")
    if bad_identity:
        print(f"!! not 1.0 for {bad_identity[:5]}")
        return 3

    # Two perturbations that must both move the score, one structural and one
    # textual.  A subset is enough: this proves the metric is sensitive, it is
    # not itself a measurement over the dataset.
    subset = {name: true_json[name] for name in sorted(true_json)[: perturb_samples or None]}
    print(f"  perturbation sensitivity over {len(subset)} sample(s)")
    structural = {name: subset[name]["html"].replace("</td>", "</td><td>X</td>", 1) for name in subset}
    textual = {name: subset[name]["html"].replace("</td>", "X</td>", 1) for name in subset}
    structural_scores = teds.batch_evaluate(structural, subset)
    textual_scores = teds.batch_evaluate(textual, subset)

    def describe(label, scores):
        moved = [value for value in scores.values() if 0.0 < value < 1.0]
        print(f"    {label}: {len(moved)}/{len(scores)} samples moved strictly inside (0, 1)")
        return len(moved)

    moved_structural = describe("extra cell added", structural_scores)
    moved_textual = describe("character appended to the first cell", textual_scores)
    if moved_structural != len(subset) or moved_textual != len(subset):
        print("!! a perturbation that should always be visible was not detected")
        return 3
    if subset:
        sample = sorted(subset)[0]
        print(f"    example {sample}: identity={identical[sample]:.6f} "
              f"structural={structural_scores[sample]:.6f} textual={textual_scores[sample]:.6f}")

    import apted
    import distance

    print(f"  metric.py  sha256={METRIC_SHA256} bytes={METRIC_BYTES}")
    print(f"  apted {getattr(apted, '__version__', 'unknown')}  distance {getattr(distance, '__version__', 'unknown')}")
    print(f"  structure_only={structure_only}  (structure_only crops cell text out of the tree)")
    return 0


def run_pair(args: argparse.Namespace) -> int:
    with Path(args.true).open(encoding="utf-8") as handle:
        true_json = json.load(handle)
    with Path(args.pred).open(encoding="utf-8") as handle:
        pred_json = json.load(handle)
    names = sorted(true_json)
    if args.restrict_to_predictions:
        # 子集跑（例如 `--limit 200`）时，真值里其余 6,904 个名字**不是**"模型答错"，
        # 而是"这次没跑"。官方 `batch_evaluate` 把缺键算 0 分，那会让子集分数变成
        # 0.016 这种毫无意义的数字。默认**不**开这个开关：全量跑时缺键就是真的缺
        # （失败样本被写成空串，键始终存在），那时"缺键算 0"正是我们要的语义。
        names = [name for name in names if name in pred_json]
    if args.limit:
        names = names[: args.limit]
    true_json = {name: true_json[name] for name in names}
    missing = [name for name in names if name not in pred_json]
    if missing:
        print(f"!! {len(missing)} sample(s) have no prediction, e.g. {missing[:3]}")
    scores = score(pred_json, true_json, structure_only=args.structure_only)
    ordered = [scores[name] for name in names]
    mean = sum(ordered) / len(ordered) if ordered else 0.0
    print(f"TEDS{' (structure only)' if args.structure_only else ''} over {len(ordered)} sample(s): mean={mean:.6f}")
    if args.out:
        Path(args.out).write_text(
            json.dumps(
                {
                    "mean": mean,
                    "samples": len(ordered),
                    "structure_only": bool(args.structure_only),
                    "restricted_to_predictions": bool(args.restrict_to_predictions),
                    "scores": scores,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        print(f"wrote {args.out}")
    return 0


def ablate(args: argparse.Namespace) -> int:
    """Score deliberate distortions of the ground truth, to give a number a scale.

    A TEDS value on its own is not interpretable: 0.71 could mean "almost right" or
    "barely better than a single empty cell".  These probes answer *what the metric
    charges for*, so the gap between the model and the truth can be attributed.

    The probes are ablations of the ground truth, not achievable predictors -- they
    are a scale, not a baseline.
    """
    import re

    with Path(args.true).open(encoding="utf-8") as handle:
        all_true = json.load(handle)
    names = sorted(all_true)
    if args.limit:
        names = names[: args.limit]
    true_json = {name: all_true[name] for name in names}
    print(f"ablation scale over {len(true_json)} sample(s)")

    def drop_sections(source: str) -> str:
        for tag in ("<thead>", "</thead>", "<tbody>", "</tbody>"):
            source = source.replace(tag, "")
        return source

    def drop_spans(source: str) -> str:
        return re.sub(r'\s(?:colspan|rowspan)="\d+"', "", source)

    probes = [
        ("identity (upper bound, must be 1.0)", lambda source: source),
        ("truth minus <thead>/<tbody>", drop_sections),
        ("truth minus every colspan/rowspan", drop_spans),
        ("trivial: one empty cell", lambda source: "<html><body><table><tr><td></td></tr></table></body></html>"),
        ("trivial: empty prediction", lambda source: ""),
    ]

    results = {}
    for label, transform in probes:
        predicted = {name: transform(true_json[name]["html"]) for name in names}
        scores = score(predicted, true_json, structure_only=args.structure_only)
        values = [scores[name] for name in names]
        mean = sum(values) / len(values)
        exact_one = sum(1 for value in values if value >= 1.0 - 1e-9)
        results[label] = {"mean": mean, "min": min(values), "max": max(values), "exact_one": exact_one}
        print(f"  {label:<38} mean={mean:.6f}  min={min(values):.4f}  exact1={exact_one}/{len(values)}")

    if results["identity (upper bound, must be 1.0)"]["mean"] < 1.0 - 1e-9:
        print("!! the identity probe is not 1.0, so this metric definition is not measuring what we think")
        return 3
    if args.out:
        Path(args.out).write_text(
            json.dumps({"samples": len(names), "structure_only": args.structure_only, "probes": results}, indent=2, sort_keys=True)
            + "\n",
            encoding="utf-8",
        )
        print(f"wrote {args.out}")
    return 0


def resolution(args: argparse.Namespace) -> int:
    """Report how small a change TEDS can express.

    TEDS is ``1 - edit_distance / max(nodes_pred, nodes_true)``, so adding or removing
    a single node moves a sample's score by exactly ``1 / max(...)``.  This matters
    because a *mean* over thousands of samples can hide a real structural change that
    only affects a handful of tables: if one table in 7,104 changes by one node, the
    dataset mean moves by well under 0.001 and looks like nothing happened.

    Two different things are reported and must not be conflated:

    * **resolution** -- the granularity of one sample's score (``1 / max(nodes)``).
    * **visibility** -- how far the dataset mean moves when one node changes in one
      table (``mean(resolution) / N``).  A change in the mean smaller than this is
      not evidence of anything.

    Both are properties of the *metric*, not of the model; they say nothing about
    whether a change is an improvement.
    """
    import lxml.html as lxml_html

    if not args.true:
        print("error: --resolution needs --true (the ground-truth html json)")
        return 2

    with Path(args.true).open(encoding="utf-8") as handle:
        all_true = json.load(handle)
    predicted = None
    if args.pred:
        with Path(args.pred).open(encoding="utf-8") as handle:
            predicted = json.load(handle)
    names = sorted(all_true)
    if args.limit:
        names = names[: args.limit]
    if not names:
        print("error: no samples to measure")
        return 2

    def node_count(source: str) -> int:
        if not source:
            return 0
        tree = lxml_html.fromstring(source, parser=parser)
        return len(tree.xpath(".//*"))

    parser = lxml_html.HTMLParser(remove_comments=True, encoding="utf-8")

    gt_nodes: list[int] = []
    resolutions: list[float] = []
    missing = 0
    for name in names:
        true_nodes = node_count(all_true[name]["html"])
        gt_nodes.append(true_nodes)
        if predicted is None:
            denominator = true_nodes
        else:
            if name not in predicted:
                missing += 1
            denominator = max(true_nodes, node_count(predicted.get(name, "")))
        resolutions.append(1.0 / denominator if denominator else 0.0)

    def describe(values: list[float]) -> dict:
        ordered = sorted(values)
        return {
            "min": ordered[0],
            "p50": ordered[(len(ordered) - 1) // 2],
            "p90": ordered[int((len(ordered) - 1) * 0.9)],
            "max": ordered[-1],
            "mean": sum(ordered) / len(ordered),
        }

    node_stats = describe([float(value) for value in gt_nodes])
    resolution_stats = describe(resolutions)
    visibility = resolution_stats["mean"] / len(names)

    print(f"resolution over {len(names)} sample(s)" + (" (paired with --pred)" if predicted else ""))
    print(
        "  ground-truth nodes   "
        f"min={node_stats['min']:.0f}  p50={node_stats['p50']:.0f}  "
        f"p90={node_stats['p90']:.0f}  max={node_stats['max']:.0f}  mean={node_stats['mean']:.2f}"
    )
    print(
        "  per-sample 1/nodes   "
        f"min={resolution_stats['min']:.6f}  p50={resolution_stats['p50']:.6f}  "
        f"p90={resolution_stats['p90']:.6f}  max={resolution_stats['max']:.6f}"
    )
    print(f"  one node in one table moves the dataset mean by {visibility:.8f}")
    print(
        "  -> a change in the mean smaller than that is not evidence of anything; "
        "compare per-sample scores instead"
    )
    if missing:
        print(f"  note: {missing} sample(s) had no prediction key and were scored against an empty tree")

    if args.out:
        Path(args.out).write_text(
            json.dumps(
                {
                    "samples": len(names),
                    "paired_with_predictions": predicted is not None,
                    "missing_predictions": missing,
                    "ground_truth_nodes": node_stats,
                    "per_sample_resolution": resolution_stats,
                    "dataset_mean_visibility": visibility,
                    "note": (
                        "resolution is a property of the metric; it does not say whether a "
                        "change is an improvement"
                    ),
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        print(f"wrote {args.out}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--fetch", action="store_true", help="download the pinned metric.py into refer/")
    parser.add_argument("--force", action="store_true", help="re-download even if the file exists")
    parser.add_argument("--self-check", action="store_true", help="pin, parseability and perturbation checks")
    parser.add_argument("--ablate", action="store_true", help="score distortions of the truth to scale a number")
    parser.add_argument("--resolution", action="store_true", help="report how small a change TEDS can express")
    parser.add_argument("--true", metavar="PATH", help="ground-truth json: {name: {'html': ...}}")
    parser.add_argument("--pred", metavar="PATH", help="prediction json: {name: html}")
    parser.add_argument("--out", metavar="PATH", help="write the per-sample scores here")
    parser.add_argument("--structure-only", action="store_true", help="TEDS-Struct (cell text ignored)")
    parser.add_argument("--limit", type=int, default=0, help="score only the first N sample names")
    parser.add_argument(
        "--restrict-to-predictions",
        action="store_true",
        help="score only the samples that have a prediction (for subset runs; off by default, "
        "because on a full run a missing prediction is a real zero)",
    )
    parser.add_argument(
        "--perturb-samples",
        type=int,
        default=250,
        help="how many samples the --self-check sensitivity perturbations cover",
    )
    args = parser.parse_args(argv)

    try:
        if args.fetch:
            return fetch(force=args.force)
        if args.self_check:
            return self_check(args.limit, args.structure_only, args.perturb_samples)
        if args.ablate:
            return ablate(args)
        if args.resolution:
            return resolution(args)
        if args.true and args.pred:
            return run_pair(args)
        parser.print_help()
        return 2
    except (FileNotFoundError, ValueError) as error:
        print(f"error: {error}")
        return 2


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
