/// Index into the AST arena. Lightweight, Copy, no lifetimes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

impl NodeId {
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

    pub fn alloc(&mut self, expr: Expr) -> NodeId {
        let id = self.nodes.len() as u32;
        self.nodes.push(expr);
        NodeId(id)
    }

    pub fn get(&self, id: NodeId) -> &Expr {
        &self.nodes[id.0 as usize]
    }

    pub fn get_mut(&mut self, id: NodeId) -> &mut Expr {
        &mut self.nodes[id.0 as usize]
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
        op: String,
        lhs: NodeId,
        rhs: NodeId,
        group: Option<GroupExpr>,
        keep_array: bool,
        pos: usize,
    },

    /// Unary operator (negation, array constructor, object constructor).
    Unary {
        op: String,
        operand: NodeId,          // for negation
        expressions: Vec<NodeId>, // for array constructor [...]
        lhs: Vec<NodeId>,         // for object constructor {...} — flat [k0,v0,k1,v1,...]
        group: Option<GroupExpr>, // group-by expression attached in infix position
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
    },

    /// Partial application: procedure(args... with ? placeholders).
    Partial {
        procedure: NodeId,
        arguments: Vec<NodeId>,
        pos: usize,
    },

    /// Lambda: function($params) <signature> { body }.
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
