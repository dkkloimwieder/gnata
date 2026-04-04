//! Fast-path analysis and evaluation for simple JSONata expressions.
//!
//! Classifies expressions at compile time into categories that can bypass
//! the full AST-walking evaluator:
//!
//! - **Pure path**: `Account.Name`, `a.b.c` — direct field traversal
//! - **Comparison**: `Price = 100`, `Name != "x"` — path + literal compare
//! - **Function**: `$exists(a.b)`, `$sum(prices)` — builtin on a path
//!
//! Port of Go `internal/parser/analysis.go` and `func_fast.go`.

use std::rc::Rc;

use crate::parser::{AstArena, Expr, NodeId};
use crate::value::Value;

/// Result of fast-path analysis on a compiled expression.
#[derive(Debug, Clone)]
pub enum FastPath {
    /// No fast path available — use full evaluator.
    None,
    /// Simple dotted field path: direct Value traversal.
    PurePath(Vec<String>),
    /// Binary = or != with pure-path LHS and literal RHS.
    Comparison(ComparisonFastPath),
    /// Supported builtin function applied to a pure-path argument.
    Function(FuncFastPath),
}

/// Metadata for comparison fast path.
#[derive(Debug, Clone)]
pub struct ComparisonFastPath {
    pub path: Vec<String>,
    pub op: ComparisonOp,
    pub rhs: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonOp {
    Equal,
    NotEqual,
}

/// Metadata for function fast path.
#[derive(Debug, Clone)]
pub struct FuncFastPath {
    pub kind: FuncFastKind,
    pub path: Vec<String>,
    /// Second string argument for `$contains(path, "literal")`.
    pub str_arg: Option<String>,
}

/// Supported fast-path function kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuncFastKind {
    Exists,
    String,
    Boolean,
    Number,
    Not,
    Type,
    Lowercase,
    Uppercase,
    Trim,
    Length,
    Contains,
    Abs,
    Floor,
    Ceil,
    Sqrt,
    Keys,
    Values,
    Count,
    Sum,
    Max,
    Min,
    Average,
    Reverse,
    Distinct,
}

// ── Analysis ────────────────────────────────────────────────────────

/// Analyze an AST node and return its fast-path classification.
/// Called once at compile time after `process_ast`.
pub fn analyze(arena: &AstArena, node: NodeId) -> FastPath {
    // Try pure path first.
    if let Some(path) = collect_pure_path(arena, node) {
        return FastPath::PurePath(path);
    }
    // Try comparison.
    if let Some(cmp) = try_comparison(arena, node) {
        return FastPath::Comparison(cmp);
    }
    // Try function.
    if let Some(func) = try_function(arena, node) {
        return FastPath::Function(func);
    }
    FastPath::None
}

/// Check if a node is a pure dotted path (no filters, wildcards, etc.).
/// Returns the path segments if so.
fn collect_pure_path(arena: &AstArena, node: NodeId) -> Option<Vec<String>> {
    match arena.get(node) {
        // Single name: `foo`
        Expr::Name {
            value,
            stages,
            group: None,
            focus: None,
            index: None,
            keep_array: false,
            ..
        } if stages.is_empty() => Some(vec![value.clone()]),

        // Dotted path: `a.b.c`
        Expr::Path {
            steps,
            group: None,
            keep_singleton_array: false,
            ..
        } => {
            let mut segments = Vec::with_capacity(steps.len());
            for &step in steps {
                match arena.get(step) {
                    Expr::Name {
                        value,
                        stages,
                        group: None,
                        focus: None,
                        index: None,
                        keep_array: false,
                        ..
                    } if stages.is_empty() => {
                        segments.push(value.clone());
                    }
                    _ => return None,
                }
            }
            Some(segments)
        }

        _ => None,
    }
}

/// Try to classify as a comparison fast path: `path = literal` or `path != literal`.
fn try_comparison(arena: &AstArena, node: NodeId) -> Option<ComparisonFastPath> {
    let (op_str, lhs, rhs) = match arena.get(node) {
        Expr::Binary { op, lhs, rhs, .. } if op == "=" || op == "!=" => {
            (op.as_str(), *lhs, *rhs)
        }
        _ => return None,
    };

    let path = collect_pure_path(arena, lhs)?;
    let rhs_val = extract_literal(arena, rhs)?;
    let op = if op_str == "=" {
        ComparisonOp::Equal
    } else {
        ComparisonOp::NotEqual
    };

    Some(ComparisonFastPath {
        path,
        op,
        rhs: rhs_val,
    })
}

/// Try to classify as a function fast path: `$func(path)` or `$contains(path, "lit")`.
fn try_function(arena: &AstArena, node: NodeId) -> Option<FuncFastPath> {
    let (procedure, arguments) = match arena.get(node) {
        Expr::Function {
            procedure,
            arguments,
            ..
        } => (*procedure, arguments.clone()),
        _ => return None,
    };

    // Procedure must be a variable reference to a builtin name.
    let func_name = match arena.get(procedure) {
        Expr::Variable { name, .. } => name.as_str(),
        _ => return None,
    };

    let kind = match func_name {
        "exists" => FuncFastKind::Exists,
        "string" => FuncFastKind::String,
        "boolean" => FuncFastKind::Boolean,
        "number" => FuncFastKind::Number,
        "not" => FuncFastKind::Not,
        "type" => FuncFastKind::Type,
        "lowercase" => FuncFastKind::Lowercase,
        "uppercase" => FuncFastKind::Uppercase,
        "trim" => FuncFastKind::Trim,
        "length" => FuncFastKind::Length,
        "abs" => FuncFastKind::Abs,
        "floor" => FuncFastKind::Floor,
        "ceil" => FuncFastKind::Ceil,
        "sqrt" => FuncFastKind::Sqrt,
        "keys" => FuncFastKind::Keys,
        "values" => FuncFastKind::Values,
        "count" => FuncFastKind::Count,
        "sum" => FuncFastKind::Sum,
        "max" => FuncFastKind::Max,
        "min" => FuncFastKind::Min,
        "average" => FuncFastKind::Average,
        "reverse" => FuncFastKind::Reverse,
        "distinct" => FuncFastKind::Distinct,
        "contains" => FuncFastKind::Contains,
        _ => return None,
    };

    // $contains special case: two args, second must be string literal.
    if kind == FuncFastKind::Contains {
        if arguments.len() != 2 {
            return None;
        }
        let path = collect_pure_path(arena, arguments[0])?;
        let str_arg = match arena.get(arguments[1]) {
            Expr::StringLit { value, .. } => value.clone(),
            _ => return None,
        };
        return Some(FuncFastPath {
            kind,
            path,
            str_arg: Some(str_arg),
        });
    }

    // Standard case: single pure-path argument.
    if arguments.len() != 1 {
        return None;
    }
    let path = collect_pure_path(arena, arguments[0])?;

    Some(FuncFastPath {
        kind,
        path,
        str_arg: None,
    })
}

/// Extract a literal value from an AST node.
fn extract_literal(arena: &AstArena, node: NodeId) -> Option<Value> {
    match arena.get(node) {
        Expr::StringLit { value, .. } => Some(Value::String(value.as_str().into())),
        Expr::NumberLit { value, .. } => Some(Value::Number(*value)),
        Expr::ValueLit { value, .. } => match value.as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            "null" => Some(Value::Null),
            _ => None,
        },
        _ => None,
    }
}

// ── Fast-path evaluation ────────────────────────────────────────────

/// Evaluate a fast-path expression against input data.
/// Returns `Some(result)` if handled, `None` if fallback to full eval is needed.
pub fn eval_fast(fast_path: &FastPath, input: &Value) -> Option<Value> {
    match fast_path {
        FastPath::None => None,
        FastPath::PurePath(segments) => Some(eval_pure_path(segments, input)),
        FastPath::Comparison(cmp) => eval_comparison(cmp, input),
        FastPath::Function(func) => eval_function(func, input),
    }
}

/// Traverse a pure dotted path on a Value.
fn eval_pure_path(segments: &[String], input: &Value) -> Value {
    let mut current = input.clone();
    for segment in segments {
        current = match &current {
            Value::Object(obj) => match obj.get(segment) {
                Some(v) => v.clone(),
                None => return Value::Undefined,
            },
            // Auto-map over arrays.
            Value::Array(arr) => {
                let mut results = Vec::new();
                for item in arr.iter() {
                    if let Value::Object(obj) = item
                        && let Some(v) = obj.get(segment)
                    {
                        match v {
                            Value::Array(inner) => results.extend(inner.iter().cloned()),
                            Value::Undefined => {}
                            other => results.push(other.clone()),
                        }
                    }
                }
                if results.is_empty() {
                    return Value::Undefined;
                }
                if results.len() == 1 {
                    results.into_iter().next().unwrap_or(Value::Undefined)
                } else {
                    Value::Array(Rc::new(results))
                }
            }
            _ => return Value::Undefined,
        };
    }
    current
}

/// Evaluate a comparison fast path.
fn eval_comparison(cmp: &ComparisonFastPath, input: &Value) -> Option<Value> {
    let lhs = eval_pure_path(&cmp.path, input);

    // Undefined comparisons always return false.
    if lhs.is_undefined() {
        return Some(Value::Bool(false));
    }

    // Arrays require auto-mapping — fall back to full evaluator.
    if matches!(lhs, Value::Array(_)) {
        return None;
    }

    let matches = lhs.deep_equal(&cmp.rhs);
    let result = match cmp.op {
        ComparisonOp::Equal => matches,
        ComparisonOp::NotEqual => !matches,
    };
    Some(Value::Bool(result))
}

/// Evaluate a function fast path.
fn eval_function(func: &FuncFastPath, input: &Value) -> Option<Value> {
    let val = eval_pure_path(&func.path, input);

    // If path resolved to undefined, most functions should fall through
    // to full eval for proper error handling, except $exists.
    if val.is_undefined() && func.kind != FuncFastKind::Exists {
        return Some(apply_func_undefined(func.kind));
    }

    apply_func(func, &val)
}

/// Handle undefined input for functions that have defined behavior on undefined.
fn apply_func_undefined(kind: FuncFastKind) -> Value {
    match kind {
        FuncFastKind::Exists => Value::Bool(false),
        FuncFastKind::Boolean => Value::Undefined,
        FuncFastKind::String => Value::Undefined,
        FuncFastKind::Number => Value::Undefined,
        FuncFastKind::Type => Value::Undefined,
        FuncFastKind::Not => Value::Undefined,
        FuncFastKind::Count => Value::Number(0.0),
        _ => Value::Undefined,
    }
}

/// Apply a fast-path function to a resolved value.
#[allow(clippy::too_many_lines)]
fn apply_func(func: &FuncFastPath, val: &Value) -> Option<Value> {
    match func.kind {
        FuncFastKind::Exists => Some(Value::Bool(!val.is_undefined())),

        FuncFastKind::Type => {
            let t = match val {
                Value::String(_) => "string",
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                Value::Null => "null",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
                Value::Undefined => return Some(Value::Undefined),
                _ => return None,
            };
            Some(Value::String(t.into()))
        }

        FuncFastKind::String => match val {
            Value::String(_) => Some(val.clone()),
            Value::Number(n) => Some(Value::String(crate::value::format_float(*n).into())),
            Value::Bool(b) => Some(Value::String(if *b { "true" } else { "false" }.into())),
            Value::Null => Some(Value::String("null".into())),
            _ => None,
        },

        FuncFastKind::Boolean => match val {
            Value::Bool(_) => Some(val.clone()),
            Value::String(s) => Some(Value::Bool(!s.is_empty())),
            Value::Number(n) => Some(Value::Bool(*n != 0.0)),
            Value::Null => Some(Value::Bool(false)),
            _ => None,
        },

        FuncFastKind::Number => match val {
            Value::Number(_) => Some(val.clone()),
            Value::String(s) => s.parse::<f64>().ok().map(Value::Number),
            Value::Bool(b) => Some(Value::Number(if *b { 1.0 } else { 0.0 })),
            _ => None,
        },

        FuncFastKind::Not => Some(Value::Bool(!val.to_boolean())),

        FuncFastKind::Lowercase => match val {
            Value::String(s) => Some(Value::String(s.to_lowercase().into())),
            _ => None,
        },

        FuncFastKind::Uppercase => match val {
            Value::String(s) => Some(Value::String(s.to_uppercase().into())),
            _ => None,
        },

        FuncFastKind::Trim => match val {
            Value::String(s) => {
                let trimmed: Vec<&str> = s.split_whitespace().collect();
                Some(Value::String(trimmed.join(" ").into()))
            }
            _ => None,
        },

        FuncFastKind::Length => match val {
            Value::String(s) => {
                Some(Value::Number(s.chars().count() as f64))
            }
            _ => None,
        },

        FuncFastKind::Contains => {
            let search = func.str_arg.as_deref()?;
            match val {
                Value::String(s) => Some(Value::Bool(s.contains(search))),
                _ => None,
            }
        }

        FuncFastKind::Abs => match val {
            Value::Number(n) => Some(Value::Number(n.abs())),
            _ => None,
        },

        FuncFastKind::Floor => match val {
            Value::Number(n) => Some(Value::Number(n.floor())),
            _ => None,
        },

        FuncFastKind::Ceil => match val {
            Value::Number(n) => Some(Value::Number(n.ceil())),
            _ => None,
        },

        FuncFastKind::Sqrt => match val {
            Value::Number(n) if *n >= 0.0 => Some(Value::Number(n.sqrt())),
            _ => None,
        },

        FuncFastKind::Count => match val {
            Value::Array(arr) => Some(Value::Number(arr.len() as f64)),
            Value::Undefined => Some(Value::Number(0.0)),
            _ => Some(Value::Number(1.0)),
        },

        FuncFastKind::Sum => {
            let nums = collect_numbers(val)?;
            Some(Value::Number(nums.iter().sum()))
        }

        FuncFastKind::Max => {
            let nums = collect_numbers(val)?;
            nums.iter().copied().reduce(f64::max).map(Value::Number)
        }

        FuncFastKind::Min => {
            let nums = collect_numbers(val)?;
            nums.iter().copied().reduce(f64::min).map(Value::Number)
        }

        FuncFastKind::Average => {
            let nums = collect_numbers(val)?;
            if nums.is_empty() {
                return Some(Value::Undefined);
            }
            let sum: f64 = nums.iter().sum();
            Some(Value::Number(sum / nums.len() as f64))
        }

        FuncFastKind::Keys => match val {
            Value::Object(obj) => {
                let keys: Vec<Value> = obj.keys().map(|k| Value::String(k.as_str().into())).collect();
                Some(match keys.len() {
                    0 => Value::Undefined,
                    1 => keys.into_iter().next().unwrap_or(Value::Undefined),
                    _ => Value::Array(Rc::new(keys)),
                })
            }
            _ => None,
        },

        FuncFastKind::Values => match val {
            Value::Object(obj) => {
                let vals: Vec<Value> = obj.values().cloned().collect();
                Some(Value::Array(Rc::new(vals)))
            }
            _ => None,
        },

        FuncFastKind::Reverse => match val {
            Value::Array(arr) => {
                let mut rev = (**arr).clone();
                rev.reverse();
                Some(Value::Array(Rc::new(rev)))
            }
            _ => None,
        },

        FuncFastKind::Distinct => match val {
            Value::Array(arr) => {
                let mut seen = std::collections::HashSet::new();
                let mut result = Vec::new();
                for item in arr.iter() {
                    let key = canonical_key(item);
                    if seen.insert(key) {
                        result.push(item.clone());
                    }
                }
                Some(Value::Array(Rc::new(result)))
            }
            _ => None,
        },
    }
}

/// Collect all numbers from a value (scalar or array). Returns None if any non-number found.
fn collect_numbers(val: &Value) -> Option<Vec<f64>> {
    match val {
        Value::Number(n) => Some(vec![*n]),
        Value::Array(arr) => {
            let mut nums = Vec::with_capacity(arr.len());
            for item in arr.iter() {
                match item {
                    Value::Number(n) => nums.push(*n),
                    _ => return None,
                }
            }
            Some(nums)
        }
        _ => None,
    }
}

/// Create a canonical string key for deduplication in $distinct.
fn canonical_key(val: &Value) -> String {
    match val {
        Value::String(s) => format!("s:{s}"),
        Value::Number(n) => format!("n:{n}"),
        Value::Bool(b) => format!("b:{b}"),
        Value::Null => "null".into(),
        _ => format!("{val:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Expression;

    /// Helper: evaluate via Expression API (uses fast path when available)
    /// and via full evaluator, then compare results.
    fn assert_fast_matches_full(expr: &str, json: &str) {
        let input = Value::from_json_str(json).unwrap_or(Value::Undefined);
        let compiled = Expression::compile(expr).expect("compile failed");
        let fast_result = compiled.evaluate(&input).expect("eval failed");

        // Also run through full evaluator (bypassing fast path).
        let (mut arena, root) = crate::parser::Parser::parse(expr).expect("parse failed");
        let root = crate::parser::process_ast(&mut arena, root).expect("process failed");
        let mut env = crate::evaluator::Environment::new();
        crate::stdlib::register_all(&mut env);
        if !input.is_undefined() {
            env.bind("$".into(), input.clone());
        }
        let env = std::rc::Rc::new(env);
        let full_result = crate::eval(&arena, root, &input, &env).expect("full eval failed");

        assert_eq!(
            fast_result.to_json(),
            full_result.to_json(),
            "fast-path and full evaluator disagree for expr={expr:?}"
        );
    }

    #[test]
    fn pure_path_simple() {
        assert_fast_matches_full("name", r#"{"name": "Alice"}"#);
    }

    #[test]
    fn pure_path_dotted() {
        assert_fast_matches_full("a.b.c", r#"{"a": {"b": {"c": 42}}}"#);
    }

    #[test]
    fn pure_path_missing() {
        assert_fast_matches_full("a.b.x", r#"{"a": {"b": {"c": 42}}}"#);
    }

    #[test]
    fn pure_path_array_auto_map() {
        assert_fast_matches_full(
            "orders.product",
            r#"{"orders": [{"product": "A"}, {"product": "B"}]}"#,
        );
    }

    #[test]
    fn comparison_equal_string() {
        assert_fast_matches_full("name = \"Alice\"", r#"{"name": "Alice"}"#);
    }

    #[test]
    fn comparison_equal_number() {
        assert_fast_matches_full("age = 30", r#"{"age": 30}"#);
    }

    #[test]
    fn comparison_not_equal() {
        assert_fast_matches_full("name != \"Bob\"", r#"{"name": "Alice"}"#);
    }

    #[test]
    fn comparison_missing_field() {
        assert_fast_matches_full("x = 1", r#"{"y": 2}"#);
    }

    #[test]
    fn func_exists() {
        assert_fast_matches_full("$exists(name)", r#"{"name": "Alice"}"#);
    }

    #[test]
    fn func_exists_missing() {
        assert_fast_matches_full("$exists(x)", r#"{"y": 1}"#);
    }

    #[test]
    fn func_type() {
        assert_fast_matches_full("$type(name)", r#"{"name": "Alice"}"#);
    }

    #[test]
    fn func_string() {
        assert_fast_matches_full("$string(age)", r#"{"age": 42}"#);
    }

    #[test]
    fn func_count() {
        assert_fast_matches_full("$count(items)", r#"{"items": [1, 2, 3]}"#);
    }

    #[test]
    fn func_sum() {
        assert_fast_matches_full("$sum(prices)", r#"{"prices": [10, 20, 30]}"#);
    }

    #[test]
    fn func_lowercase() {
        assert_fast_matches_full("$lowercase(name)", r#"{"name": "ALICE"}"#);
    }

    #[test]
    fn func_contains() {
        assert_fast_matches_full(
            "$contains(name, \"li\")",
            r#"{"name": "Alice"}"#,
        );
    }

    #[test]
    fn func_length() {
        assert_fast_matches_full("$length(name)", r#"{"name": "hello"}"#);
    }

    #[test]
    fn func_abs() {
        assert_fast_matches_full("$abs(val)", r#"{"val": -5}"#);
    }

    #[test]
    fn func_keys() {
        assert_fast_matches_full("$keys(obj)", r#"{"obj": {"a": 1, "b": 2}}"#);
    }

    #[test]
    fn classification_pure_path() {
        let expr = Expression::compile("a.b.c").unwrap();
        assert!(matches!(expr.fast_path_info(), FastPath::PurePath(_)));
    }

    #[test]
    fn classification_comparison() {
        let expr = Expression::compile("x = 1").unwrap();
        assert!(matches!(expr.fast_path_info(), FastPath::Comparison(_)));
    }

    #[test]
    fn classification_function() {
        let expr = Expression::compile("$sum(prices)").unwrap();
        assert!(matches!(expr.fast_path_info(), FastPath::Function(_)));
    }

    #[test]
    fn classification_complex_no_fast_path() {
        let expr = Expression::compile("a.b[c > 5]").unwrap();
        assert!(matches!(expr.fast_path_info(), FastPath::None));
    }

    #[test]
    fn classification_wildcard_no_fast_path() {
        let expr = Expression::compile("a.*").unwrap();
        assert!(matches!(expr.fast_path_info(), FastPath::None));
    }
}
