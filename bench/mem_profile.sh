#!/usr/bin/env bash
# Memory profiling: Go vs Rust across payload sizes.
# Measures peak RSS via /usr/bin/time -v and Go runtime.MemStats.
# Runs each measurement ONCE (no parallelism, no hyperfine).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$ROOT/bench"
RUST_DIR="$ROOT/crates/gnata-rs"
GO_BIN="$BENCH_DIR/go_bench"
RS_BIN="$RUST_DIR/target/release/gnata-bench"
DHAT_BIN="$RUST_DIR/target/profiling/gnata-dhat"

echo "=== Building ==="
cd "$ROOT" && go build -o "$GO_BIN" "$BENCH_DIR/go_bench.go"
cd "$RUST_DIR" && cargo build --release --bin gnata-bench 2>&1 | tail -1
cargo build --features dhat-heap --bin gnata-dhat --profile profiling 2>&1 | tail -1

EXPR='Account.Order.Product[UnitPrice > 50].SKU'

declare -A DATA ITERS
DATA[tiny]="$BENCH_DIR/data.json"
DATA[1k]="$BENCH_DIR/data_1k.json"
DATA[10k]="$BENCH_DIR/data_10k.json"
DATA[100k]="$BENCH_DIR/data_100k.json"
ITERS[tiny]=10000
ITERS[1k]=100
ITERS[10k]=10
ITERS[100k]=1

SIZES=(tiny 1k 10k 100k)

echo ""
printf "%-8s %8s  %-12s %-12s  %-14s %-14s  %-12s %-12s\n" \
  "Size" "Bytes" "Go RSS(KB)" "Rust RSS(KB)" "Go HeapInUse" "Go TotalAlloc" "DHAT Total" "DHAT Blocks"
printf '%.0s─' {1..120}; echo ""

for SIZE in "${SIZES[@]}"; do
    DF="${DATA[$SIZE]}"
    N="${ITERS[$SIZE]}"
    FILE_BYTES=$(wc -c < "$DF" | tr -d ' ')

    # Go: peak RSS + memstats
    GO_OUT=$(/usr/bin/time -v "$GO_BIN" -expr "$EXPR" -datafile "$DF" -n "$N" -memstats 2>&1 >/dev/null)
    GO_RSS=$(echo "$GO_OUT" | grep "Maximum resident" | awk '{print $NF}')
    GO_HEAP=$(echo "$GO_OUT" | grep "MEMSTATS:" | sed 's/.*heap_inuse=\([0-9]*\).*/\1/')
    GO_TOTAL=$(echo "$GO_OUT" | grep "MEMSTATS:" | sed 's/.*total_alloc=\([0-9]*\).*/\1/')

    # Rust: peak RSS
    RS_OUT=$(/usr/bin/time -v "$RS_BIN" -expr "$EXPR" -datafile "$DF" -n "$N" 2>&1 >/dev/null)
    RS_RSS=$(echo "$RS_OUT" | grep "Maximum resident" | awk '{print $NF}')

    # Rust: DHAT allocation profile
    cd "$RUST_DIR"
    DHAT_OUT=$("$DHAT_BIN" eval -expr "$EXPR" -datafile "$DF" -n "$N" 2>&1)
    DHAT_TOTAL=$(echo "$DHAT_OUT" | grep "dhat: Total:" | sed 's/.*Total:\s*\([0-9,]*\) bytes.*/\1/' | tr -d ',')
    DHAT_BLOCKS=$(echo "$DHAT_OUT" | grep "dhat: Total:" | sed 's/.*in \([0-9,]*\) blocks/\1/' | tr -d ',')
    rm -f dhat-heap.json
    cd "$ROOT"

    printf "%-8s %8s  %-12s %-12s  %-14s %-14s  %-12s %-12s\n" \
      "$SIZE" "$FILE_BYTES" "$GO_RSS" "$RS_RSS" "$GO_HEAP" "$GO_TOTAL" "$DHAT_TOTAL" "$DHAT_BLOCKS"
done

echo ""
echo "RSS = peak resident set size in KB (/usr/bin/time -v)"
echo "Go HeapInUse/TotalAlloc = runtime.MemStats after GC (bytes)"
echo "DHAT Total/Blocks = all allocations during run (bytes/count)"
