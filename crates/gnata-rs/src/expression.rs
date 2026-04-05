//! Compiled JSONata expression with fast-path optimization.
//!
//! The `Expression` struct wraps a parsed + processed AST with optional
//! fast-path metadata for common expression patterns. This is the
//! recommended public API for evaluating JSONata expressions.

use std::rc::Rc;
use std::sync::Arc;

use crate::error::JsonataResult;
use crate::evaluator::Environment;
use crate::fast_path::{self, FastPath};
use crate::parser::{AstArena, NodeId, Parser, process_ast};
use crate::value::Value;

/// A compiled JSONata expression, ready for evaluation.
///
/// Use [`Expression::compile`] to parse and optimize, then [`Expression::evaluate`]
/// to run against input data. The compiled form can be reused across multiple inputs.
///
/// `Expression` is `Send + Sync`: the AST is wrapped in `Arc` so the same
/// compiled expression can be shared across threads. Per-evaluation state
/// (`Environment`, `Value`) is created on the calling thread and never escapes.
pub struct Expression {
    arena: Arc<AstArena>,
    root: NodeId,
    fast_path: FastPath,
    source: String,
}

// Compile-time assertion: Expression must be Send + Sync.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Expression>();
};

impl Expression {
    /// Compile a JSONata expression string.
    ///
    /// Parses, post-processes the AST, and analyzes for fast-path optimization.
    ///
    /// # Errors
    /// Returns parse or AST processing errors.
    pub fn compile(expr: &str) -> JsonataResult<Self> {
        let (mut arena, root) = Parser::parse(expr)?;
        let root = process_ast(&mut arena, root)?;
        let fast_path = fast_path::analyze(&arena, root);

        Ok(Self {
            arena: Arc::new(arena),
            root,
            fast_path,
            source: expr.to_string(),
        })
    }

    /// Evaluate this expression against input data.
    ///
    /// Uses fast-path evaluation when possible, falling back to the full
    /// AST-walking evaluator for complex expressions.
    ///
    /// # Errors
    /// Returns JSONata evaluation errors.
    pub fn evaluate(&self, input: &Value) -> JsonataResult {
        // Try fast path first.
        if let Some(result) = fast_path::eval_fast(&self.fast_path, input) {
            return Ok(result);
        }

        // Fall back to full evaluator.
        let mut env = Environment::new();
        crate::stdlib::register_all(&mut env);
        if !input.is_undefined() {
            env.bind("$".into(), input.clone());
        }
        let env = Rc::new(env);
        crate::eval(&self.arena, self.root, input, &env)
    }

    /// Evaluate with a pre-configured environment.
    ///
    /// # Errors
    /// Returns JSONata evaluation errors.
    pub fn evaluate_with_env(&self, input: &Value, env: &Rc<Environment>) -> JsonataResult {
        if let Some(result) = fast_path::eval_fast(&self.fast_path, input) {
            return Ok(result);
        }
        crate::eval(&self.arena, self.root, input, env)
    }

    /// Returns the fast-path classification for this expression.
    pub fn fast_path_info(&self) -> &FastPath {
        &self.fast_path
    }

    /// Returns true if this expression uses a fast path.
    pub fn is_fast_path(&self) -> bool {
        !matches!(self.fast_path, FastPath::None)
    }

    /// Returns the original source expression.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Access the underlying AST arena (for advanced use).
    pub fn arena(&self) -> &AstArena {
        &self.arena
    }

    /// Access the root AST node (for advanced use).
    pub fn root(&self) -> NodeId {
        self.root
    }
}

impl std::fmt::Debug for Expression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Expression")
            .field("source", &self.source)
            .field("fast_path", &self.fast_path)
            .field("root", &self.root)
            .field("arena_size", &self.arena.len())
            .finish()
    }
}
