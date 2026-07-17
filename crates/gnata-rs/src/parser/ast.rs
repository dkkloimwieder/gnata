// Reachable only through #[doc(hidden)] re-exports for in-repo tooling;
// not part of the documented public API.
#![allow(missing_docs)]

/// Binary operator tag — replaces String for zero-cost match dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,       // +
    Sub,       // -
    Mul,       // *
    Div,       // /
    Mod,       // %
    Pow,       // **
    Concat,    // &
    Eq,        // =
    Ne,        // !=
    Lt,        // <
    Le,        // <=
    Gt,        // >
    Ge,        // >=
    And,       // and
    Or,        // or
    In,        // in
    Chain,     // ~>
    NullCoal,  // ??
    CondTern,  // ?:
    Subscript, // [
    Range,     // ..
    ObjConst,  // {
    Dot,       // . (before process_ast flattens to Path)
    Sort,      // ^  (before process_ast converts)
    Assign,    // :=
    Pipe,      // | (transform)
}

impl BinaryOp {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "+" => Some(Self::Add),
            "-" => Some(Self::Sub),
            "*" => Some(Self::Mul),
            "/" => Some(Self::Div),
            "%" => Some(Self::Mod),
            "**" => Some(Self::Pow),
            "&" => Some(Self::Concat),
            "=" => Some(Self::Eq),
            "!=" => Some(Self::Ne),
            "<" => Some(Self::Lt),
            "<=" => Some(Self::Le),
            ">" => Some(Self::Gt),
            ">=" => Some(Self::Ge),
            "and" => Some(Self::And),
            "or" => Some(Self::Or),
            "in" => Some(Self::In),
            "~>" => Some(Self::Chain),
            "??" => Some(Self::NullCoal),
            "?:" => Some(Self::CondTern),
            "[" => Some(Self::Subscript),
            ".." => Some(Self::Range),
            "{" => Some(Self::ObjConst),
            "." => Some(Self::Dot),
            "^" => Some(Self::Sort),
            ":=" => Some(Self::Assign),
            "|" => Some(Self::Pipe),
            _ => None,
        }
    }

    /// Return the canonical string representation of this operator.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Mod => "%",
            Self::Pow => "**",
            Self::Concat => "&",
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "and",
            Self::Or => "or",
            Self::In => "in",
            Self::Chain => "~>",
            Self::NullCoal => "??",
            Self::CondTern => "?:",
            Self::Subscript => "[",
            Self::Range => "..",
            Self::ObjConst => "{",
            Self::Dot => ".",
            Self::Sort => "^",
            Self::Assign => ":=",
            Self::Pipe => "|",
        }
    }
}

impl std::fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Unary operator tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Negate,    // -
    ArrayCons, // [
    ObjCons,   // {
}

impl UnaryOp {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "-" => Some(Self::Negate),
            "[" => Some(Self::ArrayCons),
            "{" => Some(Self::ObjCons),
            _ => None,
        }
    }

    /// Return the canonical string representation of this operator.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Negate => "-",
            Self::ArrayCons => "[",
            Self::ObjCons => "{",
        }
    }
}

impl std::fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Index into the AST arena. Lightweight, Copy, no lifetimes.
///
/// Only obtainable from [`AstArena::alloc`] (or as [`NodeId::EMPTY`]), so a
/// `NodeId` is always valid for the arena that produced it. Using it with a
/// *different* arena is a bug and may panic or address the wrong node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(u32);

impl NodeId {
    /// Sentinel for "no node" (e.g. the absent else-branch of a ternary).
    /// Never a valid arena index — check with [`NodeId::is_empty`] before
    /// resolving.
    pub const EMPTY: NodeId = NodeId(u32::MAX);

    pub fn is_empty(self) -> bool {
        self == Self::EMPTY
    }
}

/// Arena-based AST storage. All nodes live in a contiguous Vec.
///
/// O(1) drop (single Vec dealloc), cache-friendly, trivially serializable,
/// and `Send + Sync` safe for sharing across threads.
#[derive(Debug, Default)]
pub struct AstArena {
    nodes: Vec<Expr>,
}

impl AstArena {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Maximum AST nodes before the parser bails. Prevents OOM from complex
    /// expressions that produce a very large number of nodes.
    const MAX_NODES: usize = 100_000;

    /// Allocate a new AST node.
    ///
    /// # Errors
    /// Returns `S0210` if the arena exceeds the node limit.
    pub fn alloc(&mut self, expr: Expr) -> Result<NodeId, crate::error::JsonataError> {
        if self.nodes.len() >= Self::MAX_NODES {
            return Err(crate::error::JsonataError::new(
                "S0210",
                "expression too complex (too many AST nodes)",
            ));
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(expr);
        Ok(NodeId(id))
    }

    /// Resolve a node id to its expression.
    ///
    /// # Panics
    /// Panics if `id` is [`NodeId::EMPTY`] or was allocated by a different
    /// arena — both are caller bugs. Use [`AstArena::try_get`] to resolve
    /// ids of uncertain provenance.
    #[inline]
    pub fn get(&self, id: NodeId) -> &Expr {
        self.try_get(id).unwrap_or_else(|| {
            panic!(
                "{id:?} is not a node of this arena (len {}) — NodeIds are only \
                 valid for the arena that allocated them",
                self.nodes.len()
            )
        })
    }

    /// Resolve a node id, returning `None` for [`NodeId::EMPTY`] or an id
    /// that this arena never allocated.
    #[inline]
    pub fn try_get(&self, id: NodeId) -> Option<&Expr> {
        self.nodes.get(id.0 as usize)
    }

    /// Mutable counterpart of [`AstArena::get`].
    ///
    /// # Panics
    /// Panics if `id` is [`NodeId::EMPTY`] or was allocated by a different
    /// arena — both are caller bugs.
    #[inline]
    pub fn get_mut(&mut self, id: NodeId) -> &mut Expr {
        let len = self.nodes.len();
        self.nodes.get_mut(id.0 as usize).unwrap_or_else(|| {
            panic!(
                "{id:?} is not a node of this arena (len {len}) — NodeIds are only \
                 valid for the arena that allocated them"
            )
        })
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// AST node variants. Replaces Go's Node.Type string dispatch.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Field name lookup.
    Name {
        value: String,
        pos: usize,
        keep_array: bool,
        stages: Vec<Stage>,
        group: Option<GroupExpr>,
        focus: Option<String>,
        index: Option<String>,
    },
    /// String literal.
    StringLit { value: String, pos: usize },
    /// Numeric literal.
    NumberLit { value: f64, raw: String, pos: usize },
    /// Boolean or null literal (true, false, null).
    ValueLit {
        value: String, // "true", "false", "null"
        pos: usize,
    },
    /// Variable reference ($name).
    Variable {
        name: String, // "" for bare $, "$" for $$
        group: Option<GroupExpr>,
        keep_array: bool,
        focus: Option<String>,
        index: Option<String>,
        pos: usize,
    },
    /// Wildcard (*).
    Wildcard { pos: usize },
    /// Descendant (**).
    Descendant { pos: usize },
    /// Parent reference (%).
    Parent { pos: usize, slot: Option<Slot> },
    /// Regex literal (/pattern/flags).
    Regex {
        pattern: String,
        flags: String,
        pos: usize,
    },
    /// Partial application placeholder (?).
    Placeholder { pos: usize },

    /// Path: flattened dot-chain (a.b.c → Path { steps: [a, b, c] }).
    /// Created by AST post-processing from nested Binary dots.
    Path {
        steps: Vec<NodeId>,
        keep_singleton_array: bool,
        group: Option<GroupExpr>,
        pos: usize,
    },

    /// Binary operator.
    Binary {
        op: BinaryOp,
        lhs: NodeId,
        rhs: NodeId,
        group: Option<GroupExpr>,
        keep_array: bool,
        focus: Option<String>,
        index: Option<String>,
        pos: usize,
    },

    /// Unary operator (negation, array constructor, object constructor).
    Unary {
        op: UnaryOp,
        operand: NodeId,          // for negation
        expressions: Vec<NodeId>, // for array constructor [...]
        lhs: Vec<NodeId>,         // for object constructor {...} — flat [k0,v0,k1,v1,...]
        group: Option<GroupExpr>, // group-by expression attached in infix position
        keep_array: bool,
        pos: usize,
    },

    /// Block: semicolon-separated or parenthesized expressions.
    Block {
        expressions: Vec<NodeId>,
        pos: usize,
    },

    /// Conditional: condition ? then : else.
    Condition {
        condition: NodeId,
        then: NodeId,
        else_: Option<NodeId>,
        pos: usize,
    },

    /// Variable binding: $var := expr.
    Bind {
        lhs: NodeId,
        rhs: NodeId,
        pos: usize,
    },

    /// Function call: procedure(args...).
    Function {
        procedure: NodeId,
        arguments: Vec<NodeId>,
        pos: usize,
        thunk: bool, // TCO flag set by post-processing
        keep_array: bool,
        group: Option<GroupExpr>,
    },

    /// Partial application: procedure(args... with ? placeholders).
    Partial {
        procedure: NodeId,
        arguments: Vec<NodeId>,
        pos: usize,
    },

    /// Lambda: `function($params) <signature> { body }`.
    Lambda {
        params: Vec<NodeId>,
        body: NodeId,
        signature: Option<Signature>,
        pos: usize,
        thunk: bool,
    },

    /// Transform: |pattern|update,delete|.
    Transform {
        pattern: NodeId,
        update: NodeId,
        delete: Option<NodeId>,
        pos: usize,
    },

    /// Sort: expr ^(terms).
    Sort {
        expr: NodeId,
        terms: Vec<SortTerm>,
        keep_array: bool,
        index: Option<String>,
        focus: Option<String>,
        pos: usize,
    },
}

impl Expr {
    pub fn pos(&self) -> usize {
        match self {
            Expr::Name { pos, .. }
            | Expr::StringLit { pos, .. }
            | Expr::NumberLit { pos, .. }
            | Expr::ValueLit { pos, .. }
            | Expr::Variable { pos, .. }
            | Expr::Wildcard { pos }
            | Expr::Descendant { pos }
            | Expr::Parent { pos, .. }
            | Expr::Regex { pos, .. }
            | Expr::Placeholder { pos }
            | Expr::Path { pos, .. }
            | Expr::Binary { pos, .. }
            | Expr::Unary { pos, .. }
            | Expr::Block { pos, .. }
            | Expr::Condition { pos, .. }
            | Expr::Bind { pos, .. }
            | Expr::Function { pos, .. }
            | Expr::Partial { pos, .. }
            | Expr::Lambda { pos, .. }
            | Expr::Transform { pos, .. }
            | Expr::Sort { pos, .. } => *pos,
        }
    }

    /// Check if this is a Name node (for keep_array modification).
    pub fn is_name(&self) -> bool {
        matches!(self, Expr::Name { .. })
    }

    /// Check if this is a Variable node.
    pub fn is_variable(&self) -> bool {
        matches!(self, Expr::Variable { .. })
    }
}

/// Sort term within a Sort expression.
#[derive(Debug, Clone)]
pub struct SortTerm {
    pub descending: bool,
    pub expression: NodeId,
}

/// Predicate or index binding attached to a path step.
#[derive(Debug, Clone)]
pub struct Stage {
    pub kind: StageKind,
    pub pos: usize,
}

#[derive(Debug, Clone)]
pub enum StageKind {
    Filter { expression: NodeId },
    Index { var_name: String },
}

/// Group-by expression attached to a path step.
#[derive(Debug, Clone)]
pub struct GroupExpr {
    pub pairs: Vec<[NodeId; 2]>, // [key_expr, value_expr]
    pub pos: usize,
}

/// Parent reference slot.
#[derive(Debug, Clone)]
pub struct Slot {
    pub label: String,
    pub level: i32,
    pub index: i32,
}

/// Lambda type signature.
#[derive(Debug, Clone)]
pub struct Signature {
    pub raw: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_get_rejects_empty_and_foreign_ids() {
        let mut arena = AstArena::new();
        let id = arena
            .alloc(Expr::StringLit {
                value: "x".into(),
                pos: 0,
            })
            .unwrap();
        assert!(arena.try_get(id).is_some());
        assert!(arena.try_get(NodeId::EMPTY).is_none());
        // An id from a larger arena is out of range for this one.
        let mut bigger = AstArena::new();
        let _ = bigger.alloc(Expr::StringLit {
            value: "a".into(),
            pos: 0,
        });
        let foreign = bigger
            .alloc(Expr::StringLit {
                value: "b".into(),
                pos: 0,
            })
            .unwrap();
        assert!(arena.try_get(foreign).is_none());
    }

    #[test]
    #[should_panic(expected = "not a node of this arena")]
    fn get_panics_with_helpful_message_on_empty_id() {
        let arena = AstArena::new();
        let _ = arena.get(NodeId::EMPTY);
    }
}
