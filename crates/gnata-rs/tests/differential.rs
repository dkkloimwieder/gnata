//! Differential fast-path tests.
//!
//! Every expression here is recognized by at least one fast path
//! (`hof_fast` lambda shapes, mapped-call lifts, the `^()` sort fast
//! path, or `fast_path` expression classification). Each case is
//! evaluated twice — fast paths enabled, then disabled via
//! `fast_path::testing` — and both results must be identical,
//! including error codes. This guards against the fast-path layer
//! diverging from the general evaluator (gnata-bec.5).

use gnata::Value;
use gnata::{Expression, JsonataError};

/// Items with clean, homogeneous fields — exercises happy paths.
const CLEAN: &str = r#"{"items": [
    {"x": 3, "y": 1.5, "name": "Alpha"},
    {"x": 1, "y": 2.5, "name": "beta"},
    {"x": 2.5, "y": 0, "name": "Gamma"}
]}"#;

/// Items with missing fields, nulls, and mixed types — exercises
/// error paths and undefined propagation.
const HOSTILE: &str = r#"{"items": [
    {"x": 3, "y": 1, "name": "Alpha"},
    {"x": "str", "name": "beta"},
    {"x": true},
    {"x": null, "name": 7},
    {"y": 2},
    {}
]}"#;

/// Numbers including zeros and extremes — division/overflow behavior.
const NUMS: &str = r#"{"items": [
    {"x": 10, "y": 2},
    {"x": 1, "y": 0},
    {"x": 0, "y": 5},
    {"x": 1e308, "y": 1e308},
    {"x": -2.5, "y": 0.5}
]}"#;

/// Nested document for pure-path / comparison / function fast paths.
const NESTED: &str = r#"{"a": {"b": {"c": 42, "s": "hello"}},
    "arr": [{"v": 1}, {"v": 2}, {"v": 3}],
    "mixed": [{"v": 1}, {"w": 2}, {"v": null}],
    "empty": []}"#;

/// (expression, data) pairs. Every pair runs fast vs general.
const CASES: &[(&str, &str)] = &[
    // ── SimpleLambda::FieldAccess ──
    ("$map(items, function($v){$v.x})", CLEAN),
    ("$map(items, function($v){$v.x})", HOSTILE),
    ("$each({\"a\": 1, \"b\": 2}, function($v){$v})", CLEAN),
    // ── SimpleLambda::FieldPredicate (each relational op, both data sets) ──
    ("$filter(items, function($v){$v.x > 2})", CLEAN),
    ("$filter(items, function($v){$v.x > 2})", HOSTILE),
    ("$filter(items, function($v){$v.x < 2})", CLEAN),
    ("$filter(items, function($v){$v.x < 2})", HOSTILE),
    ("$filter(items, function($v){$v.x >= 2.5})", CLEAN),
    ("$filter(items, function($v){$v.x <= 2.5})", HOSTILE),
    ("$filter(items, function($v){$v.x = 3})", CLEAN),
    ("$filter(items, function($v){$v.x = 3})", HOSTILE),
    ("$filter(items, function($v){$v.x != 3})", CLEAN),
    ("$filter(items, function($v){$v.x != 3})", HOSTILE),
    ("$filter(items, function($v){$v.name = \"Alpha\"})", CLEAN),
    (
        "$filter(items, function($v){$v.name != \"Alpha\"})",
        HOSTILE,
    ),
    ("$filter(items, function($v){$v.name > \"B\"})", HOSTILE),
    ("$sift(items[0], function($v){$v > 1})", CLEAN),
    // ── SimpleLambda::TwoFieldPredicate ──
    ("$filter(items, function($v){$v.x = $v.y})", CLEAN),
    ("$filter(items, function($v){$v.x = $v.y})", HOSTILE),
    ("$filter(items, function($v){$v.x > $v.y})", CLEAN),
    ("$filter(items, function($v){$v.x > $v.y})", HOSTILE),
    ("$filter(items, function($v){$v.x > $v.y})", NUMS),
    // ── SimpleLambda::CompoundPredicate (and/or, short-circuit order) ──
    ("$filter(items, function($v){$v.x > 1 and $v.x < 3})", CLEAN),
    (
        "$filter(items, function($v){$v.x > 1 and $v.x < 3})",
        HOSTILE,
    ),
    ("$filter(items, function($v){$v.x > 2 or $v.y > 2})", CLEAN),
    (
        "$filter(items, function($v){$v.x > 2 or $v.y > 2})",
        HOSTILE,
    ),
    (
        "$filter(items, function($v){$v.x > 100 and $v.name > 5})",
        CLEAN,
    ),
    // ── SimpleLambda::SortComparator / SortComparatorOp ──
    ("$sort(items, function($a, $b){$a.x > $b.x})", CLEAN),
    ("$sort(items, function($a, $b){$a.x > $b.x})", HOSTILE),
    ("$sort(items, function($a, $b){$a.x < $b.x})", CLEAN),
    ("$sort(items, function($a, $b){$a.x >= $b.x})", CLEAN),
    ("$sort(items, function($a, $b){$a.x <= $b.x})", NUMS),
    ("$sort(items, function($a, $b){$a.name > $b.name})", CLEAN),
    ("$sort(items, function($a, $b){$a.name > $b.name})", HOSTILE),
    // ── SimpleLambda::ReduceAccum / ReduceCompoundAccum ──
    ("$reduce(items, function($acc, $v){$acc + $v.x}, 0)", CLEAN),
    (
        "$reduce(items, function($acc, $v){$acc + $v.x}, 0)",
        HOSTILE,
    ),
    ("$reduce(items, function($acc, $v){$acc + $v.x}, 0)", NUMS),
    ("$reduce(items, function($acc, $v){$acc * $v.x}, 1)", CLEAN),
    ("$reduce(items, function($acc, $v){$acc - $v.x}, 100)", NUMS),
    (
        "$reduce(items, function($acc, $v){$acc + ($v.x * $v.y)}, 0)",
        CLEAN,
    ),
    (
        "$reduce(items, function($acc, $v){$acc + ($v.x * $v.y)}, 0)",
        NUMS,
    ),
    (
        "$reduce(items, function($acc, $v){$acc + ($v.x / $v.y)}, 0)",
        NUMS,
    ),
    // ── SimpleLambda::ConcatTemplate ──
    (
        "$map(items, function($v){$v.name & \"-\" & $string($v.x)})",
        CLEAN,
    ),
    (
        "$map(items, function($v){$v.name & \"-\" & $string($v.x)})",
        HOSTILE,
    ),
    (
        "$map(items, function($v){$lowercase($v.name) & $uppercase($v.name)})",
        CLEAN,
    ),
    (
        "$map(items, function($v){$substring($v.name, 1, 3) & \"!\"})",
        CLEAN,
    ),
    (
        "$map(items, function($v){$substring($v.name, 1, 3) & \"!\"})",
        HOSTILE,
    ),
    // ── Mapped-call lift (path-step form, PreparedState) ──
    ("items.$round(x)", CLEAN),
    ("items.$round(x)", HOSTILE),
    ("items.$round(x, 1)", NUMS),
    ("items.$formatNumber(x, \"#,##0.00\")", CLEAN),
    ("items.$formatNumber(x, \"#,##0.00\")", NUMS),
    ("items.$contains(name, \"a\")", CLEAN),
    ("items.$contains(name, \"a\")", HOSTILE),
    ("items.$formatBase(x, 16)", CLEAN),
    ("items.$substring(name, 0, 2)", CLEAN),
    ("items.$lowercase(name)", CLEAN),
    ("items.$lowercase(name)", HOSTILE),
    ("items.$uppercase(name)", CLEAN),
    ("items.$string(x)", HOSTILE),
    // ── ^() sort operator fast path ──
    ("items^(x)", CLEAN),
    ("items^(>x)", CLEAN),
    ("items^(x)", HOSTILE),
    ("items^(x, y)", NUMS),
    ("items^(>name)", CLEAN),
    // ── Expression-level FastPath::PurePath ──
    ("a.b.c", NESTED),
    ("a.b.s", NESTED),
    ("a.b.missing", NESTED),
    ("arr.v", NESTED),
    ("mixed.v", NESTED),
    ("empty.v", NESTED),
    ("a.b", NESTED),
    // ── Expression-level FastPath::Comparison ──
    ("a.b.c = 42", NESTED),
    ("a.b.c != 42", NESTED),
    ("a.b.c = \"42\"", NESTED),
    ("a.b.s = \"hello\"", NESTED),
    ("a.b.missing = 1", NESTED),
    ("arr.v = 2", NESTED),
    // ── Expression-level FastPath::Function ──
    ("$sum(arr.v)", NESTED),
    ("$sum(mixed.v)", NESTED),
    ("$count(arr)", NESTED),
    ("$count(empty)", NESTED),
    ("$exists(a.b.c)", NESTED),
    ("$exists(a.b.missing)", NESTED),
    ("$distinct(arr.v)", NESTED),
    ("$keys(a.b)", NESTED),
    ("$sqrt(a.b.c)", NESTED),
    ("$string(a.b.c)", NESTED),
    ("$sum(a.b.missing)", NESTED),
    ("$max(mixed.v)", NESTED),
    ("$min(mixed.v)", NESTED),
    ("$max(arr.v)", NESTED),
    ("$min(a.b.missing)", NESTED),
    ("$average(arr.v)", NESTED),
    ("$average(mixed.v)", NESTED),
    ("$length(a.b.s)", NESTED),
    ("$length(arr)", NESTED),
    ("$length(arr.v)", NESTED),
    ("$abs(a.b.c)", NESTED),
    ("$floor(a.b.s)", NESTED),
    ("$ceil(a.b.c)", NESTED),
    ("$number(a.b.s)", NESTED),
    ("$number(a.b.c)", NESTED),
    ("$boolean(a.b.missing)", NESTED),
    ("$boolean(empty)", NESTED),
    ("$not(a.b.missing)", NESTED),
    ("$trim(a.b.s)", NESTED),
    ("$trim(a.b.c)", NESTED),
    ("$contains(a.b.s, \"ell\")", NESTED),
    ("$contains(a.b.c, \"4\")", NESTED),
    ("$lowercase(a.b.s)", NESTED),
    ("$uppercase(a.b.c)", NESTED),
    ("$exists(mixed.v)", NESTED),
    ("$count(mixed.v)", NESTED),
    ("$count(a.b.missing)", NESTED),
];

type EvalResult = Result<Value, JsonataError>;

fn describe(r: &EvalResult) -> String {
    match r {
        Ok(v) => format!("Ok({v:?})"),
        Err(e) => format!("Err({})", e.code),
    }
}

/// Compare fast vs general results: equal values or equal error codes.
fn diverged(fast: &EvalResult, general: &EvalResult) -> bool {
    match (fast, general) {
        (Ok(a), Ok(b)) => !(a.is_undefined() && b.is_undefined()) && !gnata::deep_equal(a, b),
        (Err(a), Err(b)) => a.code != b.code,
        _ => true,
    }
}

#[test]
fn fast_paths_match_general_evaluator() {
    let mut mismatches: Vec<String> = Vec::new();

    for &(expr, data) in CASES {
        let compiled = match Expression::compile(expr) {
            Ok(c) => c,
            Err(e) => {
                mismatches.push(format!("COMPILE FAIL {expr}: {e}"));
                continue;
            }
        };

        gnata::fast_path_testing::set_fast_paths_disabled(false);
        let fast_str = compiled.evaluate(data);
        let input = Value::from_json_str(data).expect("test data is valid JSON");
        let fast_val = compiled.evaluate_value(&input);

        gnata::fast_path_testing::set_fast_paths_disabled(true);
        let general = compiled.evaluate_value(&input);
        gnata::fast_path_testing::set_fast_paths_disabled(false);

        if diverged(&fast_str, &general) {
            mismatches.push(format!(
                "{expr}\n  data:    {data}\n  fast:    {} (evaluate)\n  general: {}",
                describe(&fast_str),
                describe(&general)
            ));
        }
        if diverged(&fast_val, &general) {
            mismatches.push(format!(
                "{expr}\n  data:    {data}\n  fast:    {} (evaluate_value)\n  general: {}",
                describe(&fast_val),
                describe(&general)
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} fast-path divergence(s):\n\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}
