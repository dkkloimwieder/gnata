pub mod ast;
pub mod process;

pub use ast::{AstArena, Expr, GroupExpr, NodeId, Signature, Slot, SortTerm, Stage, StageKind};
pub use process::process_ast;

use crate::error::JsonataError;
use crate::lexer::{Lexer, Token, TokenType};

/// Hand-written Pratt parser for JSONata expressions.
///
/// Direct port of Go `internal/parser/parser.go`.
/// Takes `&mut AstArena`, returns `NodeId` for the root expression.
pub struct Parser {
    lex: Lexer,
    token: Token,
    infix: bool,
    arena: AstArena,
}

impl Parser {
    /// Parse a JSONata expression string into an AST arena.
    /// Returns the arena and the root node ID.
    pub fn parse(src: &str) -> Result<(AstArena, NodeId), JsonataError> {
        let mut parser = Self {
            lex: Lexer::new(src),
            token: Token::eof(),
            infix: false,
            arena: AstArena::new(),
        };
        // Prime the token stream
        parser.advance()?;
        if parser.token.typ == TokenType::EOF {
            return Ok((parser.arena, NodeId::EMPTY));
        }
        let root = parser.expression(0)?;
        if parser.token.typ != TokenType::EOF {
            return Err(parse_error(
                "S0201",
                &format!("unexpected token: {}", parser.token.value),
                parser.token.pos,
            ));
        }
        Ok((parser.arena, root))
    }

    // ── Token management ─────────────────────────────────────────────

    fn advance(&mut self) -> Result<(), JsonataError> {
        self.token = self.lex.next(self.infix)?;
        Ok(())
    }

    fn advance_prefix(&mut self) -> Result<(), JsonataError> {
        self.infix = false;
        self.token = self.lex.next(false)?;
        Ok(())
    }

    fn consume(&mut self, expected: TokenType) -> Result<Token, JsonataError> {
        if self.token.typ != expected {
            return Err(parse_error(
                "S0202",
                &format!("expected {:?}, got {:?}", expected, self.token.typ),
                self.token.pos,
            ));
        }
        let tok = self.token.clone();
        self.advance()?;
        Ok(tok)
    }

    fn consume_prefix(&mut self, expected: TokenType) -> Result<Token, JsonataError> {
        if self.token.typ != expected {
            return Err(parse_error(
                "S0202",
                &format!("expected {:?}, got {:?}", expected, self.token.typ),
                self.token.pos,
            ));
        }
        let tok = self.token.clone();
        self.advance_prefix()?;
        Ok(tok)
    }

    // ── Core Pratt parser ────────────────────────────────────────────

    fn expression(&mut self, rbp: i32) -> Result<NodeId, JsonataError> {
        let mut left = self.nud()?;
        while binding_power(self.token.typ) > rbp {
            left = self.led(left)?;
        }
        Ok(left)
    }

    /// Parse RHS of a binary operator, converting EOF to S0207.
    fn binary_rhs(&mut self, bp: i32, op: &str) -> Result<NodeId, JsonataError> {
        if self.token.typ == TokenType::EOF {
            return Err(parse_error(
                "S0207",
                &format!("nothing follows the \"{op}\" operator"),
                self.token.pos,
            ));
        }
        self.expression(bp)
    }

    // ── NUD (prefix) handlers ────────────────────────────────────────

    fn nud(&mut self) -> Result<NodeId, JsonataError> {
        let tok = self.token.clone();
        match tok.typ {
            TokenType::Name => {
                self.advance()?;
                // Check for lambda: "function" or "λ" followed by "("
                if (tok.value == "function" || tok.value == "λ")
                    && self.token.typ == TokenType::LParen
                {
                    return self.parse_lambda(tok.pos);
                }
                self.infix = true;
                Ok(self.arena.alloc(Expr::Name {
                    value: tok.value,
                    pos: tok.pos,
                    keep_array: false,
                    stages: Vec::new(),
                    group: None,
                    focus: None,
                    index: None,
                }))
            }
            // Keywords can appear as field names in prefix position
            TokenType::And | TokenType::Or | TokenType::In => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::Name {
                    value: tok.value,
                    pos: tok.pos,
                    keep_array: false,
                    stages: Vec::new(),
                    group: None,
                    focus: None,
                    index: None,
                }))
            }
            TokenType::Variable => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::Variable {
                    name: tok.value,
                    pos: tok.pos,
                }))
            }
            TokenType::String => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::StringLit {
                    value: tok.value,
                    pos: tok.pos,
                }))
            }
            TokenType::Number => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::NumberLit {
                    value: tok.num_val,
                    raw: tok.value,
                    pos: tok.pos,
                }))
            }
            TokenType::Value => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::ValueLit {
                    value: tok.value,
                    pos: tok.pos,
                }))
            }
            TokenType::Regex => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::Regex {
                    pattern: tok.regex_pat,
                    flags: tok.regex_flg,
                    pos: tok.pos,
                }))
            }
            TokenType::Minus => {
                // Unary minus (bp=70)
                self.advance_prefix()?;
                let rhs = self.expression(70)?;
                // If RHS is a number literal, fold the negation directly
                if let Expr::NumberLit { value, raw, pos } = self.arena.get(rhs).clone() {
                    let neg = -value;
                    let neg_raw = if let Some(stripped) = raw.strip_prefix('-') {
                        stripped.to_string()
                    } else {
                        format!("-{raw}")
                    };
                    *self.arena.get_mut(rhs) = Expr::NumberLit {
                        value: neg,
                        raw: neg_raw,
                        pos,
                    };
                    Ok(rhs)
                } else {
                    Ok(self.arena.alloc(Expr::Unary {
                        op: "-".into(),
                        operand: rhs,
                        expressions: Vec::new(),
                        lhs: Vec::new(),
                        pos: tok.pos,
                    }))
                }
            }
            TokenType::Star => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::Wildcard { pos: tok.pos }))
            }
            TokenType::StarStar => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::Descendant { pos: tok.pos }))
            }
            TokenType::Percent => {
                self.advance()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::Parent {
                    pos: tok.pos,
                    slot: None,
                }))
            }
            TokenType::LBracket => {
                // Array constructor [...]
                self.advance_prefix()?;
                let mut exprs = Vec::new();
                while self.token.typ != TokenType::RBracket {
                    if self.token.typ == TokenType::EOF {
                        return Err(parse_error(
                            "S0204",
                            "expected , or ] in array constructor",
                            self.token.pos,
                        ));
                    }
                    let expr = self.expression(0)?;
                    exprs.push(expr);
                    if self.token.typ == TokenType::Comma {
                        self.advance_prefix()?;
                    }
                }
                self.advance()?; // consume ]
                self.infix = true;
                Ok(self.arena.alloc(Expr::Unary {
                    op: "[".into(),
                    operand: NodeId::EMPTY,
                    expressions: exprs,
                    lhs: Vec::new(),
                    pos: tok.pos,
                }))
            }
            TokenType::LBrace => {
                // Object constructor {...}
                self.advance_prefix()?;
                let pairs = self.parse_object_pairs()?;
                self.infix = true;
                Ok(self.arena.alloc(Expr::Unary {
                    op: "{".into(),
                    operand: NodeId::EMPTY,
                    expressions: Vec::new(),
                    lhs: pairs,
                    pos: tok.pos,
                }))
            }
            TokenType::LParen => {
                // Block or parenthesized expression
                self.advance_prefix()?;
                let mut exprs = Vec::new();
                while self.token.typ != TokenType::RParen {
                    if self.token.typ == TokenType::EOF {
                        return Err(parse_error(
                            "S0203",
                            "expected ) before end of expression",
                            self.token.pos,
                        ));
                    }
                    let expr = self.expression(0)?;
                    exprs.push(expr);
                    if self.token.typ == TokenType::Semicolon {
                        self.advance_prefix()?;
                    }
                }
                self.advance()?; // consume )
                self.infix = true;
                Ok(self.arena.alloc(Expr::Block {
                    expressions: exprs,
                    pos: tok.pos,
                }))
            }
            TokenType::Question => {
                self.advance()?;
                Ok(self.arena.alloc(Expr::Placeholder { pos: tok.pos }))
            }
            TokenType::Pipe | TokenType::Tilde => self.parse_transform(tok.pos),
            TokenType::Chain => Err(parse_error(
                "S0211",
                "the ~> operator cannot be used as a prefix",
                tok.pos,
            )),
            TokenType::EOF => Err(parse_error(
                "S0201",
                "unexpected end of expression",
                tok.pos,
            )),
            _ => Err(parse_error(
                "S0211",
                &format!("unexpected token: {}", tok.value),
                tok.pos,
            )),
        }
    }

    // ── LED (infix) handlers ─────────────────────────────────────────

    fn led(&mut self, left: NodeId) -> Result<NodeId, JsonataError> {
        let tok = self.token.clone();
        match tok.typ {
            TokenType::Dot => {
                self.advance_prefix()?;
                let rhs = self.expression(74)?; // right-associative
                Ok(self.arena.alloc(Expr::Binary {
                    op: ".".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::LParen => {
                // Function call
                self.advance_prefix()?;
                let mut args = Vec::new();
                let mut has_placeholder = false;
                while self.token.typ != TokenType::RParen {
                    if self.token.typ == TokenType::EOF {
                        return Err(parse_error(
                            "S0203",
                            "expected ) before end of expression",
                            self.token.pos,
                        ));
                    }
                    let arg = self.expression(0)?;
                    if matches!(self.arena.get(arg), Expr::Placeholder { .. }) {
                        has_placeholder = true;
                    }
                    args.push(arg);
                    if self.token.typ == TokenType::Comma {
                        self.advance_prefix()?;
                    }
                }
                self.advance()?; // consume )
                self.infix = true;
                if has_placeholder {
                    Ok(self.arena.alloc(Expr::Partial {
                        procedure: left,
                        arguments: args,
                        pos: tok.pos,
                    }))
                } else {
                    Ok(self.arena.alloc(Expr::Function {
                        procedure: left,
                        arguments: args,
                        pos: tok.pos,
                        thunk: false,
                    }))
                }
            }
            TokenType::LBracket => {
                // Subscript/predicate or empty [] (keep array)
                self.advance_prefix()?;
                if self.token.typ == TokenType::RBracket {
                    // Empty [] → set KeepArray on left
                    self.advance()?;
                    self.infix = true;
                    self.set_keep_array(left);
                    Ok(left)
                } else {
                    let rhs = self.expression(0)?;
                    self.consume(TokenType::RBracket)?;
                    self.infix = true;
                    Ok(self.arena.alloc(Expr::Binary {
                        op: "[".into(),
                        lhs: left,
                        rhs,
                        pos: tok.pos,
                    }))
                }
            }
            TokenType::LBrace => {
                // Group-by expression
                self.advance_prefix()?;
                let pairs_flat = self.parse_object_pairs()?;
                let pairs = pairs_to_group_pairs(&pairs_flat);
                self.infix = true;
                // Attach group to left node
                self.set_group(
                    left,
                    GroupExpr {
                        pairs,
                        pos: tok.pos,
                    },
                );
                Ok(left)
            }
            TokenType::At => {
                // Focus binding @$var
                self.advance()?;
                if self.token.typ != TokenType::Variable {
                    return Err(parse_error(
                        "S0214",
                        "the @ operator must be followed by a $variable",
                        tok.pos,
                    ));
                }
                let var_name = self.token.value.clone();
                self.advance()?;
                self.set_focus(left, var_name);
                Ok(left)
            }
            TokenType::Hash => {
                // Index binding #$var
                self.advance()?;
                if self.token.typ != TokenType::Variable {
                    return Err(parse_error(
                        "S0214",
                        "the # operator must be followed by a $variable",
                        tok.pos,
                    ));
                }
                let var_name = self.token.value.clone();
                self.advance()?;
                self.set_index(left, var_name);
                Ok(left)
            }
            TokenType::Question => {
                // Ternary conditional
                self.advance_prefix()?;
                let then = self.expression(0)?;
                let else_ = if self.token.typ == TokenType::Colon {
                    self.advance_prefix()?;
                    Some(self.expression(0)?)
                } else {
                    None
                };
                Ok(self.arena.alloc(Expr::Condition {
                    condition: left,
                    then,
                    else_,
                    pos: tok.pos,
                }))
            }
            TokenType::Assign => {
                // Binding :=
                // LHS must be a variable
                if !matches!(self.arena.get(left), Expr::Variable { .. }) {
                    return Err(parse_error(
                        "S0212",
                        "the left side of := must be a $variable name",
                        tok.pos,
                    ));
                }
                self.advance_prefix()?;
                let rhs = self.expression(9)?; // right-associative (bp-1)
                Ok(self.arena.alloc(Expr::Bind {
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::Caret => {
                // Sort expression
                self.parse_sort(left, tok.pos)
            }
            // Standard binary operators
            TokenType::Chain => {
                self.advance_prefix()?;
                let rhs = self.expression(44)?; // bp-1 for right-associativity
                Ok(self.arena.alloc(Expr::Binary {
                    op: "~>".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::Elvis => {
                self.advance_prefix()?;
                let rhs = self.expression(19)?;
                Ok(self.arena.alloc(Expr::Binary {
                    op: "?:".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::Coalesce => {
                self.advance_prefix()?;
                let rhs = self.expression(19)?;
                Ok(self.arena.alloc(Expr::Binary {
                    op: "??".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::DotDot => {
                self.advance_prefix()?;
                let rhs = self.expression(19)?;
                Ok(self.arena.alloc(Expr::Binary {
                    op: "..".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::And => {
                self.advance_prefix()?;
                let rhs = self.expression(30)?;
                Ok(self.arena.alloc(Expr::Binary {
                    op: "and".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::Or => {
                self.advance_prefix()?;
                let rhs = self.expression(25)?;
                Ok(self.arena.alloc(Expr::Binary {
                    op: "or".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::In => {
                self.advance_prefix()?;
                let rhs = self.expression(40)?;
                Ok(self.arena.alloc(Expr::Binary {
                    op: "in".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::Equals => self.binary_op(left, "=", 40, tok.pos),
            TokenType::NE => self.binary_op(left, "!=", 40, tok.pos),
            TokenType::LT => self.binary_op(left, "<", 40, tok.pos),
            TokenType::GT => self.binary_op(left, ">", 40, tok.pos),
            TokenType::LE => self.binary_op(left, "<=", 40, tok.pos),
            TokenType::GE => self.binary_op(left, ">=", 40, tok.pos),
            TokenType::Plus => self.binary_op(left, "+", 50, tok.pos),
            TokenType::Minus => self.binary_op(left, "-", 50, tok.pos),
            TokenType::Star => self.binary_op(left, "*", 60, tok.pos),
            TokenType::Slash => self.binary_op(left, "/", 60, tok.pos),
            TokenType::Percent => {
                self.advance_prefix()?;
                let rhs = self.binary_rhs(60, "%")?;
                Ok(self.arena.alloc(Expr::Binary {
                    op: "%".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            TokenType::StarStar => self.binary_op(left, "**", 60, tok.pos),
            TokenType::Amp => self.binary_op(left, "&", 50, tok.pos),
            TokenType::Pipe => {
                self.advance_prefix()?;
                let rhs = self.expression(19)?;
                Ok(self.arena.alloc(Expr::Binary {
                    op: "|".into(),
                    lhs: left,
                    rhs,
                    pos: tok.pos,
                }))
            }
            _ => Err(parse_error(
                "S0201",
                &format!("unexpected token: {}", tok.value),
                tok.pos,
            )),
        }
    }

    /// Helper for standard binary operators.
    fn binary_op(
        &mut self,
        left: NodeId,
        op: &str,
        bp: i32,
        pos: usize,
    ) -> Result<NodeId, JsonataError> {
        self.advance_prefix()?;
        let rhs = self.binary_rhs(bp, op)?;
        Ok(self.arena.alloc(Expr::Binary {
            op: op.into(),
            lhs: left,
            rhs,
            pos,
        }))
    }

    // ── Special parsers ──────────────────────────────────────────────

    fn parse_lambda(&mut self, pos: usize) -> Result<NodeId, JsonataError> {
        // Current token should be (
        self.consume_prefix(TokenType::LParen)?;

        // Parse parameter list
        let mut params = Vec::new();
        while self.token.typ != TokenType::RParen {
            if self.token.typ != TokenType::Variable {
                return Err(parse_error(
                    "S0208",
                    "expected $parameter name in lambda",
                    self.token.pos,
                ));
            }
            let param = self.arena.alloc(Expr::Variable {
                name: self.token.value.clone(),
                pos: self.token.pos,
            });
            params.push(param);
            self.advance()?;
            if self.token.typ == TokenType::Comma {
                self.advance_prefix()?;
            }
        }
        self.consume(TokenType::RParen)?;

        // Optional type signature <...>
        let signature = if self.token.typ == TokenType::LT {
            Some(self.parse_signature()?)
        } else {
            None
        };

        // Body: { expr }
        self.consume_prefix(TokenType::LBrace)?;
        let body = self.expression(0)?;
        self.consume(TokenType::RBrace)?;
        self.infix = true;

        Ok(self.arena.alloc(Expr::Lambda {
            params,
            body,
            signature,
            pos,
            thunk: false,
        }))
    }

    fn parse_signature(&mut self) -> Result<Signature, JsonataError> {
        // Consume the < token
        let mut depth = 1;
        let mut raw = String::from("<");
        self.advance()?;

        while depth > 0 {
            if self.token.typ == TokenType::EOF {
                return Err(parse_error(
                    "S0202",
                    "unterminated signature",
                    self.token.pos,
                ));
            }
            if self.token.typ == TokenType::LT {
                depth += 1;
            } else if self.token.typ == TokenType::GT {
                depth -= 1;
            }
            raw.push_str(&self.token.value);
            if depth > 0 {
                self.advance()?;
            }
        }
        self.advance()?;
        Ok(Signature { raw })
    }

    fn parse_transform(&mut self, pos: usize) -> Result<NodeId, JsonataError> {
        // Current token is | or ~
        self.advance_prefix()?;
        let pattern = self.expression(20)?;
        self.consume_prefix(TokenType::Pipe)?;
        let update = self.expression(20)?;
        let delete = if self.token.typ == TokenType::Comma {
            self.advance_prefix()?;
            Some(self.expression(20)?)
        } else {
            None
        };
        self.consume(TokenType::Pipe)?;
        self.infix = true;
        Ok(self.arena.alloc(Expr::Transform {
            pattern,
            update,
            delete,
            pos,
        }))
    }

    fn parse_sort(&mut self, left: NodeId, pos: usize) -> Result<NodeId, JsonataError> {
        self.advance()?;
        self.consume_prefix(TokenType::LParen)?;

        let mut terms = Vec::new();
        while self.token.typ != TokenType::RParen {
            if self.token.typ == TokenType::EOF {
                return Err(parse_error(
                    "S0202",
                    "expected ) in sort expression",
                    self.token.pos,
                ));
            }
            let descending = match self.token.typ {
                TokenType::GT => {
                    self.advance_prefix()?;
                    true
                }
                TokenType::LT => {
                    self.advance_prefix()?;
                    false
                }
                _ => false,
            };
            let expression = self.expression(0)?;
            terms.push(SortTerm {
                descending,
                expression,
            });
            if self.token.typ == TokenType::Comma {
                self.advance_prefix()?;
            }
        }
        self.advance()?; // consume )
        self.infix = true;
        Ok(self.arena.alloc(Expr::Sort {
            expr: left,
            terms,
            pos,
        }))
    }

    fn parse_object_pairs(&mut self) -> Result<Vec<NodeId>, JsonataError> {
        let mut pairs = Vec::new();
        while self.token.typ != TokenType::RBrace {
            if self.token.typ == TokenType::EOF {
                return Err(parse_error(
                    "S0202",
                    "expected } in object constructor",
                    self.token.pos,
                ));
            }
            let key = self.expression(0)?;
            self.consume_prefix(TokenType::Colon)?;
            let value = self.expression(0)?;
            pairs.push(key);
            pairs.push(value);
            if self.token.typ == TokenType::Comma {
                self.advance_prefix()?;
            }
        }
        self.advance()?; // consume }
        Ok(pairs)
    }

    // ── Node modification helpers ────────────────────────────────────

    fn set_keep_array(&mut self, id: NodeId) {
        if let Expr::Name { keep_array, .. } = self.arena.get_mut(id) {
            *keep_array = true;
        }
    }

    fn set_group(&mut self, id: NodeId, group: GroupExpr) {
        if let Expr::Name { group: g, .. } = self.arena.get_mut(id) {
            *g = Some(group);
        }
    }

    fn set_focus(&mut self, id: NodeId, name: String) {
        if let Expr::Name { focus, .. } = self.arena.get_mut(id) {
            *focus = Some(name);
        }
    }

    fn set_index(&mut self, id: NodeId, name: String) {
        if let Expr::Name { index, .. } = self.arena.get_mut(id) {
            *index = Some(name);
        }
    }
}

/// Convert flat [k0, v0, k1, v1, ...] pairs to [[k0, v0], [k1, v1], ...].
fn pairs_to_group_pairs(flat: &[NodeId]) -> Vec<[NodeId; 2]> {
    flat.chunks_exact(2).map(|c| [c[0], c[1]]).collect()
}

/// Binding power table for the Pratt parser.
fn binding_power(tt: TokenType) -> i32 {
    match tt {
        TokenType::LParen | TokenType::LBracket => 80,
        TokenType::Dot | TokenType::At | TokenType::Hash => 75,
        TokenType::LBrace => 70,
        TokenType::StarStar | TokenType::Star | TokenType::Slash | TokenType::Percent => 60,
        TokenType::Plus | TokenType::Minus | TokenType::Amp => 50,
        TokenType::Chain => 45,
        TokenType::NE
        | TokenType::LE
        | TokenType::GE
        | TokenType::Equals
        | TokenType::LT
        | TokenType::GT
        | TokenType::In
        | TokenType::Caret => 40,
        TokenType::And => 30,
        TokenType::Or => 25,
        TokenType::DotDot
        | TokenType::Pipe
        | TokenType::Question
        | TokenType::Elvis
        | TokenType::Coalesce => 20,
        TokenType::Assign => 10,
        _ => 0,
    }
}

fn parse_error(code: &str, msg: &str, _pos: usize) -> JsonataError {
    JsonataError::new(code, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> (AstArena, NodeId) {
        Parser::parse(src).unwrap()
    }

    #[test]
    fn parse_name() {
        let (arena, root) = parse("foo");
        assert!(matches!(arena.get(root), Expr::Name { value, .. } if value == "foo"));
    }

    #[test]
    fn parse_dotted_path() {
        let (arena, root) = parse("a.b.c");
        // Should be Binary { ".", Binary { ".", a, b }, c } (left-assoc via bp)
        // Actually with bp=74 for RHS, it's right-associative:
        // Binary { ".", a, Binary { ".", b, c } }
        assert!(matches!(arena.get(root), Expr::Binary { op, .. } if op == "."));
    }

    #[test]
    fn parse_number() {
        let (arena, root) = parse("42");
        assert!(matches!(arena.get(root), Expr::NumberLit { value, .. } if *value == 42.0));
    }

    #[test]
    fn parse_negative_number() {
        let (arena, root) = parse("-42");
        // Should fold into a single negative number literal
        assert!(matches!(arena.get(root), Expr::NumberLit { value, .. } if *value == -42.0));
    }

    #[test]
    fn parse_string() {
        let (arena, root) = parse(r#""hello""#);
        assert!(matches!(arena.get(root), Expr::StringLit { value, .. } if value == "hello"));
    }

    #[test]
    fn parse_variable() {
        let (arena, root) = parse("$x");
        assert!(matches!(arena.get(root), Expr::Variable { name, .. } if name == "x"));
    }

    #[test]
    fn parse_binary_plus() {
        let (arena, root) = parse("1 + 2");
        match arena.get(root) {
            Expr::Binary { op, lhs, rhs, .. } => {
                assert_eq!(op, "+");
                assert!(matches!(arena.get(*lhs), Expr::NumberLit { value, .. } if *value == 1.0));
                assert!(matches!(arena.get(*rhs), Expr::NumberLit { value, .. } if *value == 2.0));
            }
            other => panic!("expected Binary, got {:?}", other),
        }
    }

    #[test]
    fn parse_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let (arena, root) = parse("1 + 2 * 3");
        match arena.get(root) {
            Expr::Binary { op, rhs, .. } => {
                assert_eq!(op, "+");
                assert!(matches!(arena.get(*rhs), Expr::Binary { op, .. } if op == "*"));
            }
            other => panic!("expected Binary, got {:?}", other),
        }
    }

    #[test]
    fn parse_function_call() {
        let (arena, root) = parse("$sum(a, b)");
        match arena.get(root) {
            Expr::Function { arguments, .. } => {
                assert_eq!(arguments.len(), 2);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn parse_lambda() {
        let (arena, root) = parse("function($x){ $x + 1 }");
        assert!(matches!(arena.get(root), Expr::Lambda { params, .. } if params.len() == 1));
    }

    #[test]
    fn parse_array_constructor() {
        let (arena, root) = parse("[1, 2, 3]");
        match arena.get(root) {
            Expr::Unary {
                op, expressions, ..
            } => {
                assert_eq!(op, "[");
                assert_eq!(expressions.len(), 3);
            }
            other => panic!("expected Unary array, got {:?}", other),
        }
    }

    #[test]
    fn parse_object_constructor() {
        let (arena, root) = parse(r#"{"a": 1, "b": 2}"#);
        match arena.get(root) {
            Expr::Unary { op, lhs, .. } => {
                assert_eq!(op, "{");
                assert_eq!(lhs.len(), 4); // flat [k0, v0, k1, v1]
            }
            other => panic!("expected Unary object, got {:?}", other),
        }
    }

    #[test]
    fn parse_conditional() {
        let (arena, root) = parse("x ? 1 : 2");
        assert!(matches!(
            arena.get(root),
            Expr::Condition { else_: Some(_), .. }
        ));
    }

    #[test]
    fn parse_conditional_no_else() {
        let (arena, root) = parse("x ? 1");
        assert!(matches!(
            arena.get(root),
            Expr::Condition { else_: None, .. }
        ));
    }

    #[test]
    fn parse_binding() {
        let (arena, root) = parse("$x := 42");
        assert!(matches!(arena.get(root), Expr::Bind { .. }));
    }

    #[test]
    fn parse_wildcard() {
        let (arena, root) = parse("*");
        assert!(matches!(arena.get(root), Expr::Wildcard { .. }));
    }

    #[test]
    fn parse_descendant() {
        let (arena, root) = parse("**");
        assert!(matches!(arena.get(root), Expr::Descendant { .. }));
    }

    #[test]
    fn parse_parent() {
        let (arena, root) = parse("%");
        assert!(matches!(arena.get(root), Expr::Parent { .. }));
    }

    #[test]
    fn parse_block() {
        let (arena, root) = parse("(a; b; c)");
        match arena.get(root) {
            Expr::Block { expressions, .. } => {
                assert_eq!(expressions.len(), 3);
            }
            other => panic!("expected Block, got {:?}", other),
        }
    }

    #[test]
    fn parse_subscript() {
        let (arena, root) = parse("a[0]");
        assert!(matches!(arena.get(root), Expr::Binary { op, .. } if op == "["));
    }

    #[test]
    fn parse_keep_array() {
        let (arena, root) = parse("a[]");
        assert!(matches!(
            arena.get(root),
            Expr::Name {
                keep_array: true,
                ..
            }
        ));
    }

    #[test]
    fn parse_sort() {
        let (arena, root) = parse("data^(>price)");
        match arena.get(root) {
            Expr::Sort { terms, .. } => {
                assert_eq!(terms.len(), 1);
                assert!(terms[0].descending);
            }
            other => panic!("expected Sort, got {:?}", other),
        }
    }

    #[test]
    fn parse_transform() {
        let (arena, root) = parse("|a|b|");
        assert!(matches!(
            arena.get(root),
            Expr::Transform { delete: None, .. }
        ));
    }

    #[test]
    fn parse_transform_with_delete() {
        let (arena, root) = parse("|a|b,c|");
        assert!(matches!(
            arena.get(root),
            Expr::Transform {
                delete: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn parse_partial_application() {
        let (arena, root) = parse("$f(?, 2)");
        assert!(matches!(arena.get(root), Expr::Partial { .. }));
    }

    #[test]
    fn parse_chain() {
        let (arena, root) = parse("a ~> b");
        assert!(matches!(arena.get(root), Expr::Binary { op, .. } if op == "~>"));
    }

    #[test]
    fn parse_range() {
        let (arena, root) = parse("[1..5]");
        match arena.get(root) {
            Expr::Unary { expressions, .. } => {
                assert_eq!(expressions.len(), 1);
                assert!(matches!(arena.get(expressions[0]), Expr::Binary { op, .. } if op == ".."));
            }
            other => panic!("expected array with range, got {:?}", other),
        }
    }

    #[test]
    fn parse_empty_expression() {
        let (arena, root) = Parser::parse("").unwrap();
        assert!(root.is_empty());
        assert!(arena.is_empty());
    }

    #[test]
    fn error_unexpected_eof() {
        let result = Parser::parse("1 +");
        assert!(result.is_err());
    }

    #[test]
    fn error_binding_lhs_not_variable() {
        let result = Parser::parse("a := 1");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "S0212");
    }

    #[test]
    fn parse_complex_jsonata() {
        // Real-world JSONata: sum of order prices
        let (arena, root) = parse("$sum(Account.Order.Product.Price)");
        assert!(matches!(arena.get(root), Expr::Function { .. }));
    }

    #[test]
    fn parse_lambda_with_signature() {
        let (arena, root) = parse("function($x)<s:n>{ $x }");
        match arena.get(root) {
            Expr::Lambda { signature, .. } => {
                assert!(signature.is_some());
                assert!(signature.as_ref().unwrap().raw.starts_with('<'));
            }
            other => panic!("expected Lambda, got {:?}", other),
        }
    }
}
