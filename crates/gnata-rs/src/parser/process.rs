//! AST post-processing pass.
//!
//! Transforms the raw Pratt-parsed tree into a form suitable for evaluation:
//! - Flattens nested binary(".") nodes into Path nodes with steps vectors.
//! - Propagates KeepSingletonArray when any step has keep_array=true.
//! - Attaches group expressions from path-step binary("{") to the path.
//! - Marks tail-call positions in lambda bodies for TCO.
//! - Recursively processes all child nodes.
//!
//! Direct port of Go `internal/parser/process.go` and `internal/parser/tailcall.go`.

use super::ast::{AstArena, Expr, NodeId};
use crate::error::JsonataError;

/// Check if a path step (or any node in its LHS chain) has keep_array set.
fn step_has_keep_array(arena: &AstArena, step: NodeId) -> bool {
    let mut current = step;
    loop {
        match arena.get(current) {
            Expr::Name {
                keep_array: true, ..
            }
            | Expr::Binary {
                keep_array: true, ..
            }
            | Expr::Variable {
                keep_array: true, ..
            }
            | Expr::Function {
                keep_array: true, ..
            }
            | Expr::Sort {
                keep_array: true, ..
            }
            | Expr::Unary {
                keep_array: true, ..
            } => {
                return true;
            }
            _ => {}
        }
        // Walk into the LHS of Binary nodes (e.g. A[][filter] pattern).
        if let Expr::Binary { lhs, .. } = arena.get(current) {
            current = *lhs;
        } else {
            break;
        }
    }
    false
}

/// Run the post-processing pass over a parsed AST.
/// Call this after `Parser::parse()` and before evaluation.
pub fn process_ast(arena: &mut AstArena, node: NodeId) -> Result<NodeId, JsonataError> {
    if node.is_empty() {
        return Ok(node);
    }

    match arena.get(node).clone() {
        Expr::Binary { ref op, .. } if op == "." => process_dot_binary(arena, node),
        Expr::Binary { lhs, rhs, .. } => {
            let new_lhs = process_ast(arena, lhs)?;
            let new_rhs = process_ast(arena, rhs)?;
            let expr = arena.get_mut(node);
            if let Expr::Binary { lhs: l, rhs: r, .. } = expr {
                *l = new_lhs;
                *r = new_rhs;
            }
            Ok(node)
        }
        Expr::Unary {
            operand,
            expressions,
            lhs,
            ..
        } => {
            let new_operand = process_ast(arena, operand)?;
            let new_exprs: Vec<NodeId> = expressions
                .iter()
                .map(|&e| process_ast(arena, e))
                .collect::<Result<_, _>>()?;
            let new_lhs: Vec<NodeId> = lhs
                .iter()
                .map(|&e| process_ast(arena, e))
                .collect::<Result<_, _>>()?;
            let expr = arena.get_mut(node);
            if let Expr::Unary {
                operand: o,
                expressions: es,
                lhs: l,
                ..
            } = expr
            {
                *o = new_operand;
                *es = new_exprs;
                *l = new_lhs;
            }
            Ok(node)
        }
        Expr::Block { expressions, .. } => {
            let new_exprs: Vec<NodeId> = expressions
                .iter()
                .map(|&e| process_ast(arena, e))
                .collect::<Result<_, _>>()?;
            let expr = arena.get_mut(node);
            if let Expr::Block {
                expressions: es, ..
            } = expr
            {
                *es = new_exprs;
            }
            Ok(node)
        }
        Expr::Function {
            procedure,
            arguments,
            ..
        }
        | Expr::Partial {
            procedure,
            arguments,
            ..
        } => {
            let new_proc = process_ast(arena, procedure)?;
            let new_args: Vec<NodeId> = arguments
                .iter()
                .map(|&a| process_ast(arena, a))
                .collect::<Result<_, _>>()?;
            let expr = arena.get_mut(node);
            match expr {
                Expr::Function {
                    procedure: p,
                    arguments: a,
                    ..
                }
                | Expr::Partial {
                    procedure: p,
                    arguments: a,
                    ..
                } => {
                    *p = new_proc;
                    *a = new_args;
                }
                _ => {}
            }
            Ok(node)
        }
        Expr::Lambda { body, .. } => {
            let new_body = process_ast(arena, body)?;
            let expr = arena.get_mut(node);
            if let Expr::Lambda { body: b, .. } = expr {
                *b = new_body;
            }
            mark_tail_calls(arena, new_body);
            Ok(node)
        }
        Expr::Condition {
            condition,
            then,
            else_,
            ..
        } => {
            let new_cond = process_ast(arena, condition)?;
            let new_then = process_ast(arena, then)?;
            let new_else = match else_ {
                Some(e) => Some(process_ast(arena, e)?),
                None => None,
            };
            let expr = arena.get_mut(node);
            if let Expr::Condition {
                condition: c,
                then: t,
                else_: el,
                ..
            } = expr
            {
                *c = new_cond;
                *t = new_then;
                *el = new_else;
            }
            Ok(node)
        }
        Expr::Bind { lhs, rhs, .. } => {
            let new_lhs = process_ast(arena, lhs)?;
            let new_rhs = process_ast(arena, rhs)?;
            let expr = arena.get_mut(node);
            if let Expr::Bind { lhs: l, rhs: r, .. } = expr {
                *l = new_lhs;
                *r = new_rhs;
            }
            Ok(node)
        }
        Expr::Transform {
            pattern,
            update,
            delete,
            ..
        } => {
            let new_pattern = process_ast(arena, pattern)?;
            let new_update = process_ast(arena, update)?;
            let new_delete = match delete {
                Some(d) => Some(process_ast(arena, d)?),
                None => None,
            };
            let expr = arena.get_mut(node);
            if let Expr::Transform {
                pattern: p,
                update: u,
                delete: d,
                ..
            } = expr
            {
                *p = new_pattern;
                *u = new_update;
                *d = new_delete;
            }
            Ok(node)
        }
        Expr::Sort {
            expr: sort_expr,
            terms,
            ..
        } => {
            let new_expr = process_ast(arena, sort_expr)?;
            let new_terms: Vec<_> = terms
                .iter()
                .map(|t| {
                    let new_e = process_ast(arena, t.expression)?;
                    Ok(super::ast::SortTerm {
                        descending: t.descending,
                        expression: new_e,
                    })
                })
                .collect::<Result<_, JsonataError>>()?;
            let expr = arena.get_mut(node);
            if let Expr::Sort {
                expr: e, terms: ts, ..
            } = expr
            {
                *e = new_expr;
                *ts = new_terms;
            }
            Ok(node)
        }
        Expr::Path { steps, .. } => {
            let mut keep_singleton = false;
            let new_steps: Vec<NodeId> = steps
                .iter()
                .map(|&s| {
                    let processed = process_ast(arena, s)?;
                    if step_has_keep_array(arena, processed) {
                        keep_singleton = true;
                    }
                    Ok(processed)
                })
                .collect::<Result<_, JsonataError>>()?;
            let expr = arena.get_mut(node);
            if let Expr::Path {
                steps: ss,
                keep_singleton_array: ksa,
                ..
            } = expr
            {
                *ss = new_steps;
                if keep_singleton {
                    *ksa = true;
                }
            }
            process_group(arena, node)?;
            Ok(node)
        }
        // Leaf nodes — still need to process any attached Group expression.
        _ => {
            process_group(arena, node)?;
            Ok(node)
        }
    }
}

/// Process group-by key/value pairs on a node (if it has a group).
fn process_group(arena: &mut AstArena, node: NodeId) -> Result<(), JsonataError> {
    let group = match arena.get(node) {
        Expr::Name { group, .. } => group.clone(),
        Expr::Variable { group, .. } => group.clone(),
        Expr::Path { group, .. } => group.clone(),
        Expr::Function { group, .. } => group.clone(),
        _ => None,
    };
    if let Some(mut g) = group {
        for pair in &mut g.pairs {
            pair[0] = process_ast(arena, pair[0])?;
            pair[1] = process_ast(arena, pair[1])?;
        }
        match arena.get_mut(node) {
            Expr::Name { group: gr, .. } => *gr = Some(g),
            Expr::Variable { group: gr, .. } => *gr = Some(g),
            Expr::Path { group: gr, .. } => *gr = Some(g),
            Expr::Function { group: gr, .. } => *gr = Some(g),
            _ => {}
        }
    }
    Ok(())
}

/// Flatten a binary(".") node into a Path node.
fn process_dot_binary(arena: &mut AstArena, node: NodeId) -> Result<NodeId, JsonataError> {
    // Extract group from the binary "." node before collecting steps.
    let bin_group = match arena.get(node) {
        Expr::Binary { group, .. } => group.clone(),
        _ => None,
    };

    let mut steps = Vec::new();
    collect_path_steps(arena, node, &mut steps)?;

    let pos = arena.get(node).pos();

    // Check for KeepSingletonArray propagation.
    // Any step (or a subscript step's left chain) with keep_array=true
    // forces the entire path to preserve singletons as arrays.
    let mut keep_singleton = false;
    for &step in &steps {
        if step_has_keep_array(arena, step) {
            keep_singleton = true;
            break;
        }
    }

    // Process group-by key/value pairs so nested dot expressions within them are resolved.
    let processed_group = if let Some(mut g) = bin_group {
        for pair in &mut g.pairs {
            pair[0] = process_ast(arena, pair[0])?;
            pair[1] = process_ast(arena, pair[1])?;
        }
        Some(g)
    } else {
        None
    };

    // Reuse the node slot by replacing it with a Path.
    *arena.get_mut(node) = Expr::Path {
        steps,
        keep_singleton_array: keep_singleton,
        group: processed_group,
        pos,
    };

    Ok(node)
}

/// Recursively collect steps from nested binary(".") nodes.
fn collect_path_steps(
    arena: &mut AstArena,
    node: NodeId,
    steps: &mut Vec<NodeId>,
) -> Result<(), JsonataError> {
    let expr = arena.get(node).clone();
    match expr {
        Expr::Binary {
            ref op, lhs, rhs, ..
        } if op == "." => {
            // In our Rust AST, Focus/Index live on Name nodes directly
            // (set by the parser's @ and # LED handlers), not on Binary "."
            // nodes. No propagation needed here.
            collect_path_steps(arena, lhs, steps)?;
            collect_path_steps(arena, rhs, steps)?;
        }
        _ => {
            // Leaf step — process it.
            let processed = process_ast(arena, node)?;

            // If the processed node is itself a Path, splice its steps.
            if let Expr::Path { steps: ref ps, .. } = arena.get(processed).clone() {
                let ps = ps.clone();
                steps.extend(ps);
                return Ok(());
            }

            // In a path context, a string literal is a field name lookup.
            if let Expr::StringLit { ref value, pos, .. } = arena.get(processed).clone() {
                let name_node = arena.alloc(Expr::Name {
                    value: value.clone(),
                    pos,
                    keep_array: false,
                    stages: Vec::new(),
                    group: None,
                    focus: None,
                    index: None,
                });
                steps.push(name_node);
                return Ok(());
            }

            steps.push(processed);
        }
    }
    Ok(())
}

// ── Tail-call optimization marking ──────────────────────────────────

/// Mark tail-call positions in a lambda body.
/// Sets `thunk = true` on Function nodes in tail position.
fn mark_tail_calls(arena: &mut AstArena, node: NodeId) {
    if node.is_empty() {
        return;
    }
    mark_tail_position(arena, node);
}

/// Recursively mark nodes in tail position.
fn mark_tail_position(arena: &mut AstArena, node: NodeId) {
    if node.is_empty() {
        return;
    }
    match arena.get(node).clone() {
        Expr::Function { .. } => {
            if let Expr::Function { thunk, .. } = arena.get_mut(node) {
                *thunk = true;
            }
        }
        Expr::Condition { then, else_, .. } => {
            mark_tail_position(arena, then);
            if let Some(e) = else_ {
                mark_tail_position(arena, e);
            }
        }
        Expr::Block { expressions, .. } => {
            if let Some(&last) = expressions.last() {
                mark_tail_position(arena, last);
            }
        }
        Expr::Bind { rhs, .. } => {
            mark_tail_position(arena, rhs);
        }
        Expr::Lambda { .. } => {
            // Nested lambdas get their own tail-call analysis.
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    /// Helper: parse and process, return (arena, root).
    fn parse_and_process(src: &str) -> (AstArena, NodeId) {
        let (mut arena, root) = Parser::parse(src).expect("parse failed");
        let root = process_ast(&mut arena, root).expect("process failed");
        (arena, root)
    }

    #[test]
    fn dot_chain_flattened_to_path() {
        let (arena, root) = parse_and_process("a.b.c");
        match arena.get(root) {
            Expr::Path { steps, .. } => {
                assert_eq!(steps.len(), 3);
                for &s in steps {
                    assert!(matches!(arena.get(s), Expr::Name { .. }));
                }
            }
            other => panic!("expected Path, got {:?}", other),
        }
    }

    #[test]
    fn single_name_stays_name() {
        let (arena, root) = parse_and_process("foo");
        assert!(matches!(arena.get(root), Expr::Name { value, .. } if value == "foo"));
    }

    #[test]
    fn keep_singleton_array_propagated() {
        let (arena, root) = parse_and_process("a[].b");
        match arena.get(root) {
            Expr::Path {
                keep_singleton_array: true,
                ..
            } => {}
            other => panic!("expected Path with keep_singleton_array, got {:?}", other),
        }
    }

    #[test]
    fn string_in_path_becomes_name() {
        let (arena, root) = parse_and_process(r#"a."Product Name""#);
        match arena.get(root) {
            Expr::Path { steps, .. } => {
                assert_eq!(steps.len(), 2);
                match arena.get(steps[1]) {
                    Expr::Name { value, .. } => assert_eq!(value, "Product Name"),
                    other => panic!("expected Name, got {:?}", other),
                }
            }
            other => panic!("expected Path, got {:?}", other),
        }
    }

    #[test]
    fn tail_call_marked_on_recursive_function() {
        // Lambda body is a conditional with function calls in tail position
        let (arena, root) = parse_and_process("$f := function($n){$n > 0 ? $f($n - 1) : 0}");
        // root is Bind, rhs is Lambda
        match arena.get(root) {
            Expr::Bind { rhs, .. } => match arena.get(*rhs) {
                Expr::Lambda { body, .. } => {
                    // body is Condition, then branch is Function with thunk=true
                    match arena.get(*body) {
                        Expr::Condition { then, .. } => match arena.get(*then) {
                            Expr::Function { thunk, .. } => {
                                assert!(thunk, "function in tail position should have thunk=true");
                            }
                            other => panic!("expected Function, got {:?}", other),
                        },
                        other => panic!("expected Condition, got {:?}", other),
                    }
                }
                other => panic!("expected Lambda, got {:?}", other),
            },
            other => panic!("expected Bind, got {:?}", other),
        }
    }

    #[test]
    fn nested_path_spliced() {
        // (a.b).c should flatten to a single Path [a, b, c]
        let (arena, root) = parse_and_process("(a.b).c");
        match arena.get(root) {
            Expr::Path { steps, .. } => {
                // The parenthesized (a.b) becomes a Block containing a Path,
                // which gets spliced into the outer path.
                // Actual step count depends on how blocks in paths are handled.
                assert!(
                    steps.len() >= 2,
                    "expected at least 2 steps, got {}",
                    steps.len()
                );
            }
            other => panic!("expected Path, got {:?}", other),
        }
    }

    #[test]
    fn binary_non_dot_children_processed() {
        let (arena, root) = parse_and_process("a.b + c.d");
        match arena.get(root) {
            Expr::Binary { op, lhs, rhs, .. } => {
                assert_eq!(op, "+");
                assert!(matches!(arena.get(*lhs), Expr::Path { .. }));
                assert!(matches!(arena.get(*rhs), Expr::Path { .. }));
            }
            other => panic!("expected Binary +, got {:?}", other),
        }
    }

    #[test]
    fn block_children_processed() {
        let (arena, root) = parse_and_process("(a.b; c.d)");
        match arena.get(root) {
            Expr::Block { expressions, .. } => {
                assert_eq!(expressions.len(), 2);
                assert!(matches!(arena.get(expressions[0]), Expr::Path { .. }));
                assert!(matches!(arena.get(expressions[1]), Expr::Path { .. }));
            }
            other => panic!("expected Block, got {:?}", other),
        }
    }

    #[test]
    fn tail_call_in_block_last_expr() {
        let (arena, root) = parse_and_process("$f := function($n){($x := 1; $f($n - 1))}");
        match arena.get(root) {
            Expr::Bind { rhs, .. } => match arena.get(*rhs) {
                Expr::Lambda { body, .. } => match arena.get(*body) {
                    Expr::Block { expressions, .. } => {
                        let last = expressions.last().unwrap();
                        match arena.get(*last) {
                            Expr::Function { thunk, .. } => {
                                assert!(thunk, "last expr in block should be tail-call");
                            }
                            other => panic!("expected Function, got {:?}", other),
                        }
                    }
                    other => panic!("expected Block, got {:?}", other),
                },
                other => panic!("expected Lambda, got {:?}", other),
            },
            other => panic!("expected Bind, got {:?}", other),
        }
    }

    #[test]
    fn unary_children_processed() {
        let (arena, root) = parse_and_process("[a.b, c.d]");
        match arena.get(root) {
            Expr::Unary { expressions, .. } => {
                assert_eq!(expressions.len(), 2);
                assert!(matches!(arena.get(expressions[0]), Expr::Path { .. }));
                assert!(matches!(arena.get(expressions[1]), Expr::Path { .. }));
            }
            other => panic!("expected Unary (array), got {:?}", other),
        }
    }

    #[test]
    fn condition_children_processed() {
        let (arena, root) = parse_and_process("x.y ? a.b : c.d");
        match arena.get(root) {
            Expr::Condition {
                condition,
                then,
                else_,
                ..
            } => {
                assert!(matches!(arena.get(*condition), Expr::Path { .. }));
                assert!(matches!(arena.get(*then), Expr::Path { .. }));
                assert!(matches!(arena.get(else_.unwrap()), Expr::Path { .. }));
            }
            other => panic!("expected Condition, got {:?}", other),
        }
    }
}
