#!/usr/bin/env bash
# Full cross-language benchmark: Go vs Rust vs WASI vs JS
# at 1k, 10k, 100k payloads with short/long/mixed key variants.
# Produces JSON results per-test and a combined CSV.
#
# Usage: ./bench/run_full.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$ROOT/bench"
RUST_DIR="$ROOT/crates/gnata-rs"
GO_BIN="$BENCH_DIR/go_bench"
RS_BIN="$RUST_DIR/target/release/gnata-bench"
WASI_BIN="$RUST_DIR/target/wasm32-wasip2/release/gnata-bench.wasm"
JS_BIN="$BENCH_DIR/js_bench.js"
CSV="$BENCH_DIR/benchmark_results.csv"

echo "=== Building Go benchmark CLI ==="
cd "$ROOT"
go build -o "$GO_BIN" "$BENCH_DIR/go_bench.go"

echo "=== Building Rust benchmark CLI (release) ==="
cd "$RUST_DIR"
cargo build --release --bin gnata-bench 2>&1 | tail -1

echo "=== Building WASI benchmark CLI (release) ==="
cd "$RUST_DIR"
cargo build --release --target wasm32-wasip2 --bin gnata-bench 2>&1 | tail -1

echo "=== Checking JS (jsonata-js) dependency ==="
cd "$BENCH_DIR"
if [[ ! -d node_modules/jsonata ]]; then
    npm install 2>&1 | tail -1
fi

# ── Expression definitions ───────────────────────────────────────────────────

declare -A EXPRS_SHORT
EXPRS_SHORT[nested_path]="Account.Order.Product.SKU"
EXPRS_SHORT[filter_path]='Account.Order.Product[UnitPrice > 50].SKU'
EXPRS_SHORT[aggregation]='$sum(Account.Order.Product.(UnitPrice * Quantity * (1 - Discount)))'
EXPRS_SHORT[func_count]='$count(Account.Order.Product.SKU)'

declare -A EXPRS_LONG
EXPRS_LONG[nested_path]="Account.Order.Product.StockKeepingUnitIdentifier"
EXPRS_LONG[filter_path]='Account.Order.Product[UnitPriceWithTaxIncluded > 50].StockKeepingUnitIdentifier'
EXPRS_LONG[aggregation]='$sum(Account.Order.Product.(UnitPriceWithTaxIncluded * QuantityInWarehouseStock * (1 - ApplicableDiscountPercentage)))'
EXPRS_LONG[func_count]='$count(Account.Order.Product.StockKeepingUnitIdentifier)'

ORDER=(nested_path filter_path aggregation func_count)

# ── CSV header ───────────────────────────────────────────────────────────────

echo "payload,variant,expression,iters,go_mean_s,go_stddev_s,rust_mean_s,rust_stddev_s,wasi_mean_s,wasi_stddev_s,js_mean_s,js_stddev_s" > "$CSV"

# ── Runner ───────────────────────────────────────────────────────────────────

run_one() {
    local TAG="$1"       # e.g. "1k_short"
    local DATAFILE="$2"
    local ITERS="$3"
    local NAME="$4"      # e.g. "nested_path"
    local EXPR="$5"
    local FILE_SIZE
    FILE_SIZE=$(wc -c < "$DATAFILE" | tr -d ' ')

    echo "── $TAG / $NAME ($FILE_SIZE bytes, $ITERS iters) ──"

    cat > "$BENCH_DIR/_go.sh" << EOF
#!/bin/sh
exec "$GO_BIN" -expr '$EXPR' -datafile "$DATAFILE" -n $ITERS
EOF
    cat > "$BENCH_DIR/_rs.sh" << EOF
#!/bin/sh
exec "$RS_BIN" -expr '$EXPR' -datafile "$DATAFILE" -n $ITERS
EOF
    WASI_DATAFILE="/bench/$(basename "$DATAFILE")"
    cat > "$BENCH_DIR/_wasi.sh" << EOF
#!/bin/sh
exec wasmtime --dir "$BENCH_DIR"::/bench "$WASI_BIN" -- -expr '$EXPR' -datafile '$WASI_DATAFILE' -n $ITERS
EOF
    cat > "$BENCH_DIR/_js.sh" << EOF
#!/bin/sh
exec node "$JS_BIN" -expr '$EXPR' -datafile "$DATAFILE" -n $ITERS
EOF
    chmod +x "$BENCH_DIR/_go.sh" "$BENCH_DIR/_rs.sh" "$BENCH_DIR/_wasi.sh" "$BENCH_DIR/_js.sh"

    local RESULT_FILE="$BENCH_DIR/result_${TAG}_${NAME}.json"
    hyperfine \
        --warmup 3 \
        --min-runs 10 \
        --export-json "$RESULT_FILE" \
        -n "go"   "$BENCH_DIR/_go.sh" \
        -n "rust"  "$BENCH_DIR/_rs.sh" \
        -n "wasi"  "$BENCH_DIR/_wasi.sh" \
        -n "js"    "$BENCH_DIR/_js.sh"

    # Extract to CSV
    python3 -c "
import json
data = json.load(open('$RESULT_FILE'))
go = rust = wasi = js = {'mean': 0, 'stddev': 0}
for r in data['results']:
    if r['command'] == 'go': go = r
    elif r['command'] == 'rust': rust = r
    elif r['command'] == 'wasi': wasi = r
    elif r['command'] == 'js': js = r
# Parse TAG into payload and variant
tag = '$TAG'
parts = tag.split('_', 1)
payload = parts[0]
variant = parts[1] if len(parts) > 1 else 'short'
print(f'{payload},{variant},$NAME,$ITERS,{go[\"mean\"]:.6f},{go[\"stddev\"]:.6f},{rust[\"mean\"]:.6f},{rust[\"stddev\"]:.6f},{wasi[\"mean\"]:.6f},{wasi[\"stddev\"]:.6f},{js[\"mean\"]:.6f},{js[\"stddev\"]:.6f}')
" >> "$CSV"

    echo ""
}

run_variant() {
    local PAYLOAD="$1"   # "1k", "10k", "100k"
    local SUFFIX="$2"    # "short", "long", "mixed"
    local ITERS="$3"
    local DATAFILE="$4"
    local EXPR_REF="$5"  # "EXPRS_SHORT" or "EXPRS_LONG"

    local TAG="${PAYLOAD}_${SUFFIX}"
    local -n EXPR_MAP=$EXPR_REF

    for NAME in "${ORDER[@]}"; do
        run_one "$TAG" "$DATAFILE" "$ITERS" "$NAME" "${EXPR_MAP[$NAME]}"
    done
}

# ── Run all benchmarks ───────────────────────────────────────────────────────

echo ""
echo "╔══════════════════════════════════════════════════════════╗"
echo "║  Full Benchmark Suite: Go vs Rust vs WASI vs JS         ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

# 1k payloads (1000 iters — small data, need many iters for signal)
run_variant "1k" "short" 1000 "$BENCH_DIR/data_1k.json" "EXPRS_SHORT"
run_variant "1k" "long"  1000 "$BENCH_DIR/data_1k_long.json" "EXPRS_LONG"

# 10k payloads (100 iters)
run_variant "10k" "short" 100 "$BENCH_DIR/data_10k.json" "EXPRS_SHORT"
run_variant "10k" "long"  100 "$BENCH_DIR/data_10k_long.json" "EXPRS_LONG"
run_variant "10k" "mixed" 100 "$BENCH_DIR/data_10k_mixed.json" "EXPRS_SHORT"

# 100k payloads (10 iters)
run_variant "100k" "short" 10 "$BENCH_DIR/data_100k.json" "EXPRS_SHORT"
run_variant "100k" "long"  10 "$BENCH_DIR/data_100k_long.json" "EXPRS_LONG"
run_variant "100k" "mixed" 10 "$BENCH_DIR/data_100k_mixed.json" "EXPRS_SHORT"

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════════════════════════"
echo "  CSV written to: $CSV"
echo "════════════════════════════════════════════════════════════"
echo ""
cat "$CSV"

# Cleanup temp scripts
rm -f "$BENCH_DIR/_go.sh" "$BENCH_DIR/_rs.sh" "$BENCH_DIR/_wasi.sh" "$BENCH_DIR/_js.sh"
