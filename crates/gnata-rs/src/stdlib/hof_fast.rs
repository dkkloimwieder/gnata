//! Fast-path lambda dispatch for HOFs.
//!
//! Analyzes lambda AST at the HOF call site. When the body is a simple
//! expression (field access, comparison, arithmetic on fields), bypasses
//! full call_function dispatch and evaluates directly against the item.
//!
//! This eliminates per-call overhead: environment creation, parameter binding,
//! Value cloning for function args, and stack checks.

use std::rc::Rc;

use crate::error::JsonataResult;
use crate::evaluator::{Environment, call_function};
use crate::evaluator::functions::FunctionValue;
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
    /// function($v) { $v.A & "lit" & $string($v.B) & ... } — concat template
    ConcatTemplate {
        pieces: Vec<TemplatePiece>,
    },
}

/// A piece of a concat template — evaluated into a string buffer.
#[derive(Debug, Clone)]
pub enum TemplatePiece {
    /// A string literal known at analysis time.
    Literal(String),
    /// A field access on the lambda parameter — appends the string value.
    Field(String),
    /// $string(field) — stringify the field value into the buffer.
    StringifyField(String),
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
        Expr::Binary { op: BinaryOp::Concat, .. } if !params.is_empty() => {
            // Try concat template first, fall back to generic binary analysis
            analyze_concat_template(params, body, arena)
                .or_else(|| {
                    let Expr::Binary { op, lhs, rhs, .. } = arena.get(body) else { return None; };
                    analyze_binary(params, *op, *lhs, *rhs, arena)
                })
        }
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

/// Analyze a concat chain in a lambda body into a ConcatTemplate.
/// Flattens left-recursive Concat(Concat(a, b), c) → [a, b, c] and
/// classifies each operand as Literal, Field, or StringifyField.
fn analyze_concat_template(
    params: &[String],
    body: NodeId,
    arena: &AstArena,
) -> Option<SimpleLambda> {
    let param = &params[0];
    let mut operand_nodes = Vec::new();
    collect_concat_nodes(arena, body, &mut operand_nodes);

    if operand_nodes.len() < 2 {
        return None;
    }

    let mut pieces = Vec::with_capacity(operand_nodes.len());
    for &node in &operand_nodes {
        if let Some(piece) = classify_template_operand(node, arena, param) {
            pieces.push(piece);
        } else {
            return None; // unsupported operand, bail
        }
    }

    Some(SimpleLambda::ConcatTemplate { pieces })
}

/// Walk a left-recursive Concat tree and collect leaf nodes.
fn collect_concat_nodes(arena: &AstArena, node: NodeId, out: &mut Vec<NodeId>) {
    if let Expr::Binary { op: BinaryOp::Concat, lhs, rhs, .. } = arena.get(node) {
        collect_concat_nodes(arena, *lhs, out);
        out.push(*rhs);
    } else {
        out.push(node);
    }
}

/// Classify a single concat operand into a TemplatePiece.
fn classify_template_operand(
    node: NodeId,
    arena: &AstArena,
    param: &str,
) -> Option<TemplatePiece> {
    match arena.get(node) {
        // String literal
        Expr::StringLit { value, .. } => Some(TemplatePiece::Literal(value.clone())),

        // $param.field — direct field access (value should be a string)
        Expr::Path { steps, .. } if steps.len() == 2 => {
            if let Some(field) = extract_param_field(steps, arena, param) {
                Some(TemplatePiece::Field(field))
            } else {
                None
            }
        }

        // $string($param.field) — stringify a field value
        Expr::Function { procedure, arguments, .. } if arguments.len() == 1 => {
            // Check procedure is $string
            let is_string_fn = matches!(
                arena.get(*procedure),
                Expr::Variable { name, .. } if name == "string"
            );
            if !is_string_fn {
                return None;
            }
            // Check argument is $param.field
            match arena.get(arguments[0]) {
                Expr::Path { steps, .. } if steps.len() == 2 => {
                    extract_param_field(steps, arena, param)
                        .map(TemplatePiece::StringifyField)
                }
                _ => None,
            }
        }

        _ => None,
    }
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

/// Evaluate a ConcatTemplate against an item, writing into a single buffer.
pub fn eval_concat_template(item: &Value, pieces: &[TemplatePiece]) -> Value {
    use crate::error::JsonataResult;
    let mut buf = String::new();
    for piece in pieces {
        match piece {
            TemplatePiece::Literal(s) => buf.push_str(s),
            TemplatePiece::Field(field) => {
                let v = get_field(item, field);
                if let Value::String(s) = &v {
                    buf.push_str(s);
                } else if !v.is_undefined() {
                    // Non-string field in concat — stringify it
                    if v.stringify_into(&mut buf).is_err() {
                        return Value::Undefined;
                    }
                }
            }
            TemplatePiece::StringifyField(field) => {
                let v = get_field(item, field);
                if !v.is_undefined() {
                    if v.stringify_into(&mut buf).is_err() {
                        return Value::Undefined;
                    }
                }
            }
        }
    }
    Value::String(buf.into())
}

// ── Lifted dispatch for mapped expressions ──────────────────────────────────

/// A pre-analyzed function call that can be dispatched efficiently per item.
/// Function resolution and constant arg evaluation happen once at analysis time.
#[derive(Debug)]
pub struct MappedCall {
    pub func: Box<FunctionValue>,
    pub arg_template: Vec<CallArg>,
}

/// Classification of a function argument for lifted dispatch.
#[derive(Debug, Clone)]
pub enum CallArg {
    /// $param.field — resolved per item via get_field
    Field(String),
    /// A constant value — evaluated once at analysis time
    Const(Value),
    /// A complex expression that can't be lifted — falls back to per-item eval
    Expr(NodeId),
}

/// Analyze a function call node in a mapped context.
/// `param` is the mapping variable name (e.g., the implicit scope in `.()` or the lambda param).
///
/// Returns Some(MappedCall) if the call can be lifted, None otherwise.
pub fn analyze_mapped_call(
    node: NodeId,
    arena: &AstArena,
    param: Option<&str>,
    env: &Rc<Environment>,
) -> Option<MappedCall> {
    // The node should be a Function call, possibly wrapped in a Block.
    let func_node = unwrap_block(node, arena);

    let (procedure, arguments) = match arena.get(func_node) {
        Expr::Function { procedure, arguments, .. } => (*procedure, arguments.clone()),
        _ => return None,
    };

    // Resolve the function from the environment.
    // procedure is typically a Variable node ($formatNumber → Variable { name: "formatNumber" })
    let func_name = match arena.get(procedure) {
        Expr::Variable { name, .. } => name.as_str(),
        _ => return None,
    };
    let func_val = env.lookup(func_name)?;
    let func = match func_val {
        Value::Function(f) => f,
        _ => return None,
    };

    // Classify each argument.
    let mut arg_template = Vec::with_capacity(arguments.len());
    for &arg_node in &arguments {
        arg_template.push(classify_call_arg(arg_node, arena, param));
    }

    // Only worth lifting if at least one arg is a Field (otherwise nothing varies per item).
    let has_field = arg_template.iter().any(|a| matches!(a, CallArg::Field(_)));
    if !has_field {
        return None;
    }

    // Bail if any arg is a complex Expr — we can't fully lift.
    // (Could still partially lift, but keep it simple for now.)
    let has_complex = arg_template.iter().any(|a| matches!(a, CallArg::Expr(_)));
    if has_complex {
        return None;
    }

    Some(MappedCall {
        func,
        arg_template,
    })
}

/// Unwrap a single-expression Block to get the inner expression.
fn unwrap_block(node: NodeId, arena: &AstArena) -> NodeId {
    if let Expr::Block { expressions, .. } = arena.get(node) {
        if expressions.len() == 1 {
            return expressions[0];
        }
    }
    node
}

/// Classify a function argument as Field, Const, or Expr.
fn classify_call_arg(node: NodeId, arena: &AstArena, param: Option<&str>) -> CallArg {
    match arena.get(node) {
        // String literal
        Expr::StringLit { value, .. } => CallArg::Const(Value::String(value.clone().into())),

        // Number literal
        Expr::NumberLit { value, .. } => CallArg::Const(Value::Number(*value)),

        // Boolean/null literal
        Expr::ValueLit { value, .. } => match value.as_str() {
            "true" => CallArg::Const(Value::Bool(true)),
            "false" => CallArg::Const(Value::Bool(false)),
            "null" => CallArg::Const(Value::Null),
            _ => CallArg::Expr(node),
        },

        // $param.field or just FieldName (implicit scope)
        Expr::Path { steps, .. } if steps.len() == 2 => {
            if let Some(p) = param {
                if let Some(field) = extract_param_field(steps, arena, p) {
                    return CallArg::Field(field);
                }
            }
            CallArg::Expr(node)
        }

        // Bare field name (in .() mapping context, no explicit param)
        Expr::Name { value, stages, group, focus, index, .. }
            if stages.is_empty() && group.is_none() && focus.is_none() && index.is_none()
                && param.is_none() =>
        {
            CallArg::Field(value.clone())
        }

        // Bare $param reference (the whole object)
        Expr::Variable { name, .. } if param.is_some() && name == param.unwrap() => {
            // Pass the entire item — represented as Field("") which we handle specially
            CallArg::Expr(node) // TODO: could add a WholeItem variant
        }

        _ => CallArg::Expr(node),
    }
}

/// Execute a MappedCall for a single item. Function is already resolved,
/// constant args are already evaluated. Only field args need per-item work.
pub fn exec_mapped_call(
    mc: &MappedCall,
    item: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    // Build args from template — small vec, no heap alloc for <=4 args.
    let mut args: Vec<Value> = Vec::with_capacity(mc.arg_template.len());
    for arg in &mc.arg_template {
        match arg {
            CallArg::Field(name) => args.push(get_field(item, name)),
            CallArg::Const(val) => args.push(val.clone()),
            CallArg::Expr(_node) => unreachable!("complex args filtered out in analysis"),
        }
    }
    call_function(&mc.func, &args, item, env, arena)
}
