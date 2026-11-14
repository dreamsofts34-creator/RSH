#!/usr/bin/env python3
"""Enforce the Phase B5 benchmark thresholds (SW-R8.1).

Reads `thresholds.json` (next to this script) and criterion's
`target/criterion/<bench>/new/estimates.json` files (written by
`cargo bench -p rsh-benchmarks`), and fails (exit 1) if any measured
mean exceeds its threshold.

Checks:
  * `pipeline_tick` mean ns  <= `pipeline_tick_ns_max`
  * `hot_path_parse_seq_pipeline` mean ns <= `hot_path_tick_ns_max`
  * `alloc_probe_pipeline` / `alloc_probe_hot_path` estimate files
    exist — the probes themselves panic the bench binary on any
    allocation, so their presence proves the 0 B/tick gate ran
    (`bytes_per_tick_max` is enforced in-process, not from estimates).

The soak threshold (`soak_growth_mb_per_10min_max`) is NOT checked
here: it is enforced by `cargo test -p rsh-benchmarks --test soak`
(the 60 s proxy) and by the Phase H 10-min rig soak.

Fail-closed: a missing estimates file or a missing thresholds file
exits non-zero — a missing measurement is never a pass.

Usage (from the repo root, after `cargo bench -p rsh-benchmarks`):
    python3 tools/rsh-benchmarks/check-thresholds.py
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent  # tools/rsh-benchmarks -> repo root

# thresholds.json key -> criterion bench name.
TIMING_CHECKS = {
    "pipeline_tick_ns_max": "pipeline_tick",
    "hot_path_tick_ns_max": "hot_path_parse_seq_pipeline",
}

# Bench names whose estimates must exist as proof the alloc probes ran.
ALLOC_PROBES = ("alloc_probe_pipeline", "alloc_probe_hot_path")


def load_thresholds(path: Path) -> dict:
    if not path.is_file():
        print(f"::error::thresholds file missing: {path} (Phase B must create it)")
        sys.exit(2)
    with path.open() as f:
        return json.load(f)


def bench_mean_ns(criterion_dir: Path, bench: str) -> float | None:
    estimates = criterion_dir / bench / "new" / "estimates.json"
    if not estimates.is_file():
        return None
    with estimates.open() as f:
        data = json.load(f)
    return float(data["mean"]["point_estimate"])


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Enforce the Phase B5 benchmark thresholds.")
    parser.add_argument(
        "--thresholds",
        type=Path,
        default=HERE / "thresholds.json",
        help="thresholds file (default: tools/rsh-benchmarks/thresholds.json)",
    )
    parser.add_argument(
        "--criterion-dir",
        type=Path,
        default=REPO_ROOT / "target" / "criterion",
        help="criterion output dir (default: <repo>/target/criterion)",
    )
    args = parser.parse_args(argv)
    thresholds = load_thresholds(args.thresholds)
    criterion_dir = args.criterion_dir
    failures: list[str] = []

    for key, bench in TIMING_CHECKS.items():
        limit = thresholds.get(key)
        if limit is None:
            failures.append(f"{key}: missing from thresholds.json")
            continue
        mean = bench_mean_ns(criterion_dir, bench)
        if mean is None:
            failures.append(
                f"{bench}: no criterion estimates at "
                f"{criterion_dir / bench / 'new' / 'estimates.json'} "
                "(run `cargo bench -p rsh-benchmarks` first)"
            )
            continue
        status = "PASS" if mean <= limit else "FAIL"
        print(f"[{status}] {bench}: mean {mean:.1f} ns <= {limit} ns ({key})")
        if mean > limit:
            failures.append(f"{bench}: mean {mean:.1f} ns exceeds {key}={limit} ns")

    for bench in ALLOC_PROBES:
        mean = bench_mean_ns(criterion_dir, bench)
        if mean is None:
            failures.append(
                f"{bench}: no criterion estimates — the alloc probe did not "
                "run (its in-process assert is the 0 B/tick gate)"
            )
        else:
            print(f"[PASS] {bench}: probe ran (mean {mean:.1f} ns; 0 B/tick asserted in-process)")

    print(
        f"[INFO] bytes_per_tick_max={thresholds.get('bytes_per_tick_max')} "
        "enforced by tests/alloc_gate.rs + the alloc probes, not here"
    )
    print(
        f"[INFO] soak_growth_mb_per_10min_max="
        f"{thresholds.get('soak_growth_mb_per_10min_max')} enforced by "
        "tests/soak.rs (60 s proxy) + the Phase H 10-min rig soak"
    )

    if failures:
        for failure in failures:
            print(f"::error::threshold gate failed: {failure}")
        return 1
    print("threshold gate: all checks PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
