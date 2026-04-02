//! Core JSONata evaluator: tree-walking dispatch over the AST arena.
//!
//! Port of Go `internal/evaluator/evaluator.go` and supporting files.

pub mod environment;
pub mod functions;

pub use environment::Environment;
pub use functions::{
    BuiltinFn, EnvAwareBuiltinFn, FunctionValue, Lambda, TailCall, call_function, eval_function,
    eval_lambda, eval_partial,
};

use std::rc::Rc;

use crate::error::{JsonataError, JsonataResult};
use crate::parser::{AstArena, Expr, NodeId};
use crate::value::{Sequence, Value};

/// Evaluate an AST node against input data in the given environment.
///
/// Returns `Value::Undefined` for missing/undefined results.
/// This is the main entry point — all node types dispatch through here.
pub fn eval(arena: &AstArena, node: NodeId, input: &Value, env: &Rc<Environment>) -> JsonataResult {
    if node.is_empty() {
        return Ok(Value::Undefined);
    }

    // Check cancellation at every expression boundary.
    if env.is_cancelled() {
        return Err(JsonataError::new("D3001", "evaluation cancelled"));
    }

    // If the node has a Group expression, evaluate the base node first,
    // then apply group-by reduction. (Deferred to Phase 10.)
    // TODO: evalGroupBy

    match arena.get(node) {
        Expr::ValueLit { value, .. } => eval_value_lit(value),
        Expr::StringLit { value, .. } => Ok(Value::String(value.clone())),
        Expr::NumberLit { value: n, .. } => Ok(Value::Number(*n)),
        Expr::Variable { name, .. } => eval_variable(name, input, env),
        Expr::Name { value, .. } => eval_name(value, input),
        Expr::Wildcard { .. } => eval_wildcard(input),
        Expr::Descendant { .. } => Ok(descendant_lookup(input)),
        Expr::Path { .. } => eval_path(arena, node, input, env),
        Expr::Binary { .. } => eval_binary(arena, node, input, env),
        Expr::Unary { .. } => eval_unary(arena, node, input, env),
        Expr::Block { .. } => eval_block(arena, node, input, env),
        Expr::Condition { .. } => eval_condition(arena, node, input, env),
        Expr::Bind { .. } => eval_bind(arena, node, input, env),
        Expr::Function { .. } => eval_function(arena, node, input, env),
        Expr::Lambda { .. } => eval_lambda(arena, node, input, env),
        Expr::Partial { .. } => eval_partial(arena, node, input, env),
        Expr::Sort { .. } => {
            // Deferred to Phase 10.
            Err(JsonataError::new("D3001", "sort not yet implemented"))
        }
        Expr::Regex { pattern, flags, .. } => Ok(eval_regex(pattern, flags)),
        Expr::Transform { .. } => {
            // Deferred to Phase 10.
            Err(JsonataError::new("D3001", "transform not yet implemented"))
        }
        Expr::Parent { .. } => {
            // % retrieves parent context stored by path tuple evaluation.
            if let Some(val) = env.lookup("%%") {
                Ok(val.clone())
            } else {
                Err(JsonataError::new(
                    "S0217",
                    "% operator used outside of a valid path context",
                ))
            }
        }
        Expr::Placeholder { .. } => Ok(Value::Undefined),
    }
}

/// Public API for calling any function value with given args.
/// Used by standard library functions to dispatch callbacks.
pub fn apply_function(
    func: &FunctionValue,
    args: &[Value],
    focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    call_function(func, args, focus, env, arena)
}

// ── Literal evaluators ──────────────────────────────────────────────

fn eval_value_lit(value: &str) -> JsonataResult {
    match value {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "null" => Ok(Value::Null),
        _ => Ok(Value::Undefined),
    }
}

fn eval_variable(name: &str, input: &Value, env: &Rc<Environment>) -> JsonataResult {
    if name.is_empty() {
        // Bare $ — refers to the current input context.
        return Ok(input.clone());
    }
    match env.lookup(name) {
        Some(val) => Ok(val.clone()),
        None => Ok(Value::Undefined),
    }
}

// ── Name (field access) ─────────────────────────────────────────────

fn eval_name(name: &str, input: &Value) -> JsonataResult {
    match input {
        Value::Object(obj) => match obj.get(name) {
            Some(val) => Ok(val.clone()),
            None => Ok(Value::Undefined),
        },
        Value::Array(arr) => {
            // JSONata auto-maps field lookups across arrays.
            let mut seq = Sequence::new();
            let mut field_found = false;
            for item in arr {
                let val = eval_name(name, item)?;
                if val.is_undefined() {
                    continue;
                }
                field_found = true;
                // Flatten plain arrays from navigating through arrays.
                match val {
                    Value::Array(inner) => {
                        for sv in inner {
                            seq.values
                                .push(if sv.is_undefined() { Value::Null } else { sv });
                        }
                    }
                    other => {
                        seq.append(other);
                    }
                }
            }
            if seq.values.is_empty() {
                if field_found {
                    // Field exists but was empty array — return empty array.
                    return Ok(Value::Array(vec![]));
                }
                return Ok(Value::Undefined);
            }
            Ok(seq.collapse())
        }
        Value::Sequence(s) => eval_name(name, &s.collapse()),
        _ => Ok(Value::Undefined),
    }
}

// ── Wildcard (*) ────────────────────────────────────────────────────

fn eval_wildcard(input: &Value) -> JsonataResult {
    match input {
        Value::Object(obj) => {
            if obj.is_empty() {
                return Ok(Value::Undefined);
            }
            let mut seq = Sequence::new();
            for (_, val) in obj {
                match val {
                    Value::Array(arr) => seq.values.extend(arr.iter().cloned()),
                    other => seq.values.push(other.clone()),
                }
            }
            if seq.values.is_empty() {
                Ok(Value::Undefined)
            } else {
                Ok(seq.collapse())
            }
        }
        Value::Array(arr) => {
            let mut seq = Sequence::new();
            for item in arr {
                if item.is_object() {
                    let val = eval_wildcard(item)?;
                    if !val.is_undefined() {
                        seq.append(val);
                    }
                } else if !item.is_undefined() {
                    seq.values.push(item.clone());
                }
            }
            Ok(seq.collapse())
        }
        _ => Ok(Value::Undefined),
    }
}

// ── Descendant (**) ─────────────────────────────────────────────────

fn descendant_lookup(input: &Value) -> Value {
    let mut seq = Sequence::new();
    collect_descendants(input, &mut seq);
    seq.collapse()
}

fn collect_descendants(input: &Value, seq: &mut Sequence) {
    match input {
        Value::Object(obj) => {
            for (_, val) in obj {
                if !val.is_undefined() {
                    seq.values.push(val.clone());
                    collect_descendants(val, seq);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_descendants(item, seq);
            }
        }
        _ => {}
    }
}

// ── Regex ───────────────────────────────────────────────────────────

fn eval_regex(pattern: &str, flags: &str) -> Value {
    // For now, return a map with pattern and flags for the regex.
    // Full regex evaluation will be implemented with the stdlib.
    let mut obj = indexmap::IndexMap::new();
    obj.insert("pattern".into(), Value::String(pattern.to_string()));
    obj.insert("flags".into(), Value::String(flags.to_string()));
    Value::Object(obj)
}

// ── Path evaluation (simplified for Phase 5) ────────────────────────

fn eval_path(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (steps, keep_singleton_array) = match arena.get(node) {
        Expr::Path {
            steps,
            keep_singleton_array,
            ..
        } => (steps.clone(), *keep_singleton_array),
        _ => unreachable!(),
    };

    if steps.is_empty() {
        return Ok(Value::Undefined);
    }

    let mut result = eval(arena, steps[0], input, env)?;

    for &step in &steps[1..] {
        if result.is_undefined() {
            return Ok(Value::Undefined);
        }
        result = eval(arena, step, &result, env)?;
    }

    // Apply sequence collapse with keep_singleton_array.
    if keep_singleton_array && let Value::Sequence(mut seq) = result {
        seq.keep_singleton = true;
        return Ok(seq.collapse());
    }

    Ok(result)
}

// ── Binary operators (stub for Phase 5, full impl in Phase 7) ───────

fn eval_binary(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (op, lhs, rhs) = match arena.get(node) {
        Expr::Binary { op, lhs, rhs, .. } => (op.as_str(), *lhs, *rhs),
        _ => unreachable!(),
    };

    match op {
        // Short-circuit operators.
        "and" => {
            let left = eval(arena, lhs, input, env)?;
            if !left.to_boolean() {
                return Ok(Value::Bool(false));
            }
            let right = eval(arena, rhs, input, env)?;
            Ok(Value::Bool(right.to_boolean()))
        }
        "or" => {
            let left = eval(arena, lhs, input, env)?;
            if left.to_boolean() {
                return Ok(left);
            }
            eval(arena, rhs, input, env)
        }
        "?:" => {
            // Elvis: left if defined, else right.
            let left = eval(arena, lhs, input, env)?;
            if !left.is_undefined() {
                Ok(left)
            } else {
                eval(arena, rhs, input, env)
            }
        }
        "??" => {
            // Null-coalescing: left if not undefined and not null.
            let left = eval(arena, lhs, input, env)?;
            if !left.is_undefined() && !left.is_null() {
                Ok(left)
            } else {
                eval(arena, rhs, input, env)
            }
        }
        "~>" => {
            // Chain/pipe operator.
            let piped = eval(arena, lhs, input, env)?;
            eval_chain(arena, rhs, &piped, input, env)
        }
        "[" => {
            // Subscript/filter — evaluate left, then apply right as index or predicate.
            let left = eval(arena, lhs, input, env)?;
            eval_subscript(arena, rhs, &left, input, env)
        }
        // Arithmetic operators.
        "+" | "-" | "*" | "/" | "%" | "**" => eval_arithmetic(arena, op, lhs, rhs, input, env),
        // String concatenation.
        "&" => {
            let left = eval(arena, lhs, input, env)?;
            let right = eval(arena, rhs, input, env)?;
            let ls = if left.is_undefined() {
                String::new()
            } else {
                left.stringify()?
            };
            let rs = if right.is_undefined() {
                String::new()
            } else {
                right.stringify()?
            };
            Ok(Value::String(format!("{ls}{rs}")))
        }
        // Equality.
        "=" => {
            let left = eval(arena, lhs, input, env)?;
            let right = eval(arena, rhs, input, env)?;
            if left.is_undefined() || right.is_undefined() {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(left.deep_equal(&right)))
        }
        "!=" => {
            let left = eval(arena, lhs, input, env)?;
            let right = eval(arena, rhs, input, env)?;
            if left.is_undefined() || right.is_undefined() {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(!left.deep_equal(&right)))
        }
        // Comparison.
        "<" | "<=" | ">" | ">=" => {
            let left = eval(arena, lhs, input, env)?;
            let right = eval(arena, rhs, input, env)?;
            left.compare(&right, op)
        }
        // Membership.
        "in" => {
            let left = eval(arena, lhs, input, env)?;
            let right = eval(arena, rhs, input, env)?;
            Ok(Value::Bool(left.contained_in(&right)))
        }
        // Range.
        ".." => eval_range(arena, lhs, rhs, input, env),
        _ => Err(JsonataError::new(
            "D3001",
            format!("unknown binary operator: {op}"),
        )),
    }
}

fn eval_arithmetic(
    arena: &AstArena,
    op: &str,
    lhs: NodeId,
    rhs: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let left = eval(arena, lhs, input, env)?;
    let right = eval(arena, rhs, input, env)?;
    if left.is_undefined() || right.is_undefined() {
        return Ok(Value::Undefined);
    }
    let ln = left
        .as_f64()
        .ok_or_else(|| JsonataError::new("T2001", "the left operand must be a number"))?;
    let rn = right
        .as_f64()
        .ok_or_else(|| JsonataError::new("T2002", "the right operand must be a number"))?;
    let result = match op {
        "+" => ln + rn,
        "-" => ln - rn,
        "*" => ln * rn,
        "/" => ln / rn,
        "%" => ln % rn,
        "**" => ln.powf(rn),
        _ => unreachable!(),
    };
    Ok(Value::Number(result))
}

fn eval_range(
    arena: &AstArena,
    lhs: NodeId,
    rhs: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let left = eval(arena, lhs, input, env)?;
    let right = eval(arena, rhs, input, env)?;
    let ln = left.as_f64().ok_or_else(|| {
        JsonataError::new(
            "T2003",
            "the left operand of the range operator (..) must be a number",
        )
    })?;
    let rn = right.as_f64().ok_or_else(|| {
        JsonataError::new(
            "T2004",
            "the right operand of the range operator (..) must be a number",
        )
    })?;
    let start = ln.trunc() as i64;
    let end = rn.trunc() as i64;
    if start > end {
        return Ok(Value::Undefined);
    }
    let count = (end - start + 1) as usize;
    if count > 10_000_000 {
        return Err(JsonataError::new("D2014", "range operator too large"));
    }
    let arr: Vec<Value> = (start..=end).map(|i| Value::Number(i as f64)).collect();
    Ok(Value::Array(arr))
}

fn eval_subscript(
    arena: &AstArena,
    rhs: NodeId,
    left: &Value,
    _input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let index = eval(arena, rhs, left, env)?;

    // Numeric index access.
    if let Some(n) = index.as_f64() {
        let idx = n.trunc() as i64;
        let arr = match left {
            Value::Array(a) => a.clone(),
            _ => vec![left.clone()],
        };
        let len = arr.len() as i64;
        let actual = if idx < 0 { len + idx } else { idx };
        if actual < 0 || actual >= len {
            return Ok(Value::Undefined);
        }
        return Ok(arr[actual as usize].clone());
    }

    // Predicate filter — evaluate rhs against each element.
    let arr = match left {
        Value::Array(a) => a.clone(),
        Value::Sequence(s) => s.to_vec(),
        _ => vec![left.clone()],
    };

    let mut seq = Sequence::new();
    for item in arr.iter() {
        let test = eval(arena, rhs, item, env)?;
        // Numeric result = index selection.
        if let Some(n) = test.as_f64() {
            let idx = n.trunc() as i64;
            let len = arr.len() as i64;
            let actual = if idx < 0 { len + idx } else { idx };
            if actual >= 0 && actual < len {
                return Ok(arr[actual as usize].clone());
            }
            return Ok(Value::Undefined);
        }
        // Boolean predicate — keep item if truthy.
        if test.to_boolean() {
            seq.values.push(item.clone());
        }
    }
    Ok(seq.collapse())
}

// ── Chain operator (~>) ─────────────────────────────────────────────

fn eval_chain(
    arena: &AstArena,
    rhs: NodeId,
    piped: &Value,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    // Handle right-associative chaining: a ~> (f ~> g) → (a ~> f) ~> g
    if let Expr::Binary {
        op, lhs, rhs: rr, ..
    } = arena.get(rhs)
        && op == "~>"
    {
        let r1 = eval_chain(arena, *lhs, piped, input, env)?;
        if r1.is_undefined() {
            return Ok(Value::Undefined);
        }
        return eval_chain(arena, *rr, &r1, input, env);
    }

    // Function call node: prepend piped as first argument.
    if let Expr::Function {
        procedure,
        arguments,
        ..
    } = arena.get(rhs)
    {
        let procedure = *procedure;
        let arguments = arguments.clone();
        let fn_val = eval(arena, procedure, input, env)?;
        let func = match fn_val {
            Value::Function(f) => f,
            _ => {
                return Err(JsonataError::new(
                    "T1006",
                    "attempted to invoke undefined function",
                ));
            }
        };
        let mut args = vec![piped.clone()];
        for &arg_node in &arguments {
            if matches!(arena.get(arg_node), Expr::Placeholder { .. }) {
                args.push(Value::Undefined);
                continue;
            }
            args.push(eval(arena, arg_node, input, env)?);
        }
        return call_function(&func, &args, input, env, arena);
    }

    // Otherwise evaluate right side and call it.
    let fn_val = eval(arena, rhs, input, env)?;
    match fn_val {
        Value::Function(func) => {
            call_function(&func, std::slice::from_ref(piped), input, env, arena)
        }
        _ => Err(JsonataError::new(
            "T2006",
            "the right-hand side of the ~> operator must be a function",
        )),
    }
}

// ── Unary operators ─────────────────────────────────────────────────

fn eval_unary(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (op, operand, expressions, lhs_nodes) = match arena.get(node) {
        Expr::Unary {
            op,
            operand,
            expressions,
            lhs,
            ..
        } => (op.clone(), *operand, expressions.clone(), lhs.clone()),
        _ => unreachable!(),
    };

    match op.as_str() {
        "-" => {
            let val = eval(arena, operand, input, env)?;
            if val.is_undefined() {
                return Ok(Value::Undefined);
            }
            let n = val
                .as_f64()
                .ok_or_else(|| JsonataError::new("D1002", "cannot negate a non-numeric value"))?;
            Ok(Value::Number(-n))
        }
        "[" => {
            // Array constructor.
            let mut result = Vec::new();
            for &expr in &expressions {
                let val = eval(arena, expr, input, env)?;
                match val {
                    Value::Undefined => {}
                    Value::Array(arr) => result.push(Value::Array(arr)),
                    Value::Sequence(seq) if seq.cons_array => {
                        result.push(Value::Array(seq.values));
                    }
                    Value::Sequence(seq) => {
                        for v in seq.values {
                            result.push(v);
                        }
                    }
                    other => result.push(other),
                }
            }
            Ok(Value::Array(result))
        }
        "{" => {
            // Object constructor.
            let mut obj = indexmap::IndexMap::new();
            // lhs is flat [k0,v0,k1,v1,...]
            let mut i = 0;
            while i + 1 < lhs_nodes.len() {
                let key_val = eval(arena, lhs_nodes[i], input, env)?;
                let val_val = eval(arena, lhs_nodes[i + 1], input, env)?;
                let key = match key_val {
                    Value::String(s) => s,
                    _ => key_val.stringify()?,
                };
                obj.insert(key, val_val);
                i += 2;
            }
            Ok(Value::Object(obj))
        }
        _ => Err(JsonataError::new(
            "D3001",
            format!("unknown unary operator: {op}"),
        )),
    }
}

// ── Block, condition, bind ──────────────────────────────────────────

fn eval_block(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let expressions = match arena.get(node) {
        Expr::Block { expressions, .. } => expressions.clone(),
        _ => unreachable!(),
    };

    let child_env = Rc::new(Environment::new_child(Rc::clone(env)));
    let mut last = Value::Undefined;
    for &expr in &expressions {
        last = eval(arena, expr, input, &child_env)?;
    }
    Ok(last)
}

fn eval_condition(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (condition, then, else_) = match arena.get(node) {
        Expr::Condition {
            condition,
            then,
            else_,
            ..
        } => (*condition, *then, *else_),
        _ => unreachable!(),
    };

    let cond = eval(arena, condition, input, env)?;
    if cond.to_boolean() {
        eval(arena, then, input, env)
    } else if let Some(e) = else_ {
        eval(arena, e, input, env)
    } else {
        Ok(Value::Undefined)
    }
}

fn eval_bind(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (lhs, rhs) = match arena.get(node) {
        Expr::Bind { lhs, rhs, .. } => (*lhs, *rhs),
        _ => unreachable!(),
    };

    let val = eval(arena, rhs, input, env)?;

    // The LHS is a Variable node — extract the name.
    let name = match arena.get(lhs) {
        Expr::Variable { name, .. } => name.clone(),
        _ => {
            return Err(JsonataError::new("D3001", "bind target must be a variable"));
        }
    };

    // Bind in the current environment.
    // We need interior mutability — but Environment uses HashMap which needs &mut.
    // For now, we'll use a workaround: binds only work in block scopes where
    // the env is freshly created and we hold the only Rc reference.
    if let Some(env_mut) = Rc::get_mut(&mut env.clone()) {
        env_mut.bind(name, val.clone());
    } else {
        // If we can't get exclusive access, we need to use interior mutability.
        // This is a known limitation that will be addressed with RefCell or similar.
        // For now, bindings in shared envs won't work correctly.
        // TODO: Use RefCell<HashMap> for bindings to allow mutation through Rc.
    }

    Ok(val)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Parser, process_ast};

    /// Helper: parse, process, and evaluate.
    fn eval_expr(src: &str, input: &Value) -> JsonataResult {
        let (mut arena, root) = Parser::parse(src).expect("parse failed");
        let root = process_ast(&mut arena, root).expect("process failed");
        let env = Rc::new(Environment::new());
        eval(&arena, root, input, &env)
    }

    fn eval_simple(src: &str) -> Value {
        eval_expr(src, &Value::Undefined).expect("eval failed")
    }

    fn eval_with_data(src: &str, json: &str) -> Value {
        let input = Value::from_json_str(json).expect("invalid JSON");
        eval_expr(src, &input).expect("eval failed")
    }

    // ── Literals ────────────────────────────────────────────────

    #[test]
    fn eval_number() {
        assert_eq!(eval_simple("42"), Value::Number(42.0));
    }

    #[test]
    fn eval_string() {
        assert_eq!(eval_simple(r#""hello""#), Value::String("hello".into()));
    }

    #[test]
    fn eval_true() {
        assert_eq!(eval_simple("true"), Value::Bool(true));
    }

    #[test]
    fn eval_false() {
        assert_eq!(eval_simple("false"), Value::Bool(false));
    }

    #[test]
    fn eval_null() {
        assert_eq!(eval_simple("null"), Value::Null);
    }

    // ── Variables ───────────────────────────────────────────────

    #[test]
    fn bare_dollar_returns_input() {
        let input = Value::Number(99.0);
        let result = eval_expr("$", &input).unwrap();
        assert_eq!(result, Value::Number(99.0));
    }

    #[test]
    fn undefined_variable() {
        assert!(eval_simple("$x").is_undefined());
    }

    // ── Name (field access) ─────────────────────────────────────

    #[test]
    fn field_access_object() {
        assert_eq!(
            eval_with_data("name", r#"{"name": "Alice"}"#),
            Value::String("Alice".into())
        );
    }

    #[test]
    fn field_access_missing() {
        assert!(eval_with_data("age", r#"{"name": "Alice"}"#).is_undefined());
    }

    #[test]
    fn field_access_array_auto_map() {
        let result = eval_with_data("name", r#"[{"name": "A"}, {"name": "B"}]"#);
        assert_eq!(
            result,
            Value::Array(vec![Value::String("A".into()), Value::String("B".into())])
        );
    }

    // ── Path ────────────────────────────────────────────────────

    #[test]
    fn dot_path() {
        assert_eq!(
            eval_with_data("a.b", r#"{"a": {"b": 42}}"#),
            Value::Number(42.0)
        );
    }

    #[test]
    fn deep_path() {
        assert_eq!(
            eval_with_data("a.b.c", r#"{"a": {"b": {"c": "deep"}}}"#),
            Value::String("deep".into())
        );
    }

    // ── Wildcard ────────────────────────────────────────────────

    #[test]
    fn wildcard_object() {
        let result = eval_with_data("*", r#"{"a": 1, "b": 2}"#);
        assert_eq!(
            result,
            Value::Array(vec![Value::Number(1.0), Value::Number(2.0)])
        );
    }

    // ── Arithmetic ──────────────────────────────────────────────

    #[test]
    fn addition() {
        assert_eq!(eval_simple("2 + 3"), Value::Number(5.0));
    }

    #[test]
    fn subtraction() {
        assert_eq!(eval_simple("10 - 4"), Value::Number(6.0));
    }

    #[test]
    fn multiplication() {
        assert_eq!(eval_simple("3 * 7"), Value::Number(21.0));
    }

    #[test]
    fn division() {
        assert_eq!(eval_simple("15 / 3"), Value::Number(5.0));
    }

    #[test]
    fn power() {
        assert_eq!(eval_simple("2 ** 10"), Value::Number(1024.0));
    }

    // ── Comparison ──────────────────────────────────────────────

    #[test]
    fn equals_true() {
        assert_eq!(eval_simple("1 = 1"), Value::Bool(true));
    }

    #[test]
    fn equals_false() {
        assert_eq!(eval_simple("1 = 2"), Value::Bool(false));
    }

    #[test]
    fn not_equals() {
        assert_eq!(eval_simple("1 != 2"), Value::Bool(true));
    }

    #[test]
    fn less_than() {
        assert_eq!(eval_simple("1 < 2"), Value::Bool(true));
    }

    #[test]
    fn greater_than() {
        assert_eq!(eval_simple("2 > 1"), Value::Bool(true));
    }

    // ── Boolean operators ───────────────────────────────────────

    #[test]
    fn and_true() {
        assert_eq!(eval_simple("true and true"), Value::Bool(true));
    }

    #[test]
    fn and_false() {
        assert_eq!(eval_simple("true and false"), Value::Bool(false));
    }

    #[test]
    fn or_true() {
        assert_eq!(eval_simple("false or true"), Value::Bool(true));
    }

    // ── String concatenation ────────────────────────────────────

    #[test]
    fn string_concat() {
        assert_eq!(
            eval_simple(r#""hello" & " " & "world""#),
            Value::String("hello world".into())
        );
    }

    // ── Condition ───────────────────────────────────────────────

    #[test]
    fn ternary_true() {
        assert_eq!(eval_simple("true ? 1 : 2"), Value::Number(1.0));
    }

    #[test]
    fn ternary_false() {
        assert_eq!(eval_simple("false ? 1 : 2"), Value::Number(2.0));
    }

    // ── Block ───────────────────────────────────────────────────

    #[test]
    fn block_returns_last() {
        assert_eq!(eval_simple("(1; 2; 3)"), Value::Number(3.0));
    }

    // ── Variable binding ────────────────────────────────────────

    // Note: bind tests are limited until RefCell is added for env mutation.

    // ── Array constructor ───────────────────────────────────────

    #[test]
    fn array_constructor() {
        assert_eq!(
            eval_simple("[1, 2, 3]"),
            Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0)
            ])
        );
    }

    // ── Object constructor ──────────────────────────────────────

    #[test]
    fn object_constructor() {
        let result = eval_simple(r#"{"a": 1, "b": 2}"#);
        match result {
            Value::Object(obj) => {
                assert_eq!(obj.get("a"), Some(&Value::Number(1.0)));
                assert_eq!(obj.get("b"), Some(&Value::Number(2.0)));
            }
            other => panic!("expected Object, got {:?}", other),
        }
    }

    // ── Range ───────────────────────────────────────────────────

    #[test]
    fn range_operator() {
        assert_eq!(
            eval_simple("[1..5]"),
            Value::Array(vec![Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
                Value::Number(5.0),
            ])])
        );
    }

    // ── Unary minus ─────────────────────────────────────────────

    #[test]
    fn unary_minus() {
        assert_eq!(eval_simple("-5"), Value::Number(-5.0));
    }

    // ── Lambda ──────────────────────────────────────────────────

    #[test]
    fn lambda_creation() {
        let result = eval_simple("function($x){$x}");
        assert!(result.is_function());
    }

    // ── Descendant ──────────────────────────────────────────────

    #[test]
    fn descendant_lookup_test() {
        let result = eval_with_data("**", r#"{"a": {"b": 1}, "c": 2}"#);
        // Should collect all values recursively.
        match result {
            Value::Array(arr) => {
                assert!(arr.len() >= 3); // {"b":1}, 1, 2
            }
            _ => panic!("expected Array, got {:?}", result),
        }
    }

    // ── Membership ──────────────────────────────────────────────

    #[test]
    fn in_operator() {
        assert_eq!(eval_simple("2 in [1, 2, 3]"), Value::Bool(true));
    }

    #[test]
    fn not_in_operator() {
        assert_eq!(eval_simple("4 in [1, 2, 3]"), Value::Bool(false));
    }

    // ── Elvis and null-coalescing ───────────────────────────────

    #[test]
    fn elvis_defined() {
        assert_eq!(eval_simple("42 ?: 0"), Value::Number(42.0));
    }

    #[test]
    fn null_coalescing() {
        assert_eq!(eval_simple("null ?? 0"), Value::Number(0.0));
    }
}
