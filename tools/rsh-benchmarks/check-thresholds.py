#!/usr/bin/env python3
"""Enforce the Phase B5 benchmark thresholds (SW-R8.1).

Reads `thresholds.json` (next to this script) and criterion's
`target/criterion/<bench>/new/estimates.json` files (written by
`cargo bench -p rsh-benchmarks`), and fails (exit 1) if any measured
mean exceeds its threshold.

Checks:
  * `pipeline_tick` mean ns  <= `pipeline_tick_ns_max`
  * `hot_path_parse_seq_pipeline` mean ns <= `hot_path_tick_ns_max`
  * `pin_validate_blob_factory` mean ns <=
    `pin_validate_blob_factory_ns_max` (H-6, 1.5x measured baseline)
  * `pin_validate_factory_modules` mean ns <=
    `pin_validate_factory_modules_ns_max` (H-6, 1.5x measured baseline)
  * `alloc_probe_pipeline` / `alloc_probe_hot_path` estimate files
    exist — the probes themselves panic the bench binary on any
    allocation, so their presence proves the 0 B/tick gate ran
    (`bytes_per_tick_max` is enforced in-process, not from estimates).
  * REGRESSION GATE (M-2/H-6): EVERY bench with a measured baseline
    (protocol benches in `baseline/runs-summary.json`,
    `mean_of_means_ns`; engine benches in `thresholds.json`
    `measured_baseline_ns`) fails when its current mean exceeds
    `regression_factor_max` (1.25x) of that baseline. The plan SLO
    caps (pipeline <= 1100 ns, pin <= 1.5x baseline) are the outer
    limits; the 1.25x regression gate is the early warning.
  * FRESHNESS (M-2): an `estimates.json` older than
    `--max-estimate-age-hours` (default 24 h) fails — a stale
    measurement is never a pass. Re-run `cargo bench` instead of
    reusing last week's numbers.
  * LOAD AVERAGE (M-2, R3-04): the run's `/proc/loadavg` is
    printed with the gate output, so a slow bench can be read
    against host load — and it is now USED: when the 1-min
    loadavg exceeds `--load-bound` (default 4.0) AND a threshold
    fails, the script exits 2 (LOAD-SUSPECT) instead of 1, so the
    CI bench-gate job retries the bench + check once instead of
    failing a good product for a busy runner. A pass under high
    load still exits 0 (passing while loaded is stronger, not
    weaker). The bound is absolute, not per-CPU: the measurement
    host is a 1.5-CPU shared sandbox and CI runners are the same
    loaded class — a 1-min loadavg over 4 means the host was
    heavily contended during the run.

The soak threshold (`soak_growth_mb_per_10min_max`) is NOT checked
here: it is enforced by `cargo test -p rsh-benchmarks --test soak`
(the 60 s proxy) and by the Phase H 10-min rig soak.

Fail-closed: a missing estimates file or a missing thresholds file
exits non-zero — a missing measurement is never a pass.

Exit codes: 0 = all checks pass; 1 = a threshold failed on a quiet
host (real regression — re-measure and explain, do not silently
raise); 2 = LOAD-SUSPECT, a threshold failed while the 1-min
loadavg exceeded `--load-bound` (retry once on a quieter runner).

Usage (from the repo root, after `cargo bench -p rsh-benchmarks`):
    python3 tools/rsh-benchmarks/check-thresholds.py
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent  # tools/rsh-benchmarks -> repo root

# thresholds.json key -> criterion bench name.
TIMING_CHECKS = {
    "pipeline_tick_ns_max": "pipeline_tick",
    "hot_path_tick_ns_max": "hot_path_parse_seq_pipeline",
    "pin_validate_blob_factory_ns_max": "pin_validate_blob_factory",
    "pin_validate_factory_modules_ns_max": "pin_validate_factory_modules",
}

# Bench names whose estimates must exist as proof the alloc probes ran.
ALLOC_PROBES = ("alloc_probe_pipeline", "alloc_probe_hot_path")

# Engine benches with baselines in thresholds.json measured_baseline_ns.
ENGINE_BASELINE_KEYS = (
    "pipeline_tick",
    "hot_path_parse_seq_pipeline",
    "sequence_watchdog_check",
)


def load_thresholds(path: Path) -> dict:
    if not path.is_file():
        print(f"::error::thresholds file missing: {path} (Phase B must create it)")
        sys.exit(2)
    with path.open() as f:
        return json.load(f)


def load_baseline(path: Path) -> dict[str, float]:
    """runs-summary.json bench name -> mean_of_means_ns."""
    if not path.is_file():
        return {}
    with path.open() as f:
        data = json.load(f)
    out: dict[str, float] = {}
    for bench, entry in data.items():
        if isinstance(entry, dict) and "mean_of_means_ns" in entry:
            out[bench] = float(entry["mean_of_means_ns"])
    return out


def estimates_path(criterion_dir: Path, bench: str) -> Path:
    return criterion_dir / bench / "new" / "estimates.json"


def bench_mean_ns(criterion_dir: Path, bench: str) -> float | None:
    estimates = estimates_path(criterion_dir, bench)
    if not estimates.is_file():
        return None
    with estimates.open() as f:
        data = json.load(f)
    return float(data["mean"]["point_estimate"])


def estimates_age_hours(criterion_dir: Path, bench: str) -> float | None:
    estimates = estimates_path(criterion_dir, bench)
    if not estimates.is_file():
        return None
    return (time.time() - estimates.stat().st_mtime) / 3600.0


def load_average() -> str:
    try:
        with Path("/proc/loadavg").open() as f:
            parts = f.read().split()
        return f"{parts[0]} / {parts[1]} / {parts[2]} (1/5/15 min)"
    except (OSError, IndexError):
        return "unavailable (non-Linux host)"


def load_1min() -> float | None:
    """1-min loadavg as a float, or None when it cannot be read."""
    try:
        with Path("/proc/loadavg").open() as f:
            return float(f.read().split()[0])
    except (OSError, IndexError, ValueError):
        return None


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
    parser.add_argument(
        "--baseline",
        type=Path,
        default=HERE / "baseline" / "runs-summary.json",
        help="measured-baseline file (default: tools/rsh-benchmarks/baseline/runs-summary.json)",
    )
    parser.add_argument(
        "--load-bound",
        type=float,
        default=4.0,
        help="1-min loadavg above which a threshold failure exits 2 "
        "(LOAD-SUSPECT, retry once) instead of 1 (default: 4.0)",
    )
    parser.add_argument(
        "--max-estimate-age-hours",
        type=float,
        default=24.0,
        help="fail when an estimates.json is older than this (default: 24)",
    )
    args = parser.parse_args(argv)
    thresholds = load_thresholds(args.thresholds)
    criterion_dir = args.criterion_dir
    failures: list[str] = []

    # --- Absolute threshold checks (plan SLO / 1.5x pin baselines). ---
    for key, bench in TIMING_CHECKS.items():
        limit = thresholds.get(key)
        if limit is None:
            failures.append(f"{key}: missing from thresholds.json")
            continue
        mean = bench_mean_ns(criterion_dir, bench)
        if mean is None:
            failures.append(
                f"{bench}: no criterion estimates at "
                f"{estimates_path(criterion_dir, bench)} "
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

    # --- Regression gate (M-2/H-6): every bench vs 1.25x its baseline. ---
    factor = float(thresholds.get("regression_factor_max", 1.25))
    baselines = load_baseline(args.baseline)
    measured = thresholds.get("measured_baseline_ns", {})
    for key in ENGINE_BASELINE_KEYS:
        if isinstance(measured.get(key), (int, float)):
            baselines.setdefault(key, float(measured[key]))
    if not baselines:
        print("[INFO] no measured baselines found — regression gate skipped")
    else:
        checked = 0
        for bench in sorted(baselines):
            mean = bench_mean_ns(criterion_dir, bench)
            if mean is None:
                # Benches the current run did not produce are covered
                # by the fail-closed checks above when they are core
                # benches; a missing optional bench is reported, not
                # silently treated as a pass.
                print(f"[INFO] {bench}: no estimates — regression check skipped (bench did not run)")
                continue
            checked += 1
            limit = baselines[bench] * factor
            status = "PASS" if mean <= limit else "FAIL"
            print(
                f"[{status}] regression {bench}: mean {mean:.1f} ns "
                f"<= {factor}x baseline {baselines[bench]:.1f} ns "
                f"(limit {limit:.1f} ns)"
            )
            if mean > limit:
                failures.append(
                    f"{bench}: mean {mean:.1f} ns exceeds {factor}x baseline "
                    f"{baselines[bench]:.1f} ns (limit {limit:.1f} ns) — "
                    "re-measure and explain, do not silently raise the baseline"
                )
        print(f"[INFO] regression gate checked {checked} benches against baselines")

    # --- Freshness (M-2): stale estimates fail. ---
    for bench in sorted(set(TIMING_CHECKS.values()) | set(baselines)):
        age = estimates_age_hours(criterion_dir, bench)
        if age is None:
            continue
        if age > args.max_estimate_age_hours:
            failures.append(
                f"{bench}: estimates.json is {age:.1f} h old "
                f"(max {args.max_estimate_age_hours} h) — re-run "
                "`cargo bench -p rsh-benchmarks`, do not reuse stale numbers"
            )
        else:
            print(f"[INFO] {bench}: estimates {age:.2f} h old (fresh)")

    load1 = load_1min()
    print(f"[INFO] load average at gate time: {load_average()}")
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
        # R3-04: a failure under high load is LOAD-SUSPECT, not a
        # proven regression — exit 2 so the CI bench-gate retries
        # once instead of failing the run for a busy host.
        if load1 is not None and load1 > args.load_bound:
            print(
                f"::warning::threshold gate failed with 1-min loadavg "
                f"{load1:.2f} > bound {args.load_bound} — LOAD-SUSPECT, "
                "retry once on a quieter runner (exit 2)"
            )
            return 2
        return 1
    print("threshold gate: all checks PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
