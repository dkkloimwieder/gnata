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
///
/// # Errors
/// Returns JSONata-spec error codes for type mismatches, undefined references,
/// stack overflows, cancellation, and other evaluation failures.
/// Public eval entry point. Checks stack on first call, then dispatches
/// to eval_inner which is used for all internal recursive calls (no stack check overhead).
pub fn eval(arena: &AstArena, node: NodeId, input: &Value, env: &Rc<Environment>) -> JsonataResult {
    stacker::maybe_grow(128 * 1024, 1024 * 1024, || {
        eval_inner(arena, node, input, env)
    })
}

/// Fast internal eval — no stack check. Used for all recursive calls within
/// the evaluator. Stack growth is handled at deep-recursion entry points
/// (call_function for lambda bodies).
#[inline(always)]
pub(crate) fn eval_fast_inner(arena: &AstArena, node: NodeId, input: &Value, env: &Rc<Environment>) -> JsonataResult {
    eval_inner(arena, node, input, env)
}

/// Check remaining stack and grow if needed. Called from deep-recursion
/// entry points (call_function lambda body, etc.).
pub(crate) fn eval_with_stack_check(arena: &AstArena, node: NodeId, input: &Value, env: &Rc<Environment>) -> JsonataResult {
    stacker::maybe_grow(128 * 1024, 1024 * 1024, || {
        eval_inner(arena, node, input, env)
    })
}

#[allow(clippy::too_many_lines, clippy::needless_continue)]
fn eval_inner(arena: &AstArena, node: NodeId, input: &Value, env: &Rc<Environment>) -> JsonataResult {
    // Iterative evaluation loop with tail-call optimization.
    // Tail positions (Block last expr, Condition then/else, Binary ?:/??/~>)
    // update `cur_node`/`cur_env` and continue the loop instead of recursing.
    let mut cur_node = node;
    let mut cur_env = Rc::clone(env);

    loop {
        if cur_node.is_empty() {
            return Ok(Value::Undefined);
        }

        // Check cancellation at every expression boundary.
        if cur_env.is_cancelled() {
            return Err(JsonataError::new("D3001", "evaluation cancelled"));
        }

        // If the node has a Group expression, evaluate the base node first,
        // then apply group-by reduction.
        // Exception: Path nodes with tuple steps (#$var) handle groups internally.
        match arena.get(cur_node) {
            Expr::Path {
                group: Some(_),
                steps,
                ..
            }
                if !path_has_tuple_step(arena, steps) => {
                    return eval_group_by(arena, cur_node, input, &cur_env);
                }
            Expr::Name { group: Some(_), .. }
            | Expr::Variable { group: Some(_), .. }
            | Expr::Function { group: Some(_), .. } => {
                return eval_group_by(arena, cur_node, input, &cur_env);
            }
            _ => {}
        }

        match arena.get(cur_node) {
            // ── Leaf nodes ──
            Expr::ValueLit { value, .. } => return eval_value_lit(value),
            Expr::StringLit { value, .. } => return Ok(Value::String(value.clone().into())),
            Expr::NumberLit { value: n, .. } => return Ok(Value::Number(*n)),
            Expr::Variable { name, .. } => return eval_variable(name, input, &cur_env),
            Expr::Name { value, .. } => return eval_name(value, input),
            Expr::Wildcard { .. } => return eval_wildcard(input),
            Expr::Descendant { .. } => {
                let result = descendant_lookup(input);
                return match result {
                    Value::Sequence(seq) => Ok(seq.collapse()),
                    other => Ok(other),
                };
            }
            Expr::Regex { pattern, flags, .. } => return Ok(eval_regex(pattern, flags)),
            Expr::Parent { .. } => {
                return match cur_env.lookup("%%") {
                    Some(val) if !val.is_null() && !val.is_undefined() => Ok(val),
                    _ => Err(JsonataError::new(
                        "S0217",
                        "% operator used outside of a valid path context",
                    )),
                };
            }
            Expr::Placeholder { .. } => return Ok(Value::Undefined),

            // ── Tail-call optimized: Block ──
            // Evaluate all but last expression, then loop for last.
            Expr::Block { expressions, .. } => {
                let expressions = expressions.clone();
                let child_env = Rc::new(Environment::new_child(Rc::clone(&cur_env)));
                if expressions.is_empty() {
                    return Ok(Value::Undefined);
                }
                for &expr in &expressions[..expressions.len() - 1] {
                    eval_fast_inner(arena, expr, input, &child_env)?;
                }
                cur_node = expressions[expressions.len() - 1];
                cur_env = child_env;
                continue;
            }

            // ── Tail-call optimized: Condition ──
            // Evaluate condition, then loop for the chosen branch.
            Expr::Condition {
                condition,
                then,
                else_,
                ..
            } => {
                let (cond_id, then_id, else_id) = (*condition, *then, *else_);
                let cond_val = eval_fast_inner(arena, cond_id, input, &cur_env)?;
                if cond_val.to_boolean() {
                    cur_node = then_id;
                    continue;
                } else if let Some(e) = else_id {
                    cur_node = e;
                    continue;
                }
                return Ok(Value::Undefined);
            }

            // ── Tail-call optimized: Binary ?:, ??, ~> ──
            Expr::Binary { op, lhs, rhs, .. } if op == "?:" || op == "??" || op == "~>" => {
                let (op, lhs, rhs) = (op.clone(), *lhs, *rhs);
                match op.as_str() {
                    "?:" => {
                        let left = eval_fast_inner(arena, lhs, input, &cur_env)?;
                        if left.to_boolean() {
                            return Ok(left);
                        }
                        cur_node = rhs;
                        continue;
                    }
                    "??" => {
                        let left = eval_fast_inner(arena, lhs, input, &cur_env)?;
                        if !left.is_undefined() {
                            return Ok(left);
                        }
                        cur_node = rhs;
                        continue;
                    }
                    "~>" => {
                        let piped = eval_fast_inner(arena, lhs, input, &cur_env)?;
                        return eval_chain(arena, rhs, &piped, input, &cur_env);
                    }
                    _ => unreachable!(),
                }
            }

            // ── Non-tail dispatch ──
            Expr::Path { .. } => return eval_path(arena, cur_node, input, &cur_env),
            Expr::Binary { .. } => return eval_binary(arena, cur_node, input, &cur_env),
            Expr::Unary { .. } => return eval_unary(arena, cur_node, input, &cur_env),
            Expr::Bind { .. } => return eval_bind(arena, cur_node, input, &cur_env),
            Expr::Function { .. } => return eval_function(arena, cur_node, input, &cur_env),
            Expr::Lambda { .. } => return eval_lambda(arena, cur_node, input, &cur_env),
            Expr::Partial { .. } => return eval_partial(arena, cur_node, input, &cur_env),
            Expr::Sort { .. } => return eval_sort(arena, cur_node, input, &cur_env),
            Expr::Transform { .. } => return eval_transform(arena, cur_node, input, &cur_env),
        }
    }
}

/// Public API for calling any function value with given args.
/// Used by standard library functions to dispatch callbacks.
///
/// # Errors
/// Returns any error produced by the callee function.
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

// Consistent return type with eval dispatch table.
#[allow(clippy::unnecessary_wraps)]
fn eval_value_lit(value: &str) -> JsonataResult {
    match value {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "null" => Ok(Value::Null),
        _ => Ok(Value::Undefined),
    }
}

// Consistent return type with eval dispatch table.
#[allow(clippy::unnecessary_wraps)]
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
            for item in arr.iter() {
                let val = eval_name(name, item)?;
                if val.is_undefined() {
                    continue;
                }
                field_found = true;
                // Flatten plain arrays from navigating through arrays.
                match val {
                    Value::Array(inner) => {
                        for sv in inner.iter() {
                            seq.values
                                .push(if sv.is_undefined() { Value::Null } else { sv.clone() });
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
                    return Ok(Value::Array(Rc::new(vec![])));
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
            for (_, val) in obj.iter() {
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
            for item in arr.iter() {
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
    // Return as Sequence (not collapsed) so that appendToSequence callers
    // can flatten properly. collapse() is called by the caller when needed.
    Value::Sequence(seq)
}

/// Recursively collect all values at all depths from objects and arrays.
///
/// Matches Go's `descendantLookup`: for objects, each field value is added and
/// recursed into (with arrays expanded to individual items). For arrays, each
/// element is added and recursed into.
fn collect_descendants(input: &Value, seq: &mut Sequence) {
    match input {
        Value::Object(obj) => {
            for (_, val) in obj.iter() {
                if val.is_undefined() {
                    continue;
                }
                // When a field value is an array, iterate its elements directly
                // (add each + recurse), matching Go's descendantLookup which
                // treats arrays as transparent containers.
                if let Value::Array(arr) = val {
                    for item in arr.iter() {
                        seq.append(item.clone());
                        collect_descendants(item, seq);
                    }
                } else {
                    seq.append(val.clone());
                    collect_descendants(val, seq);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter() {
                seq.append(item.clone());
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
    obj.insert("pattern".into(), Value::String(pattern.into()));
    obj.insert("flags".into(), Value::String(flags.into()));
    Value::Object(Rc::new(obj))
}

// ── Path evaluation (simplified for Phase 5) ────────────────────────

fn eval_path(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (steps, keep_singleton_array, group) = match arena.get(node) {
        Expr::Path {
            steps,
            keep_singleton_array,
            group,
            ..
        } => (steps.clone(), *keep_singleton_array, group.clone()),
        _ => unreachable!(),
    };

    if steps.is_empty() {
        return Ok(Value::Undefined);
    }

    if path_has_tuple_step(arena, &steps) {
        return eval_path_tuple(
            arena,
            &steps,
            keep_singleton_array,
            group.as_ref(),
            input,
            env,
        );
    }
    eval_path_simple(arena, &steps, keep_singleton_array, input, env)
}

/// Check whether any path step requires tuple-aware evaluation (#$var index bindings, @$var focus, or % parent refs).
fn path_has_tuple_step(arena: &AstArena, steps: &[NodeId]) -> bool {
    for &step in steps {
        // Direct check: step itself has index or focus.
        match arena.get(step) {
            Expr::Name { index: Some(_), .. } | Expr::Name { focus: Some(_), .. } => return true,
            Expr::Variable { index: Some(_), .. } | Expr::Variable { focus: Some(_), .. } => {
                return true;
            }
            Expr::Sort { index: Some(_), .. } | Expr::Sort { focus: Some(_), .. } => {
                return true;
            }
            // A subscript step whose left child has an Index or Focus binding also requires
            // tuple-aware path evaluation so each element gets its own env for $pos/$var.
            Expr::Binary { op, lhs, .. } if op == "[" && !lhs.is_empty() => {
                let lhs = *lhs;
                match arena.get(lhs) {
                    Expr::Name { index: Some(_), .. }
                    | Expr::Name { focus: Some(_), .. }
                    | Expr::Variable { index: Some(_), .. }
                    | Expr::Variable { focus: Some(_), .. }
                    | Expr::Sort { index: Some(_), .. }
                    | Expr::Sort { focus: Some(_), .. } => {
                        return true;
                    }
                    // Also check for nested binary (e.g., books@$b[pred][1]).
                    Expr::Binary {
                        op: inner_op,
                        lhs: inner_lhs,
                        ..
                    } if inner_op == "[" && !inner_lhs.is_empty() => {
                        let inner_lhs = *inner_lhs;
                        if matches!(
                            arena.get(inner_lhs),
                            Expr::Name { focus: Some(_), .. }
                                | Expr::Name { index: Some(_), .. }
                                | Expr::Variable { focus: Some(_), .. }
                                | Expr::Variable { index: Some(_), .. }
                        ) {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        // Recursive check for deeply nested bindings.
        if node_has_index_binding(arena, step) {
            return true;
        }
        // Any step that references % (Parent) requires parent-chain tracking.
        if node_has_parent_ref(arena, step) {
            return true;
        }
    }
    false
}

/// Recursively check if a node or its sub-expression contains an index (#$var) or focus (@$var) binding.
fn node_has_index_binding(arena: &AstArena, node: NodeId) -> bool {
    match arena.get(node) {
        Expr::Name { index: Some(_), .. } | Expr::Name { focus: Some(_), .. } => true,
        Expr::Variable { index: Some(_), .. } | Expr::Variable { focus: Some(_), .. } => true,
        Expr::Binary { index: Some(_), .. } | Expr::Binary { focus: Some(_), .. } => true,
        Expr::Sort { index: Some(_), .. } | Expr::Sort { focus: Some(_), .. } => true,
        Expr::Binary { op, lhs, rhs, .. } if op == "[" => {
            let lhs = *lhs;
            let rhs = *rhs;
            node_has_index_binding(arena, lhs) || node_has_index_binding(arena, rhs)
        }
        Expr::Sort { expr, .. } => {
            // Check the expression inside the sort.
            node_has_index_binding(arena, *expr)
        }
        Expr::Path { steps, .. } => steps.iter().any(|&s| node_has_index_binding(arena, s)),
        _ => false,
    }
}

/// Recursively check if an AST node or any of its descendants is a Parent (%) reference.
// Large recursive match over all AST variants.
#[allow(clippy::too_many_lines)]
fn node_has_parent_ref(arena: &AstArena, node: NodeId) -> bool {
    if node.is_empty() {
        return false;
    }
    match arena.get(node) {
        Expr::Parent { .. } => true,
        Expr::Name { stages, group, .. } => {
            stages.iter().any(|s| match &s.kind {
                crate::parser::StageKind::Filter { expression } => {
                    node_has_parent_ref(arena, *expression)
                }
                crate::parser::StageKind::Index { .. } => false,
            }) || group_has_parent_ref(arena, group.as_ref())
        }
        Expr::Binary { lhs, rhs, .. } => {
            node_has_parent_ref(arena, *lhs) || node_has_parent_ref(arena, *rhs)
        }
        Expr::Unary {
            operand,
            expressions,
            lhs,
            group,
            ..
        } => {
            let operand = *operand;
            let expressions = expressions.clone();
            let lhs = lhs.clone();
            let group = group.clone();
            node_has_parent_ref(arena, operand)
                || expressions.iter().any(|&e| node_has_parent_ref(arena, e))
                || lhs.iter().any(|&e| node_has_parent_ref(arena, e))
                || group_has_parent_ref(arena, group.as_ref())
        }
        Expr::Path { steps, group, .. } => {
            let steps = steps.clone();
            let group = group.clone();
            steps.iter().any(|&s| node_has_parent_ref(arena, s))
                || group_has_parent_ref(arena, group.as_ref())
        }
        Expr::Block { expressions, .. } => {
            let expressions = expressions.clone();
            expressions.iter().any(|&e| node_has_parent_ref(arena, e))
        }
        Expr::Condition {
            condition,
            then,
            else_,
            ..
        } => {
            let (condition, then, else_) = (*condition, *then, *else_);
            node_has_parent_ref(arena, condition)
                || node_has_parent_ref(arena, then)
                || else_.is_some_and(|e| node_has_parent_ref(arena, e))
        }
        Expr::Function {
            procedure,
            arguments,
            ..
        } => {
            let procedure = *procedure;
            let arguments = arguments.clone();
            node_has_parent_ref(arena, procedure)
                || arguments.iter().any(|&a| node_has_parent_ref(arena, a))
        }
        Expr::Lambda { body, .. } => {
            let body = *body;
            node_has_parent_ref(arena, body)
        }
        Expr::Sort { expr, terms, .. } => {
            let expr = *expr;
            let terms = terms.clone();
            node_has_parent_ref(arena, expr)
                || terms
                    .iter()
                    .any(|t| node_has_parent_ref(arena, t.expression))
        }
        Expr::Bind { lhs, rhs, .. } => {
            let (lhs, rhs) = (*lhs, *rhs);
            node_has_parent_ref(arena, lhs) || node_has_parent_ref(arena, rhs)
        }
        Expr::Transform {
            pattern,
            update,
            delete,
            ..
        } => {
            let (pattern, update, delete) = (*pattern, *update, *delete);
            node_has_parent_ref(arena, pattern)
                || node_has_parent_ref(arena, update)
                || delete.is_some_and(|d| node_has_parent_ref(arena, d))
        }
        Expr::Partial {
            procedure,
            arguments,
            ..
        } => {
            let procedure = *procedure;
            let arguments = arguments.clone();
            node_has_parent_ref(arena, procedure)
                || arguments.iter().any(|&a| node_has_parent_ref(arena, a))
        }
        // Leaf nodes that can't contain % references.
        Expr::StringLit { .. }
        | Expr::NumberLit { .. }
        | Expr::ValueLit { .. }
        | Expr::Variable { .. }
        | Expr::Wildcard { .. }
        | Expr::Descendant { .. }
        | Expr::Regex { .. }
        | Expr::Placeholder { .. } => false,
    }
}

/// Check if a GroupExpr contains any % (Parent) references in its key/value expressions.
fn group_has_parent_ref(arena: &AstArena, group: Option<&crate::parser::GroupExpr>) -> bool {
    match group {
        Some(grp) => grp
            .pairs
            .iter()
            .any(|pair| node_has_parent_ref(arena, pair[0]) || node_has_parent_ref(arena, pair[1])),
        None => false,
    }
}

/// Extract a step-level group from a node, if it has one.
fn extract_step_group(arena: &AstArena, step: NodeId) -> Option<crate::parser::GroupExpr> {
    match arena.get(step) {
        Expr::Name { group, .. } => group.clone(),
        Expr::Variable { group, .. } => group.clone(),
        Expr::Function { group, .. } => group.clone(),
        _ => None,
    }
}

/// Evaluate a path step, bypassing any group expression attached to it.
/// This is used in tuple-aware evaluation where groups are applied at the end.
fn eval_path_step_no_group(
    arena: &AstArena,
    step: NodeId,
    input: &Value,
    env: &Rc<Environment>,
    prev_was_mapper: bool,
    keep_singleton_array: bool,
) -> JsonataResult {
    // Check if step has a group; if so, evaluate the step WITHOUT the group.
    let has_group = matches!(
        arena.get(step),
        Expr::Name { group: Some(_), .. }
            | Expr::Variable { group: Some(_), .. }
            | Expr::Function { group: Some(_), .. }
    );
    if !has_group {
        return eval_path_step(
            arena,
            step,
            input,
            env,
            prev_was_mapper,
            keep_singleton_array,
        );
    }

    // For Name nodes with groups, evaluate as a plain name lookup.
    match arena.get(step) {
        Expr::Name { value, .. } => eval_name(value, input),
        Expr::Variable { name, .. } => eval_variable(name, input, env),
        _ => eval_path_step(
            arena,
            step,
            input,
            env,
            prev_was_mapper,
            keep_singleton_array,
        ),
    }
}

/// Tuple-aware path evaluation for paths containing #$var index bindings or % parent refs.
///
/// Maintains a list of (value, env) contexts so that position variables bound
/// at one step remain accessible in all subsequent steps.
// Large dispatch function handling all tuple-aware path step types.
#[allow(clippy::too_many_lines)]
fn eval_path_tuple(
    arena: &AstArena,
    steps: &[NodeId],
    keep_singleton_array: bool,
    group: Option<&crate::parser::GroupExpr>,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    // Each context pairs a value with the env it was produced under.
    let mut ctxs: Vec<(Value, Rc<Environment>)> = vec![(input.clone(), env.clone())];

    // Detect the first step-level group expression; it will be applied at the
    // end via eval_tuple_group instead of during per-element evaluation.
    let mut final_group: Option<crate::parser::GroupExpr> = None;
    for &step in steps {
        if final_group.is_none()
            && let Some(grp) = extract_step_group(arena, step) {
                final_group = Some(grp);
            }
    }

    for (step_idx, &step) in steps.iter().enumerate() {
        let mut next_ctxs: Vec<(Value, Rc<Environment>)> = Vec::new();

        // Sort steps must be applied globally to all tuples simultaneously.
        if let Expr::Sort { expr, terms, .. } = arena.get(step) {
            let expr = *expr;
            let terms = terms.clone();
            // Match Go's evalTupleSort: if the sort has a non-variable inner expression
            // (needsNavigation), expand it in tuple mode so parent bindings are preserved.
            let needs_navigation =
                !expr.is_empty() && !matches!(arena.get(expr), Expr::Variable { .. });
            if needs_navigation {
                // Evaluate the inner expression as a tuple path.
                let mut inner_ctxs: Vec<(Value, Rc<Environment>)> = Vec::new();
                for (val, ctx_env) in &ctxs {
                    // Extract inner path steps or evaluate the expression.
                    if let Expr::Path {
                        steps: inner_steps, ..
                    } = arena.get(expr)
                    {
                        let inner_steps = inner_steps.clone();
                        let expanded = expand_path_tuple(
                            arena,
                            &inner_steps,
                            &[(val.clone(), ctx_env.clone())],
                        )?;
                        inner_ctxs.extend(expanded);
                    } else {
                        // Non-path inner expression — evaluate and bind %% for parent context.
                        let result = eval_fast_inner(arena, expr, val, ctx_env)?;
                        if !result.is_undefined() {
                            let items = flatten_to_vec(result);
                            let (index_var, focus_var) = get_step_bindings(arena, expr);
                            let is_join = focus_var.is_some();
                            for (j, elem) in items.iter().enumerate() {
                                let child_env = Environment::new_child(Rc::clone(ctx_env));
                                child_env.bind("%%".into(), val.clone());
                                if is_join {
                                    child_env.bind("%%j".into(), Value::Bool(true));
                                }
                                if let Some(ref var_name) = index_var {
                                    child_env.bind(var_name.clone(), Value::Number(j as f64));
                                }
                                if let Some(ref var_name) = focus_var {
                                    child_env.bind(var_name.clone(), elem.clone());
                                }
                                let ctx_value = if is_join { val.clone() } else { elem.clone() };
                                inner_ctxs.push((ctx_value, Rc::new(child_env)));
                            }
                        }
                    }
                }
                ctxs = inner_ctxs;
            }
            // Now sort the tuples.
            let mut arr: Vec<(Value, Rc<Environment>)> = ctxs;
            let mut sort_err: Option<JsonataError> = None;
            arr.sort_by(|a, b| {
                if sort_err.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                match compare_sort_terms(arena, &terms, &a.0, &b.0, &a.1, &b.1) {
                    Ok(cmp) => cmp.cmp(&0),
                    Err(e) => {
                        sort_err = Some(e);
                        std::cmp::Ordering::Equal
                    }
                }
            });
            if let Some(e) = sort_err {
                return Err(e);
            }
            ctxs = arr;
            continue;
        }

        // Parent (%) steps navigate up the parent chain using the %% env bindings
        // set by append_tuple_results. The parent value is retrieved via %%, and the
        // new env is the parent of the binding env so chained %.% walks upward.
        if matches!(arena.get(step), Expr::Parent { .. }) {
            for (_, ctx_env) in &ctxs {
                if let Some((parent_val, binding_env)) = Environment::lookup_with_env(ctx_env, "%%")
                {
                    // In Go, nil parent means "no valid parent context" → S0217.
                    if parent_val.is_null() || parent_val.is_undefined() {
                        return Err(JsonataError::new(
                            "S0217",
                            "% operator used outside of a valid path context",
                        ));
                    }
                    // Use the binding env's parent so chained %.% walks up correctly.
                    let mut parent_env = binding_env
                        .parent()
                        .cloned()
                        .unwrap_or_else(|| binding_env.clone());
                    // When the current binding was made by a join step, skip
                    // through any ancestor envs that are ALSO join bindings with
                    // the same parent value.
                    if binding_env.lookup_direct("%%j").is_some() {
                        loop {
                            if parent_env.lookup_direct("%%j").is_none() {
                                break;
                            }
                            if let Some((pv, pe)) = Environment::lookup_with_env(&parent_env, "%%")
                            {
                                if value_ptr_eq(&pv, &parent_val) {
                                    parent_env = pe.parent().cloned().unwrap_or_else(|| pe.clone());
                                } else {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    next_ctxs.push((parent_val, parent_env));
                } else {
                    return Err(JsonataError::new(
                        "S0217",
                        "% operator used outside of a valid path context",
                    ));
                }
            }
            ctxs = next_ctxs;
            if ctxs.is_empty() {
                return Ok(Value::Undefined);
            }
            continue;
        }

        // Subscript steps whose LEFT is Parent (%[predicate]) require special
        // handling: navigate % to the parent first, then apply the predicate using
        // the parent's own env (so that nested % inside the predicate refers to
        // the grandparent correctly).
        if let Expr::Binary { op, lhs, rhs, .. } = arena.get(step) {
            let (op, lhs, rhs) = (op.clone(), *lhs, *rhs);
            if op == "[" && !lhs.is_empty() && matches!(arena.get(lhs), Expr::Parent { .. }) {
                for (_, ctx_env) in &ctxs {
                    if let Some((parent_val, binding_env)) =
                        Environment::lookup_with_env(ctx_env, "%%")
                    {
                        if parent_val.is_null() || parent_val.is_undefined() {
                            return Err(JsonataError::new(
                                "S0217",
                                "% operator used outside of a valid path context",
                            ));
                        }
                        let parent_env = binding_env
                            .parent()
                            .cloned()
                            .unwrap_or_else(|| binding_env.clone());
                        let pred_result = eval_fast_inner(arena, rhs, &parent_val, &parent_env)?;
                        if pred_result.to_boolean() {
                            next_ctxs.push((parent_val, parent_env));
                        }
                    } else {
                        return Err(JsonataError::new(
                            "S0217",
                            "% operator used outside of a valid path context",
                        ));
                    }
                }
                ctxs = next_ctxs;
                if ctxs.is_empty() {
                    return Ok(Value::Undefined);
                }
                continue;
            }
        }

        // Subscript whose Left is a Block containing a path expression, and
        // whose Right (predicate) references %. The block would normally discard
        // per-element parent context, so we evaluate the block's inner path in
        // tuple mode to preserve parent bindings, then apply the predicate per-tuple.
        // Example: (Account.Order.Product)[%.OrderID='order104'].SKU
        if let Expr::Binary { op, lhs, rhs, .. } = arena.get(step) {
            let (op, lhs, rhs) = (op.clone(), *lhs, *rhs);
            if op == "["
                && !lhs.is_empty()
                && matches!(arena.get(lhs), Expr::Block { .. })
                && node_has_parent_ref(arena, rhs)
            {
                for (val, ctx_env) in &ctxs {
                    let mut tuple_ctxs: Vec<(Value, Rc<Environment>)> = Vec::new();
                    if let Expr::Block { expressions, .. } = arena.get(lhs) {
                        let expressions = expressions.clone();
                        if expressions.len() == 1
                            && let Expr::Path {
                                steps: inner_steps, ..
                            } = arena.get(expressions[0])
                            {
                                let inner_steps = inner_steps.clone();
                                tuple_ctxs = expand_path_tuple(
                                    arena,
                                    &inner_steps,
                                    &[(val.clone(), ctx_env.clone())],
                                )?;
                            }
                        if tuple_ctxs.is_empty() {
                            // Fallback: evaluate block normally.
                            let block_result = eval_fast_inner(arena, lhs, val, ctx_env)?;
                            if block_result.is_undefined() {
                                continue;
                            }
                            match block_result {
                                Value::Array(arr) => {
                                    for item in arr.iter() {
                                        tuple_ctxs.push((item.clone(), ctx_env.clone()));
                                    }
                                }
                                other => tuple_ctxs.push((other, ctx_env.clone())),
                            }
                        }
                    }
                    for (tval, tenv) in &tuple_ctxs {
                        let pred_result = eval_fast_inner(arena, rhs, tval, tenv)?;
                        if pred_result.to_boolean() {
                            next_ctxs.push((tval.clone(), tenv.clone()));
                        }
                    }
                }
                ctxs = next_ctxs;
                if ctxs.is_empty() {
                    return Ok(Value::Undefined);
                }
                continue;
            }
        }

        // When the step is a Block containing a single Path expression,
        // expand the inner path in tuple mode to preserve parent bindings
        // for the % operator (e.g., Account.(Order.Product).{%.OrderID}).
        if let Expr::Block { expressions, .. } = arena.get(step) {
            let expressions = expressions.clone();
            if expressions.len() == 1
                && let Expr::Path {
                    steps: inner_steps, ..
                } = arena.get(expressions[0])
                {
                    let inner_steps = inner_steps.clone();
                    next_ctxs = expand_path_tuple(arena, &inner_steps, &ctxs)?;
                    ctxs = next_ctxs;
                    if ctxs.is_empty() {
                        return Ok(Value::Undefined);
                    }
                    continue;
                }
        }

        // Subscript step whose Left has a Focus binding (join operator @):
        // e.g., Contact@$c[$c.ssn = $e.SSN]. Bind focus var and apply predicate.
        if let Expr::Binary {
            op,
            lhs,
            rhs,
            index: post_filter_index,
            ..
        } = arena.get(step)
        {
            let (op, lhs, rhs) = (op.clone(), *lhs, *rhs);
            let post_filter_index = post_filter_index.clone();
            if op == "["
                && !lhs.is_empty()
                && matches!(arena.get(lhs), Expr::Name { focus: Some(_), .. })
            {
                let (focus_var, index_var) = match arena.get(lhs) {
                    Expr::Name { focus, index, .. } => (focus.clone(), index.clone()),
                    _ => (None, None),
                };
                if let Some(ref focus_name) = focus_var {
                    next_ctxs = eval_join_filter(
                        arena, &ctxs, next_ctxs, lhs, rhs, focus_name, index_var.as_ref(),
                    )?;
                    // Bind post-filter index if the Binary `[` node itself has #$var.
                    if let Some(ref pfi_name) = post_filter_index {
                        for (k, (_, env)) in next_ctxs.iter().enumerate() {
                            env.bind(pfi_name.clone(), Value::Number(k as f64));
                        }
                    }
                    ctxs = next_ctxs;
                    if ctxs.is_empty() {
                        return Ok(Value::Undefined);
                    }
                    continue;
                }
            }
        }

        // Compound subscript after a join-filter: binary "[" whose Left is a
        // binary "[" with Left.Focus set. E.g., books@$b[pred][1] or
        // books@$b[pred][]. Process the inner join-filter first to collect
        // tuples, then apply the outer subscript to the entire tuple collection.
        if let Expr::Binary { op, lhs, rhs, .. } = arena.get(step) {
            let (op, outer_lhs, outer_rhs) = (op.clone(), *lhs, *rhs);
            if op == "[" && !outer_lhs.is_empty()
                && let Expr::Binary {
                    op: inner_op,
                    lhs: inner_lhs,
                    rhs: inner_rhs,
                    ..
                } = arena.get(outer_lhs)
                {
                    let (inner_op, inner_lhs, inner_rhs) =
                        (inner_op.clone(), *inner_lhs, *inner_rhs);
                    if inner_op == "["
                        && !inner_lhs.is_empty()
                        && matches!(arena.get(inner_lhs), Expr::Name { focus: Some(_), .. })
                    {
                        let (focus_var, index_var) = match arena.get(inner_lhs) {
                            Expr::Name { focus, index, .. } => (focus.clone(), index.clone()),
                            _ => (None, None),
                        };
                        if let Some(ref focus_name) = focus_var {
                            // Process the inner join-filter.
                            next_ctxs = eval_join_filter(
                                arena, &ctxs, next_ctxs, inner_lhs, inner_rhs, focus_name,
                                index_var.as_ref(),
                            )?;

                            // Apply the outer subscript to the collected tuples.
                            if !next_ctxs.is_empty() {
                                let outer_result =
                                    eval_fast_inner(arena, outer_rhs, &next_ctxs[0].0, &next_ctxs[0].1)?;
                                if let Some(idx) = outer_result.as_f64() {
                                    let mut i = idx as i64;
                                    if i < 0 {
                                        i += next_ctxs.len() as i64;
                                    }
                                    if i >= 0 && (i as usize) < next_ctxs.len() {
                                        next_ctxs = vec![next_ctxs[i as usize].clone()];
                                    } else {
                                        next_ctxs = vec![];
                                    }
                                }
                            }

                            ctxs = next_ctxs;
                            if ctxs.is_empty() {
                                return Ok(Value::Undefined);
                            }
                            continue;
                        }
                    }
                }
        }

        for (val, ctx_env) in &ctxs {
            // Collapse sequences between steps.
            let val = collapse_val(val);
            if step_idx > 0 && val.is_undefined() {
                continue;
            }

            let result =
                eval_path_step_no_group(arena, step, &val, ctx_env, false, keep_singleton_array)?;
            if result.is_undefined() {
                continue;
            }

            // Skip parent binding for step 0 when it is $ or $$
            // (root references don't have a parent context).
            let skip_parent = step_idx == 0
                && matches!(
                    arena.get(step),
                    Expr::Variable { name, .. } if name.is_empty() || name == "$"
                );

            // Get index var and focus var from this step.
            let (index_var, focus_var) = get_step_bindings(arena, step);
            let is_join = focus_var.is_some();

            // Flatten the result into individual (value, env) contexts.
            let items = flatten_to_vec(result);

            for (j, elem) in items.iter().enumerate() {
                let child_env = Environment::new_child(Rc::clone(ctx_env));
                // Bind parent context (for % operator), unless this is a root step.
                if !skip_parent {
                    child_env.bind("%%".into(), val.clone());
                    if is_join {
                        child_env.bind("%%j".into(), Value::Bool(true));
                    }
                }
                // Bind index variable if present.
                if let Some(ref var_name) = index_var {
                    child_env.bind(var_name.clone(), Value::Number(j as f64));
                }
                // Bind focus variable if present (join @$var).
                if let Some(ref var_name) = focus_var {
                    child_env.bind(var_name.clone(), elem.clone());
                }
                // For join steps, the context value stays at parent level.
                let ctx_value = if is_join { val.clone() } else { elem.clone() };
                next_ctxs.push((ctx_value, Rc::new(child_env)));
            }
        }

        ctxs = next_ctxs;
        if ctxs.is_empty() {
            return Ok(Value::Undefined);
        }
    }

    // Determine which group expression to apply (step-level or path-level).
    let effective_group = final_group.as_ref().or(group);
    if let Some(grp) = effective_group {
        return eval_tuple_group(arena, grp, &ctxs);
    }

    // Collect final values.
    let mut seq = Sequence::new();
    for (val, _) in &ctxs {
        seq.append(val.clone());
    }
    let result = seq.collapse();

    if keep_singleton_array {
        match result {
            Value::Array(_) => return Ok(result),
            Value::Undefined => return Ok(Value::Undefined),
            _ => return Ok(Value::Array(Rc::new(vec![result]))),
        }
    }
    Ok(result)
}

/// Expand a sequence of path steps in tuple mode, preserving parent bindings.
/// Port of Go's `expandPathTuple`.
fn expand_path_tuple(
    arena: &AstArena,
    steps: &[NodeId],
    ctxs: &[(Value, Rc<Environment>)],
) -> Result<Vec<(Value, Rc<Environment>)>, JsonataError> {
    let mut current = ctxs.to_vec();
    for &step in steps {
        let mut next: Vec<(Value, Rc<Environment>)> = Vec::new();
        for (val, ctx_env) in &current {
            let val = collapse_val(val);
            if val.is_undefined() {
                continue;
            }
            let result = eval_path_step(arena, step, &val, ctx_env, false, false)?;
            if result.is_undefined() {
                continue;
            }
            let (index_var, focus_var) = get_step_bindings(arena, step);
            let is_join = focus_var.is_some();
            let items = flatten_to_vec(result);
            for (j, elem) in items.iter().enumerate() {
                let child_env = Environment::new_child(Rc::clone(ctx_env));
                child_env.bind("%%".into(), val.clone());
                if is_join {
                    child_env.bind("%%j".into(), Value::Bool(true));
                }
                if let Some(ref var_name) = index_var {
                    child_env.bind(var_name.clone(), Value::Number(j as f64));
                }
                if let Some(ref var_name) = focus_var {
                    child_env.bind(var_name.clone(), elem.clone());
                }
                let ctx_value = if is_join { val.clone() } else { elem.clone() };
                next.push((ctx_value, Rc::new(child_env)));
            }
        }
        current = next;
        if current.is_empty() {
            return Ok(vec![]);
        }
    }
    Ok(current)
}

/// Evaluate a join-filter step: walk ctxs, evaluate left_node against each context,
/// bind focus_var (and optionally index_var) in a child env, then keep only contexts
/// whose predicate evaluates to true. Port of Go's evalJoinFilter.
fn eval_join_filter(
    arena: &AstArena,
    ctxs: &[(Value, Rc<Environment>)],
    mut dst: Vec<(Value, Rc<Environment>)>,
    left_node: NodeId,
    predicate: NodeId,
    focus_var: &str,
    index_var: Option<&String>,
) -> Result<Vec<(Value, Rc<Environment>)>, JsonataError> {
    for (val, ctx_env) in ctxs {
        let val = collapse_val(val);
        if val.is_undefined() {
            continue;
        }
        let left_result = eval_path_step(arena, left_node, &val, ctx_env, false, false)?;
        if left_result.is_undefined() {
            continue;
        }
        let items = flatten_to_vec(left_result);
        for (j, item) in items.iter().enumerate() {
            let child_env = Environment::new_child(Rc::clone(ctx_env));
            child_env.bind("%%".into(), val.clone());
            child_env.bind("%%j".into(), Value::Bool(true));
            child_env.bind(focus_var.into(), item.clone());
            if let Some(idx_name) = index_var {
                child_env.bind(idx_name.clone(), Value::Number(j as f64));
            }
            let child_rc = Rc::new(child_env);
            let pred_result = eval_fast_inner(arena, predicate, item, &child_rc)?;
            if pred_result.to_boolean() {
                dst.push((val.clone(), child_rc));
            }
        }
    }
    Ok(dst)
}

/// Collapse a value (unwrap Sequence), returning the inner value.
fn collapse_val(val: &Value) -> Value {
    match val {
        Value::Sequence(seq) => seq.collapse(),
        other => other.clone(),
    }
}

/// Flatten a result value into a Vec of individual items.
fn flatten_to_vec(val: Value) -> Vec<Value> {
    match val {
        Value::Array(a) => (*a).clone(),
        Value::Sequence(s) => {
            let collapsed = s.collapse();
            match collapsed {
                Value::Array(a) => (*a).clone(),
                Value::Undefined => vec![],
                other => vec![other],
            }
        }
        other => vec![other],
    }
}

/// Extract index and focus variable names from a path step.
fn get_step_bindings(arena: &AstArena, step: NodeId) -> (Option<String>, Option<String>) {
    match arena.get(step) {
        Expr::Name { index, focus, .. } => (index.clone(), focus.clone()),
        Expr::Variable { index, focus, .. } => (index.clone(), focus.clone()),
        Expr::Sort { index, focus, .. } => (index.clone(), focus.clone()),
        Expr::Binary {
            op,
            lhs,
            index,
            focus,
            ..
        } if op == "[" => {
            // If the Binary node itself has index/focus (e.g. from `#$var` after `]`), use those.
            // Otherwise look at the lhs.
            let mut idx = index.clone();
            let mut foc = focus.clone();
            let lhs = *lhs;
            match arena.get(lhs) {
                Expr::Name {
                    index: li,
                    focus: lf,
                    ..
                }
                | Expr::Variable {
                    index: li,
                    focus: lf,
                    ..
                }
                | Expr::Sort {
                    index: li,
                    focus: lf,
                    ..
                } => {
                    if idx.is_none() {
                        idx.clone_from(li);
                    }
                    if foc.is_none() {
                        foc.clone_from(lf);
                    }
                }
                Expr::Binary {
                    index: li,
                    focus: lf,
                    ..
                } => {
                    if idx.is_none() {
                        idx.clone_from(li);
                    }
                    if foc.is_none() {
                        foc.clone_from(lf);
                    }
                }
                _ => {}
            }
            (idx, foc)
        }
        Expr::Binary { index, focus, .. } => (index.clone(), focus.clone()),
        _ => (None, None),
    }
}

/// Shallow equality check for Values (used for join flag parent comparison).
/// This approximates Go's pointer equality by checking structural equality.
fn value_ptr_eq(a: &Value, b: &Value) -> bool {
    // In Go, this is a pointer comparison. We use structural equality
    // as an approximation which is correct for the parent chain use case.
    a == b
}

/// Apply a group-by expression to tuple contexts, using per-element environments.
///
/// JSONata group-by semantics: records are grouped by key, then the value
/// expression is evaluated once per group with the context set to the array
/// of all group members (or a single value when the group has one member).
/// This allows aggregate functions like $join or $sum to operate on the
/// full group rather than individual records.
fn eval_tuple_group(
    arena: &AstArena,
    group: &crate::parser::GroupExpr,
    ctxs: &[(Value, Rc<Environment>)],
) -> JsonataResult {
    let mut result_map = indexmap::IndexMap::<String, Value>::new();

    for pair in &group.pairs {
        let key_node = pair[0];
        let val_node = pair[1];

        // Phase 1: group ctxs by key.
        let mut key_order: Vec<String> = Vec::new();
        let mut groups: indexmap::IndexMap<String, (Vec<Value>, Vec<Rc<Environment>>)> =
            indexmap::IndexMap::new();

        for (item, item_env) in ctxs {
            let key_val = eval_fast_inner(arena, key_node, item, item_env)?;
            let key_str: String = match &key_val {
                Value::String(s) => s.to_string(),
                _ => {
                    return Err(JsonataError::new(
                        "T1003",
                        "key expression must evaluate to a string",
                    ));
                }
            };
            if let Some(g) = groups.get_mut(&key_str) {
                g.0.push(item.clone());
                g.1.push(Rc::clone(item_env));
            } else {
                key_order.push(key_str.clone());
                groups.insert(key_str, (vec![item.clone()], vec![Rc::clone(item_env)]));
            }
        }

        // Phase 2: evaluate value expression per group.
        for key in &key_order {
            let (values, envs) = groups.get(key.as_str()).ok_or_else(|| JsonataError::new("D0000", "key from key_order must exist in groups"))?;
            let (group_ctx, group_env) = if values.len() == 1 {
                (values[0].clone(), Rc::clone(&envs[0]))
            } else {
                let merged = merge_group_envs(envs);
                (Value::Array(Rc::new(values.clone())), Rc::new(merged))
            };
            let val = if val_node.is_empty() {
                group_ctx
            } else {
                eval_fast_inner(arena, val_node, &group_ctx, &group_env)?
            };
            if !val.is_undefined() {
                result_map.insert(key.clone(), val);
            }
        }
    }

    if result_map.is_empty() {
        Ok(Value::Undefined)
    } else {
        Ok(Value::Object(Rc::new(result_map)))
    }
}

/// Merge environments from a group of records.
/// Variables that differ across records are collected into arrays.
fn merge_group_envs(envs: &[Rc<Environment>]) -> Environment {
    if envs.is_empty() {
        return Environment::new();
    }
    if envs.len() == 1 {
        return envs[0].shallow_clone();
    }

    let merged = Environment::new_child(
        envs[0]
            .parent()
            .cloned()
            .unwrap_or_else(|| Rc::new(Environment::new())),
    );

    // Collect variable names from tuple-specific envs (those with %%).
    let mut var_names: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for env in envs {
        let mut current: Option<&Rc<Environment>> = Some(env);
        while let Some(e) = current {
            if e.lookup_direct("%%").is_none() {
                break;
            }
            e.for_each_direct(|name, _| {
                if seen.insert(name.to_string()) {
                    var_names.push(name.to_string());
                }
            });
            current = e.parent();
        }
    }

    // For each variable, collect values from each env via full lookup.
    for name in &var_names {
        if name == "%%" || name == "%%j" {
            if let Some(v) = envs[0].lookup(name) {
                merged.bind(name.clone(), v);
            }
            continue;
        }
        let mut vals: Vec<Value> = Vec::new();
        for env in envs {
            if let Some(v) = env.lookup(name) {
                vals.push(v);
            }
        }
        if vals.len() == 1 {
            merged.bind(name.clone(), vals.into_iter().next().unwrap_or(Value::Undefined));
        } else if !vals.is_empty() {
            // Check if all values are identical.
            let all_same = vals.windows(2).all(|w| w[0] == w[1]);
            if all_same {
                merged.bind(name.clone(), vals.into_iter().next().unwrap_or(Value::Undefined));
            } else {
                merged.bind(name.clone(), Value::Array(Rc::new(vals)));
            }
        }
    }

    merged
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
            _ => return Ok(Value::Array(Rc::new(vec![result]))),
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
            return eval_fast_inner(arena, step, input, env);
        }
        Expr::Block { .. } if !prev_was_mapper => {
            return eval_fast_inner(arena, step, input, env);
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
                    for item in arr.iter() {
                        seq.append(item.clone());
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
            return eval_fast_inner(arena, step, input, env);
        }
        _ => {}
    }

    // Array constructor steps not preceded by mapper are literal expressions.
    if let Expr::Unary { op, .. } = expr
        && op == "["
        && !prev_was_mapper
    {
        return eval_fast_inner(arena, step, input, env);
    }

    // For all other step types, map over array input.
    let arr = if let Value::Array(a) = input { a.clone() } else {
        // Single item — check for function step with path-element prepend.
        if matches!(expr, Expr::Function { .. }) {
            return eval_path_function_step(arena, step, input, env);
        }
        return eval_fast_inner(arena, step, input, env);
    };

    let is_group_step = matches!(expr, Expr::Unary { op, .. } if op == "[");
    let mut seq = Sequence::new();

    for item in arr.iter() {
        let val = if matches!(arena.get(step), Expr::Function { .. }) {
            eval_path_function_step(arena, step, item, env)?
        } else {
            eval_fast_inner(arena, step, item, env)?
        };
        if val.is_undefined() {
            continue;
        }
        if is_group_step {
            seq.values.push(val);
            continue;
        }
        match val {
            Value::Array(inner) => seq.values.extend(inner.iter().cloned()),
            Value::Sequence(s) => seq.values.extend(s.values),
            other => seq.append(other),
        }
    }

    if seq.values.is_empty() {
        return Ok(Value::Undefined);
    }
    if is_group_step && keep_singleton_array {
        return Ok(Value::Array(Rc::new(seq.values)));
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
        _ => return eval_fast_inner(arena, step, item, env),
    };

    let fn_val = eval_fast_inner(arena, procedure, item, env)?;
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
        args.push(eval_fast_inner(arena, arg_node, item, env)?);
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

// Large dispatch function for all binary operator types.
// Flattens left-associative chains iteratively to avoid deep recursion.
#[allow(clippy::too_many_lines)]
fn eval_binary(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    // Handle subscript `[` separately — it needs AST-level lhs access
    // for Descendant checks, index_var extraction, and keep_array.
    if let Expr::Binary { op, lhs, rhs, .. } = arena.get(node)
        && op == "["
    {
        return eval_subscript_binary(arena, node, *lhs, *rhs, input, env);
    }

    // Flatten the left-associative chain. Walk the lhs spine collecting
    // (operator, rhs) pairs until we hit a non-flattenable node.
    // `~>`, `?:`, `??` are handled by the TCO loop in eval().
    let mut chain: Vec<(String, NodeId)> = Vec::new();
    let mut leftmost = node;

    loop {
        match arena.get(leftmost) {
            Expr::Binary { op, lhs, rhs, .. }
                if !matches!(op.as_str(), "[" | "~>" | "?:" | "??") =>
            {
                chain.push((op.clone(), *rhs));
                leftmost = *lhs;
            }
            _ => break,
        }
    }

    // chain is outermost-first. Reverse to get innermost-first (left-to-right eval order).
    chain.reverse();

    // Evaluate the leftmost (non-binary) node.
    let mut result = eval_fast_inner(arena, leftmost, input, env)?;

    // Apply each operator iteratively.
    for (op, rhs) in &chain {
        result = apply_binary_op(arena, op, result, *rhs, leftmost, input, env)?;
    }
    Ok(result)
}

/// Subscript/filter binary `[` — needs AST-level access to lhs for
/// Descendant checks, index_var extraction, and keep_array detection.
fn eval_subscript_binary(
    arena: &AstArena,
    node: NodeId,
    lhs: NodeId,
    rhs: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let left = if matches!(arena.get(lhs), Expr::Descendant { .. }) {
        let descendants = descendant_lookup(input);
        let mut seq = Sequence::new();
        seq.append(input.clone());
        match descendants {
            Value::Array(arr) => {
                for item in arr.iter() {
                    seq.append(item.clone());
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
        seq.collapse()
    } else {
        eval_fast_inner(arena, lhs, input, env)?
    };
    // Extract index variable from the LHS (e.g. $#$pos[...] → index_var="pos").
    let index_var = match arena.get(lhs) {
        Expr::Name { index, .. }
        | Expr::Variable { index, .. }
        | Expr::Binary { index, .. }
        | Expr::Sort { index, .. } => index.clone(),
        _ => None,
    };
    // Check keep_array: the [] suffix on this node or anywhere in the LHS chain.
    let keep_array = has_keep_array(arena, node);
    let result = eval_subscript(arena, rhs, &left, input, env, index_var.as_ref())?;
    if keep_array {
        match result {
            Value::Array(_) => Ok(result),
            Value::Undefined => Ok(Value::Array(Rc::new(vec![]))),
            scalar => Ok(Value::Array(Rc::new(vec![scalar]))),
        }
    } else {
        Ok(result)
    }
}

/// Apply a single binary operator given a pre-evaluated left value and an unevaluated rhs node.
/// `lhs_node` is the original LHS NodeId (used only for subscript `[` AST inspection).
#[allow(clippy::too_many_lines)]
fn apply_binary_op(
    arena: &AstArena,
    op: &str,
    left: Value,
    rhs: NodeId,
    _lhs_node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    match op {
        // Short-circuit operators.
        "and" => {
            if !left.to_boolean() {
                return Ok(Value::Bool(false));
            }
            let right = eval_fast_inner(arena, rhs, input, env)?;
            Ok(Value::Bool(right.to_boolean()))
        }
        "or" => {
            if left.to_boolean() {
                return Ok(Value::Bool(true));
            }
            let right = eval_fast_inner(arena, rhs, input, env)?;
            Ok(Value::Bool(right.to_boolean()))
        }
        "?:" => {
            if left.to_boolean() {
                Ok(left)
            } else {
                eval_fast_inner(arena, rhs, input, env)
            }
        }
        "??" => {
            if left.is_undefined() {
                eval_fast_inner(arena, rhs, input, env)
            } else {
                Ok(left)
            }
        }
        "~>" => eval_chain(arena, rhs, &left, input, env),
        // Arithmetic operators.
        "+" | "-" | "*" | "/" | "%" | "**" => {
            let right = eval_fast_inner(arena, rhs, input, env)?;
            apply_arithmetic(op, &left, &right)
        }
        // String concatenation.
        "&" => {
            let right = eval_fast_inner(arena, rhs, input, env)?;
            let ls = if left.is_undefined() {
                String::new()
            } else {
                left.stringify(false)?
            };
            let rs = if right.is_undefined() {
                String::new()
            } else {
                right.stringify(false)?
            };
            Ok(Value::String(format!("{ls}{rs}").into()))
        }
        // Equality.
        "=" => {
            let right = eval_fast_inner(arena, rhs, input, env)?;
            if left.is_undefined() || right.is_undefined() {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(left.deep_equal(&right)))
        }
        "!=" => {
            let right = eval_fast_inner(arena, rhs, input, env)?;
            if left.is_undefined() || right.is_undefined() {
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(!left.deep_equal(&right)))
        }
        // Comparison.
        "<" | "<=" | ">" | ">=" => {
            let right = eval_fast_inner(arena, rhs, input, env)?;
            left.compare(&right, op)
        }
        // Membership.
        "in" => {
            let right = eval_fast_inner(arena, rhs, input, env)?;
            Ok(Value::Bool(left.contained_in(&right)))
        }
        // Range.
        ".." => {
            let right = eval_fast_inner(arena, rhs, input, env)?;
            apply_range(op, &left, &right)
        }
        _ => Err(JsonataError::new(
            "D3001",
            format!("unknown binary operator: {op}"),
        )),
    }
}

/// Apply arithmetic operator to pre-evaluated values.
fn apply_arithmetic(op: &str, left: &Value, right: &Value) -> JsonataResult {
    // Type-check non-undefined operands BEFORE undefined propagation.
    if !left.is_undefined() && !left.is_number() {
        return Err(JsonataError::new(
            "T2001",
            "the left operand must be a number",
        ));
    }
    if !right.is_undefined() && !right.is_number() {
        return Err(JsonataError::new(
            "T2002",
            "the right operand must be a number",
        ));
    }
    if left.is_undefined() || right.is_undefined() {
        return Ok(Value::Undefined);
    }
    let ln = left.as_f64().ok_or_else(|| JsonataError::new("D0000", "left verified as number above"))?;
    let rn = right.as_f64().ok_or_else(|| JsonataError::new("D0000", "right verified as number above"))?;
    // Modulo by zero → D3001 immediately (matches Go).
    if op == "%" && rn == 0.0 {
        return Err(JsonataError::new("D3001", "modulo by zero"));
    }
    let result = match op {
        "+" => ln + rn,
        "-" => ln - rn,
        "*" => ln * rn,
        "/" => ln / rn,
        "%" => ln % rn,
        "**" => ln.powf(rn),
        _ => unreachable!(),
    };
    // Division by zero → let Inf propagate (error comes from downstream use).
    // Other non-finite results → D1001 "number out of range".
    if op == "/" {
        return Ok(Value::Number(result));
    }
    if !result.is_finite() {
        return Err(JsonataError::new(
            "D1001",
            format!(
                "Number out of range: {}",
                crate::value::format_float(result)
            ),
        ));
    }
    Ok(Value::Number(result))
}

/// Apply range operator to pre-evaluated values.
fn apply_range(_op: &str, left: &Value, right: &Value) -> JsonataResult {
    // Type-check non-undefined operands BEFORE undefined propagation.
    if !left.is_undefined() && left.as_f64().is_none() {
        return Err(JsonataError::new(
            "T2003",
            "the left operand of the range operator (..) must be a number",
        ));
    }
    if !right.is_undefined() && right.as_f64().is_none() {
        return Err(JsonataError::new(
            "T2004",
            "the right operand of the range operator (..) must be a number",
        ));
    }

    // Undefined operands → undefined (not an error).
    if left.is_undefined() || right.is_undefined() {
        return Ok(Value::Undefined);
    }

    let ln = left.as_f64().ok_or_else(|| JsonataError::new("D0000", "left verified as number above"))?;
    let rn = right.as_f64().ok_or_else(|| JsonataError::new("D0000", "right verified as number above"))?;

    // Must be integers (no fractional part).
    if ln != ln.trunc() {
        return Err(JsonataError::new(
            "T2003",
            "the left operand of the range operator (..) must be an integer",
        ));
    }
    if rn != rn.trunc() {
        return Err(JsonataError::new(
            "T2004",
            "the right operand of the range operator (..) must be an integer",
        ));
    }

    let start = ln as i64;
    let end = rn as i64;
    if start > end {
        return Ok(Value::Undefined);
    }
    let count = (end - start + 1) as usize;
    if count > 10_000_000 {
        return Err(JsonataError::new("D2014", "range operator too large"));
    }
    let arr: Vec<Value> = (start..=end).map(|i| Value::Number(i as f64)).collect();
    Ok(Value::Array(Rc::new(arr)))
}

/// Walk the left chain of a Binary "[" node to check if keep_array is set
/// on the node itself or anywhere in the LHS chain.
/// Go equivalent: `hasKeepArrayInChain` in `eval_binary.go`.
fn has_keep_array(arena: &AstArena, node: NodeId) -> bool {
    let mut current = node;
    loop {
        match arena.get(current) {
            Expr::Name { keep_array, .. }
            | Expr::Binary { keep_array, .. }
            | Expr::Variable { keep_array, .. }
            | Expr::Function { keep_array, .. }
            | Expr::Sort { keep_array, .. }
            | Expr::Unary { keep_array, .. }
                if *keep_array => {
                    return true;
                }
            _ => {}
        }
        // Walk into the LHS of Binary nodes or the expr of Sort nodes.
        match arena.get(current) {
            Expr::Binary { lhs, .. } => current = *lhs,
            Expr::Sort { expr, .. } => current = *expr,
            _ => break,
        }
    }
    false
}

fn eval_subscript(
    arena: &AstArena,
    rhs: NodeId,
    left: &Value,
    input: &Value,
    env: &Rc<Environment>,
    index_var: Option<&String>,
) -> JsonataResult {
    // For non-array inputs without index variable, evaluate directly.
    if !matches!(left, Value::Array(_) | Value::Sequence(_))
        && index_var.is_none() {
            // Bind %% → input so the % operator can navigate to the parent.
            let filter_env = Rc::new(Environment::new_child(Rc::clone(env)));
            filter_env.bind("%%".into(), input.clone());
            let index = eval_fast_inner(arena, rhs, left, &filter_env)?;
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
        // When there's an index variable, wrap in array so the predicate filter
        // path handles index binding correctly (like Go's evalSubscriptLeft).

    let arr: Vec<Value> = match left {
        Value::Array(a) => a.to_vec(),
        Value::Sequence(s) => s.to_vec(),
        _ => vec![left.clone()],
    };

    // Try evaluating RHS as a simple expression (might be a numeric literal or
    // variable). If it resolves to a number, use it as a direct index.
    // If it resolves to an array of all-numeric values, use as index list.
    // If it errors or is non-numeric/non-index, fall through to per-element predicate filter.
    if let Ok(index) = eval_fast_inner(arena, rhs, left, env) {
        // Array of all-numeric values → select those indices (e.g. [[1..4]]).
        // Matches Go's selectByIndices: resolve negative indices (add len), sort
        // ascending, then select. This ensures [[1..3,8,-1]] on a 10-element array
        // gives sorted actual indices [1,2,3,8,9] → elements [2,3,4,9,10].
        if let Value::Array(ref indices) = index
            && !indices.is_empty()
            && indices.iter().all(|v| v.as_f64().is_some())
        {
            let len = arr.len() as i64;
            let mut actual_indices: Vec<i64> = indices
                .iter()
                .map(|v| {
                    let idx = v.as_f64().unwrap_or(0.0) as i64;
                    if idx < 0 { len + idx } else { idx }
                })
                .collect();
            actual_indices.sort_unstable();
            let result: Vec<Value> = actual_indices
                .into_iter()
                .filter_map(|actual| {
                    if actual >= 0 && actual < len {
                        Some(arr[actual as usize].clone())
                    } else {
                        None
                    }
                })
                .collect();
            return if result.is_empty() {
                Ok(Value::Undefined)
            } else {
                Ok(Value::Array(Rc::new(result)))
            };
        }
        // Single numeric index.
        if let Some(n) = index.as_f64() {
            let idx = n.trunc() as i64;
            let len = arr.len() as i64;
            let actual = if idx < 0 { len + idx } else { idx };
            if actual < 0 || actual >= len {
                return Ok(Value::Undefined);
            }
            return Ok(arr[actual as usize].clone());
        }
        // Not numeric — fall through to per-element predicate filter below.
        let _ = index;
    }

    // Predicate filter — evaluate rhs against each element.
    // Bind %% → input (parent context) so the % operator can navigate upward.
    // This matches Go's filterByPredicate which binds parentKey to the input.
    let filter_env = Rc::new(Environment::new_child(Rc::clone(env)));
    filter_env.bind("%%".into(), input.clone());
    let mut seq = Sequence::new();
    for (i, item) in arr.iter().enumerate() {
        // Bind index variable if present (e.g. $#$pos[...]).
        if let Some(var_name) = index_var {
            filter_env.bind(var_name.clone(), Value::Number(i as f64));
        }
        let test = eval_fast_inner(arena, rhs, item, &filter_env)?;
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
    // Flatten right-associative chain: a ~> (f ~> g) → [f, g], then iterate.
    let mut steps = Vec::new();
    let mut current = rhs;
    while let Expr::Binary {
        op, lhs, rhs: rr, ..
    } = arena.get(current)
        && op == "~>"
    {
        steps.push(*lhs);
        current = *rr;
    }
    steps.push(current);

    let mut result = piped.clone();
    for step in steps {
        result = eval_chain_step(arena, step, &result, input, env)?;
        if result.is_undefined() {
            return Ok(Value::Undefined);
        }
    }
    Ok(result)
}

fn eval_chain_step(
    arena: &AstArena,
    rhs: NodeId,
    piped: &Value,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {

    // Function call node: prepend piped as first argument.
    if let Expr::Function {
        procedure,
        arguments,
        keep_array,
        ..
    } = arena.get(rhs)
    {
        let procedure = *procedure;
        let arguments = arguments.clone();
        let keep_array = *keep_array;
        let fn_val = eval_fast_inner(arena, procedure, input, env)?;
        let Value::Function(func) = fn_val else {
            return Err(JsonataError::new(
                "T1006",
                "attempted to invoke undefined function",
            ));
        };
        let mut args = vec![piped.clone()];
        for &arg_node in &arguments {
            if matches!(arena.get(arg_node), Expr::Placeholder { .. }) {
                args.push(Value::Undefined);
                continue;
            }
            args.push(eval_fast_inner(arena, arg_node, input, env)?);
        }
        let result = call_function(&func, &args, input, env, arena)?;
        // Apply keep_array wrapping if [] suffix present.
        if keep_array {
            return match result {
                Value::Sequence(seq) => Ok(seq.collapse_and_keep(true)),
                Value::Array(_) => Ok(result),
                Value::Undefined => Ok(Value::Undefined),
                scalar => Ok(Value::Array(Rc::new(vec![scalar]))),
            };
        }
        // Collapse sequences from function results.
        return match result {
            Value::Sequence(seq) => Ok(seq.collapse()),
            other => Ok(other),
        };
    }

    // Otherwise evaluate right side and call it.
    let fn_val = eval_fast_inner(arena, rhs, input, env)?;

    // If right side is a regex object, apply regex test (like $contains).
    if let Value::Object(ref obj) = fn_val
        && obj.contains_key("pattern") {
            return apply_regex_chain(piped, obj);
        }

    match &fn_val {
        Value::Function(func) => {
            // If piped is itself a function, compose them rather than calling fn(piped).
            if let Value::Function(piped_fn) = piped {
                let outer = func.clone();
                let inner = piped_fn.clone();
                let env_clone = Rc::clone(env);
                let composed: Rc<crate::evaluator::EnvAwareBuiltinFn> = Rc::new(
                    move |args: &[Value],
                          focus: &Value,
                          _env: &Rc<Environment>,
                          arena: &AstArena| {
                        let intermediate = call_function(&inner, args, focus, &env_clone, arena)?;
                        // Collapse sequences between composition steps.
                        let intermediate = match intermediate {
                            Value::Sequence(seq) => seq.collapse(),
                            other => other,
                        };
                        call_function(&outer, &[intermediate], focus, &env_clone, arena)
                    },
                );
                return Ok(Value::Function(FunctionValue::EnvAwareBuiltin(composed)));
            }
            {
                let result = call_function(func, std::slice::from_ref(piped), input, env, arena)?;
                match result {
                    Value::Sequence(seq) => Ok(seq.collapse()),
                    other => Ok(other),
                }
            }
        }
        _ => Err(JsonataError::new(
            "T2006",
            "the right-hand side of the ~> operator must be a function",
        )),
    }
}

/// Apply a regex test to a piped value in chain context (~> /regex/).
/// Returns the first match object if the regex matches, or Undefined if not.
fn apply_regex_chain(
    piped: &Value,
    regex_obj: &indexmap::IndexMap<String, Value>,
) -> JsonataResult {
    let s = match piped {
        Value::String(s) => &**s,
        _ => return Ok(Value::Undefined),
    };
    let pattern = match regex_obj.get("pattern") {
        Some(Value::String(p)) => &**p,
        _ => return Ok(Value::Undefined),
    };
    let flags = match regex_obj.get("flags") {
        Some(Value::String(f)) => &**f,
        _ => "",
    };
    let re = crate::stdlib::regex::compile_regex(pattern, flags)
        .map_err(|e| JsonataError::new("D1002", format!("invalid regex: {}", e.message)))?;
    if let Some(caps) = re.captures(s) {
        let m = caps.get(0).ok_or_else(|| JsonataError::new("D0000", "capture group 0 always exists when captures succeed"))?;
        // Build match object similar to $match.
        let mut obj = indexmap::IndexMap::new();
        obj.insert("match".into(), Value::String(m.as_str().into()));
        let start = s[..m.start()].chars().count();
        let end = s[..m.end()].chars().count();
        obj.insert("start".into(), Value::Number(start as f64));
        obj.insert("end".into(), Value::Number(end as f64));
        // Collect capture groups (skip group 0 which is the full match).
        let mut groups = Vec::new();
        for i in 1..caps.len() {
            match caps.get(i) {
                Some(g) => groups.push(Value::String(g.as_str().into())),
                None => groups.push(Value::String("".into())),
            }
        }
        obj.insert("groups".into(), Value::Array(Rc::new(groups)));
        Ok(Value::Object(Rc::new(obj)))
    } else {
        Ok(Value::Undefined)
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
            let val = eval_fast_inner(arena, operand, input, env)?;
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
                let val = eval_fast_inner(arena, expr, input, env)?;
                if val.is_undefined() {
                    continue;
                }
                // Explicit inner array constructors [expr] are preserved as nested elements.
                // All other arrays/sequences are spread (flattened).
                let is_explicit_array = matches!(
                    arena.get(expr),
                    Expr::Unary { op, .. } if op == "["
                );
                match val {
                    Value::Sequence(seq) => {
                        if seq.cons_array || is_explicit_array {
                            result.push(seq.collapse());
                        } else {
                            result.extend(seq.values);
                        }
                    }
                    Value::Array(arr) => {
                        if is_explicit_array {
                            result.push(Value::Array(arr));
                        } else {
                            result.extend(arr.iter().cloned());
                        }
                    }
                    other => result.push(other),
                }
            }
            Ok(Value::Array(Rc::new(result)))
        }
        "{" => {
            // Object constructor.
            let mut obj = indexmap::IndexMap::new();
            // lhs is flat [k0,v0,k1,v1,...]
            let mut i = 0;
            while i + 1 < lhs_nodes.len() {
                let key_val = eval_fast_inner(arena, lhs_nodes[i], input, env)?;
                // Skip if key is undefined.
                if key_val.is_undefined() {
                    i += 2;
                    continue;
                }
                let key: String = match &key_val {
                    Value::String(s) => s.to_string(),
                    _ => {
                        return Err(JsonataError::new(
                            "T1003",
                            format!(
                                "key expression must evaluate to a string, got {key_val:?}"
                            ),
                        ));
                    }
                };
                if obj.contains_key(&key) {
                    return Err(JsonataError::new(
                        "D1009",
                        format!("duplicate key: \"{key}\""),
                    ));
                }
                let val_val = eval_fast_inner(arena, lhs_nodes[i + 1], input, env)?;
                // Collapse sequences.
                let val_val = match val_val {
                    Value::Sequence(seq) => seq.collapse(),
                    other => other,
                };
                // Skip if value is undefined.
                if val_val.is_undefined() {
                    i += 2;
                    continue;
                }
                obj.insert(key, val_val);
                i += 2;
            }
            Ok(Value::Object(Rc::new(obj)))
        }
        _ => Err(JsonataError::new(
            "D3001",
            format!("unknown unary operator: {op}"),
        )),
    }
}

// ── Block, condition, bind ──────────────────────────────────────────

// eval_block and eval_condition are handled inline in eval() loop for TCO.

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

    let val = eval_fast_inner(arena, rhs, input, env)?;

    // The LHS is a Variable node — extract the name.
    let name = match arena.get(lhs) {
        Expr::Variable { name, .. } => name.clone(),
        _ => {
            return Err(JsonataError::new("D3001", "bind target must be a variable"));
        }
    };

    // Bind in the current environment (uses RefCell for interior mutability).
    env.bind(name, val.clone());
    Ok(val)
}

// ── Sort expression (^) ─────────────────────────────────────────────

fn eval_sort(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (sort_expr, terms) = match arena.get(node) {
        Expr::Sort { expr, terms, .. } => (*expr, terms.clone()),
        _ => unreachable!(),
    };

    // If any sort term references % (parent), use parent-tracking sort.
    let needs_parent = terms
        .iter()
        .any(|t| node_has_parent_ref(arena, t.expression));
    if needs_parent {
        return eval_sort_with_parent_tracking(arena, sort_expr, &terms, input, env);
    }

    let items = eval_fast_inner(arena, sort_expr, input, env)?;
    if items.is_undefined() {
        return Ok(Value::Undefined);
    }

    let (mut arr, was_array) = match items {
        Value::Array(a) => ((*a).clone(), true),
        Value::Sequence(seq) => {
            let collapsed = seq.collapse();
            match collapsed {
                Value::Undefined => return Ok(Value::Undefined),
                Value::Array(a) => ((*a).clone(), true),
                other => (vec![other], false),
            }
        }
        other => (vec![other], false),
    };

    if terms.is_empty() {
        if !was_array && arr.len() == 1 {
            return arr.into_iter().next().ok_or_else(|| JsonataError::new("D0000", "len is 1 but next() returned None"));
        }
        return Ok(Value::Array(Rc::new(arr)));
    }

    // Stable sort with error propagation.
    let mut sort_err: Option<JsonataError> = None;
    arr.sort_by(|a, b| {
        if sort_err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        match compare_sort_terms(arena, &terms, a, b, env, env) {
            Ok(cmp) => cmp.cmp(&0),
            Err(e) => {
                sort_err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = sort_err {
        return Err(e);
    }

    if !was_array && arr.len() == 1 {
        return arr.into_iter().next().ok_or_else(|| JsonataError::new("D0000", "len is 1 but next() returned None"));
    }
    Ok(Value::Array(Rc::new(arr)))
}

/// Sort with parent-tracking: when sort terms reference %, we need to build
/// tuple contexts so each item has its parent environment for % evaluation.
fn eval_sort_with_parent_tracking(
    arena: &AstArena,
    sort_expr: NodeId,
    terms: &[crate::parser::SortTerm],
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    // Build tuple contexts from the sort expression's inner path.
    let ctxs = build_sort_ctxs(arena, sort_expr, input, env)?;
    if ctxs.is_empty() {
        return Ok(Value::Undefined);
    }

    let mut sorted = ctxs;
    let mut sort_err: Option<JsonataError> = None;
    sorted.sort_by(|a, b| {
        if sort_err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        match compare_sort_terms(arena, terms, &a.0, &b.0, &a.1, &b.1) {
            Ok(cmp) => cmp.cmp(&0),
            Err(e) => {
                sort_err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = sort_err {
        return Err(e);
    }

    let mut seq = Sequence::new();
    for (val, _) in &sorted {
        seq.append(val.clone());
    }
    Ok(seq.collapse())
}

/// Build pathCtx tuples for a sort expression, splitting paths into prefix + lastStep
/// so that parent bindings are preserved.
fn build_sort_ctxs(
    arena: &AstArena,
    sort_expr: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> Result<Vec<(Value, Rc<Environment>)>, JsonataError> {
    if let Expr::Path { steps, .. } = arena.get(sort_expr) {
        let steps = steps.clone();
        if steps.is_empty() {
            return Ok(vec![]);
        }
        // Walk prefix steps in tuple mode to build parent contexts.
        let prefix_ctxs = walk_prefix_steps(arena, &steps[..steps.len() - 1], input, env)?;
        // Expand the last step with parent tracking.
        return expand_last_step(arena, steps[steps.len() - 1], &prefix_ctxs);
    }

    // Non-path expression: evaluate normally and wrap results.
    let result = eval_fast_inner(arena, sort_expr, input, env)?;
    if result.is_undefined() {
        return Ok(vec![]);
    }
    match result {
        Value::Array(arr) => Ok(arr.iter().cloned().map(|v| (v, env.clone())).collect()),
        other => Ok(vec![(other, env.clone())]),
    }
}

/// Walk prefix path steps in tuple mode, binding parent context at each step.
fn walk_prefix_steps(
    arena: &AstArena,
    steps: &[NodeId],
    input: &Value,
    env: &Rc<Environment>,
) -> Result<Vec<(Value, Rc<Environment>)>, JsonataError> {
    let mut ctxs: Vec<(Value, Rc<Environment>)> = vec![(input.clone(), env.clone())];
    for &step in steps {
        let mut next: Vec<(Value, Rc<Environment>)> = Vec::new();
        for (val, ctx_env) in &ctxs {
            let val = collapse_val(val);
            if val.is_undefined() {
                continue;
            }
            let result = eval_path_step(arena, step, &val, ctx_env, false, false)?;
            if result.is_undefined() {
                continue;
            }
            let (index_var, focus_var) = get_step_bindings(arena, step);
            let is_join = focus_var.is_some();
            let items = flatten_to_vec(result);
            for (j, elem) in items.iter().enumerate() {
                let child_env = Environment::new_child(Rc::clone(ctx_env));
                child_env.bind("%%".into(), val.clone());
                if is_join {
                    child_env.bind("%%j".into(), Value::Bool(true));
                }
                if let Some(ref var_name) = index_var {
                    child_env.bind(var_name.clone(), Value::Number(j as f64));
                }
                if let Some(ref var_name) = focus_var {
                    child_env.bind(var_name.clone(), elem.clone());
                }
                let ctx_value = if is_join { val.clone() } else { elem.clone() };
                next.push((ctx_value, Rc::new(child_env)));
            }
        }
        ctxs = next;
        if ctxs.is_empty() {
            return Ok(vec![]);
        }
    }
    Ok(ctxs)
}

/// Expand prefix contexts via the final step with parent tracking.
fn expand_last_step(
    arena: &AstArena,
    last_step: NodeId,
    prefix_ctxs: &[(Value, Rc<Environment>)],
) -> Result<Vec<(Value, Rc<Environment>)>, JsonataError> {
    let mut ctxs: Vec<(Value, Rc<Environment>)> = Vec::new();
    for (val, ctx_env) in prefix_ctxs {
        let val = collapse_val(val);
        if val.is_undefined() {
            continue;
        }
        let result = eval_path_step(arena, last_step, &val, ctx_env, false, false)?;
        if result.is_undefined() {
            continue;
        }
        let (index_var, focus_var) = get_step_bindings(arena, last_step);
        let is_join = focus_var.is_some();
        let items = flatten_to_vec(result);
        for (j, elem) in items.iter().enumerate() {
            let child_env = Environment::new_child(Rc::clone(ctx_env));
            child_env.bind("%%".into(), val.clone());
            if is_join {
                child_env.bind("%%j".into(), Value::Bool(true));
            }
            if let Some(ref var_name) = index_var {
                child_env.bind(var_name.clone(), Value::Number(j as f64));
            }
            if let Some(ref var_name) = focus_var {
                child_env.bind(var_name.clone(), elem.clone());
            }
            let ctx_value = if is_join { val.clone() } else { elem.clone() };
            ctxs.push((ctx_value, Rc::new(child_env)));
        }
    }
    Ok(ctxs)
}

fn compare_sort_terms(
    arena: &AstArena,
    terms: &[crate::parser::SortTerm],
    a: &Value,
    b: &Value,
    a_env: &Rc<Environment>,
    b_env: &Rc<Environment>,
) -> Result<i8, JsonataError> {
    for term in terms {
        let av = eval_fast_inner(arena, term.expression, a, a_env)?;
        let bv = eval_fast_inner(arena, term.expression, b, b_env)?;
        let cmp = av.compare_order(&bv)?;
        if cmp != 0 {
            return if term.descending { Ok(-cmp) } else { Ok(cmp) };
        }
    }
    Ok(0)
}

// ── Transform expression (|pattern|update,delete|) ──────────────────

// Consistent return type with eval dispatch table.
#[allow(clippy::unnecessary_wraps)]
fn eval_transform(
    arena: &AstArena,
    node: NodeId,
    _input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    // Transform returns a function that, when applied to input, performs the transformation.
    let node_id = node;
    let env_clone = Rc::clone(env);
    let func: Rc<BuiltinFn> = Rc::new(move |args: &[Value], focus: &Value| {
        let doc = if !args.is_empty() && !args[0].is_undefined() {
            &args[0]
        } else {
            focus
        };
        // We need the arena to evaluate sub-expressions, but BuiltinFn doesn't have it.
        // Return a placeholder — actual transform needs EnvAwareBuiltin.
        // For now, return the doc unchanged as a workaround.
        let _ = (&env_clone, node_id);
        Ok(doc.clone())
    });

    // Actually, transform needs arena access. Use EnvAwareBuiltin pattern instead.
    // Let's implement it inline since we have arena access here.
    // The Go impl returns a BuiltinFunction that captures the node — but we can
    // directly evaluate if the transform is called immediately in a path context.
    // For standalone transform evaluation, we need the function approach.

    // For direct evaluation (transform expression applied to input):
    let (pattern, update, delete) = match arena.get(node) {
        Expr::Transform {
            pattern,
            update,
            delete,
            ..
        } => (*pattern, *update, *delete),
        _ => unreachable!(),
    };

    let _ = func; // unused — we use EnvAwareBuiltin approach

    // Return a function value that performs the transform when called.
    // Go equivalent: if len(args) > 0 { doc = args[0] } else { doc = focus }
    // When called via the pipe operator (~>), piped is args[0].
    // If piped is undefined (e.g. `foo ~> |...|` where foo is missing), we pass
    // undefined to apply_transform which immediately returns undefined.
    // When called standalone (no args), we use focus (the current input context).
    let env_for_fn = Rc::clone(env);
    let transform_fn: Rc<crate::evaluator::EnvAwareBuiltinFn> = Rc::new(
        move |args: &[Value], focus: &Value, _env: &Rc<Environment>, arena: &AstArena| {
            let doc = if args.is_empty() {
                focus.clone()
            } else {
                args[0].clone()
            };
            apply_transform(arena, pattern, update, delete, &doc, &env_for_fn)
        },
    );

    Ok(Value::Function(FunctionValue::EnvAwareBuiltin(
        transform_fn,
    )))
}

fn deep_clone(v: &Value) -> Value {
    match v {
        Value::Object(obj) => {
            let cloned: indexmap::IndexMap<String, Value> = obj
                .iter()
                .map(|(k, v)| (k.clone(), deep_clone(v)))
                .collect();
            Value::Object(Rc::new(cloned))
        }
        Value::Array(arr) => Value::Array(Rc::new(arr.iter().map(deep_clone).collect())),
        other => other.clone(),
    }
}

fn apply_transform(
    arena: &AstArena,
    pattern: NodeId,
    update: NodeId,
    delete: Option<NodeId>,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    if input.is_undefined() {
        return Ok(Value::Undefined);
    }
    let cloned = deep_clone(input);

    let matched = eval_fast_inner(arena, pattern, &cloned, env)?;

    // Collect target objects from the matched result.
    // These are VALUE copies of the objects found at matched positions.
    // Sequences may appear if the pattern expression produces one as its final result.
    let matched = match matched {
        Value::Sequence(seq) => seq.collapse(),
        other => other,
    };
    let mut targets: Vec<Value> = Vec::new();
    match &matched {
        Value::Object(_) => targets.push(matched.clone()),
        Value::Array(arr) => {
            for item in arr.iter() {
                if item.is_object() {
                    targets.push(item.clone());
                }
            }
        }
        _ => {}
    }

    if targets.is_empty() && !matched.is_undefined() {
        // Non-object match: validate clauses but don't mutate.
        validate_transform_clauses(arena, update, delete, &cloned, env)?;
        return Ok(cloned);
    }

    // For each matched target, compute what it looks like AFTER applying the transform.
    // Then walk the clone and replace every occurrence of the original target value
    // with its updated version. This mirrors Go's pointer-based mutation: since Go
    // eval returns pointers into the clone, mutating them mutates the clone directly.
    // In Rust we use value equality to locate the same structural positions.
    let mut replacements: Vec<(Value, Value)> = Vec::new();
    for target in &targets {
        let updated = compute_updated_object(arena, update, delete, target, env)?;
        replacements.push((target.clone(), updated));
    }

    let mut result = cloned;
    for (original, updated) in &replacements {
        replace_in_value(&mut result, original, updated);
    }
    Ok(result)
}

/// Compute the updated form of a single matched object by applying update and delete clauses.
/// The update/delete expressions are evaluated with the original (pre-update) object as context,
/// matching Go's behavior where Eval(node.Update, target, env) uses the unmodified target.
fn compute_updated_object(
    arena: &AstArena,
    update: NodeId,
    delete: Option<NodeId>,
    target: &Value,
    env: &Rc<Environment>,
) -> Result<Value, JsonataError> {
    let mut result = target.clone();

    if !update.is_empty() {
        // Evaluate update expression with the original target as context.
        let update_val = eval_fast_inner(arena, update, target, env)?;
        if !update_val.is_undefined() && !update_val.is_null() {
            if let Value::Object(updates) = update_val {
                if let Value::Object(ref mut obj) = result {
                    let obj = Rc::make_mut(obj);
                    for (k, v) in updates.iter() {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            } else {
                return Err(JsonataError::new(
                    "T2011",
                    "the insert/update clause of the transform expression must evaluate to an object",
                ));
            }
        }
    }

    if let Some(del) = delete {
        // Evaluate delete expression with the original target as context (pre-update),
        // matching Go: Eval(node.Delete, target, env) where target is the original object.
        let delete_val = eval_fast_inner(arena, del, target, env)?;
        if !delete_val.is_undefined() && !delete_val.is_null() {
            match delete_val {
                Value::String(key) => {
                    if let Value::Object(ref mut obj) = result {
                        Rc::make_mut(obj).shift_remove(&*key);
                    }
                }
                Value::Array(keys) => {
                    if let Value::Object(ref mut obj) = result {
                        let obj = Rc::make_mut(obj);
                        for k in keys.iter() {
                            if let Value::String(key) = k {
                                obj.shift_remove(&**key);
                            }
                        }
                    }
                }
                _ => {
                    return Err(JsonataError::new(
                        "T2012",
                        "the delete clause of the transform expression must evaluate to an array of strings",
                    ));
                }
            }
        }
    }

    Ok(result)
}

/// Recursively walk `value` and replace every occurrence of `original` (by value equality)
/// with `replacement`. This is how we apply mutations from Go's pointer-based approach
/// in Rust's owned-value model.
fn replace_in_value(value: &mut Value, original: &Value, replacement: &Value) {
    // Check if the current value itself matches the original (before borrowing internals).
    if value == original {
        *value = replacement.clone();
        return;
    }
    match value {
        Value::Array(arr) => {
            for item in Rc::make_mut(arr).iter_mut() {
                replace_in_value(item, original, replacement);
            }
        }
        Value::Object(obj) => {
            for v in Rc::make_mut(obj).values_mut() {
                replace_in_value(v, original, replacement);
            }
        }
        _ => {}
    }
}

fn validate_transform_clauses(
    arena: &AstArena,
    update: NodeId,
    delete: Option<NodeId>,
    target: &Value,
    env: &Rc<Environment>,
) -> Result<(), JsonataError> {
    if !update.is_empty() {
        let update_val = eval_fast_inner(arena, update, target, env)?;
        if !update_val.is_undefined() && !update_val.is_null() && !update_val.is_object() {
            return Err(JsonataError::new(
                "T2011",
                "the insert/update clause of the transform expression must evaluate to an object",
            ));
        }
    }
    if let Some(del) = delete {
        let delete_val = eval_fast_inner(arena, del, target, env)?;
        if !delete_val.is_undefined() && !delete_val.is_null() {
            match &delete_val {
                Value::Array(_) | Value::String(_) => {}
                _ => {
                    return Err(JsonataError::new(
                        "T2012",
                        "the delete clause of the transform expression must evaluate to an array of strings",
                    ));
                }
            }
        }
    }
    Ok(())
}

// ── Group-by expression ({key:val}) ─────────────────────────────────

// Large dispatch function for group-by evaluation.
#[allow(clippy::too_many_lines)]
fn eval_group_by(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    // Extract group pairs from the node.
    let group = match arena.get(node) {
        Expr::Name { group: Some(g), .. } => g.clone(),
        Expr::Path { group: Some(g), .. } => g.clone(),
        Expr::Variable { group: Some(g), .. } => g.clone(),
        Expr::Function { group: Some(g), .. } => g.clone(),
        _ => return eval_fast_inner(arena, node, input, env),
    };

    // Evaluate the base expression without the group-by reduction.
    // We dispatch based on node type to avoid recursion back into eval_group_by.
    let base = match arena.get(node) {
        Expr::Name { value, .. } => eval_name(value, input)?,
        Expr::Path { .. } => eval_path(arena, node, input, env)?,
        Expr::Variable { name, .. } => eval_variable(name, input, env)?,
        Expr::Function { .. } => eval_function(arena, node, input, env)?,
        _ => eval_fast_inner(arena, node, input, env)?,
    };
    if base.is_undefined() {
        return Ok(Value::Undefined);
    }

    let items: Vec<Value> = match base {
        Value::Array(a) => (*a).clone(),
        Value::Sequence(seq) => {
            let collapsed = seq.collapse();
            match collapsed {
                Value::Undefined => return Ok(Value::Undefined),
                Value::Array(a) => (*a).clone(),
                other => vec![other],
            }
        }
        other => vec![other],
    };

    let mut out_obj = indexmap::IndexMap::new();
    let mut key_set = std::collections::HashSet::new();

    for pair in &group.pairs {
        let key_node = pair[0];
        let val_node = pair[1];
        let mut group_order: Vec<String> = Vec::new();
        let mut groups: std::collections::HashMap<String, (Vec<Value>, usize)> =
            std::collections::HashMap::new();

        for (i, item) in items.iter().enumerate() {
            let key_val = eval_fast_inner(arena, key_node, item, env)?;
            if key_val.is_undefined() || key_val.is_null() {
                continue;
            }
            let key_str: String = match &key_val {
                Value::String(s) => s.to_string(),
                _ => {
                    return Err(JsonataError::new(
                        "T1003",
                        "key expression must evaluate to a string",
                    ));
                }
            };
            if let Some(entry) = groups.get_mut(&key_str) {
                entry.0.push(item.clone());
            } else {
                group_order.push(key_str.clone());
                groups.insert(key_str, (vec![item.clone()], i));
            }
        }

        for key_str in &group_order {
            if key_set.contains(key_str) {
                return Err(JsonataError::new(
                    "D1009",
                    format!("duplicate key: \"{key_str}\""),
                ));
            }
            let (group_items, first_idx) = groups.get(key_str.as_str()).ok_or_else(|| JsonataError::new("D0000", "key from group_order must exist in groups"))?;
            let group_input = if group_items.len() == 1 {
                group_items[0].clone()
            } else {
                Value::Array(Rc::new(group_items.clone()))
            };

            let child_env = Environment::new_child(Rc::clone(env));
            child_env.bind("index".into(), Value::Number(*first_idx as f64));
            child_env.bind("key".into(), Value::String(key_str.as_str().into()));
            let child_env = Rc::new(child_env);

            let mut val_result = if val_node.is_empty() {
                group_input
            } else {
                eval_fast_inner(arena, val_node, &group_input, &child_env)?
            };

            // Apply keep_array wrapping for value nodes with [] suffix.
            let val_keep_array = match arena.get(val_node) {
                Expr::Name { keep_array, .. }
                | Expr::Binary { keep_array, .. }
                | Expr::Variable { keep_array, .. }
                | Expr::Function { keep_array, .. }
                | Expr::Sort { keep_array, .. }
                | Expr::Unary { keep_array, .. } => *keep_array,
                Expr::Path {
                    keep_singleton_array,
                    ..
                } => *keep_singleton_array,
                _ => false,
            };
            if val_keep_array {
                val_result = match val_result {
                    Value::Undefined => Value::Array(Rc::new(vec![])),
                    Value::Array(_) => val_result,
                    scalar => Value::Array(Rc::new(vec![scalar])),
                };
            }

            if !val_result.is_undefined() {
                key_set.insert(key_str.clone());
                out_obj.insert(key_str.clone(), val_result);
            }
        }
    }

    Ok(Value::Object(Rc::new(out_obj)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use crate::parser::{Parser, process_ast};

    /// Helper: parse, process, and evaluate.
    fn eval_expr(src: &str, input: &Value) -> JsonataResult {
        let (mut arena, root) = Parser::parse(src).expect("parse failed");
        let root = process_ast(&mut arena, root).expect("process failed");
        let mut env = Environment::new();
        crate::stdlib::register_all(&mut env);
        if !input.is_undefined() {
            env.bind("$".into(), input.clone());
        }
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
            Value::Array(Rc::new(vec![Value::String("A".into()), Value::String("B".into())]))
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
            Value::Array(Rc::new(vec![Value::Number(10.0), Value::Number(20.0)]))
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
            Value::Array(Rc::new(vec![Value::Number(3.0), Value::Number(4.0)]))
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
            Value::Array(Rc::new(vec![Value::Number(1.0), Value::Number(2.0)]))
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
            Value::Array(Rc::new(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0)
            ]))
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
            Value::Array(Rc::new(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
                Value::Number(5.0),
            ]))
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
        // null is a value, not undefined — ?? returns null.
        assert_eq!(eval_simple("null ?? 0"), Value::Null);
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
            Value::Array(Rc::new(vec![
                Value::String("a".into()),
                Value::String("b".into()),
                Value::String("c".into()),
            ]))
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
            Value::Array(Rc::new(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
            ]))
        );
    }

    #[test]
    fn stdlib_reverse() {
        assert_eq!(
            eval_simple("$reverse([1, 2, 3])"),
            Value::Array(Rc::new(vec![
                Value::Number(3.0),
                Value::Number(2.0),
                Value::Number(1.0),
            ]))
        );
    }

    #[test]
    fn stdlib_keys() {
        let result = eval_with_data("$keys($)", r#"{"a": 1, "b": 2}"#);
        assert_eq!(
            result,
            Value::Array(Rc::new(vec![Value::String("a".into()), Value::String("b".into()),]))
        );
    }

    #[test]
    fn stdlib_values() {
        let result = eval_with_data("$values($)", r#"{"a": 1, "b": 2}"#);
        assert_eq!(
            result,
            Value::Array(Rc::new(vec![Value::Number(1.0), Value::Number(2.0)]))
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
        // $map returns a Sequence that collapses to Array for 3+ elements.
        let result = eval_simple("$map([1, 2, 3], function($v){$v * 2})");
        let collapsed = match result {
            Value::Sequence(seq) => seq.collapse(),
            other => other,
        };
        assert_eq!(
            collapsed,
            Value::Array(Rc::new(vec![
                Value::Number(2.0),
                Value::Number(4.0),
                Value::Number(6.0),
            ]))
        );
    }

    #[test]
    fn stdlib_filter() {
        assert_eq!(
            eval_simple("$filter([1, 2, 3, 4], function($v){$v > 2})"),
            Value::Array(Rc::new(vec![Value::Number(3.0), Value::Number(4.0)]))
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
            Value::Array(Rc::new(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
            ]))
        );
    }

    #[test]
    fn stdlib_distinct() {
        assert_eq!(
            eval_simple("$distinct([1, 2, 2, 3, 1])"),
            Value::Array(Rc::new(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
            ]))
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
            Value::Array(Rc::new(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
                Value::Number(5.0),
            ]))
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

    // ── Sort expression ─────────────────────────────────────────

    #[test]
    fn sort_ascending() {
        let result = eval_with_data(
            "items^(value)",
            r#"{"items": [{"value": 3}, {"value": 1}, {"value": 2}]}"#,
        );
        match result {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 3);
                // Check first and last.
                assert_eq!(arr[0], Value::from_json(serde_json::json!({"value": 1})));
                assert_eq!(arr[2], Value::from_json(serde_json::json!({"value": 3})));
            }
            other => panic!("expected Array, got {:?}", other),
        }
    }

    #[test]
    fn sort_descending() {
        let result = eval_with_data(
            "items^(>value)",
            r#"{"items": [{"value": 1}, {"value": 3}, {"value": 2}]}"#,
        );
        match result {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 3);
                assert_eq!(arr[0], Value::from_json(serde_json::json!({"value": 3})));
                assert_eq!(arr[2], Value::from_json(serde_json::json!({"value": 1})));
            }
            other => panic!("expected Array, got {:?}", other),
        }
    }

    // ── Group-by expression ─────────────────────────────────────

    #[test]
    fn group_by_simple() {
        let result = eval_with_data(
            "items{color: $}",
            r#"{"items": [
                {"color": "red", "name": "a"},
                {"color": "blue", "name": "b"},
                {"color": "red", "name": "c"}
            ]}"#,
        );
        match result {
            Value::Object(obj) => {
                assert!(obj.contains_key("red"));
                assert!(obj.contains_key("blue"));
                // red group has 2 items.
                match obj.get("red") {
                    Some(Value::Array(arr)) => assert_eq!(arr.len(), 2),
                    other => panic!("expected Array for red group, got {:?}", other),
                }
            }
            other => panic!("expected Object, got {:?}", other),
        }
    }

    #[test]
    fn group_by_path() {
        // Test group-by on a path expression: Account.Order{OrderID: ...}
        let data = r#"{"Account": {"Order": [
            {"OrderID": "A", "Value": 10},
            {"OrderID": "B", "Value": 20},
            {"OrderID": "A", "Value": 30}
        ]}}"#;
        let result = eval_with_data(r#"Account.Order{OrderID: Value}"#, data);
        match &result {
            Value::Object(obj) => {
                assert!(obj.contains_key("A"), "expected key A, got {:?}", result);
                assert!(obj.contains_key("B"), "expected key B, got {:?}", result);
            }
            other => panic!("expected Object, got {:?}", other),
        }
    }

    #[test]
    fn group_by_variable() {
        // Test group-by on a $$ variable (case026)
        let result = eval_expr(r#"$${id: value}"#, &Value::from_json_str("[]").unwrap()).unwrap();
        assert_eq!(result, Value::Object(Rc::new(indexmap::IndexMap::new())));
    }

    // ── Variable binding in blocks ──────────────────────────────

    #[test]
    fn variable_binding_in_block() {
        // Test that $x := 5 works in a block context.
        assert_eq!(eval_simple("($x := 5; $x + 1)"), Value::Number(6.0));
    }

    #[test]
    fn lambda_with_recursion() {
        // Test tail-call optimized recursion.
        assert_eq!(
            eval_simple("($f := function($n){$n <= 0 ? 0 : $f($n - 1)}; $f(100))"),
            Value::Number(0.0)
        );
    }
}
