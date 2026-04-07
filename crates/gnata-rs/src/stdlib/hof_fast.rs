//! Fast-path lambda dispatch for HOFs.
//!
//! Analyzes lambda AST at the HOF call site. When the body is a simple
//! expression (field access, comparison, arithmetic on fields), bypasses
//! full call_function dispatch and evaluates directly against the item.
//!
//! This eliminates per-call overhead: environment creation, parameter binding,
//! Value cloning for function args, and stack checks.

use crate::parser::ast::{AstArena, BinaryOp, Expr, NodeId};
use crate::value::Value;

/// A simple lambda pattern that can be evaluated without full dispatch.
#[derive(Debug)]
pub enum SimpleLambda {
    /// function($v) { $v.field } — direct field access
    FieldAccess {
        param: String,
        field: String,
    },
    /// function($v) { $v.field op literal } — field compared to constant
    FieldPredicate {
        param: String,
        field: String,
        op: BinaryOp,
        literal: Value,
    },
    /// function($v) { $v.field = $v.field2 } — two fields compared
    TwoFieldPredicate {
        param: String,
        field1: String,
        op: BinaryOp,
        field2: String,
    },
    /// function($a, $b) { $a.field > $b.field } — sort comparator on field
    SortComparator {
        param_a: String,
        param_b: String,
        field: String,
    },
    /// function($a, $b) { $a.field op $b.field } — sort comparator with any relational op
    SortComparatorOp {
        param_a: String,
        param_b: String,
        field: String,
        op: BinaryOp,
    },
    /// function($prev, $curr) { $prev op $curr.field } — simple reduce accumulator
    ReduceAccum {
        param_prev: String,
        param_curr: String,
        field: String,
        op: BinaryOp,
    },
}

/// Try to analyze a lambda body into a SimpleLambda for fast dispatch.
pub fn analyze_lambda(params: &[String], body: NodeId, arena: &AstArena) -> Option<SimpleLambda> {
    let expr = arena.get(body);
    match expr {
        // Body is a path: $v.field
        Expr::Path { steps, .. } if steps.len() == 2 => {
            analyze_field_access(params, &steps[0], &steps[1], arena)
        }
        // Body is a binary op
        Expr::Binary { op, lhs, rhs, .. } => {
            analyze_binary(params, *op, *lhs, *rhs, arena)
        }
        _ => None,
    }
}

/// Check if a node is `$param` (variable reference matching a parameter name).
fn is_param_ref(node: NodeId, arena: &AstArena, param: &str) -> bool {
    matches!(arena.get(node), Expr::Variable { name, .. } if name == param)
}

/// Check if a path is `$param.field` and return the field name.
fn extract_param_field(steps: &[NodeId], arena: &AstArena, param: &str) -> Option<String> {
    if steps.len() != 2 {
        return None;
    }
    if !is_param_ref(steps[0], arena, param) {
        return None;
    }
    match arena.get(steps[1]) {
        Expr::Name { value, stages, group, focus, index, .. }
            if stages.is_empty() && group.is_none() && focus.is_none() && index.is_none() =>
        {
            Some(value.clone())
        }
        _ => None,
    }
}

/// Try to extract a field access pattern: function($v) { $v.field }
fn analyze_field_access(
    params: &[String],
    step0: &NodeId,
    step1: &NodeId,
    arena: &AstArena,
) -> Option<SimpleLambda> {
    if params.is_empty() {
        return None;
    }
    let param = &params[0];
    if !is_param_ref(*step0, arena, param) {
        return None;
    }
    match arena.get(*step1) {
        Expr::Name { value, stages, group, focus, index, .. }
            if stages.is_empty() && group.is_none() && focus.is_none() && index.is_none() =>
        {
            Some(SimpleLambda::FieldAccess {
                param: param.clone(),
                field: value.clone(),
            })
        }
        _ => None,
    }
}

/// Extract a literal value from a node.
fn extract_literal(node: NodeId, arena: &AstArena) -> Option<Value> {
    match arena.get(node) {
        Expr::NumberLit { value, .. } => Some(Value::Number(*value)),
        Expr::StringLit { value, .. } => Some(Value::String(value.clone().into())),
        Expr::ValueLit { value, .. } => match value.as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            "null" => Some(Value::Null),
            _ => None,
        },
        _ => None,
    }
}

/// Try to extract `$param.field` from a node that's either a Path or an inlined Name.
fn extract_param_dot_field(node: NodeId, arena: &AstArena, param: &str) -> Option<String> {
    match arena.get(node) {
        Expr::Path { steps, .. } => extract_param_field(steps, arena, param),
        _ => None,
    }
}

/// Analyze a binary expression in a lambda body.
fn analyze_binary(
    params: &[String],
    op: BinaryOp,
    lhs: NodeId,
    rhs: NodeId,
    arena: &AstArena,
) -> Option<SimpleLambda> {
    // Sort comparator: function($a, $b) { $a.field > $b.field }
    if params.len() == 2 && is_relational(op) {
        let param_a = &params[0];
        let param_b = &params[1];
        if let (Some(field_a), Some(field_b)) = (
            extract_param_dot_field(lhs, arena, param_a),
            extract_param_dot_field(rhs, arena, param_b),
        ) {
            if field_a == field_b {
                return Some(SimpleLambda::SortComparator {
                    param_a: param_a.clone(),
                    param_b: param_b.clone(),
                    field: field_a,
                });
            }
            return Some(SimpleLambda::SortComparatorOp {
                param_a: param_a.clone(),
                param_b: param_b.clone(),
                field: field_a,
                op,
            });
        }
    }

    // Reduce accumulator: function($prev, $curr) { $prev + $curr.field }
    if params.len() >= 2 && is_arithmetic(op) {
        let param_prev = &params[0];
        let param_curr = &params[1];
        if is_param_ref(lhs, arena, param_prev) {
            if let Some(field) = extract_param_dot_field(rhs, arena, param_curr) {
                return Some(SimpleLambda::ReduceAccum {
                    param_prev: param_prev.clone(),
                    param_curr: param_curr.clone(),
                    field,
                    op,
                });
            }
        }
    }

    // Field predicate: function($v) { $v.field op literal }
    if !params.is_empty() {
        let param = &params[0];

        // $v.field op literal
        if let Some(field) = extract_param_dot_field(lhs, arena, param) {
            if let Some(lit) = extract_literal(rhs, arena) {
                return Some(SimpleLambda::FieldPredicate {
                    param: param.clone(),
                    field,
                    op,
                    literal: lit,
                });
            }
            // $v.field1 op $v.field2
            if let Some(field2) = extract_param_dot_field(rhs, arena, param) {
                return Some(SimpleLambda::TwoFieldPredicate {
                    param: param.clone(),
                    field1: field,
                    op,
                    field2,
                });
            }
        }

        // literal op $v.field (reversed)
        if let Some(lit) = extract_literal(lhs, arena) {
            if let Some(field) = extract_param_dot_field(rhs, arena, param) {
                return Some(SimpleLambda::FieldPredicate {
                    param: param.clone(),
                    field,
                    op: flip_relational(op),
                    literal: lit,
                });
            }
        }
    }

    None
}

fn is_relational(op: BinaryOp) -> bool {
    matches!(op, BinaryOp::Gt | BinaryOp::Lt | BinaryOp::Ge | BinaryOp::Le | BinaryOp::Eq | BinaryOp::Ne)
}

fn is_arithmetic(op: BinaryOp) -> bool {
    matches!(op, BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod)
}

fn flip_relational(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Gt => BinaryOp::Lt,
        BinaryOp::Lt => BinaryOp::Gt,
        BinaryOp::Ge => BinaryOp::Le,
        BinaryOp::Le => BinaryOp::Ge,
        other => other, // Eq, Ne are symmetric
    }
}

// ── Fast-path evaluation helpers ────────────────────────────────────────────

/// Get a field value from an object without cloning (returns reference-safe clone).
#[inline]
pub fn get_field(item: &Value, field: &str) -> Value {
    match item {
        Value::Object(obj) => obj.get(field).cloned().unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    }
}

/// Evaluate a binary op on two Values (for predicates and comparators).
#[inline]
pub fn eval_binary_simple(lhs: &Value, op: BinaryOp, rhs: &Value) -> Value {
    match op {
        BinaryOp::Gt => compare_simple(lhs, rhs, |a, b| a > b, |a, b| a > b),
        BinaryOp::Lt => compare_simple(lhs, rhs, |a, b| a < b, |a, b| a < b),
        BinaryOp::Ge => compare_simple(lhs, rhs, |a, b| a >= b, |a, b| a >= b),
        BinaryOp::Le => compare_simple(lhs, rhs, |a, b| a <= b, |a, b| a <= b),
        BinaryOp::Eq => Value::Bool(lhs.deep_equal(rhs)),
        BinaryOp::Ne => Value::Bool(!lhs.deep_equal(rhs)),
        BinaryOp::Add => arithmetic_simple(lhs, rhs, |a, b| a + b),
        BinaryOp::Sub => arithmetic_simple(lhs, rhs, |a, b| a - b),
        BinaryOp::Mul => arithmetic_simple(lhs, rhs, |a, b| a * b),
        BinaryOp::Div => arithmetic_simple(lhs, rhs, |a, b| if b != 0.0 { a / b } else { f64::NAN }),
        BinaryOp::Mod => arithmetic_simple(lhs, rhs, |a, b| if b != 0.0 { a % b } else { f64::NAN }),
        _ => Value::Undefined,
    }
}

#[inline]
fn compare_simple(
    lhs: &Value,
    rhs: &Value,
    num_cmp: impl Fn(f64, f64) -> bool,
    str_cmp: impl Fn(&str, &str) -> bool,
) -> Value {
    match (lhs, rhs) {
        (Value::Number(a), Value::Number(b)) => Value::Bool(num_cmp(*a, *b)),
        (Value::String(a), Value::String(b)) => Value::Bool(str_cmp(a.as_str(), b.as_str())),
        _ => Value::Undefined,
    }
}

#[inline]
fn arithmetic_simple(lhs: &Value, rhs: &Value, op: impl Fn(f64, f64) -> f64) -> Value {
    match (lhs, rhs) {
        (Value::Number(a), Value::Number(b)) => Value::Number(op(*a, *b)),
        _ => Value::Undefined,
    }
}

/// Compare two items by a field for sorting. Returns Ordering.
/// Used by the fast-path sort comparator.
#[inline]
pub fn compare_by_field(a: &Value, b: &Value, field: &str) -> std::cmp::Ordering {
    let va = get_field(a, field);
    let vb = get_field(b, field);
    match (&va, &vb) {
        (Value::Number(na), Value::Number(nb)) => na.partial_cmp(nb).unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(sa), Value::String(sb)) => sa.cmp(sb),
        (Value::Undefined, Value::Undefined) => std::cmp::Ordering::Equal,
        (Value::Undefined, _) => std::cmp::Ordering::Greater,
        (_, Value::Undefined) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
    }
}
