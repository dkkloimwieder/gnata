#!/usr/bin/env bash
# Cross-language JSONata benchmark using hyperfine.
# Compares Go and Rust on identical expressions and data.
#
# Usage: ./bench/run.sh [ITERS]
#   ITERS: inner loop iterations per hyperfine run (default: 10000)
set -euo pipefail

ITERS="${1:-10000}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$ROOT/bench"
RUST_DIR="$ROOT/crates/gnata-rs"
DATAFILE="$BENCH_DIR/data.json"
GO_BIN="$BENCH_DIR/go_bench"
RS_BIN="$RUST_DIR/target/release/gnata-bench"

echo "=== Building Go benchmark CLI ==="
cd "$ROOT"
go build -o "$GO_BIN" "$BENCH_DIR/go_bench.go"

echo "=== Building Rust benchmark CLI (release) ==="
cd "$RUST_DIR"
cargo build --release --bin gnata-bench 2>&1 | tail -1

echo ""
echo "=== Benchmark: $ITERS eval iterations per run ==="
echo "=== Using hyperfine --shell=none for accurate measurement ==="
echo ""

declare -A EXPRS
EXPRS[simple_path]="Account.Name"
EXPRS[nested_path]="Account.Order.Product.SKU"
EXPRS[filter_path]='Account.Order.Product[UnitPrice > 50].SKU'
EXPRS[aggregation]='$sum(Account.Order.Product.(UnitPrice * Quantity * (1 - Discount)))'
EXPRS[comparison]='Account.Name = "Firefly"'
EXPRS[func_exists]='$exists(Account.Order)'
EXPRS[func_count]='$count(Account.Order.Product.SKU)'
EXPRS[string_concat]='Account.Name & " Corp"'

ORDER=(simple_path nested_path filter_path aggregation comparison func_exists func_count string_concat)

for NAME in "${ORDER[@]}"; do
    EXPR="${EXPRS[$NAME]}"
    echo "── $NAME: $EXPR ──"
    # Write wrapper scripts to avoid quoting issues with hyperfine --shell=none
    echo "#!/bin/sh" > "$BENCH_DIR/_go.sh"
    echo "exec \"$GO_BIN\" -expr '$EXPR' -datafile \"$DATAFILE\" -n $ITERS" >> "$BENCH_DIR/_go.sh"
    echo "#!/bin/sh" > "$BENCH_DIR/_rs.sh"
    echo "exec \"$RS_BIN\" -expr '$EXPR' -datafile \"$DATAFILE\" -n $ITERS" >> "$BENCH_DIR/_rs.sh"
    chmod +x "$BENCH_DIR/_go.sh" "$BENCH_DIR/_rs.sh"

    hyperfine \
        --warmup 5 \
        --min-runs 20 \
        --export-json "$BENCH_DIR/result_${NAME}.json" \
        -n "go"   "$BENCH_DIR/_go.sh" \
        -n "rust"  "$BENCH_DIR/_rs.sh"
    echo ""
done

echo "=== Summary ==="
python3 -c "
import json, os

order = ['simple_path', 'nested_path', 'filter_path', 'aggregation',
         'comparison', 'func_exists', 'func_count', 'string_concat']
exprs = {
    'simple_path': 'Account.Name',
    'nested_path': 'Account.Order.Product.SKU',
    'filter_path': 'Account.Order.Product[UnitPrice > 50].SKU',
    'aggregation': '\$sum(Account.Order.Product.(UnitPrice * Quantity * (1 - Discount)))',
    'comparison': 'Account.Name = \"Firefly\"',
    'func_exists': '\$exists(Account.Order)',
    'func_count': '\$count(Account.Order.Product.SKU)',
    'string_concat': 'Account.Name & \" Corp\"',
}

results = []
for name in order:
    f = '$BENCH_DIR/result_{}.json'.format(name)
    if not os.path.exists(f):
        continue
    data = json.load(open(f))
    go_s = rust_s = 0
    for r in data['results']:
        if r['command'] == 'go' or 'go_bench' in r.get('command',''):
            go_s = r['mean']
        else:
            rust_s = r['mean']
    # Convert total time to per-iteration ns
    iters = $ITERS
    go_ns = (go_s * 1e9) / iters
    rust_ns = (rust_s * 1e9) / iters
    ratio = rust_ns / go_ns if go_ns > 0 else 0
    results.append((name, exprs.get(name,''), go_ns, rust_ns, ratio))

print()
print(f'{\"Benchmark\":<16} {\"Expression\":<50} {\"Go ns/op\":>10} {\"Rust ns/op\":>10} {\"Ratio\":>8}')
print('─' * 100)
for name, expr, go_ns, rust_ns, ratio in results:
    winner = '✓ Rs' if ratio < 1.0 else ''
    print(f'{name:<16} {expr[:48]:<50} {go_ns:>10.0f} {rust_ns:>10.0f} {ratio:>7.2f}x {winner}')
print()
print('Ratio < 1.0 = Rust faster. Ratio > 1.0 = Go faster.')
"
