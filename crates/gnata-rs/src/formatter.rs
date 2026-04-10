//! JSONata expression formatter.
//!
//! Parses an expression using the existing parser, then walks the AST to emit
//! pretty-printed text with consistent indentation and line-breaking rules.

use crate::error::JsonataError;
use crate::parser::ast::*;
use crate::parser::{Parser, process_ast};

const INDENT: &str = "  ";
const BREAK_THRESHOLD: usize = 3;
const LINE_WIDTH: usize = 60;

/// A block comment extracted from source: `/* text */` with its byte offset.
#[derive(Debug, Clone)]
struct Comment {
    text: String, // includes /* and */
    pos: usize,   // byte offset of the /*
}

/// Extract all block comments from source with their positions.
fn extract_comments(src: &str) -> Vec<Comment> {
    let bytes = src.as_bytes();
    let mut comments = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < bytes.len() {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            comments.push(Comment {
                text: src[start..i].to_string(),
                pos: start,
            });
        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
            // Skip string literals to avoid false positives
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] != b'*' {
            // Could be regex or division — skip to avoid false comment detection
            i += 1;
        } else {
            i += 1;
        }
    }
    comments
}

/// Format a JSONata expression string.
pub fn format(expr: &str) -> Result<String, JsonataError> {
    let comments = extract_comments(expr);
    let (mut arena, root) = Parser::parse(expr)?;
    let root = process_ast(&mut arena, root)?;
    let mut f = Formatter::new(&arena, &comments);
    f.emit(root, 0);
    f.emit_trailing_comments();
    Ok(f.out.trim_end().to_string())
}

struct Formatter<'a> {
    arena: &'a AstArena,
    comments: &'a [Comment],
    comment_idx: usize, // next comment to emit
    out: String,
}

impl<'a> Formatter<'a> {
    fn new(arena: &'a AstArena, comments: &'a [Comment]) -> Self {
        Self {
            arena,
            comments,
            comment_idx: 0,
            out: String::new(),
        }
    }

    fn indent(&mut self, depth: usize) {
        for _ in 0..depth {
            self.out.push_str(INDENT);
        }
    }

    /// Emit any comments whose source position is before `pos`.
    /// Comments always go on their own line.
    fn emit_comments_before(&mut self, pos: usize, depth: usize) {
        while self.comment_idx < self.comments.len() && self.comments[self.comment_idx].pos < pos {
            let comment = &self.comments[self.comment_idx];
            // Ensure we're on a new line
            if !self.out.is_empty() && !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            self.indent(depth);
            self.out.push_str(&comment.text);
            self.out.push('\n');
            self.indent(depth);
            self.comment_idx += 1;
        }
    }

    /// Emit any remaining comments that haven't been placed yet.
    fn emit_trailing_comments(&mut self) {
        while self.comment_idx < self.comments.len() {
            let comment = &self.comments[self.comment_idx];
            if !self.out.is_empty() && !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            self.out.push_str(&comment.text);
            self.comment_idx += 1;
        }
    }

    fn emit(&mut self, id: NodeId, depth: usize) {
        if id.is_empty() {
            return;
        }
        let expr = self.arena.get(id).clone();
        // Emit comments that appear before this node in the source
        self.emit_comments_before(expr.pos(), depth);
        match expr {
            Expr::Name { ref value, ref stages, ref group, keep_array, .. } => {
                if keep_array {
                    self.out.push_str(&escape_name(value));
                    self.out.push_str("[]");
                } else {
                    self.out.push_str(&escape_name(value));
                }
                self.emit_stages(&stages, depth);
                self.emit_group(&group, depth);
            }
            Expr::StringLit { ref value, .. } => {
                self.out.push('"');
                self.out.push_str(&escape_string(value));
                self.out.push('"');
            }
            Expr::NumberLit { ref raw, .. } => {
                self.out.push_str(raw);
            }
            Expr::ValueLit { ref value, .. } => {
                self.out.push_str(value);
            }
            Expr::Variable { ref name, ref group, keep_array, .. } => {
                if name == "$" {
                    self.out.push_str("$$");
                } else if name.is_empty() {
                    self.out.push('$');
                } else {
                    self.out.push('$');
                    self.out.push_str(name);
                }
                if keep_array {
                    self.out.push_str("[]");
                }
                self.emit_group(&group, depth);
            }
            Expr::Wildcard { .. } => self.out.push('*'),
            Expr::Descendant { .. } => self.out.push_str("**"),
            Expr::Parent { ref slot, .. } => {
                self.out.push('%');
                if let Some(slot) = slot {
                    self.out.push_str(&slot.label);
                }
            }
            Expr::Regex { ref pattern, ref flags, .. } => {
                self.out.push('/');
                self.out.push_str(pattern);
                self.out.push('/');
                self.out.push_str(flags);
            }
            Expr::Placeholder { .. } => self.out.push('?'),

            Expr::Path { ref steps, ref group, .. } => {
                self.emit_path(&steps, &group, depth);
            }

            Expr::Binary { op, lhs, rhs, ref group, .. } => {
                self.emit_binary(op, lhs, rhs, &group, depth);
            }

            Expr::Unary { op, operand, ref expressions, ref lhs, ref group, .. } => {
                match op {
                    UnaryOp::Negate => {
                        self.out.push('-');
                        self.emit(operand, depth);
                    }
                    UnaryOp::ArrayCons => {
                        self.emit_array(&expressions, depth);
                    }
                    UnaryOp::ObjCons => {
                        self.emit_object(&lhs, &group, depth);
                    }
                }
            }

            Expr::Block { ref expressions, .. } => {
                self.emit_block(&expressions, depth);
            }

            Expr::Condition { condition, then, else_, .. } => {
                self.emit_condition(condition, then, else_, depth);
            }

            Expr::Bind { lhs, rhs, .. } => {
                self.emit(lhs, depth);
                self.out.push_str(" := ");
                self.emit(rhs, depth);
            }

            Expr::Function { procedure, ref arguments, ref group, .. } => {
                self.emit(procedure, depth);
                self.emit_args(&arguments, depth);
                self.emit_group(&group, depth);
            }

            Expr::Partial { procedure, ref arguments, .. } => {
                self.emit(procedure, depth);
                self.emit_args(&arguments, depth);
            }

            Expr::Lambda { ref params, body, ref signature, .. } => {
                self.emit_lambda(&params, body, &signature, depth);
            }

            Expr::Transform { pattern, update, delete, .. } => {
                self.out.push_str("|");
                self.emit(pattern, depth);
                self.out.push_str("|");
                self.emit(update, depth);
                if let Some(del) = delete {
                    self.out.push_str(", ");
                    self.emit(del, depth);
                }
                self.out.push('|');
            }

            Expr::Sort { expr, ref terms, .. } => {
                self.emit(expr, depth);
                self.out.push_str("^(");
                for (i, term) in terms.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    if term.descending {
                        self.out.push('>');
                    } else {
                        self.out.push('<');
                    }
                    self.emit(term.expression, depth);
                }
                self.out.push(')');
            }
        }
    }

    /// Render to string without pushing to self.out, for length estimation.
    /// Uses empty comments slice so comment state isn't affected.
    fn render(&self, id: NodeId, depth: usize) -> String {
        let empty: Vec<Comment> = Vec::new();
        let mut f = Formatter::new(self.arena, &empty);
        f.emit(id, depth);
        f.out
    }

    fn emit_path(&mut self, steps: &[NodeId], group: &Option<GroupExpr>, depth: usize) {
        if steps.len() > BREAK_THRESHOLD {
            self.emit(steps[0], depth);
            for &step in &steps[1..] {
                self.out.push('\n');
                self.indent(depth + 1);
                self.out.push('.');
                self.emit(step, depth + 1);
            }
        } else {
            for (i, &step) in steps.iter().enumerate() {
                if i > 0 {
                    self.out.push('.');
                }
                self.emit(step, depth);
            }
        }
        self.emit_group(group, depth);
    }

    fn emit_binary(&mut self, op: BinaryOp, lhs: NodeId, rhs: NodeId, group: &Option<GroupExpr>, depth: usize) {
        match op {
            BinaryOp::Subscript => {
                self.emit(lhs, depth);
                self.out.push('[');
                self.emit(rhs, depth);
                self.out.push(']');
            }
            BinaryOp::Range => {
                self.emit(lhs, depth);
                self.out.push_str("..");
                self.emit(rhs, depth);
            }
            _ => {
                self.emit(lhs, depth);
                let op_str = op.as_str();
                if op_str.chars().next().map_or(false, |c| c.is_alphabetic()) {
                    // Word operators: and, or, in
                    self.out.push(' ');
                    self.out.push_str(op_str);
                    self.out.push(' ');
                } else {
                    self.out.push(' ');
                    self.out.push_str(op_str);
                    self.out.push(' ');
                }
                self.emit(rhs, depth);
            }
        }
        self.emit_group(group, depth);
    }

    fn emit_args(&mut self, args: &[NodeId], depth: usize) {
        if args.len() > BREAK_THRESHOLD {
            self.out.push_str("(\n");
            for (i, &arg) in args.iter().enumerate() {
                self.indent(depth + 1);
                self.emit(arg, depth + 1);
                if i < args.len() - 1 {
                    self.out.push(',');
                }
                self.out.push('\n');
            }
            self.indent(depth);
            self.out.push(')');
        } else {
            self.out.push('(');
            for (i, &arg) in args.iter().enumerate() {
                if i > 0 {
                    self.out.push_str(", ");
                }
                self.emit(arg, depth);
            }
            self.out.push(')');
        }
    }

    fn emit_array(&mut self, elements: &[NodeId], depth: usize) {
        if elements.len() > BREAK_THRESHOLD {
            self.out.push_str("[\n");
            for (i, &el) in elements.iter().enumerate() {
                self.indent(depth + 1);
                self.emit(el, depth + 1);
                if i < elements.len() - 1 {
                    self.out.push(',');
                }
                self.out.push('\n');
            }
            self.indent(depth);
            self.out.push(']');
        } else {
            self.out.push('[');
            for (i, &el) in elements.iter().enumerate() {
                if i > 0 {
                    self.out.push_str(", ");
                }
                self.emit(el, depth);
            }
            self.out.push(']');
        }
    }

    fn emit_object(&mut self, lhs: &[NodeId], group: &Option<GroupExpr>, depth: usize) {
        // lhs is flat [k0, v0, k1, v1, ...]
        let pair_count = lhs.len() / 2;
        if pair_count > BREAK_THRESHOLD {
            self.out.push_str("{\n");
            for i in 0..pair_count {
                self.indent(depth + 1);
                self.emit(lhs[i * 2], depth + 1);
                self.out.push_str(": ");
                self.emit(lhs[i * 2 + 1], depth + 1);
                if i < pair_count - 1 {
                    self.out.push(',');
                }
                self.out.push('\n');
            }
            self.indent(depth);
            self.out.push('}');
        } else {
            self.out.push('{');
            for i in 0..pair_count {
                if i > 0 {
                    self.out.push_str(", ");
                }
                self.emit(lhs[i * 2], depth);
                self.out.push_str(": ");
                self.emit(lhs[i * 2 + 1], depth);
            }
            self.out.push('}');
        }
        self.emit_group(group, depth);
    }

    fn emit_block(&mut self, expressions: &[NodeId], depth: usize) {
        if expressions.len() == 1 {
            self.out.push('(');
            self.emit(expressions[0], depth);
            self.out.push(')');
        } else {
            self.out.push_str("(\n");
            for (i, &expr) in expressions.iter().enumerate() {
                self.indent(depth + 1);
                self.emit(expr, depth + 1);
                if i < expressions.len() - 1 {
                    self.out.push(';');
                }
                self.out.push('\n');
            }
            self.indent(depth);
            self.out.push(')');
        }
    }

    fn emit_condition(&mut self, condition: NodeId, then: NodeId, else_: Option<NodeId>, depth: usize) {
        // Try inline first
        let cond_str = self.render(condition, depth);
        let then_str = self.render(then, depth);
        let else_str = else_.map(|e| self.render(e, depth));

        let inline_len = cond_str.len() + 3 + then_str.len()
            + else_str.as_ref().map_or(0, |s| 3 + s.len());

        if inline_len <= LINE_WIDTH && !cond_str.contains('\n') && !then_str.contains('\n')
            && else_str.as_ref().map_or(true, |s| !s.contains('\n'))
        {
            self.emit(condition, depth);
            self.out.push_str(" ? ");
            self.emit(then, depth);
            if let Some(e) = else_ {
                self.out.push_str(" : ");
                self.emit(e, depth);
            }
        } else {
            self.emit(condition, depth);
            self.out.push('\n');
            self.indent(depth + 1);
            self.out.push_str("? ");
            self.emit(then, depth + 1);
            if let Some(e) = else_ {
                self.out.push('\n');
                self.indent(depth + 1);
                self.out.push_str(": ");
                self.emit(e, depth + 1);
            }
        }
    }

    fn emit_lambda(&mut self, params: &[NodeId], body: NodeId, signature: &Option<Signature>, depth: usize) {
        self.out.push_str("function(");
        for (i, &p) in params.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.emit(p, depth);
        }
        self.out.push(')');
        if let Some(sig) = signature {
            self.out.push_str("<");
            self.out.push_str(&sig.raw);
            self.out.push_str(">");
        }
        self.out.push_str(" {\n");
        self.indent(depth + 1);
        self.emit(body, depth + 1);
        self.out.push('\n');
        self.indent(depth);
        self.out.push('}');
    }

    fn emit_stages(&mut self, stages: &[Stage], depth: usize) {
        for stage in stages {
            match &stage.kind {
                StageKind::Filter { expression } => {
                    self.out.push('[');
                    self.emit(*expression, depth);
                    self.out.push(']');
                }
                StageKind::Index { var_name } => {
                    self.out.push('#');
                    self.out.push_str(var_name);
                }
            }
        }
    }

    fn emit_group(&mut self, group: &Option<GroupExpr>, depth: usize) {
        if let Some(g) = group {
            let pair_count = g.pairs.len();
            if pair_count > BREAK_THRESHOLD {
                self.out.push_str("{\n");
                for (i, pair) in g.pairs.iter().enumerate() {
                    self.indent(depth + 1);
                    self.emit(pair[0], depth + 1);
                    self.out.push_str(": ");
                    self.emit(pair[1], depth + 1);
                    if i < pair_count - 1 {
                        self.out.push(',');
                    }
                    self.out.push('\n');
                }
                self.indent(depth);
                self.out.push('}');
            } else {
                self.out.push('{');
                for (i, pair) in g.pairs.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    self.emit(pair[0], depth);
                    self.out.push_str(": ");
                    self.emit(pair[1], depth);
                }
                self.out.push('}');
            }
        }
    }
}

/// Escape a name that contains special characters or is a keyword.
fn escape_name(name: &str) -> String {
    if name.is_empty() || needs_backtick(name) {
        format!("`{}`", name)
    } else {
        name.to_string()
    }
}

fn needs_backtick(name: &str) -> bool {
    // Keywords and names with special chars need backtick quoting
    let keywords = ["and", "or", "in", "true", "false", "null", "function"];
    if keywords.contains(&name) {
        return true;
    }
    let first = name.chars().next().unwrap_or(' ');
    if !first.is_alphabetic() && first != '_' {
        return true;
    }
    name.chars().any(|c| !c.is_alphanumeric() && c != '_')
}

fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(expr: &str) -> String {
        format(expr).unwrap_or_else(|e| panic!("format failed for `{expr}`: {e}"))
    }

    #[test]
    fn literals() {
        assert_eq!(fmt("42"), "42");
        assert_eq!(fmt("\"hello\""), "\"hello\"");
        assert_eq!(fmt("true"), "true");
        assert_eq!(fmt("false"), "false");
        assert_eq!(fmt("null"), "null");
    }

    #[test]
    fn simple_path() {
        assert_eq!(fmt("a.b.c"), "a.b.c");
    }

    #[test]
    fn long_path_breaks() {
        let result = fmt("a.b.c.d");
        assert!(result.contains('\n'), "expected multiline, got: {result}");
        assert!(result.contains(".d"));
    }

    #[test]
    fn short_function_call() {
        assert_eq!(fmt("$sum(a, b, c)"), "$sum(a, b, c)");
    }

    #[test]
    fn long_function_call_breaks() {
        let result = fmt("$foo(a, b, c, d)");
        assert!(result.contains('\n'), "expected multiline, got: {result}");
    }

    #[test]
    fn block_semicolons() {
        let result = fmt("($x := 1; $y := 2; $x + $y)");
        assert!(result.contains('\n'));
        assert!(result.contains("$x := 1;"));
        assert!(result.contains("$y := 2;"));
    }

    #[test]
    fn simple_condition_inline() {
        let result = fmt("x ? 1 : 0");
        assert_eq!(result, "x ? 1 : 0");
    }

    #[test]
    fn lambda_multiline() {
        let result = fmt("function($x) { $x + 1 }");
        assert!(result.contains('\n'));
        assert!(result.contains("function($x)"));
        assert!(result.contains("$x + 1"));
    }

    #[test]
    fn variable_binding() {
        assert_eq!(fmt("$x := 42"), "$x := 42");
    }

    #[test]
    fn preserves_comments() {
        let result = fmt("/* header */ $x + /* inline */ $y");
        assert!(result.contains("/* header */"), "missing header comment: {result}");
        assert!(result.contains("/* inline */"), "missing inline comment: {result}");
        // Comments should be on their own lines
        for line in result.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("/*") {
                assert!(trimmed.ends_with("*/"), "comment not on own line: {result}");
            }
        }
    }

    #[test]
    fn trailing_comment() {
        let result = fmt("$x + $y /* end */");
        assert!(result.contains("/* end */"), "missing trailing comment: {result}");
    }

    #[test]
    fn multiline_comment() {
        let result = fmt("$x /* \n  ***\n  */ + $y");
        assert!(result.contains("/* \n  ***\n  */"), "missing multiline comment: {result}");
    }

    #[test]
    fn idempotent() {
        let exprs = [
            "$sum(a, b, c)",
            "a.b.c",
            "($x := 1; $y := 2; $x + $y)",
            "x ? 1 : 0",
            "/* comment */ $x + $y",
            "$x + $y /* trailing */",
        ];
        for expr in exprs {
            let first = fmt(expr);
            let second = fmt(&first);
            assert_eq!(first, second, "not idempotent for: {expr}\nfirst:  {first}\nsecond: {second}");
        }
    }
}
