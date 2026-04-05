//! Compiled JSONata expression with fast-path optimization.
//!
//! The `Expression` struct wraps a parsed + processed AST with optional
//! fast-path metadata for common expression patterns. This is the
//! recommended public API for evaluating JSONata expressions.

use std::rc::Rc;
use std::sync::Arc;

use crate::error::JsonataResult;
use crate::evaluator::{Environment, FunctionValue};
use crate::fast_path::{self, FastPath};
use crate::parser::{AstArena, NodeId, Parser, process_ast};
use crate::value::Value;

/// A user-defined function that extends the standard JSONata library.
///
/// Receives evaluated arguments and the current context value (focus).
/// Must be `Send + Sync` for use with thread-safe `Expression` and `StreamEvaluator`.
///
/// Function names are registered without the leading `$` — users call them as
/// `$functionName()` in expressions.
pub type CustomFunc = Arc<dyn Fn(&[Value], &Value) -> JsonataResult + Send + Sync>;

/// Create a root environment with all standard library functions plus custom functions.
///
/// The returned environment can be reused across multiple evaluations via
/// [`Expression::evaluate_with_env`]. Reusing the environment avoids re-registering
/// stdlib on every call.
pub fn new_custom_env(custom_funcs: &[(String, CustomFunc)]) -> Rc<Environment> {
    let mut env = Environment::new();
    crate::stdlib::register_all(&mut env);
    for (name, func) in custom_funcs {
        let arc_fn = Arc::clone(func);
        let builtin: Rc<crate::evaluator::BuiltinFn> =
            Rc::new(move |args: &[Value], focus: &Value| arc_fn(args, focus));
        env.bind(
            name.clone(),
            Value::Function(Box::new(FunctionValue::Builtin(builtin))),
        );
    }
    Rc::new(env)
}

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

    /// Evaluate with user-defined custom functions.
    ///
    /// Creates a fresh environment with stdlib + the provided custom functions,
    /// then evaluates. For repeated evaluations with the same custom functions,
    /// prefer [`new_custom_env`] + [`Expression::evaluate_with_env`] to avoid
    /// re-registering on every call.
    ///
    /// # Errors
    /// Returns JSONata evaluation errors.
    pub fn evaluate_with_custom_funcs(
        &self,
        input: &Value,
        custom_funcs: &[(String, CustomFunc)],
    ) -> JsonataResult {
        if let Some(result) = fast_path::eval_fast(&self.fast_path, input) {
            return Ok(result);
        }
        let env = new_custom_env(custom_funcs);
        if !input.is_undefined() {
            env.bind("$".into(), input.clone());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_func_basic() {
        let double: CustomFunc = Arc::new(|args: &[Value], _focus: &Value| {
            let n = args.first().and_then(Value::as_f64).unwrap_or(0.0);
            Ok(Value::Number(n * 2.0))
        });
        let expr = Expression::compile("$double(21)").unwrap();
        let result = expr
            .evaluate_with_custom_funcs(&Value::Undefined, &[("double".into(), double)])
            .unwrap();
        assert_eq!(result.as_f64(), Some(42.0));
    }

    #[test]
    fn custom_func_with_focus() {
        let get_type: CustomFunc = Arc::new(|_args: &[Value], focus: &Value| {
            let t = if focus.is_object() { "object" } else { "other" };
            Ok(Value::String(t.into()))
        });
        let expr = Expression::compile("$getType()").unwrap();
        let input = Value::from_json_str(r#"{"a":1}"#).unwrap();
        let result = expr
            .evaluate_with_custom_funcs(&input, &[("getType".into(), get_type)])
            .unwrap();
        assert_eq!(result.as_str(), Some("object"));
    }

    #[test]
    fn custom_func_alongside_stdlib() {
        let greet: CustomFunc = Arc::new(|args: &[Value], _focus: &Value| {
            let name = args.first().and_then(Value::as_str).unwrap_or("world");
            Ok(Value::String(format!("hello {name}").into()))
        });
        let expr = Expression::compile("$uppercase($greet(name))").unwrap();
        let input = Value::from_json_str(r#"{"name":"alice"}"#).unwrap();
        let result = expr
            .evaluate_with_custom_funcs(&input, &[("greet".into(), greet)])
            .unwrap();
        assert_eq!(result.as_str(), Some("HELLO ALICE"));
    }

    #[test]
    fn custom_func_error_propagation() {
        let fail: CustomFunc = Arc::new(|_args: &[Value], _focus: &Value| {
            Err(crate::error::JsonataError::new("D3030", "custom error"))
        });
        let expr = Expression::compile("$fail()").unwrap();
        let err = expr
            .evaluate_with_custom_funcs(&Value::Undefined, &[("fail".into(), fail)])
            .unwrap_err();
        assert_eq!(err.code, "D3030");
    }

    #[test]
    fn new_custom_env_reusable() {
        let add_one: CustomFunc = Arc::new(|args: &[Value], _focus: &Value| {
            let n = args.first().and_then(Value::as_f64).unwrap_or(0.0);
            Ok(Value::Number(n + 1.0))
        });
        let env = new_custom_env(&[("addOne".into(), add_one)]);

        let expr1 = Expression::compile("$addOne(10)").unwrap();
        let expr2 = Expression::compile("$addOne(20)").unwrap();

        // Bind $ for each eval
        env.bind("$".into(), Value::Undefined);
        let r1 = expr1.evaluate_with_env(&Value::Undefined, &env).unwrap();
        let r2 = expr2.evaluate_with_env(&Value::Undefined, &env).unwrap();

        assert_eq!(r1.as_f64(), Some(11.0));
        assert_eq!(r2.as_f64(), Some(21.0));
    }

    #[test]
    fn custom_func_multiple() {
        let add: CustomFunc = Arc::new(|args: &[Value], _focus: &Value| {
            let a = args.first().and_then(Value::as_f64).unwrap_or(0.0);
            let b = args.get(1).and_then(Value::as_f64).unwrap_or(0.0);
            Ok(Value::Number(a + b))
        });
        let mul: CustomFunc = Arc::new(|args: &[Value], _focus: &Value| {
            let a = args.first().and_then(Value::as_f64).unwrap_or(0.0);
            let b = args.get(1).and_then(Value::as_f64).unwrap_or(0.0);
            Ok(Value::Number(a * b))
        });
        let expr = Expression::compile("$mul($add(2, 3), 4)").unwrap();
        let result = expr
            .evaluate_with_custom_funcs(
                &Value::Undefined,
                &[("add".into(), add), ("mul".into(), mul)],
            )
            .unwrap();
        assert_eq!(result.as_f64(), Some(20.0));
    }

    #[test]
    fn custom_func_is_send_sync() {
        // Compile-time check that CustomFunc is Send + Sync
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CustomFunc>();
    }
}
