//! Core JSONata evaluator: tree-walking dispatch over the AST arena.
//!
//! Port of Go `internal/evaluator/evaluator.go` and supporting files.

pub mod environment;
pub mod functions;
pub mod signature;

pub use environment::Environment;
pub use functions::{
    BuiltinFn, EnvAwareBuiltinFn, FunctionValue, Lambda, TailCall, call_function, eval_function,
    eval_lambda, eval_partial,
};
pub use signature::{ParamSpec, parse_signature, process_call_args};

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

    // TODO: pathHasTupleStep check → evalPathTuple (deferred until needed for conformance).
    eval_path_simple(arena, &steps, keep_singleton_array, input, env)
}

/// Simple (non-tuple) path evaluation: thread each step's result into the next.
fn eval_path_simple(
    arena: &AstArena,
    steps: &[NodeId],
    keep_singleton_array: bool,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let mut result = input.clone();
    let mut prev_was_mapper = false;

    for (i, &step) in steps.iter().enumerate() {
        if i > 0 && result.is_undefined() {
            return Ok(Value::Undefined);
        }
        // Collapse sequences between steps.
        if let Value::Sequence(seq) = result {
            result = seq.collapse();
            if i > 0 && result.is_undefined() {
                return Ok(Value::Undefined);
            }
        }

        result = eval_path_step(
            arena,
            step,
            &result,
            env,
            prev_was_mapper,
            keep_singleton_array,
        )?;

        // Empty arrays produced by auto-mapping mean "nothing found" → undefined,
        // UNLESS the previous step was NOT a mapper (the empty array is a genuine field value).
        if let Value::Array(ref arr) = result
            && arr.is_empty()
            && prev_was_mapper
            && !matches!(arena.get(step), Expr::Name { .. } | Expr::StringLit { .. })
        {
            return Ok(Value::Undefined);
        }

        prev_was_mapper = matches!(result, Value::Array(_) | Value::Sequence(_));
    }

    if keep_singleton_array {
        match result {
            Value::Array(_) => return Ok(result),
            Value::Undefined => return Ok(Value::Undefined),
            _ => return Ok(Value::Array(vec![result])),
        }
    }
    Ok(result)
}

/// Evaluate a single path step, handling auto-mapping over arrays.
fn eval_path_step(
    arena: &AstArena,
    step: NodeId,
    input: &Value,
    env: &Rc<Environment>,
    prev_was_mapper: bool,
    keep_singleton_array: bool,
) -> JsonataResult {
    let expr = arena.get(step);

    // Steps that natively handle array inputs (field lookup, wildcard, variable, etc.)
    match expr {
        Expr::NumberLit { raw, .. } => {
            return Err(JsonataError::new(
                "S0213",
                format!("invalid step in path: numeric literal {raw} is not a field name"),
            ));
        }
        Expr::Name { .. }
        | Expr::Wildcard { .. }
        | Expr::Variable { .. }
        | Expr::StringLit { .. }
        | Expr::ValueLit { .. }
        | Expr::Sort { .. } => {
            return eval(arena, step, input, env);
        }
        Expr::Block { .. } if !prev_was_mapper => {
            return eval(arena, step, input, env);
        }
        Expr::Descendant { .. } => {
            // In path context, descendant includes the current node itself.
            let mut seq = Sequence::new();
            if !matches!(input, Value::Array(_)) {
                seq.append(input.clone());
            }
            let descendants = descendant_lookup(input);
            match descendants {
                Value::Array(arr) => {
                    for item in arr {
                        seq.append(item);
                    }
                }
                Value::Sequence(s) => {
                    for item in s.values {
                        seq.append(item);
                    }
                }
                Value::Undefined => {}
                other => seq.append(other),
            }
            return if seq.values.is_empty() {
                Ok(Value::Undefined)
            } else {
                Ok(Value::Sequence(seq))
            };
        }
        Expr::Binary { op, lhs, .. } if op == "[" && !lhs.is_empty() && !prev_was_mapper => {
            return eval(arena, step, input, env);
        }
        _ => {}
    }

    // Array constructor steps not preceded by mapper are literal expressions.
    if let Expr::Unary { op, .. } = expr
        && op == "["
        && !prev_was_mapper
    {
        return eval(arena, step, input, env);
    }

    // For all other step types, map over array input.
    let arr = match input {
        Value::Array(a) => a.clone(),
        _ => {
            // Single item — check for function step with path-element prepend.
            if matches!(expr, Expr::Function { .. }) {
                return eval_path_function_step(arena, step, input, env);
            }
            return eval(arena, step, input, env);
        }
    };

    let is_group_step = matches!(expr, Expr::Unary { op, .. } if op == "[");
    let mut seq = Sequence::new();

    for item in &arr {
        let val = if matches!(arena.get(step), Expr::Function { .. }) {
            eval_path_function_step(arena, step, item, env)?
        } else {
            eval(arena, step, item, env)?
        };
        if val.is_undefined() {
            continue;
        }
        if is_group_step {
            seq.values.push(val);
            continue;
        }
        match val {
            Value::Array(inner) => seq.values.extend(inner),
            Value::Sequence(s) => seq.values.extend(s.values),
            other => seq.append(other),
        }
    }

    if seq.values.is_empty() {
        return Ok(Value::Undefined);
    }
    if is_group_step && keep_singleton_array {
        return Ok(Value::Array(seq.values));
    }
    Ok(seq.collapse())
}

/// Evaluate a function call step in path context.
/// For lambdas, prepend the path element as the first argument.
fn eval_path_function_step(
    arena: &AstArena,
    step: NodeId,
    item: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (procedure, arguments) = match arena.get(step) {
        Expr::Function {
            procedure,
            arguments,
            ..
        } => (*procedure, arguments.clone()),
        _ => return eval(arena, step, item, env),
    };

    let fn_val = eval(arena, procedure, item, env)?;
    let func = match &fn_val {
        Value::Function(f) => f.clone(),
        _ => {
            return Err(JsonataError::new(
                "T1006",
                "attempted to invoke undefined function",
            ));
        }
    };

    let mut args = Vec::with_capacity(arguments.len());
    for &arg_node in &arguments {
        if matches!(arena.get(arg_node), Expr::Placeholder { .. }) {
            args.push(Value::Undefined);
            continue;
        }
        args.push(eval(arena, arg_node, item, env)?);
    }

    // For lambdas, prepend path element when fewer args than params.
    if let FunctionValue::Lambda(ref lam) = func
        && args.len() < lam.params.len()
    {
        args.insert(0, item.clone());
    }

    call_function(&func, &args, item, env, arena)
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
    // For non-array inputs, evaluate directly.
    if !matches!(left, Value::Array(_) | Value::Sequence(_)) {
        let index = eval(arena, rhs, left, env)?;
        if let Some(n) = index.as_f64() {
            // Numeric index on a single value — treat as array of one.
            let idx = n.trunc() as i64;
            if idx == 0 || idx == -1 {
                return Ok(left.clone());
            }
            return Ok(Value::Undefined);
        }
        // Boolean predicate on single value.
        if index.to_boolean() {
            return Ok(left.clone());
        }
        return Ok(Value::Undefined);
    }

    let arr = match left {
        Value::Array(a) => a.clone(),
        Value::Sequence(s) => s.to_vec(),
        _ => unreachable!(),
    };

    // Try evaluating RHS as a simple expression (might be a numeric literal or
    // variable). If it resolves to a number, use it as a direct index.
    // If it errors or is non-numeric, fall through to per-element predicate filter.
    if let Ok(index) = eval(arena, rhs, left, env)
        && let Some(n) = index.as_f64()
    {
        let idx = n.trunc() as i64;
        let len = arr.len() as i64;
        let actual = if idx < 0 { len + idx } else { idx };
        if actual < 0 || actual >= len {
            return Ok(Value::Undefined);
        }
        return Ok(arr[actual as usize].clone());
    }

    // Predicate filter — evaluate rhs against each element.
    let mut seq = Sequence::new();
    for item in arr.iter() {
        let test = eval(arena, rhs, item, env)?;
        // Numeric result = index selection from entire array.
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
        let mut env = Environment::new();
        crate::stdlib::register_all(&mut env);
        let env = Rc::new(env);
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

    #[test]
    fn path_auto_mapping() {
        // Field access on array of objects maps across elements.
        let result = eval_with_data(
            "Account.Order.Product.Price",
            r#"{"Account": {"Order": [{"Product": {"Price": 10}}, {"Product": {"Price": 20}}]}}"#,
        );
        assert_eq!(
            result,
            Value::Array(vec![Value::Number(10.0), Value::Number(20.0)])
        );
    }

    #[test]
    fn path_string_in_dot() {
        // String literal in path is a field name lookup.
        assert_eq!(
            eval_with_data(r#"a."b c""#, r#"{"a": {"b c": 99}}"#),
            Value::Number(99.0)
        );
    }

    #[test]
    fn path_undefined_short_circuits() {
        // If any intermediate step is undefined, the whole path is undefined.
        assert!(eval_with_data("a.b.c", r#"{"a": {"x": 1}}"#).is_undefined());
    }

    #[test]
    fn path_subscript_filter() {
        let result = eval_with_data("nums[$ > 2]", r#"{"nums": [1, 2, 3, 4]}"#);
        assert_eq!(
            result,
            Value::Array(vec![Value::Number(3.0), Value::Number(4.0)])
        );
    }

    #[test]
    fn path_subscript_index() {
        assert_eq!(
            eval_with_data("items[0]", r#"{"items": ["a", "b", "c"]}"#),
            Value::String("a".into())
        );
    }

    #[test]
    fn path_subscript_negative_index() {
        assert_eq!(
            eval_with_data("items[-1]", r#"{"items": ["a", "b", "c"]}"#),
            Value::String("c".into())
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

    // ── Stdlib functions ────────────────────────────────────────

    #[test]
    fn stdlib_string() {
        assert_eq!(eval_simple(r#"$string(42)"#), Value::String("42".into()));
    }

    #[test]
    fn stdlib_length() {
        assert_eq!(eval_simple(r#"$length("hello")"#), Value::Number(5.0));
    }

    #[test]
    fn stdlib_uppercase() {
        assert_eq!(
            eval_simple(r#"$uppercase("hello")"#),
            Value::String("HELLO".into())
        );
    }

    #[test]
    fn stdlib_lowercase() {
        assert_eq!(
            eval_simple(r#"$lowercase("HELLO")"#),
            Value::String("hello".into())
        );
    }

    #[test]
    fn stdlib_trim() {
        assert_eq!(
            eval_simple(r#"$trim("  hello   world  ")"#),
            Value::String("hello world".into())
        );
    }

    #[test]
    fn stdlib_substring() {
        assert_eq!(
            eval_simple(r#"$substring("hello", 1, 3)"#),
            Value::String("ell".into())
        );
    }

    #[test]
    fn stdlib_contains() {
        assert_eq!(
            eval_simple(r#"$contains("hello world", "world")"#),
            Value::Bool(true)
        );
    }

    #[test]
    fn stdlib_split() {
        assert_eq!(
            eval_simple(r#"$split("a,b,c", ",")"#),
            Value::Array(vec![
                Value::String("a".into()),
                Value::String("b".into()),
                Value::String("c".into()),
            ])
        );
    }

    #[test]
    fn stdlib_join() {
        assert_eq!(
            eval_simple(r#"$join(["a", "b", "c"], "-")"#),
            Value::String("a-b-c".into())
        );
    }

    #[test]
    fn stdlib_number() {
        assert_eq!(eval_simple(r#"$number("42")"#), Value::Number(42.0));
    }

    #[test]
    fn stdlib_abs() {
        assert_eq!(eval_simple("$abs(-5)"), Value::Number(5.0));
    }

    #[test]
    fn stdlib_floor() {
        assert_eq!(eval_simple("$floor(3.7)"), Value::Number(3.0));
    }

    #[test]
    fn stdlib_ceil() {
        assert_eq!(eval_simple("$ceil(3.2)"), Value::Number(4.0));
    }

    #[test]
    fn stdlib_round() {
        assert_eq!(eval_simple("$round(3.456, 2)"), Value::Number(3.46));
    }

    #[test]
    fn stdlib_sum() {
        assert_eq!(eval_simple("$sum([1, 2, 3])"), Value::Number(6.0));
    }

    #[test]
    fn stdlib_max() {
        assert_eq!(eval_simple("$max([1, 5, 3])"), Value::Number(5.0));
    }

    #[test]
    fn stdlib_min() {
        assert_eq!(eval_simple("$min([1, 5, 3])"), Value::Number(1.0));
    }

    #[test]
    fn stdlib_average() {
        assert_eq!(eval_simple("$average([2, 4, 6])"), Value::Number(4.0));
    }

    #[test]
    fn stdlib_count() {
        assert_eq!(eval_simple("$count([1, 2, 3])"), Value::Number(3.0));
    }

    #[test]
    fn stdlib_append() {
        assert_eq!(
            eval_simple("$append([1, 2], [3, 4])"),
            Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
            ])
        );
    }

    #[test]
    fn stdlib_reverse() {
        assert_eq!(
            eval_simple("$reverse([1, 2, 3])"),
            Value::Array(vec![
                Value::Number(3.0),
                Value::Number(2.0),
                Value::Number(1.0),
            ])
        );
    }

    #[test]
    fn stdlib_keys() {
        let result = eval_with_data("$keys($)", r#"{"a": 1, "b": 2}"#);
        assert_eq!(
            result,
            Value::Array(vec![Value::String("a".into()), Value::String("b".into()),])
        );
    }

    #[test]
    fn stdlib_values() {
        let result = eval_with_data("$values($)", r#"{"a": 1, "b": 2}"#);
        assert_eq!(
            result,
            Value::Array(vec![Value::Number(1.0), Value::Number(2.0)])
        );
    }

    #[test]
    fn stdlib_boolean() {
        assert_eq!(eval_simple("$boolean(1)"), Value::Bool(true));
        assert_eq!(eval_simple("$boolean(0)"), Value::Bool(false));
        assert_eq!(eval_simple(r#"$boolean("")"#), Value::Bool(false));
    }

    #[test]
    fn stdlib_not() {
        assert_eq!(eval_simple("$not(true)"), Value::Bool(false));
        assert_eq!(eval_simple("$not(false)"), Value::Bool(true));
    }

    #[test]
    fn stdlib_exists() {
        assert_eq!(eval_simple("$exists(42)"), Value::Bool(true));
        assert_eq!(eval_simple("$exists($nothing)"), Value::Bool(false));
    }

    #[test]
    fn stdlib_type() {
        assert_eq!(eval_simple(r#"$type(42)"#), Value::String("number".into()));
        assert_eq!(
            eval_simple(r#"$type("hi")"#),
            Value::String("string".into())
        );
        assert_eq!(
            eval_simple(r#"$type(true)"#),
            Value::String("boolean".into())
        );
    }

    #[test]
    fn stdlib_map() {
        assert_eq!(
            eval_simple("$map([1, 2, 3], function($v){$v * 2})"),
            Value::Array(vec![
                Value::Number(2.0),
                Value::Number(4.0),
                Value::Number(6.0),
            ])
        );
    }

    #[test]
    fn stdlib_filter() {
        assert_eq!(
            eval_simple("$filter([1, 2, 3, 4], function($v){$v > 2})"),
            Value::Array(vec![Value::Number(3.0), Value::Number(4.0)])
        );
    }

    #[test]
    fn stdlib_reduce() {
        assert_eq!(
            eval_simple("$reduce([1, 2, 3], function($prev, $curr){$prev + $curr})"),
            Value::Number(6.0)
        );
    }

    #[test]
    fn stdlib_sort_default() {
        assert_eq!(
            eval_simple("$sort([3, 1, 2])"),
            Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
            ])
        );
    }

    #[test]
    fn stdlib_distinct() {
        assert_eq!(
            eval_simple("$distinct([1, 2, 2, 3, 1])"),
            Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
            ])
        );
    }

    #[test]
    fn stdlib_merge() {
        let result = eval_simple(r#"$merge([{"a": 1}, {"b": 2}])"#);
        match result {
            Value::Object(obj) => {
                assert_eq!(obj.get("a"), Some(&Value::Number(1.0)));
                assert_eq!(obj.get("b"), Some(&Value::Number(2.0)));
            }
            other => panic!("expected Object, got {:?}", other),
        }
    }

    #[test]
    fn stdlib_flatten() {
        assert_eq!(
            eval_simple("$flatten([[1, 2], [3, [4, 5]]])"),
            Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
                Value::Number(5.0),
            ])
        );
    }

    #[test]
    fn stdlib_base64() {
        assert_eq!(
            eval_simple(r#"$base64encode("hello")"#),
            Value::String("aGVsbG8=".into())
        );
        assert_eq!(
            eval_simple(r#"$base64decode("aGVsbG8=")"#),
            Value::String("hello".into())
        );
    }
}
