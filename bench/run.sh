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
WASI_BIN="$RUST_DIR/target/wasm32-wasip2/release/gnata-bench.wasm"
JS_BIN="$BENCH_DIR/js_bench.js"

echo "=== Building Go benchmark CLI ==="
cd "$ROOT"
go build -o "$GO_BIN" "$BENCH_DIR/go_bench.go"

echo "=== Building Rust benchmark CLI (release) ==="
cd "$RUST_DIR"
cargo build --release --features bench-bin --bin gnata-bench 2>&1 | tail -1

echo "=== Building WASI benchmark CLI (release) ==="
cd "$RUST_DIR"
cargo build --release --target wasm32-wasip2 --features bench-bin --bin gnata-bench 2>&1 | tail -1

echo "=== Checking JS (jsonata-js) dependency ==="
cd "$BENCH_DIR"
if [[ ! -d node_modules/jsonata ]]; then
    npm install 2>&1 | tail -1
fi

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
    echo "#!/bin/sh" > "$BENCH_DIR/_wasi.sh"
    echo "exec wasmtime --dir \"$BENCH_DIR\"::/bench \"$WASI_BIN\" -- -expr '$EXPR' -datafile /bench/data.json -n $ITERS" >> "$BENCH_DIR/_wasi.sh"
    echo "#!/bin/sh" > "$BENCH_DIR/_js.sh"
    echo "exec node \"$JS_BIN\" -expr '$EXPR' -datafile \"$DATAFILE\" -n $ITERS" >> "$BENCH_DIR/_js.sh"
    chmod +x "$BENCH_DIR/_go.sh" "$BENCH_DIR/_rs.sh" "$BENCH_DIR/_wasi.sh" "$BENCH_DIR/_js.sh"

    hyperfine \
        --warmup 5 \
        --min-runs 20 \
        --export-json "$BENCH_DIR/result_${NAME}.json" \
        -n "go"   "$BENCH_DIR/_go.sh" \
        -n "rust"  "$BENCH_DIR/_rs.sh" \
        -n "wasi"  "$BENCH_DIR/_wasi.sh" \
        -n "js"    "$BENCH_DIR/_js.sh"
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
    go_s = rust_s = wasi_s = js_s = 0
    for r in data['results']:
        cmd = r['command']
        if cmd == 'go' or 'go_bench' in cmd:
            go_s = r['mean']
        elif cmd == 'wasi' or 'wasmtime' in cmd:
            wasi_s = r['mean']
        elif cmd == 'js' or 'js_bench' in cmd:
            js_s = r['mean']
        else:
            rust_s = r['mean']
    iters = $ITERS
    go_ns = (go_s * 1e9) / iters
    rust_ns = (rust_s * 1e9) / iters
    wasi_ns = (wasi_s * 1e9) / iters
    js_ns = (js_s * 1e9) / iters
    rs_go = rust_ns / go_ns if go_ns > 0 else 0
    wasi_go = wasi_ns / go_ns if go_ns > 0 else 0
    js_go = js_ns / go_ns if go_ns > 0 else 0
    results.append((name, exprs.get(name,''), go_ns, rust_ns, wasi_ns, js_ns, rs_go, wasi_go, js_go))

print()
print(f'{\"Benchmark\":<16} {\"Expression\":<35} {\"Go ns/op\":>10} {\"Rust ns/op\":>10} {\"WASI ns/op\":>10} {\"JS ns/op\":>10} {\"Rs/Go\":>7} {\"WASI/Go\":>8} {\"JS/Go\":>7}')
print('─' * 120)
for name, expr, go_ns, rust_ns, wasi_ns, js_ns, rs_go, wasi_go, js_go in results:
    print(f'{name:<16} {expr[:33]:<35} {go_ns:>10.0f} {rust_ns:>10.0f} {wasi_ns:>10.0f} {js_ns:>10.0f} {rs_go:>6.2f}x {wasi_go:>7.2f}x {js_go:>6.2f}x')
print()
print('Ratio < 1.0 = faster than Go. Ratio > 1.0 = slower than Go.')
"
