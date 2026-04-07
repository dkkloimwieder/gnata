#!/usr/bin/env python3
"""Configurable benchmark runner for gnata JSONata engine.

Reads bench/catalog.toml, dispatches to hyperfine, collects and formats results.
Supports Go, Rust, and (future) JS runners.

Usage:
    python3 bench/run_bench.py --quick              # ~15 benchmarks, tiny data, ~3-5 min
    python3 bench/run_bench.py --section=path,hof   # specific categories
    python3 bench/run_bench.py --full               # all benchmarks, all sizes
    python3 bench/run_bench.py --dry-run             # print commands without executing
"""

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BENCH_DIR = ROOT / "bench"
CATALOG_PATH = BENCH_DIR / "catalog.toml"
FIXTURES_DIR = BENCH_DIR / "fixtures"
BASELINES_DIR = BENCH_DIR / "baselines"
RESULTS_DIR = BENCH_DIR / "results"

RUNNERS = {
    "go": {
        "build": f"go build -o {BENCH_DIR / 'go_bench'} {BENCH_DIR / 'go_bench.go'}",
        "binary": str(BENCH_DIR / "go_bench"),
    },
    "rust": {
        "build": f"cargo build --release --bin gnata-bench --manifest-path {ROOT / 'crates' / 'gnata-rs' / 'Cargo.toml'}",
        "binary": str(ROOT / "crates" / "gnata-rs" / "target" / "release" / "gnata-bench"),
    },
    "js": {
        "build": None,
        "binary": f"node {BENCH_DIR / 'js_bench.js'}",
    },
}


@dataclass
class BenchDef:
    id: str
    expr: str
    category: str
    fixture: str
    sizes: list[str]
    tags: list[str]
    iters: dict[str, int]


@dataclass
class BenchResult:
    id: str
    category: str
    expr: str
    size: str
    runner: str
    iters: int
    mean_s: float
    stddev_s: float
    median_s: float
    min_s: float
    max_s: float
    ns_per_op: float


def load_catalog() -> list[BenchDef]:
    with open(CATALOG_PATH, "rb") as f:
        raw = tomllib.load(f)

    defaults = raw.get("defaults", {})
    default_fixture = defaults.get("fixture", "account")
    default_sizes = defaults.get("sizes", ["tiny", "1k", "10k"])
    default_iters = defaults.get("iters", {"tiny": 10000, "1k": 1000, "10k": 100, "100k": 10, "full": 10})

    benchmarks = []
    for cat_name, cat in raw.get("categories", {}).items():
        cat_fixture = cat.get("fixture", default_fixture)
        cat_sizes = cat.get("sizes", default_sizes)
        cat_iters = {**default_iters, **cat.get("iters", {})}

        for b in cat.get("bench", []):
            benchmarks.append(BenchDef(
                id=b["id"],
                expr=b["expr"],
                category=cat_name,
                fixture=b.get("fixture", cat_fixture),
                sizes=b.get("sizes", cat_sizes),
                tags=b.get("tags", []),
                iters={**cat_iters, **b.get("iters", {})},
            ))

    return benchmarks


def select_benchmarks(benchmarks: list[BenchDef], args) -> list[BenchDef]:
    selected = benchmarks

    if args.section:
        sections = set(args.section.split(","))
        selected = [b for b in selected if b.category in sections]

    if args.id:
        ids = set(args.id.split(","))
        selected = [b for b in selected if b.id in ids]

    if args.tag:
        tags = set(args.tag.split(","))
        selected = [b for b in selected if set(b.tags) & tags]

    return selected


def resolve_sizes(bench: BenchDef, args) -> list[str]:
    if args.size:
        requested = args.size.split(",")
        matched = [s for s in requested if s in bench.sizes]
        if matched:
            return matched
        # No overlap: pick the smallest available size from the bench's list.
        # This ensures benchmarks with specialized fixtures (e.g., regex at 1k)
        # still run when --size=tiny is requested.
        size_order = ["tiny", "1k", "10k", "100k", "full"]
        for s in size_order:
            if s in bench.sizes:
                return [s]
        return bench.sizes[:1] if bench.sizes else []
    return bench.sizes


def resolve_fixture(fixture_name: str, size: str) -> Path:
    """Resolve fixture name + size to a file path."""
    fixture_dir = FIXTURES_DIR / fixture_name
    candidate = fixture_dir / f"{size}.json"
    if candidate.exists():
        return candidate

    # Fall back to largest available fixture <= requested size
    size_order = ["tiny", "1k", "10k", "100k", "full"]
    try:
        requested_idx = size_order.index(size)
    except ValueError:
        requested_idx = len(size_order) - 1

    for i in range(requested_idx, -1, -1):
        fallback = fixture_dir / f"{size_order[i]}.json"
        if fallback.exists():
            return fallback

    # Last resort: any json file in the directory
    jsons = sorted(fixture_dir.glob("*.json"))
    if jsons:
        return jsons[0]

    print(f"  WARNING: no fixture found for {fixture_name}/{size}, skipping", file=sys.stderr)
    return None


def resolve_iters(bench: BenchDef, size: str, args) -> int:
    if args.iters_override:
        return args.iters_override
    return bench.iters.get(size, 100)


def ensure_built(runners: list[str]):
    for runner in runners:
        info = RUNNERS.get(runner)
        if not info:
            print(f"  WARNING: unknown runner '{runner}', skipping", file=sys.stderr)
            continue
        if runner == "js":
            js_bin = BENCH_DIR / "js_bench.js"
            if not js_bin.exists():
                print(f"  NOTE: JS runner not yet available (see gnata-1g6), skipping 'js'", file=sys.stderr)
            continue
        if info["build"]:
            print(f"  Building {runner}...", file=sys.stderr)
            result = subprocess.run(info["build"], shell=True, cwd=str(ROOT),
                                    capture_output=True, text=True)
            if result.returncode != 0:
                print(f"  BUILD FAILED for {runner}:\n{result.stderr}", file=sys.stderr)
                sys.exit(1)


def get_binary(runner: str) -> str:
    return RUNNERS[runner]["binary"]


def write_wrapper(runner: str, expr: str, fixture: Path, iters: int, tmp_dir: str) -> str:
    """Write a shell wrapper script for hyperfine to invoke."""
    binary = get_binary(runner)
    # Use a heredoc-style approach to avoid shell quoting issues with expressions
    script_path = os.path.join(tmp_dir, f"_{runner}.sh")
    # Write expr to a temp file to completely avoid shell quoting issues
    expr_file = os.path.join(tmp_dir, f"_{runner}_expr.txt")
    with open(expr_file, "w") as f:
        f.write(expr)

    with open(script_path, "w") as f:
        f.write("#!/bin/sh\n")
        if runner == "js":
            f.write(f'exec {binary} -expr "$(cat \'{expr_file}\')" -datafile \'{fixture}\' -n {iters}\n')
        else:
            f.write(f'exec \'{binary}\' -expr "$(cat \'{expr_file}\')" -datafile \'{fixture}\' -n {iters}\n')
    os.chmod(script_path, 0o755)
    return script_path


def run_hyperfine(cmd_pairs: list[tuple[str, str]], result_file: str,
                  warmup: int, min_runs: int, dry_run: bool) -> bool:
    """Run hyperfine with the given runner name/script pairs."""
    cmd = ["hyperfine", "--warmup", str(warmup), "--min-runs", str(min_runs),
           "--export-json", result_file, "--shell=none"]
    for name, script in cmd_pairs:
        cmd.extend(["-n", name, script])

    if dry_run:
        print(f"  DRY-RUN: {' '.join(cmd)}")
        return True

    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"  HYPERFINE FAILED:\n{result.stderr}", file=sys.stderr)
        return False
    return True


def parse_hyperfine_results(result_file: str, bench: BenchDef, size: str,
                            iters: int) -> list[BenchResult]:
    """Parse hyperfine JSON output into BenchResult objects."""
    try:
        with open(result_file) as f:
            data = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return []

    results = []
    for r in data.get("results", []):
        runner = r.get("command", "unknown")
        mean_s = r.get("mean", 0)
        ns_per_op = (mean_s * 1e9) / iters if iters > 0 else 0
        results.append(BenchResult(
            id=bench.id,
            category=bench.category,
            expr=bench.expr,
            size=size,
            runner=runner,
            iters=iters,
            mean_s=mean_s,
            stddev_s=r.get("stddev", 0),
            median_s=r.get("median", 0),
            min_s=r.get("min", 0),
            max_s=r.get("max", 0),
            ns_per_op=ns_per_op,
        ))
    return results


def format_ns(ns: float) -> str:
    """Format nanoseconds with appropriate unit."""
    if ns >= 1e6:
        return f"{ns / 1e6:.1f}ms"
    elif ns >= 1e3:
        return f"{ns / 1e3:.1f}µs"
    else:
        return f"{ns:.0f}ns"


def output_table(results: list[BenchResult], runners: list[str]):
    """Print results as a human-readable table grouped by category."""
    if not results:
        print("No results to display.")
        return

    # Group by category
    categories = {}
    for r in results:
        categories.setdefault(r.category, []).append(r)

    # Group by (id, size) within each category for side-by-side comparison
    for cat_name in sorted(categories.keys()):
        cat_results = categories[cat_name]
        print(f"\n{'═' * 90}")
        print(f"  {cat_name.upper()}")
        print(f"{'═' * 90}")

        # Build lookup: (id, size) -> {runner: BenchResult}
        grouped = {}
        for r in cat_results:
            key = (r.id, r.size)
            grouped.setdefault(key, {})[r.runner] = r

        # Determine which runners are present
        present_runners = sorted({r.runner for r in cat_results})

        # Header
        runner_cols = "".join(f"{rn:>12}" for rn in present_runners)
        ratio_col = "   Ratio" if len(present_runners) == 2 else ""
        print(f"\n  {'ID':<28} {'Size':<6} {runner_cols}{ratio_col}")
        print(f"  {'─' * 28} {'─' * 6} {'─' * (12 * len(present_runners))}{('─' * 8) if ratio_col else ''}")

        for (bench_id, size) in sorted(grouped.keys()):
            runner_map = grouped[(bench_id, size)]
            short_id = bench_id.split(".", 1)[1] if "." in bench_id else bench_id

            vals = []
            for rn in present_runners:
                if rn in runner_map:
                    vals.append(format_ns(runner_map[rn].ns_per_op))
                else:
                    vals.append("—")

            val_str = "".join(f"{v:>12}" for v in vals)

            # Compute ratio if exactly 2 runners
            ratio_str = ""
            if len(present_runners) == 2:
                r0 = runner_map.get(present_runners[0])
                r1 = runner_map.get(present_runners[1])
                if r0 and r1 and r0.ns_per_op > 0:
                    ratio = r1.ns_per_op / r0.ns_per_op
                    winner = f"{'✓ ' + present_runners[1] if ratio < 1.0 else '✓ ' + present_runners[0] if ratio > 1.0 else ''}"
                    ratio_str = f" {ratio:>6.2f}x {winner}"

            print(f"  {short_id:<28} {size:<6} {val_str}{ratio_str}")

    print()


def output_json(results: list[BenchResult]):
    print(json.dumps([asdict(r) for r in results], indent=2))


def output_csv(results: list[BenchResult]):
    fields = ["id", "category", "size", "runner", "iters", "ns_per_op", "mean_s", "stddev_s"]
    print(",".join(fields))
    for r in results:
        d = asdict(r)
        print(",".join(str(d[f]) for f in fields))


def save_baseline(label: str, results: list[BenchResult], raw_files: list[str]):
    baseline_dir = BASELINES_DIR / label
    baseline_dir.mkdir(parents=True, exist_ok=True)

    # Manifest
    git_commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True,
                                text=True, cwd=str(ROOT)).stdout.strip()
    git_branch = subprocess.run(["git", "branch", "--show-current"], capture_output=True,
                                text=True, cwd=str(ROOT)).stdout.strip()
    manifest = {
        "label": label,
        "timestamp": datetime.now().isoformat(),
        "git_commit": git_commit,
        "git_branch": git_branch,
        "system": {
            "os": platform.system(),
            "arch": platform.machine(),
            "python": platform.python_version(),
        },
        "benchmark_count": len(results),
    }
    with open(baseline_dir / "manifest.json", "w") as f:
        json.dump(manifest, f, indent=2)

    # Results summary
    with open(baseline_dir / "results.json", "w") as f:
        json.dump([asdict(r) for r in results], f, indent=2)

    # Copy raw hyperfine files
    raw_dir = baseline_dir / "raw"
    raw_dir.mkdir(exist_ok=True)
    for rf in raw_files:
        if os.path.exists(rf):
            shutil.copy2(rf, raw_dir)

    print(f"\n  Baseline saved: {baseline_dir}", file=sys.stderr)


def compare_baseline(label: str, current: list[BenchResult], threshold: float) -> list[dict]:
    baseline_dir = BASELINES_DIR / label
    results_file = baseline_dir / "results.json"
    if not results_file.exists():
        print(f"  ERROR: baseline '{label}' not found at {baseline_dir}", file=sys.stderr)
        sys.exit(1)

    with open(results_file) as f:
        baseline_data = json.load(f)

    # Build lookup
    baseline_lookup = {}
    for b in baseline_data:
        key = (b["id"], b["size"], b["runner"])
        baseline_lookup[key] = b

    regressions = []
    print(f"\n{'═' * 90}")
    print(f"  Regression Report (vs baseline '{label}', threshold: {threshold}%)")
    print(f"{'═' * 90}")
    print(f"\n  {'ID':<28} {'Size':<6} {'Runner':<8} {'Baseline':>12} {'Current':>12} {'Δ%':>8}")
    print(f"  {'─' * 28} {'─' * 6} {'─' * 8} {'─' * 12} {'─' * 12} {'─' * 8}")

    for r in current:
        key = (r.id, r.size, r.runner)
        b = baseline_lookup.get(key)
        if not b:
            continue

        b_ns = b["ns_per_op"]
        c_ns = r.ns_per_op
        if b_ns <= 0:
            continue

        pct = ((c_ns - b_ns) / b_ns) * 100
        marker = " ⚠" if pct > threshold else ""
        print(f"  {r.id:<28} {r.size:<6} {r.runner:<8} {format_ns(b_ns):>12} {format_ns(c_ns):>12} {pct:>+7.1f}%{marker}")

        if pct > threshold:
            regressions.append({
                "id": r.id, "size": r.size, "runner": r.runner,
                "baseline_ns": b_ns, "current_ns": c_ns, "pct_change": pct,
            })

    if regressions:
        print(f"\n  ⚠ {len(regressions)} regression(s) exceeded {threshold}% threshold")
    else:
        print(f"\n  ✓ No regressions detected")
    print()

    return regressions


def parse_args():
    p = argparse.ArgumentParser(
        description="Configurable benchmark runner for gnata JSONata engine.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Presets:
  --quick     tag=quick, size=tiny, warmup=3, min-runs=10  (~3-5 min)
  --full      all categories, all sizes, warmup=5, min-runs=20  (~60-90 min)
  (default)   all categories, size=tiny, warmup=5, min-runs=20  (~15-20 min)

Examples:
  %(prog)s --quick                          # fast smoke test
  %(prog)s --section=path,hof --size=1k     # paths + HOFs at 1k scale
  %(prog)s --id=hof.map,hof.filter          # specific benchmarks
  %(prog)s --quick --save-baseline v0.1     # save baseline
  %(prog)s --quick --compare-baseline v0.1  # compare against baseline
""")

    # Selection
    sel = p.add_argument_group("selection")
    sel.add_argument("--section", help="Categories to run (comma-separated)")
    sel.add_argument("--id", help="Specific benchmark IDs (comma-separated)")
    sel.add_argument("--tag", help="Run benchmarks with this tag")
    sel.add_argument("--quick", action="store_true", help="Quick preset: tag=quick, size=tiny")
    sel.add_argument("--full", action="store_true", help="Full preset: all categories, all sizes")

    # Data
    data = p.add_argument_group("data")
    data.add_argument("--size", help="Data sizes to test (comma-separated)")

    # Runners
    run = p.add_argument_group("runners")
    run.add_argument("--runner", default="go,rust", help="Implementations to compare (default: go,rust)")

    # Output
    out = p.add_argument_group("output")
    out.add_argument("--output", choices=["table", "json", "csv"], default="table",
                     help="Output format (default: table)")

    # Baselines
    bl = p.add_argument_group("baselines")
    bl.add_argument("--save-baseline", metavar="LABEL", help="Save results as named baseline")
    bl.add_argument("--compare-baseline", metavar="LABEL", help="Compare against saved baseline")
    bl.add_argument("--threshold", type=float, default=5.0, help="Regression threshold %% (default: 5)")

    # Tuning
    tune = p.add_argument_group("tuning")
    tune.add_argument("--warmup", type=int, default=5, help="Hyperfine warmup runs (default: 5)")
    tune.add_argument("--min-runs", type=int, default=20, help="Hyperfine minimum runs (default: 20)")
    tune.add_argument("--iters-override", type=int, default=0, help="Override all iteration counts")
    tune.add_argument("--dry-run", action="store_true", help="Print commands without executing")

    args = p.parse_args()

    # Apply presets
    if args.quick:
        if not args.tag:
            args.tag = "quick"
        if not args.size:
            args.size = "tiny"
        args.warmup = min(args.warmup, 3)
        args.min_runs = min(args.min_runs, 10)

    if args.full:
        if not args.size:
            args.size = "tiny,1k,10k,100k"
        args.warmup = max(args.warmup, 5)
        args.min_runs = max(args.min_runs, 20)

    # Default size if nothing specified
    if not args.size:
        args.size = "tiny"

    return args


def main():
    args = parse_args()
    catalog = load_catalog()
    benchmarks = select_benchmarks(catalog, args)

    if not benchmarks:
        print("No benchmarks matched the selection criteria.", file=sys.stderr)
        sys.exit(1)

    runners = [r.strip() for r in args.runner.split(",")]
    # Filter out js if not available
    if "js" in runners and not (BENCH_DIR / "js_bench.js").exists():
        print("  NOTE: JS runner not yet available (see gnata-1g6), skipping 'js'", file=sys.stderr)
        runners = [r for r in runners if r != "js"]

    total_runs = sum(len(resolve_sizes(b, args)) for b in benchmarks)
    print(f"\n  Benchmark suite: {len(benchmarks)} expressions, {total_runs} runs", file=sys.stderr)
    print(f"  Runners: {', '.join(runners)}", file=sys.stderr)
    print(f"  Sizes: {args.size}", file=sys.stderr)
    if args.dry_run:
        print(f"  Mode: DRY RUN\n", file=sys.stderr)
    else:
        print(file=sys.stderr)

    if not args.dry_run:
        ensure_built(runners)

    all_results: list[BenchResult] = []
    raw_files: list[str] = []

    with tempfile.TemporaryDirectory(prefix="gnata_bench_") as tmp_dir:
        for i, bench in enumerate(benchmarks):
            sizes = resolve_sizes(bench, args)
            if not sizes:
                continue

            for size in sizes:
                fixture = resolve_fixture(bench.fixture, size)
                if fixture is None:
                    continue

                iters = resolve_iters(bench, size, args)
                print(f"  [{i+1}/{len(benchmarks)}] {bench.id} ({size}, {iters} iters)", file=sys.stderr)

                # Build hyperfine commands
                cmd_pairs = []
                for runner in runners:
                    script = write_wrapper(runner, bench.expr, fixture, iters, tmp_dir)
                    cmd_pairs.append((runner, script))

                result_file = os.path.join(tmp_dir, f"{bench.id}_{size}.json")
                ok = run_hyperfine(cmd_pairs, result_file, args.warmup, args.min_runs, args.dry_run)

                if ok and not args.dry_run:
                    results = parse_hyperfine_results(result_file, bench, size, iters)
                    all_results.extend(results)
                    raw_files.append(result_file)

        # Copy raw files to results dir if saving baseline
        if args.save_baseline and not args.dry_run:
            save_baseline(args.save_baseline, all_results, raw_files)

    # Output
    if not args.dry_run:
        if args.output == "table":
            output_table(all_results, runners)
        elif args.output == "json":
            output_json(all_results)
        elif args.output == "csv":
            output_csv(all_results)

    # Baseline comparison
    if args.compare_baseline and not args.dry_run:
        regressions = compare_baseline(args.compare_baseline, all_results, args.threshold)
        if regressions:
            sys.exit(1)


if __name__ == "__main__":
    main()
