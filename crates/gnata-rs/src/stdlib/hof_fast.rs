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
use crate::evaluator::functions::FunctionValue;
use crate::evaluator::{Environment, call_function};
use crate::parser::ast::{AstArena, BinaryOp, Expr, NodeId};
use crate::value::Value;

/// A simple lambda pattern that can be evaluated without full dispatch.
#[derive(Debug)]
pub enum SimpleLambda {
    /// function($v) { $v.field } — direct field access
    FieldAccess { param: String, field: String },
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
    /// function($a, $b) { $a.field op $b.field } — sort comparator (same field both sides)
    SortComparator {
        param_a: String,
        param_b: String,
        field: String,
        op: BinaryOp,
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
    /// function($prev, $curr) { $prev op ($curr.field1 op2 $curr.field2) } — compound reduce
    ReduceCompoundAccum {
        param_prev: String,
        param_curr: String,
        field1: String,
        field2: String,
        outer_op: BinaryOp,
        inner_op: BinaryOp,
    },
    /// function($v) { $v.A & "lit" & $string($v.B) & ... } — concat template
    ConcatTemplate { pieces: Vec<TemplatePiece> },
    /// function($v) { $v.field1 op1 lit1 and/or $v.field2 op2 lit2 ... } — compound predicate
    CompoundPredicate {
        clauses: Vec<PredicateClause>,
        combiner: BinaryOp, // And or Or
    },
}

/// A single clause in a compound predicate: field op literal.
#[derive(Debug, Clone)]
pub struct PredicateClause {
    pub field: String,
    pub op: BinaryOp,
    pub literal: Value,
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
    /// $substring(field, start, len?) — extract substring from field value.
    SubstringField {
        field: String,
        start: i64,
        length: Option<usize>,
    },
    /// $lowercase(field) — lowercase the field value.
    LowercaseField(String),
    /// $uppercase(field) — uppercase the field value.
    UppercaseField(String),
}

/// Try to analyze a lambda body into a SimpleLambda for fast dispatch.
pub fn analyze_lambda(params: &[String], body: NodeId, arena: &AstArena) -> Option<SimpleLambda> {
    let expr = arena.get(body);
    match expr {
        // Body is a path: $v.field
        Expr::Path { steps, .. } if steps.len() == 2 => {
            analyze_field_access(params, steps[0], steps[1], arena)
        }
        // Body is a binary op
        Expr::Binary {
            op: BinaryOp::Concat,
            ..
        } if !params.is_empty() => {
            // Try concat template first, fall back to generic binary analysis
            analyze_concat_template(params, body, arena).or_else(|| {
                let Expr::Binary { op, lhs, rhs, .. } = arena.get(body) else {
                    return None;
                };
                analyze_binary(params, *op, *lhs, *rhs, arena)
            })
        }
        Expr::Binary { op, lhs, rhs, .. } => analyze_binary(params, *op, *lhs, *rhs, arena),
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
        Expr::Name {
            value,
            stages,
            group,
            focus,
            index,
            ..
        } if stages.is_empty() && group.is_none() && focus.is_none() && index.is_none() => {
            Some(value.clone())
        }
        _ => None,
    }
}

/// Try to extract a field access pattern: function($v) { $v.field }
fn analyze_field_access(
    params: &[String],
    step0: NodeId,
    step1: NodeId,
    arena: &AstArena,
) -> Option<SimpleLambda> {
    if params.is_empty() {
        return None;
    }
    let param = &params[0];
    if !is_param_ref(step0, arena, param) {
        return None;
    }
    match arena.get(step1) {
        Expr::Name {
            value,
            stages,
            group,
            focus,
            index,
            ..
        } if stages.is_empty() && group.is_none() && focus.is_none() && index.is_none() => {
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
                    op,
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
    // or compound: function($prev, $curr) { $prev + $curr.field1 * $curr.field2 }
    if params.len() >= 2 && is_arithmetic(op) {
        let param_prev = &params[0];
        let param_curr = &params[1];
        if is_param_ref(lhs, arena, param_prev) {
            // Simple: $prev op $curr.field
            if let Some(field) = extract_param_dot_field(rhs, arena, param_curr) {
                return Some(SimpleLambda::ReduceAccum {
                    param_prev: param_prev.clone(),
                    param_curr: param_curr.clone(),
                    field,
                    op,
                });
            }
            // Compound: $prev op ($curr.field1 inner_op $curr.field2)
            if let Expr::Binary {
                op: inner_op,
                lhs: inner_lhs,
                rhs: inner_rhs,
                ..
            } = arena.get(rhs)
            {
                if is_arithmetic(*inner_op) {
                    if let (Some(field1), Some(field2)) = (
                        extract_param_dot_field(*inner_lhs, arena, param_curr),
                        extract_param_dot_field(*inner_rhs, arena, param_curr),
                    ) {
                        return Some(SimpleLambda::ReduceCompoundAccum {
                            param_prev: param_prev.clone(),
                            param_curr: param_curr.clone(),
                            field1,
                            field2,
                            outer_op: op,
                            inner_op: *inner_op,
                        });
                    }
                }
            }
        }
    }

    // Compound predicate: function($v) { $v.field1 op1 lit1 and/or $v.field2 op2 lit2 }
    if !params.is_empty() && (op == BinaryOp::And || op == BinaryOp::Or) {
        let param = &params[0];
        let mut clauses = Vec::new();
        if collect_predicate_clauses(arena, lhs, param, op, &mut clauses)
            && collect_predicate_clauses(arena, rhs, param, op, &mut clauses)
            && clauses.len() >= 2
        {
            return Some(SimpleLambda::CompoundPredicate {
                clauses,
                combiner: op,
            });
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
        if let Some(lit) = extract_literal(lhs, arena)
            && let Some(field) = extract_param_dot_field(rhs, arena, param)
        {
            return Some(SimpleLambda::FieldPredicate {
                param: param.clone(),
                field,
                op: flip_relational(op),
                literal: lit,
            });
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
    if let Expr::Binary {
        op: BinaryOp::Concat,
        lhs,
        rhs,
        ..
    } = arena.get(node)
    {
        collect_concat_nodes(arena, *lhs, out);
        out.push(*rhs);
    } else {
        out.push(node);
    }
}

/// Classify a single concat operand into a TemplatePiece.
fn classify_template_operand(node: NodeId, arena: &AstArena, param: &str) -> Option<TemplatePiece> {
    match arena.get(node) {
        // String literal
        Expr::StringLit { value, .. } => Some(TemplatePiece::Literal(value.clone())),

        // $param.field — direct field access (value should be a string)
        Expr::Path { steps, .. } if steps.len() == 2 => {
            extract_param_field(steps, arena, param).map(TemplatePiece::Field)
        }

        // Function calls on fields: $string, $substring, $lowercase, $uppercase
        Expr::Function {
            procedure,
            arguments,
            ..
        } => {
            let func_name = match arena.get(*procedure) {
                Expr::Variable { name, .. } => name.as_str(),
                _ => return None,
            };
            match func_name {
                // $string($param.field)
                "string" if arguments.len() == 1 => {
                    extract_field_from_arg(arguments[0], arena, param)
                        .map(TemplatePiece::StringifyField)
                }
                // $lowercase($param.field)
                "lowercase" if arguments.len() == 1 => {
                    extract_field_from_arg(arguments[0], arena, param)
                        .map(TemplatePiece::LowercaseField)
                }
                // $uppercase($param.field)
                "uppercase" if arguments.len() == 1 => {
                    extract_field_from_arg(arguments[0], arena, param)
                        .map(TemplatePiece::UppercaseField)
                }
                // $substring($param.field, start [, length])
                "substring" if arguments.len() >= 2 && arguments.len() <= 3 => {
                    let field = extract_field_from_arg(arguments[0], arena, param)?;
                    let start = match arena.get(arguments[1]) {
                        Expr::NumberLit { value, .. } => *value as i64,
                        _ => return None,
                    };
                    let length = if arguments.len() == 3 {
                        match arena.get(arguments[2]) {
                            Expr::NumberLit { value, .. } if *value >= 0.0 => Some(*value as usize),
                            _ => return None,
                        }
                    } else {
                        None
                    };
                    Some(TemplatePiece::SubstringField {
                        field,
                        start,
                        length,
                    })
                }
                _ => None,
            }
        }

        _ => None,
    }
}

/// Extract a field name from a function argument that is $param.field.
fn extract_field_from_arg(arg: NodeId, arena: &AstArena, param: &str) -> Option<String> {
    match arena.get(arg) {
        Expr::Path { steps, .. } if steps.len() == 2 => extract_param_field(steps, arena, param),
        _ => None,
    }
}

/// Recursively collect field-op-literal clauses from an and/or tree.
/// Returns false if any leaf is not a simple field predicate.
fn collect_predicate_clauses(
    arena: &AstArena,
    node: NodeId,
    param: &str,
    combiner: BinaryOp,
    out: &mut Vec<PredicateClause>,
) -> bool {
    match arena.get(node) {
        // Same combiner: flatten nested and/or
        Expr::Binary { op, lhs, rhs, .. } if *op == combiner => {
            collect_predicate_clauses(arena, *lhs, param, combiner, out)
                && collect_predicate_clauses(arena, *rhs, param, combiner, out)
        }
        // Leaf: must be $param.field op literal (or literal op $param.field)
        Expr::Binary { op, lhs, rhs, .. } if is_relational(*op) || is_arithmetic(*op) => {
            if let Some(field) = extract_param_dot_field(*lhs, arena, param) {
                if let Some(lit) = extract_literal(*rhs, arena) {
                    out.push(PredicateClause {
                        field,
                        op: *op,
                        literal: lit,
                    });
                    return true;
                }
            }
            // Reversed: literal op $param.field
            if let Some(lit) = extract_literal(*lhs, arena) {
                if let Some(field) = extract_param_dot_field(*rhs, arena, param) {
                    out.push(PredicateClause {
                        field,
                        op: flip_relational(*op),
                        literal: lit,
                    });
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn is_relational(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Gt | BinaryOp::Lt | BinaryOp::Ge | BinaryOp::Le | BinaryOp::Eq | BinaryOp::Ne
    )
}

fn is_arithmetic(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod
    )
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
        BinaryOp::Div => {
            arithmetic_simple(lhs, rhs, |a, b| if b == 0.0 { f64::NAN } else { a / b })
        }
        BinaryOp::Mod => {
            arithmetic_simple(lhs, rhs, |a, b| if b == 0.0 { f64::NAN } else { a % b })
        }
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
        (Value::Number(na), Value::Number(nb)) => {
            na.partial_cmp(nb).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Value::String(sa), Value::String(sb)) => sa.cmp(sb),
        (Value::Undefined, Value::Undefined) => std::cmp::Ordering::Equal,
        (Value::Undefined, _) => std::cmp::Ordering::Greater,
        (_, Value::Undefined) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
    }
}

/// Like [`compare_by_field`], but surfaces JSONata sort type errors:
/// T2007 for string/number key mismatches, T2008 for non-sortable keys
/// (null, boolean, array, object). Missing (undefined) keys sort last
/// without error. Used by the `^()` operator fast path, which must match
/// the general path's error behavior (`Value::compare_order`).
///
/// # Errors
/// Returns T2007 or T2008 as described above.
pub fn compare_by_field_checked(
    a: &Value,
    b: &Value,
    field: &str,
) -> JsonataResult<std::cmp::Ordering> {
    let va = get_field(a, field);
    let vb = get_field(b, field);
    Ok(va.compare_order(&vb)?.cmp(&0))
}

/// Evaluate a ConcatTemplate against an item, writing into a single buffer.
pub fn eval_concat_template(item: &Value, pieces: &[TemplatePiece]) -> Value {
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
                if !v.is_undefined() && v.stringify_into(&mut buf).is_err() {
                    return Value::Undefined;
                }
            }
            TemplatePiece::SubstringField {
                field,
                start,
                length,
            } => {
                let v = get_field(item, field);
                if let Value::String(s) = &v {
                    let chars: Vec<char> = s.chars().collect();
                    let len = chars.len() as i64;
                    // JSONata $substring semantics: negative start counts from end
                    let start_idx = if *start < 0 {
                        (len + start).max(0) as usize
                    } else {
                        (*start as usize).min(chars.len())
                    };
                    let end_idx = match length {
                        Some(l) => (start_idx + l).min(chars.len()),
                        None => chars.len(),
                    };
                    if start_idx < end_idx {
                        let sub: String = chars[start_idx..end_idx].iter().collect();
                        buf.push_str(&sub);
                    }
                }
            }
            TemplatePiece::LowercaseField(field) => {
                let v = get_field(item, field);
                if let Value::String(s) = &v {
                    buf.push_str(&s.to_lowercase());
                }
            }
            TemplatePiece::UppercaseField(field) => {
                let v = get_field(item, field);
                if let Value::String(s) = &v {
                    buf.push_str(&s.to_uppercase());
                }
            }
        }
    }
    Value::String(buf.into())
}

// ── Lifted dispatch for mapped expressions ──────────────────────────────────

/// Pre-computed function-specific state for lifted dispatch.
/// Each variant captures what a specific function needs to skip per-call setup.
#[allow(clippy::large_enum_variant)]
pub(crate) enum PreparedState {
    /// $formatNumber: pre-parsed picture into SubPicture + FmtChars
    FormatNumber {
        pos_pic: super::format_number::SubPicture,
        neg_pic: super::format_number::SubPicture,
        fc: super::format_number::FmtChars,
    },
    /// $round: pre-extracted precision
    Round { precision: i64 },
    /// $substring: pre-extracted start and optional length
    Substring { start: f64, length: Option<f64> },
    /// $pad: pre-extracted width and pad char
    Pad { width: i64, pad_char: char },
    /// $contains with string arg: pre-extracted needle
    Contains { needle: String },
    /// $split with string arg: pre-extracted separator and optional limit
    Split {
        separator: String,
        limit: Option<usize>,
    },
    /// $formatBase: pre-extracted radix
    FormatBase { radix: u32 },
}

impl std::fmt::Debug for PreparedState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FormatNumber { .. } => write!(f, "FormatNumber(...)"),
            Self::Round { precision } => write!(f, "Round({precision})"),
            Self::Substring { start, length } => write!(f, "Substring({start}, {length:?})"),
            Self::Pad { width, pad_char } => write!(f, "Pad({width}, {pad_char:?})"),
            Self::Contains { needle } => write!(f, "Contains({needle:?})"),
            Self::Split { separator, limit } => write!(f, "Split({separator:?}, {limit:?})"),
            Self::FormatBase { radix } => write!(f, "FormatBase({radix})"),
        }
    }
}

/// A pre-analyzed function call that can be dispatched efficiently per item.
/// Function resolution and constant arg evaluation happen once at analysis time.
#[derive(Debug)]
pub(crate) struct MappedCall {
    func: Box<FunctionValue>,
    arg_template: Vec<CallArg>,
    prepared: Option<PreparedState>,
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
pub(crate) fn analyze_mapped_call(
    node: NodeId,
    arena: &AstArena,
    param: Option<&str>,
    env: &Rc<Environment>,
) -> Option<MappedCall> {
    // The node should be a Function call, possibly wrapped in a Block.
    let func_node = unwrap_block(node, arena);

    let (procedure, arguments) = match arena.get(func_node) {
        Expr::Function {
            procedure,
            arguments,
            ..
        } => (*procedure, arguments.clone()),
        _ => return None,
    };

    // Resolve the function from the environment.
    // procedure is typically a Variable node ($formatNumber → Variable { name: "formatNumber" })
    let func_name = match arena.get(procedure) {
        Expr::Variable { name, .. } => name.as_str(),
        _ => return None,
    };
    let func_val = env.lookup(func_name)?;
    let Value::Function(func) = func_val else {
        return None;
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

    // Try to pre-compute function-specific state from constant args.
    let prepared = try_prepare(func_name, &arg_template);

    Some(MappedCall {
        func,
        arg_template,
        prepared,
    })
}

/// Unwrap a single-expression Block to get the inner expression.
fn unwrap_block(node: NodeId, arena: &AstArena) -> NodeId {
    if let Expr::Block { expressions, .. } = arena.get(node)
        && expressions.len() == 1
    {
        return expressions[0];
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
            if let Some(p) = param
                && let Some(field) = extract_param_field(steps, arena, p)
            {
                return CallArg::Field(field);
            }
            CallArg::Expr(node)
        }

        // Bare field name (in .() mapping context, no explicit param)
        Expr::Name {
            value,
            stages,
            group,
            focus,
            index,
            ..
        } if stages.is_empty()
            && group.is_none()
            && focus.is_none()
            && index.is_none()
            && param.is_none() =>
        {
            CallArg::Field(value.clone())
        }

        // Bare $param reference (the whole object)
        Expr::Variable { name, .. } if param.is_some_and(|p| name == p) => CallArg::Expr(node),

        _ => CallArg::Expr(node),
    }
}

/// Try to pre-compute function-specific state from the argument template.
#[allow(clippy::too_many_lines)]
fn try_prepare(func_name: &str, args: &[CallArg]) -> Option<PreparedState> {
    match func_name {
        "formatNumber" => {
            // args: [Field(number), Const(picture), optional opts]
            // An options argument changes FmtChars — defer to the general
            // path rather than formatting with defaults and wrong output.
            if args.len() > 2 {
                return None;
            }
            let picture = match args.get(1) {
                Some(CallArg::Const(Value::String(s))) => s.to_string(),
                _ => return None,
            };
            let fc = super::format_number::FmtChars::default();
            let pics = super::format_number::split_on_pattern_sep(&picture, fc.pattern_sep);
            if pics.len() > 2 {
                return None;
            }
            let pos_pic = super::format_number::parse_sub_picture(&pics[0], &fc).ok()?;
            let neg_pic = if pics.len() == 2 {
                super::format_number::parse_sub_picture(&pics[1], &fc).ok()?
            } else {
                let mut np = pos_pic.clone();
                np.prefix = format!("-{}", pos_pic.prefix);
                np
            };
            Some(PreparedState::FormatNumber {
                pos_pic,
                neg_pic,
                fc,
            })
        }
        "round" => {
            let precision = match args.get(1) {
                Some(CallArg::Const(Value::Number(n))) => *n as i64,
                None => 0,
                _ => return None,
            };
            Some(PreparedState::Round { precision })
        }
        "substring" => {
            let start = match args.get(1) {
                Some(CallArg::Const(Value::Number(n))) => *n,
                _ => return None,
            };
            let length = match args.get(2) {
                Some(CallArg::Const(Value::Number(n))) => Some(*n),
                None => None,
                _ => return None,
            };
            Some(PreparedState::Substring { start, length })
        }
        "pad" => {
            let width = match args.get(1) {
                Some(CallArg::Const(Value::Number(n))) => *n as i64,
                _ => return None,
            };
            let pad_char = match args.get(2) {
                Some(CallArg::Const(Value::String(s))) => s.chars().next().unwrap_or(' '),
                None => ' ',
                _ => return None,
            };
            Some(PreparedState::Pad { width, pad_char })
        }
        "contains" => {
            let needle = match args.get(1) {
                Some(CallArg::Const(Value::String(s))) => s.to_string(),
                _ => return None,
            };
            Some(PreparedState::Contains { needle })
        }
        "split" => {
            let separator = match args.get(1) {
                Some(CallArg::Const(Value::String(s))) => s.to_string(),
                _ => return None,
            };
            let limit = match args.get(2) {
                Some(CallArg::Const(Value::Number(n))) => Some(*n as usize),
                None => None,
                _ => return None,
            };
            Some(PreparedState::Split { separator, limit })
        }
        "formatBase" => {
            let radix = match args.get(1) {
                Some(CallArg::Const(Value::Number(n))) => *n as u32,
                _ => return None,
            };
            if !(2..=36).contains(&radix) {
                return None;
            }
            Some(PreparedState::FormatBase { radix })
        }
        _ => None,
    }
}

/// Execute a prepared function call directly, skipping internal parsing.
fn exec_prepared(prepared: &PreparedState, field_val: &Value) -> Option<JsonataResult> {
    match prepared {
        PreparedState::FormatNumber {
            pos_pic,
            neg_pic,
            fc,
        } => {
            let n = match field_val {
                Value::Number(f) => *f,
                _ => return None,
            };
            let negative = n < 0.0;
            let sp = if negative { neg_pic } else { pos_pic };
            let mut value = if negative { -n } else { n };
            match sp.scale {
                1 => value *= 100.0,
                2 => value *= 1000.0,
                _ => {}
            }
            let inner = if sp.exp_mandatory > 0 {
                super::format_number::format_with_exponent(value, sp, fc)
            } else {
                super::format_number::format_fixed(value, sp, fc)
            };
            let inner = super::format_number::apply_digit_family(&inner, fc.zero_digit);
            Some(Ok(Value::String(
                format!("{}{}{}", sp.prefix, inner, sp.suffix).into(),
            )))
        }
        PreparedState::Round { precision } => {
            let n = match field_val {
                Value::Number(f) => *f,
                _ => return None,
            };
            // Must match $round exactly: half-to-even, same as numeric::fn_round.
            Some(Ok(Value::Number(super::numeric::bankers_round(
                n,
                *precision as i32,
            ))))
        }
        PreparedState::Contains { needle } => {
            let Value::String(s) = field_val else {
                return None;
            };
            Some(Ok(Value::Bool(s.contains(needle.as_str()))))
        }
        PreparedState::FormatBase { radix } => {
            let n = match field_val {
                Value::Number(f) => *f as i64,
                _ => return None,
            };
            let formatted = match radix {
                2 => format!("{n:b}"),
                8 => format!("{n:o}"),
                16 => format!("{n:x}"),
                _ => {
                    // Generic radix formatting
                    if n == 0 {
                        return Some(Ok(Value::String("0".into())));
                    }
                    let mut result = String::new();
                    let mut val = n.unsigned_abs();
                    let r = u64::from(*radix);
                    while val > 0 {
                        let digit = (val % r) as u32;
                        result.push(char::from_digit(digit, *radix).unwrap_or('?'));
                        val /= r;
                    }
                    if n < 0 {
                        result.push('-');
                    }
                    let s: String = result.chars().rev().collect();
                    return Some(Ok(Value::String(s.into())));
                }
            };
            Some(Ok(Value::String(formatted.into())))
        }
        // For other prepared states, fall through to generic dispatch
        _ => None,
    }
}

/// Execute a MappedCall for a single item. Function is already resolved,
/// constant args are already evaluated. Only field args need per-item work.
///
/// # Errors
/// Returns evaluation errors from the underlying function call.
pub(crate) fn exec_mapped_call(
    mc: &MappedCall,
    item: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    // If we have prepared state and the first arg is a field, try the fast path.
    if let Some(ref prepared) = mc.prepared {
        // Get the field value (first Field arg).
        if let Some(CallArg::Field(name)) = mc.arg_template.first() {
            let field_val = get_field(item, name);
            if let Some(result) = exec_prepared(prepared, &field_val) {
                return result;
            }
        }
    }

    // Fallback: generic dispatch with pre-resolved function.
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

#[cfg(test)]
mod tests {
    use crate::evaluator::{Environment, eval};
    use crate::parser::{Parser, process_ast};
    use crate::value::Value;
    use std::rc::Rc;

    /// Helper: parse, process, and evaluate against `input`.
    fn eval_expr(src: &str, input: &Value) -> Value {
        let (mut arena, root) = Parser::parse(src).expect("parse failed");
        let root = process_ast(&mut arena, root).expect("process failed");
        let mut env = Environment::new();
        crate::stdlib::register_all(&mut env);
        env.bind("$", input.clone());
        let env = Rc::new(env);
        eval(&arena, root, input, &env).expect("eval failed")
    }

    fn nums_input(values: &[f64]) -> Value {
        let items: Vec<Value> = values
            .iter()
            .map(|&x| {
                let mut obj = crate::value::ObjectMap::new();
                obj.insert("x".into(), Value::Number(x));
                Value::Object(Rc::new(obj))
            })
            .collect();
        let mut root = crate::value::ObjectMap::new();
        root.insert("nums".into(), Value::Array(Rc::from(items)));
        Value::Object(Rc::new(root))
    }

    /// Regression test for gnata-bec.1: the mapped-call fast path used
    /// round-half-away-from-zero while $round is half-to-even (banker's).
    /// `nums.$round(x)` is lifted by analyze_mapped_call; `nums.($round(x + 0))`
    /// has a complex argument, so it takes the general path. Both must agree.
    #[test]
    fn round_fast_path_matches_general_path() {
        let input = nums_input(&[0.5, 1.5, 2.5, 3.5, -0.5, -1.5, -2.5, 2.345]);
        let fast = eval_expr("nums.$round(x)", &input);
        let general = eval_expr("nums.($round(x + 0))", &input);
        assert!(
            fast.deep_equal(&general),
            "fast path {fast:?} != general path {general:?}"
        );
        let expected = eval_expr("[0, 2, 2, 4, -0, -2, -2, 2]", &Value::Undefined);
        assert!(
            fast.deep_equal(&expected),
            "banker's rounding expected, got {fast:?}"
        );
    }

    /// Regression test for gnata-bec.2: the fast path formatted with default
    /// separators, silently dropping the $formatNumber options argument.
    /// With options present the call must not be lifted with defaults.
    #[test]
    fn format_number_fast_path_honors_options_arg() {
        let input = nums_input(&[1234.56]);
        let with_opts = eval_expr(
            r##"nums.$formatNumber(x, "#.###,00", {"decimal-separator": ",", "grouping-separator": "."})"##,
            &input,
        );
        let expected = eval_expr(r#""1.234,56""#, &Value::Undefined);
        assert!(
            with_opts.deep_equal(&expected),
            "options ignored: got {with_opts:?}, expected {expected:?}"
        );
        // Without options the lift still applies and must agree with the general path.
        let fast = eval_expr(r##"nums.$formatNumber(x, "#,###.00")"##, &input);
        let general = eval_expr(r##"nums.($formatNumber(x + 0, "#,###.00"))"##, &input);
        assert!(
            fast.deep_equal(&general),
            "fast path {fast:?} != general path {general:?}"
        );
    }

    #[test]
    fn round_fast_path_with_precision_matches_general_path() {
        let input = nums_input(&[0.25, 0.35, -0.25, 1.05]);
        let fast = eval_expr("nums.$round(x, 1)", &input);
        let general = eval_expr("nums.($round(x + 0, 1))", &input);
        assert!(
            fast.deep_equal(&general),
            "fast path {fast:?} != general path {general:?}"
        );
    }
}
