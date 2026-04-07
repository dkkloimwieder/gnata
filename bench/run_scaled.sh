#!/usr/bin/env bash
# Cross-language Go vs Rust benchmark at realistic payload sizes.
# Uses hyperfine for statistically rigorous measurement.
#
# Usage: ./bench/run_scaled.sh [VARIANT]
#   VARIANT: short (default), long, mixed, all
#
# Each variant runs 10k and 100k payloads.
set -euo pipefail

VARIANT="${1:-all}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$ROOT/bench"
RUST_DIR="$ROOT/crates/gnata-rs"
GO_BIN="$BENCH_DIR/go_bench"
RS_BIN="$RUST_DIR/target/release/gnata-bench"
JS_BIN="$BENCH_DIR/js_bench.js"

echo "=== Building Go benchmark CLI ==="
cd "$ROOT"
go build -o "$GO_BIN" "$BENCH_DIR/go_bench.go"

echo "=== Building Rust benchmark CLI (release) ==="
cd "$RUST_DIR"
cargo build --release --bin gnata-bench 2>&1 | tail -1

echo "=== Checking JS (jsonata-js) dependency ==="
cd "$BENCH_DIR"
if [[ ! -d node_modules/jsonata ]]; then
    npm install 2>&1 | tail -1
fi

# Short-key expressions (for short + mixed fixtures)
declare -A EXPRS_SHORT
EXPRS_SHORT[nested_path]="Account.Order.Product.SKU"
EXPRS_SHORT[filter_path]='Account.Order.Product[UnitPrice > 50].SKU'
EXPRS_SHORT[aggregation]='$sum(Account.Order.Product.(UnitPrice * Quantity * (1 - Discount)))'
EXPRS_SHORT[func_count]='$count(Account.Order.Product.SKU)'

# Long-key expressions (for long fixtures)
declare -A EXPRS_LONG
EXPRS_LONG[nested_path]="Account.Order.Product.StockKeepingUnitIdentifier"
EXPRS_LONG[filter_path]='Account.Order.Product[UnitPriceWithTaxIncluded > 50].StockKeepingUnitIdentifier'
EXPRS_LONG[aggregation]='$sum(Account.Order.Product.(UnitPriceWithTaxIncluded * QuantityInWarehouseStock * (1 - ApplicableDiscountPercentage)))'
EXPRS_LONG[func_count]='$count(Account.Order.Product.StockKeepingUnitIdentifier)'

ORDER=(nested_path filter_path aggregation func_count)

run_size() {
    local TAG="$1"       # e.g. "10k_short", "100k_long"
    local DATAFILE="$2"
    local ITERS="$3"
    local -n EXPR_MAP=$4  # nameref to expression associative array
    local FILE_SIZE
    FILE_SIZE=$(wc -c < "$DATAFILE" | tr -d ' ')

    echo ""
    echo "════════════════════════════════════════════════════════════"
    echo "  Payload: $TAG ($FILE_SIZE bytes), $ITERS eval iters per run"
    echo "════════════════════════════════════════════════════════════"
    echo ""

    for NAME in "${ORDER[@]}"; do
        EXPR="${EXPR_MAP[$NAME]}"
        echo "── $NAME: $EXPR ──"

        cat > "$BENCH_DIR/_go.sh" << EOF
#!/bin/sh
exec "$GO_BIN" -expr '$EXPR' -datafile "$DATAFILE" -n $ITERS
EOF
        cat > "$BENCH_DIR/_rs.sh" << EOF
#!/bin/sh
exec "$RS_BIN" -expr '$EXPR' -datafile "$DATAFILE" -n $ITERS
EOF
        cat > "$BENCH_DIR/_js.sh" << EOF
#!/bin/sh
exec node "$JS_BIN" -expr '$EXPR' -datafile "$DATAFILE" -n $ITERS
EOF
        chmod +x "$BENCH_DIR/_go.sh" "$BENCH_DIR/_rs.sh" "$BENCH_DIR/_js.sh"

        hyperfine \
            --warmup 5 \
            --min-runs 30 \
            --export-json "$BENCH_DIR/result_${TAG}_${NAME}.json" \
            -n "go"   "$BENCH_DIR/_go.sh" \
            -n "rust"  "$BENCH_DIR/_rs.sh" \
            -n "js"    "$BENCH_DIR/_js.sh"
        echo ""
    done
}

print_summary() {
    local TAG="$1"
    local ITERS="$2"

    python3 -c "
import json, os

order = ['nested_path', 'filter_path', 'aggregation', 'func_count']

results = []
for name in order:
    f = '$BENCH_DIR/result_${TAG}_{}.json'.format(name)
    if not os.path.exists(f):
        continue
    data = json.load(open(f))
    go_s = rust_s = js_s = 0
    for r in data['results']:
        cmd = r['command']
        if cmd == 'go':
            go_s = r['mean']
        elif cmd == 'js':
            js_s = r['mean']
        else:
            rust_s = r['mean']
    iters = $ITERS
    go_us = (go_s * 1e6) / iters
    rust_us = (rust_s * 1e6) / iters
    js_us = (js_s * 1e6) / iters
    rs_go = rust_us / go_us if go_us > 0 else 0
    js_go = js_us / go_us if go_us > 0 else 0
    results.append((name, go_us, rust_us, js_us, rs_go, js_go))

def fmt_time(us):
    if us > 1000:
        return f'{us/1000:>9.1f}ms'
    return f'{us:>9.1f}µs'

print()
print(f'=== $TAG payload ({$ITERS} iters) ===')
print()
print(f'{\"Benchmark\":<16} {\"Go\":>11} {\"Rust\":>11} {\"JS\":>11} {\"Rs/Go\":>7} {\"JS/Go\":>7}')
print('─' * 67)
for name, go_us, rust_us, js_us, rs_go, js_go in results:
    print(f'{name:<16} {fmt_time(go_us)} {fmt_time(rust_us)} {fmt_time(js_us)} {rs_go:>6.2f}x {js_go:>6.2f}x')
print()
print('Ratio < 1.0 = faster than Go. Ratio > 1.0 = slower than Go.')
"
}

run_variant() {
    local SUFFIX="$1"  # "short", "long", "mixed"

    if [[ "$SUFFIX" == "short" ]]; then
        local EXPR_REF="EXPRS_SHORT"
        local FILE_10K="$BENCH_DIR/data_10k.json"
        local FILE_100K="$BENCH_DIR/data_100k.json"
    elif [[ "$SUFFIX" == "long" ]]; then
        local EXPR_REF="EXPRS_LONG"
        local FILE_10K="$BENCH_DIR/data_10k_long.json"
        local FILE_100K="$BENCH_DIR/data_100k_long.json"
    elif [[ "$SUFFIX" == "mixed" ]]; then
        local EXPR_REF="EXPRS_SHORT"
        local FILE_10K="$BENCH_DIR/data_10k_mixed.json"
        local FILE_100K="$BENCH_DIR/data_100k_mixed.json"
    fi

    run_size "10k_${SUFFIX}" "$FILE_10K" 100 "$EXPR_REF"
    print_summary "10k_${SUFFIX}" 100

    run_size "100k_${SUFFIX}" "$FILE_100K" 10 "$EXPR_REF"
    print_summary "100k_${SUFFIX}" 10
}

if [[ "$VARIANT" == "short" || "$VARIANT" == "all" ]]; then
    run_variant "short"
fi
if [[ "$VARIANT" == "long" || "$VARIANT" == "all" ]]; then
    run_variant "long"
fi
if [[ "$VARIANT" == "mixed" || "$VARIANT" == "all" ]]; then
    run_variant "mixed"
fi
