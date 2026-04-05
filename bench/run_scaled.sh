#!/usr/bin/env bash
# Cross-language Go vs Rust benchmark at realistic payload sizes.
# Uses hyperfine for statistically rigorous measurement.
#
# Usage: ./bench/run_scaled.sh [SIZE]
#   SIZE: 1k (default), 10k, or both
set -euo pipefail

SIZE="${1:-both}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$ROOT/bench"
RUST_DIR="$ROOT/crates/gnata-rs"
GO_BIN="$BENCH_DIR/go_bench"
RS_BIN="$RUST_DIR/target/release/gnata-bench"

echo "=== Building Go benchmark CLI ==="
cd "$ROOT"
go build -o "$GO_BIN" "$BENCH_DIR/go_bench.go"

echo "=== Building Rust benchmark CLI (release) ==="
cd "$RUST_DIR"
cargo build --release --bin gnata-bench 2>&1 | tail -1

# Expressions to benchmark
declare -A EXPRS
EXPRS[nested_path]="Account.Order.Product.SKU"
EXPRS[filter_path]='Account.Order.Product[UnitPrice > 50].SKU'
EXPRS[aggregation]='$sum(Account.Order.Product.(UnitPrice * Quantity * (1 - Discount)))'
EXPRS[func_count]='$count(Account.Order.Product.SKU)'

ORDER=(nested_path filter_path aggregation func_count)

run_size() {
    local SIZE_NAME="$1"
    local DATAFILE="$2"
    local ITERS="$3"
    local FILE_SIZE
    FILE_SIZE=$(wc -c < "$DATAFILE" | tr -d ' ')

    echo ""
    echo "════════════════════════════════════════════════════════════"
    echo "  Payload: $SIZE_NAME ($FILE_SIZE bytes), $ITERS eval iters per run"
    echo "════════════════════════════════════════════════════════════"
    echo ""

    for NAME in "${ORDER[@]}"; do
        EXPR="${EXPRS[$NAME]}"
        echo "── $NAME: $EXPR ──"

        # Wrapper scripts avoid quoting issues with hyperfine
        cat > "$BENCH_DIR/_go.sh" << EOF
#!/bin/sh
exec "$GO_BIN" -expr '$EXPR' -datafile "$DATAFILE" -n $ITERS
EOF
        cat > "$BENCH_DIR/_rs.sh" << EOF
#!/bin/sh
exec "$RS_BIN" -expr '$EXPR' -datafile "$DATAFILE" -n $ITERS
EOF
        chmod +x "$BENCH_DIR/_go.sh" "$BENCH_DIR/_rs.sh"

        hyperfine \
            --warmup 5 \
            --min-runs 30 \
            --export-json "$BENCH_DIR/result_${SIZE_NAME}_${NAME}.json" \
            -n "go"   "$BENCH_DIR/_go.sh" \
            -n "rust"  "$BENCH_DIR/_rs.sh"
        echo ""
    done
}

print_summary() {
    local SIZE_NAME="$1"
    local ITERS="$2"

    python3 -c "
import json, os

order = ['nested_path', 'filter_path', 'aggregation', 'func_count']
exprs = {
    'nested_path': 'Account.Order.Product.SKU',
    'filter_path': 'Account.Order.Product[UnitPrice > 50].SKU',
    'aggregation': '\$sum(Account.Order.Product.(UnitPrice * Quantity...))',
    'func_count': '\$count(Account.Order.Product.SKU)',
}

results = []
for name in order:
    f = '$BENCH_DIR/result_${SIZE_NAME}_{}.json'.format(name)
    if not os.path.exists(f):
        continue
    data = json.load(open(f))
    go_s = rust_s = 0
    for r in data['results']:
        if r['command'] == 'go':
            go_s = r['mean']
        else:
            rust_s = r['mean']
    iters = $ITERS
    go_us = (go_s * 1e6) / iters
    rust_us = (rust_s * 1e6) / iters
    ratio = rust_us / go_us if go_us > 0 else 0
    results.append((name, exprs.get(name,''), go_us, rust_us, ratio))

print()
print(f'=== $SIZE_NAME payload ({$ITERS} iters) ===')
print()
print(f'{\"Benchmark\":<16} {\"Expression\":<48} {\"Go µs/op\":>10} {\"Rust µs/op\":>10} {\"Ratio\":>8}')
print('─' * 96)
for name, expr, go_us, rust_us, ratio in results:
    winner = '✓ Rs' if ratio < 1.0 else ('✓ Go' if ratio > 1.0 else '')
    if go_us > 1000:
        print(f'{name:<16} {expr[:46]:<48} {go_us/1000:>9.1f}ms {rust_us/1000:>9.1f}ms {ratio:>7.2f}x {winner}')
    else:
        print(f'{name:<16} {expr[:46]:<48} {go_us:>9.1f}µs {rust_us:>9.1f}µs {ratio:>7.2f}x {winner}')
print()
print('Ratio < 1.0 = Rust faster. Ratio > 1.0 = Go faster.')
"
}

if [[ "$SIZE" == "1k" || "$SIZE" == "both" ]]; then
    run_size "1k" "$BENCH_DIR/data_1k.json" 1000
    print_summary "1k" 1000
fi

if [[ "$SIZE" == "10k" || "$SIZE" == "both" ]]; then
    run_size "10k" "$BENCH_DIR/data_10k.json" 100
    print_summary "10k" 100
fi
