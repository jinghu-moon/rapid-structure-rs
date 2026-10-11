"""门禁 G5 判定：页面级表格路由的整页预算。

读 `tools/run_table_page_budget.ps1` 产出的交替配对报告，对每个配对算**对内差值**
（轮间漂移被消掉），再把"可归因到表格的那一部分"与预算比较。

# 判据为什么是"可归因分量之和"而不是 `ocr_total_ms.p50`

表格的开销是**双峰**的：不含表格的页面增量为 0，含表格的页面增量为整份表格成本。
`ocr_total_ms.p50` 取的是中位样本，而中位样本恰好落在两个峰的分界附近，因此它是最
不稳定的统计量 —— 实测 4 个配对的 Δp50 中位数是 +10.80 %，已经落进本仓库用来表示
"测不出变化"的 ±10–15 % 噪声带（docs/09 §0.5 的 G1 判据）。用它当门禁，等于用一个
与噪声同宽的尺子量一个比噪声还小的量。

可归因分量之和（`table_detect_ms + table_structure_infer_ms + table_backfill_ms`）
是**同一次请求内部**测出来的（`src/runtime/timing.rs` 的三个分量，E1 已把它们计入
`attributed_ms` / `inference_ms` / `rust_ms`），因此不受轮间漂移影响；而且它正好覆盖
两种真实的退化模式：
- 每个单元格各跑一次结构推理（而不是每张表一次）→ 分量随单元格数增长；
- 每张图开一次 `TableStructureSession`（而不是引擎构建期开一次）→ 分量含图优化成本。

`ocr_total_ms.p50` / `avg` 的增量仍然打印出来供人读，但**不参与判定**。

# 预算数值

`BUDGET_MS = 120.0`，推导（docs/11 §2.5）：
- 12 图集、`max_side_len=2000`、`intra_threads=16` 上实测中位 **56.5 ms**（4 配对，
  极差 2.5 ms）；
- 预算给到实测值的约 **2.1 倍**：既容得下单样本的分布（本机整页 p50 的轮间离散度
  约 3–9 %），又能在上面两种退化模式发生时失败 —— 它们会让这个数变成数百毫秒；
- 它是**第一版、可被后续实测修正**的阈值，不是从别处抄来的常数。

用法：
    python tools/check_table_page_budget.py                     # 默认 Temp/table-page-budget
    python tools/check_table_page_budget.py --dir Temp/e14-g5
    python tools/check_table_page_budget.py --budget-ms 150
退出码：0 = 通过；3 = 超预算或输入不合格。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import statistics
import sys

BUDGET_MS = 120.0
TABLE_COMPONENTS = ("table_detect_ms", "table_structure_infer_ms", "table_backfill_ms")


def load_pairs(directory: pathlib.Path) -> list[tuple[str, dict, dict]]:
    """按 `pairN-off.json` / `pairN-on.json` 成对读入，编号升序。"""
    pairs = []
    indices = sorted(
        int(path.name[len("pair") : -len("-off.json")])
        for path in directory.glob("pair*-off.json")
    )
    for index in indices:
        off_path = directory / f"pair{index}-off.json"
        on_path = directory / f"pair{index}-on.json"
        if not on_path.exists():
            raise SystemExit(f"pair{index}: missing {on_path.name}")
        pairs.append(
            (
                f"pair{index}",
                json.loads(off_path.read_text(encoding="utf-8")),
                json.loads(on_path.read_text(encoding="utf-8")),
            )
        )
    return pairs


def attributable_ms(report: dict) -> float:
    totals = report["timing_ledger"]["totals"]
    return sum(totals[key] for key in TABLE_COMPONENTS)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", default="Temp/table-page-budget", help="报告目录")
    parser.add_argument("--budget-ms", type=float, default=BUDGET_MS)
    args = parser.parse_args()

    directory = pathlib.Path(args.dir)
    if not directory.is_dir():
        raise SystemExit(f"not a directory: {directory}")
    pairs = load_pairs(directory)
    if not pairs:
        raise SystemExit(f"no pair*-off.json found under {directory}")

    problems = []
    rows = []
    for label, off, on in pairs:
        for name, report in ((f"{label}-off", off), (f"{label}-on", on)):
            profile = report["meta"]["benchmark"]["build_profile"]
            if profile != "release":
                problems.append(f"{name}: build_profile is {profile!r}, not 'release'")
            table = report["meta"].get("table")
            if table is None:
                problems.append(f"{name}: report has no meta.table (built before E14.7?)")
        if off["meta"].get("table", {}).get("enabled") is not False:
            problems.append(f"{label}: the 'off' run did not disable the table route")
        if on["meta"].get("table", {}).get("enabled") is not True:
            problems.append(f"{label}: the 'on' run did not enable the table route")
        off_components = [
            off["timing_ledger"]["totals"][key] for key in TABLE_COMPONENTS
        ]
        if any(value != 0.0 for value in off_components):
            problems.append(
                f"{label}: the table route is off but its ledger components are {off_components}"
            )
        off_p50 = off["stats"]["ocr_total_ms"]["p50"]
        on_p50 = on["stats"]["ocr_total_ms"]["p50"]
        off_avg = off["stats"]["ocr_total_ms"]["avg"]
        on_avg = on["stats"]["ocr_total_ms"]["avg"]
        rows.append(
            {
                "label": label,
                "ms": attributable_ms(on),
                "detect": on["timing_ledger"]["totals"]["table_detect_ms"],
                "infer": on["timing_ledger"]["totals"]["table_structure_infer_ms"],
                "backfill": on["timing_ledger"]["totals"]["table_backfill_ms"],
                "d_p50_pct": (on_p50 - off_p50) / off_p50 * 100.0,
                "d_avg_pct": (on_avg - off_avg) / off_avg * 100.0,
                "off_regions": off["stats"]["regions"]["avg"],
                "on_regions": on["stats"]["regions"]["avg"],
                "d_peak_mb": (
                    on["memory"]["peak_working_set_bytes"]
                    - off["memory"]["peak_working_set_bytes"]
                )
                / 1e6,
            }
        )

    print(f"directory: {directory}")
    print(
        f"  {'pair':8} {'attributable ms':>16} {'detect':>8} {'infer':>8} {'backfill':>9} "
        f"{'Δp50 %':>8} {'Δavg %':>8} {'Δregions':>9} {'Δpeak MB':>9}"
    )
    for row in rows:
        print(
            f"  {row['label']:8} {row['ms']:16.3f} {row['detect']:8.3f} {row['infer']:8.3f} "
            f"{row['backfill']:9.3f} {row['d_p50_pct']:+7.2f}% {row['d_avg_pct']:+7.2f}% "
            f"{row['on_regions'] - row['off_regions']:+9.4f} {row['d_peak_mb']:+8.1f}"
        )

    values = [row["ms"] for row in rows]
    median = statistics.median(values)
    print()
    print(
        f"attributable: n={len(values)} min={min(values):.3f} median={median:.3f} "
        f"max={max(values):.3f} spread={max(values) - min(values):.3f} ms"
    )
    print(
        f"end-to-end (informational, NOT the gate): "
        f"Δp50 % median={statistics.median([r['d_p50_pct'] for r in rows]):+.2f} "
        f"Δavg % median={statistics.median([r['d_avg_pct'] for r in rows]):+.2f}"
    )
    print(f"budget: {args.budget_ms:.1f} ms  (median/budget = {median / args.budget_ms:.2f})")

    if problems:
        print()
        for problem in problems:
            print(f"  problem: {problem}")
        print("G5 FAILED: the inputs are not a valid measurement")
        return 3
    if median > args.budget_ms:
        print(f"G5 FAILED: {median:.3f} ms > {args.budget_ms:.1f} ms")
        return 3
    print("G5 PASSED")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
