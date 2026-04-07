#!/usr/bin/env bash
# Configurable benchmark runner for gnata JSONata engine.
# Drives hyperfine to compare Go vs Rust (and future JS) across 102 expressions.
#
# Usage:
#   ./bench/run_bench.sh                        # all categories, tiny data
#   ./bench/run_bench.sh --quick                # ~17 tagged benchmarks, fast
#   ./bench/run_bench.sh --full                 # all categories, all sizes
#   ./bench/run_bench.sh --section path,hof     # specific categories
#   ./bench/run_bench.sh --id path.simple       # specific benchmark
#   ./bench/run_bench.sh --size 1k,10k          # specific data sizes
#   ./bench/run_bench.sh --dry-run              # print what would run
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$ROOT/bench"
RUST_DIR="$ROOT/crates/gnata-rs"
FIXTURES_DIR="$BENCH_DIR/fixtures"
GO_BIN="$BENCH_DIR/go_bench"
RS_BIN="$RUST_DIR/target/release/gnata-bench"

# ── Defaults ─────────────────────────────────────────────────────────────────
SECTIONS=""
BENCH_ID=""
TAG=""
SIZES="tiny"
ITERS=10
WARMUP=5
MIN_RUNS=20
DRY_RUN=false
RUNNERS="go rust js"

# ── Parse args ───────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --section)  SECTIONS="$2"; shift 2 ;;
        --id)       BENCH_ID="$2"; shift 2 ;;
        --tag)      TAG="$2"; shift 2 ;;
        --size)     SIZES="$2"; shift 2 ;;
        --warmup)   WARMUP="$2"; shift 2 ;;
        --min-runs) MIN_RUNS="$2"; shift 2 ;;
        --iters)    ITERS="$2"; shift 2 ;;
        --runner)   RUNNERS="${2//,/ }"; shift 2 ;;
        --dry-run)  DRY_RUN=true; shift ;;
        --quick)    TAG="quick"; SIZES="tiny"; WARMUP=3; MIN_RUNS=10; shift ;;
        --full)     SIZES="tiny,1k,10k,100k"; WARMUP=5; MIN_RUNS=20; shift ;;
        *)          echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ── Fixture resolution ───────────────────────────────────────────────────────
# Returns the fixture file path for a given fixture type and size.
# Falls back to smallest available if requested size doesn't exist.
resolve_fixture() {
    local fixture_type="$1" size="$2"
    local candidate="$FIXTURES_DIR/$fixture_type/$size.json"
    if [[ -e "$candidate" ]]; then
        echo "$candidate"
        return
    fi
    # Fallback: smallest available
    for fallback in tiny 1k 10k 100k full; do
        local f="$FIXTURES_DIR/$fixture_type/$fallback.json"
        if [[ -e "$f" ]]; then
            echo "$f"
            return
        fi
    done
    echo ""
}

# ── Benchmark catalog ────────────────────────────────────────────────────────
# Each benchmark: "id|category|fixture|sizes|tags|expression"
# sizes and tags are semicolon-separated within their field.
BENCHMARKS=()

bench() {
    local id="$1" cat="$2" fixture="$3" sizes="$4" tags="$5" expr="$6"
    BENCHMARKS+=("$id|$cat|$fixture|$sizes|$tags|$expr")
}

# ── PATH ─────────────────────────────────────────────────────────────────────
bench "path.simple"      path account "tiny;1k;10k;100k" "quick" 'Account.Name'
bench "path.nested"      path account "tiny;1k;10k;100k" "quick" 'Account.Order.Product.SKU'
bench "path.wildcard"    path account "tiny;1k;10k"      ""      'Account.Order.*'
bench "path.descendant"  path account "tiny;1k;10k"      ""      '**[SKU]'
bench "path.parent"      path account "tiny;1k"          ""      'Account.Order.Product.%.OrderID'
bench "path.filter"      path account "tiny;1k;10k;100k" "quick" 'Account.Order.Product[UnitPrice > 50].SKU'
bench "path.index"       path account "tiny;1k;10k"      ""      'Account.Order[0].Product[0].SKU'
bench "path.keep_array"  path account "tiny;1k;10k"      ""      'Account.Order[].Product.SKU'

# ── STRING ───────────────────────────────────────────────────────────────────
bench "string.substring" string strings "1k" "quick" '$substring(items[0].text, 0, 10)'
bench "string.replace"   string strings "1k" ""      '$replace(items[0].text, '\''error'\'', '\''warning'\'')'
bench "string.split"     string strings "1k" ""      '$split(items[0].text, '\'' '\'')'
bench "string.join"      string strings "1k" ""      '$join($split(items[0].text, '\'' '\''), '\''-'\'')'
bench "string.trim"      string strings "1k" ""      '$trim(items[0].text)'
bench "string.pad"       string strings "1k" ""      '$pad(items[0].code, 10, '\''#'\'')'
bench "string.match"     string strings "1k" ""      '$match(items[0].text, /[A-Z][a-z]+/)'
bench "string.contains"  string strings "1k" ""      '$contains(items[0].text, '\''critical'\'')'
bench "string.lowercase" string strings "1k" ""      '$lowercase(items[0].text)'
bench "string.uppercase" string strings "1k" ""      '$uppercase(items[0].text)'
bench "string.length"    string strings "1k" ""      '$length(items[0].text)'
bench "string.before"    string strings "1k" ""      '$substringBefore(items[0].text, '\'' '\'')'
bench "string.after"     string strings "1k" ""      '$substringAfter(items[0].text, '\'' '\'')'

# ── NUMERIC ──────────────────────────────────────────────────────────────────
bench "numeric.abs"          numeric account "tiny;1k;10k" ""      'Account.Order.Product.($abs(UnitPrice - 50))'
bench "numeric.floor"        numeric account "tiny;1k;10k" ""      'Account.Order.Product.($floor(UnitPrice))'
bench "numeric.ceil"         numeric account "tiny;1k;10k" ""      'Account.Order.Product.($ceil(UnitPrice))'
bench "numeric.round"        numeric account "tiny;1k;10k" ""      'Account.Order.Product.($round(UnitPrice, 1))'
bench "numeric.power"        numeric account "tiny;1k;10k" ""      'Account.Order.Product.($power(Quantity, 2))'
bench "numeric.sqrt"         numeric account "tiny;1k;10k" ""      'Account.Order.Product.($sqrt(UnitPrice))'
bench "numeric.sum"          numeric account "tiny;1k;10k" "quick" '$sum(Account.Order.Product.(UnitPrice * Quantity))'
bench "numeric.max"          numeric account "tiny;1k;10k" ""      '$max(Account.Order.Product.UnitPrice)'
bench "numeric.min"          numeric account "tiny;1k;10k" ""      '$min(Account.Order.Product.UnitPrice)'
bench "numeric.average"      numeric account "tiny;1k;10k" ""      '$average(Account.Order.Product.UnitPrice)'
bench "numeric.formatNumber" numeric account "tiny;1k;10k" ""      'Account.Order.Product.($formatNumber(UnitPrice, '\''#,##0.00'\''))'
bench "numeric.formatBase"   numeric account "tiny;1k;10k" ""      '$formatBase(255, 16)'

# ── ARRAY ────────────────────────────────────────────────────────────────────
bench "array.append"   array account "tiny;1k;10k" ""      '$append(Account.Order[0].Product, Account.Order[1].Product)'
bench "array.sort"     array account "tiny;1k;10k" "quick" '$sort(Account.Order.Product, function($a,$b){$a.UnitPrice > $b.UnitPrice})'
bench "array.reverse"  array account "tiny;1k;10k" ""      '$reverse(Account.Order.Product)'
bench "array.distinct" array account "tiny;1k;10k" ""      '$distinct(Account.Order.Product.SKU)'
bench "array.flatten"  array account "tiny;1k;10k" ""      '$flatten([Account.Order.Product])'
bench "array.zip"      array account "tiny;1k;10k" ""      '$zip(Account.Order.Product.SKU, Account.Order.Product.UnitPrice)'
bench "array.count"    array account "tiny;1k;10k" ""      '$count(Account.Order.Product)'
bench "array.shuffle"  array account "tiny;1k;10k" ""      '$shuffle(Account.Order.Product)'

# ── OBJECT ───────────────────────────────────────────────────────────────────
bench "object.keys"   object account "tiny;1k;10k" "" '$keys(Account.Order[0].Product[0])'
bench "object.values" object account "tiny;1k;10k" "" '$values(Account.Order[0].Product[0])'
bench "object.spread" object account "tiny;1k;10k" "" '$spread(Account.Order[0].Product[0])'
bench "object.merge"  object account "tiny;1k;10k" "" '$merge(Account.Order.Product)'
bench "object.lookup" object account "tiny;1k;10k" "" '$lookup(Account.Order[0].Product[0], '\''SKU'\'')'
bench "object.each"   object account "tiny;1k;10k" "" '$each(Account.Order[0].Product[0], function($v, $k) { $k & '\''='\'' & $string($v) })'
bench "object.sift"   object account "tiny;1k;10k" "" '$sift(Account.Order[0].Product[0], function($v) { $type($v) = '\''number'\'' })'

# ── HOF ──────────────────────────────────────────────────────────────────────
bench "hof.map"      hof account "tiny;1k;10k" "quick" '$map(Account.Order.Product, function($v){$v.SKU & '\'': $'\'' & $string($v.UnitPrice)})'
bench "hof.filter"   hof account "tiny;1k;10k" "quick" '$filter(Account.Order.Product, function($v){$v.UnitPrice > 50})'
bench "hof.reduce"   hof account "tiny;1k;10k" "quick" '$reduce(Account.Order.Product, function($prev,$curr){$prev + $curr.UnitPrice}, 0)'
bench "hof.each"     hof account "tiny;1k;10k" ""      '$each(Account.Order[0].Product[0], function($v,$k){$k})'
bench "hof.sort_cmp" hof account "tiny;1k;10k" ""      '$sort(Account.Order.Product, function($a,$b){$a.Quantity > $b.Quantity})'
bench "hof.single"   hof account "tiny" ""      '$single(Account.Order.Product, function($v){$v.SKU = '\''040657863'\''})'
bench "hof.sift"     hof account "tiny;1k;10k" ""      '$sift(Account.Order[0].Product[0], function($v){$type($v) = '\''string'\''})'

# ── TYPE ─────────────────────────────────────────────────────────────────────
bench "type.type"    type account "tiny;1k;10k" ""      '$type(Account.Order)'
bench "type.string"  type account "tiny;1k;10k" "quick" 'Account.Order.Product.($string($))'
bench "type.number"  type account "tiny;1k;10k" ""      'Account.Order.Product.($number(UnitPrice))'
bench "type.boolean" type account "tiny;1k;10k" ""      '$boolean(Account.Order)'
bench "type.exists"  type account "tiny;1k;10k" ""      '$exists(Account.Order.Product)'

# ── DATETIME ─────────────────────────────────────────────────────────────────
bench "datetime.now"        datetime account  "tiny" ""  '$now()'
bench "datetime.toMillis"   datetime datetime "1k"   ""  '$toMillis(events[0].timestamp)'
bench "datetime.fromMillis" datetime datetime "1k"   ""  '$fromMillis(events[0].epoch_ms, '\''[Y]-[M01]-[D01]'\'')'

# ── REGEX ────────────────────────────────────────────────────────────────────
bench "regex.match"    regex strings "1k" "quick" '$match(items[0].text, /\b[A-Z]{2,}\b/)'
bench "regex.replace"  regex strings "1k" ""      '$replace(items[0].text, /[0-9]+/, '\''N'\'')'
bench "regex.split"    regex strings "1k" ""      '$split(items[0].text, /\s+/)'
bench "regex.contains" regex strings "1k" ""      '$contains(items[0].text, /error|fail/i)'

# ── ENCODING ─────────────────────────────────────────────────────────────────
bench "encoding.b64enc"      encoding strings "1k" "" '$base64encode(items[0].text)'
bench "encoding.b64dec"      encoding strings "1k" "" '$base64decode(items[0].b64)'
bench "encoding.urlenc"      encoding strings "1k" "" '$encodeUrlComponent(items[0].text)'
bench "encoding.urldec"      encoding strings "1k" "" '$decodeUrlComponent(items[0].urlencoded)'
bench "encoding.urlenc_full" encoding strings "1k" "" '$encodeUrl(items[0].url)'
bench "encoding.urldec_full" encoding strings "1k" "" '$decodeUrl(items[0].urlencoded_full)'

# ── OPERATOR ─────────────────────────────────────────────────────────────────
bench "op.arithmetic" operator account "tiny;1k;10k" "quick" 'Account.Order.Product.(UnitPrice * Quantity * (1 - Discount))'
bench "op.comparison" operator account "tiny;1k;10k" ""      'Account.Name = '\''Firefly'\'''
bench "op.lt_chain"   operator account "tiny;1k;10k" ""      'Account.Order.Product[UnitPrice > 20 and UnitPrice < 100]'
bench "op.logical"    operator account "tiny;1k;10k" ""      '$exists(Account.Order) and $count(Account.Order.Product) > 0'
bench "op.concat"     operator account "tiny;1k;10k" ""      'Account.Order.Product.(Description & '\'' (SKU: '\'' & SKU & '\'')'\'')'
bench "op.range"      operator account "tiny;1k;10k" ""      '[1..10]'
bench "op.coalesce"   operator account "tiny;1k;10k" ""      'Account.Missing ?? '\''default'\'''
bench "op.in"         operator account "tiny;1k;10k" ""      'Account.Order[0].Product[0].SKU in Account.Order.Product.SKU'
bench "op.ternary"    operator account "tiny;1k;10k" ""      '$count(Account.Order) > 1 ? '\''multi'\'' : '\''single'\'''
bench "op.compound"   operator account "tiny;1k;10k" ""      '$sum([1..100])'

# ── LAMBDA ───────────────────────────────────────────────────────────────────
bench "lambda.basic"   lambda account "tiny;1k;10k" "quick" '($f := function($x){$x * 2}; $f(21))'
bench "lambda.closure" lambda account "tiny;1k;10k" ""      '($factor := 1.1; $map(Account.Order.Product.UnitPrice, function($p){$p * $factor}))'
bench "lambda.nested"  lambda account "tiny;1k;10k" ""      '$map(Account.Order, function($o){$map($o.Product, function($p){$p.SKU})})'
bench "lambda.partial" lambda account "tiny;1k;10k" ""      '($add := function($a,$b){$a+$b}; $p := $add(10, ?); $p(5))'

# ── TRANSFORM ────────────────────────────────────────────────────────────────
bench "transform.chain"       transform account "tiny;1k;10k" "quick" 'Account.Order.Product ~> $map(function($v){$v.UnitPrice}) ~> $sum()'
bench "transform.pipe_filter" transform account "tiny;1k;10k" ""      'Account.Order.Product ~> $filter(function($v){$v.UnitPrice > 30}) ~> $count()'

# ── BLOCK ────────────────────────────────────────────────────────────────────
bench "block.binding"     block account "tiny;1k;10k" ""      '($x := Account.Name; $y := $count(Account.Order); $x & '\'': '\'' & $string($y) & '\'' orders'\'')'
bench "block.conditional" block account "tiny;1k;10k" ""      'Account.Order.Product.(UnitPrice > 50 ? '\''expensive'\'' : '\''cheap'\'')'
bench "block.compound"    block account "tiny;1k;10k" "quick" '($items := Account.Order.Product; $total := $sum($items.UnitPrice); $avg := $total / $count($items); {'\''total'\'': $total, '\''avg'\'': $avg})'

# ── COMPLEX ──────────────────────────────────────────────────────────────────
bench "complex.chained_hof"  complex account "tiny;1k;10k" "quick" 'Account.Order.Product ~> $filter(function($v){$v.UnitPrice > 30}) ~> $map(function($v){$v.SKU}) ~> $count()'
bench "complex.group_by"     complex account "tiny;1k;10k" "quick" 'Account.Order{OrderID: $sum(Product.UnitPrice)}'
bench "complex.nested_group" complex account "tiny;1k;10k" ""      'Account.Order{OrderID: Product{SKU: UnitPrice}}'
bench "complex.multi_sort"   complex account "tiny;1k;10k" ""      '$sort(Account.Order.Product, function($a,$b){$a.UnitPrice > $b.UnitPrice})'
bench "complex.recur_agg"    complex account "tiny;1k;10k" ""      '$sum(**.UnitPrice)'

# ── INVENTORY (real-world, 21MB) ─────────────────────────────────────────────
bench "inv.deep_path"     inventory inventory "full" "" 'data.inventory.edges.node.grn.grnPurchaseOrders.edges.node.purchaseOrder.number'
bench "inv.filter_active" inventory inventory "full" "" 'data.inventory.edges[node.active = true].node.number'
bench "inv.weight_sum"    inventory inventory "full" "" '$sum(data.inventory.edges.node.grossWeight)'
bench "inv.group_status"  inventory inventory "full" "" 'data.inventory.edges.node{inventoryStatusId: $count($)}'
bench "inv.address"       inventory inventory "full" "" 'data.inventory.edges.node.grn.grnPurchaseOrders.edges.node.purchaseOrder.shipToAddress.city'

# ── Filter + count ───────────────────────────────────────────────────────────
FILTERED=()
for entry in "${BENCHMARKS[@]}"; do
    IFS='|' read -r id cat fixture sizes tags expr <<< "$entry"

    # Filter by --section
    if [[ -n "$SECTIONS" ]]; then
        match=false
        IFS=',' read -ra sec_arr <<< "$SECTIONS"
        for s in "${sec_arr[@]}"; do
            [[ "$cat" == "$s" ]] && match=true
        done
        $match || continue
    fi

    # Filter by --id
    if [[ -n "$BENCH_ID" ]]; then
        match=false
        IFS=',' read -ra id_arr <<< "$BENCH_ID"
        for i in "${id_arr[@]}"; do
            [[ "$id" == "$i" ]] && match=true
        done
        $match || continue
    fi

    # Filter by --tag
    if [[ -n "$TAG" ]]; then
        [[ ";$tags;" == *";$TAG;"* ]] || continue
    fi

    FILTERED+=("$entry")
done

if [[ ${#FILTERED[@]} -eq 0 ]]; then
    echo "No benchmarks matched the selection." >&2
    exit 1
fi

echo ""
echo "  Benchmark suite: ${#FILTERED[@]} expressions"
echo "  Runners: $RUNNERS"
echo "  Sizes: $SIZES"
$DRY_RUN && echo "  Mode: DRY RUN"
echo ""

# ── Build ────────────────────────────────────────────────────────────────────
if ! $DRY_RUN; then
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
    cd "$ROOT"
    echo ""
fi

# ── Run ──────────────────────────────────────────────────────────────────────
run_count=0
total_filtered=${#FILTERED[@]}

for entry in "${FILTERED[@]}"; do
    IFS='|' read -r id cat fixture bench_sizes tags expr <<< "$entry"
    run_count=$((run_count + 1))

    # Resolve which sizes to run
    IFS=',' read -ra requested_sizes <<< "$SIZES"
    IFS=';' read -ra available_sizes <<< "$bench_sizes"
    run_sizes=()
    for rs in "${requested_sizes[@]}"; do
        for as in "${available_sizes[@]}"; do
            [[ "$rs" == "$as" ]] && run_sizes+=("$rs")
        done
    done
    # Fallback: if no overlap, use smallest available
    if [[ ${#run_sizes[@]} -eq 0 ]]; then
        run_sizes=("${available_sizes[0]}")
    fi

    for size in "${run_sizes[@]}"; do
        datafile=$(resolve_fixture "$fixture" "$size")
        if [[ -z "$datafile" ]]; then
            echo "  [$run_count/$total_filtered] $id ($size) — SKIPPED: no fixture" >&2
            continue
        fi
        iters=$ITERS

        echo "── [$run_count/$total_filtered] $id ($size, $iters iters) ──"

        if $DRY_RUN; then
            for runner in $RUNNERS; do
                echo "  DRY-RUN: would run $runner: $expr"
            done
            continue
        fi

        # Write expr to a temp file to avoid all shell quoting issues
        EXPR_FILE=$(mktemp)
        echo -n "$expr" > "$EXPR_FILE"

        # Build wrapper scripts
        HF_ARGS=(--warmup "$WARMUP" --min-runs "$MIN_RUNS" --shell=none)
        for runner in $RUNNERS; do
            WRAPPER=$(mktemp)
            case "$runner" in
                go)
                    cat > "$WRAPPER" <<SCRIPT
#!/bin/sh
exec '$GO_BIN' -expr "\$(cat '$EXPR_FILE')" -datafile '$datafile' -n $iters
SCRIPT
                    ;;
                rust)
                    cat > "$WRAPPER" <<SCRIPT
#!/bin/sh
exec '$RS_BIN' -expr "\$(cat '$EXPR_FILE')" -datafile '$datafile' -n $iters
SCRIPT
                    ;;
                js)
                    if [[ ! -f "$BENCH_DIR/js_bench.js" ]]; then
                        echo "  NOTE: JS runner not available yet, skipping" >&2
                        continue
                    fi
                    cat > "$WRAPPER" <<SCRIPT
#!/bin/sh
exec node '$BENCH_DIR/js_bench.js' -expr "\$(cat '$EXPR_FILE')" -datafile '$datafile' -n $iters
SCRIPT
                    ;;
            esac
            chmod +x "$WRAPPER"
            HF_ARGS+=(-n "$runner" "$WRAPPER")
        done

        hyperfine "${HF_ARGS[@]}"
        echo ""

        rm -f "$EXPR_FILE"
    done
done

echo "=== Done: ${#FILTERED[@]} expressions benchmarked ==="
