//! High-throughput streaming evaluator for multiple compiled expressions.
//!
//! Port of Go `stream.go` — manages a COW expression list with lock-free
//! reads and serialized writes. Thread-safe for concurrent evaluation.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use parking_lot::Mutex;

use crate::error::{JsonataError, JsonataResult};
use crate::expression::{CustomFunc, Expression};
use crate::value::Value;

/// Telemetry hook for [`StreamEvaluator`]. Must be thread-safe.
pub trait MetricsHook: Send + Sync {
    /// Called after each expression evaluation with timing and path info.
    fn on_eval(
        &self,
        expr_index: usize,
        fast_path: bool,
        duration: Duration,
        err: Option<&JsonataError>,
    );
}

/// Cache statistics returned by [`StreamEvaluator::stats`].
#[derive(Debug, Clone, Default)]
pub struct StreamStats {
    /// Number of expression slots (including removed).
    pub expressions: usize,
}

/// High-throughput evaluator for multiple compiled expressions.
///
/// Expressions are stored in an atomically swapped `Vec` so reads in
/// [`eval_many`](StreamEvaluator::eval_many) are fully lock-free.
/// Writes ([`add`](StreamEvaluator::add), [`compile`](StreamEvaluator::compile),
/// [`replace`](StreamEvaluator::replace), [`remove`](StreamEvaluator::remove))
/// take a mutex and publish a new copy-on-write snapshot.
///
/// Thread-safe for concurrent `eval_many` + `add`/`compile` calls.
///
/// Go equivalent: `StreamEvaluator` in `stream.go`.
pub struct StreamEvaluator {
    exprs: ArcSwap<Vec<Option<Arc<Expression>>>>,
    mu: Mutex<()>,
    metrics: Option<Arc<dyn MetricsHook>>,
    custom_funcs: Arc<Vec<(String, CustomFunc)>>,
}

impl StreamEvaluator {
    /// Create a new evaluator with the given compiled expressions.
    pub fn new(expressions: Vec<Arc<Expression>>) -> Self {
        let exprs: Vec<Option<Arc<Expression>>> = expressions.into_iter().map(Some).collect();
        Self {
            exprs: ArcSwap::from_pointee(exprs),
            mu: Mutex::new(()),
            metrics: None,
            custom_funcs: Arc::new(Vec::new()),
        }
    }

    /// Register user-defined functions that extend the standard JSONata library.
    ///
    /// Functions are stored at construction time. During evaluation, a shared
    /// environment is created once per `eval_many` call (not per expression).
    /// Function names should not include the leading `$`.
    #[must_use]
    pub fn with_custom_functions(mut self, fns: Vec<(String, CustomFunc)>) -> Self {
        self.custom_funcs = Arc::new(fns);
        self
    }

    /// Attach a metrics hook for evaluation telemetry.
    #[must_use]
    pub fn with_metrics(mut self, hook: Arc<dyn MetricsHook>) -> Self {
        self.metrics = Some(hook);
        self
    }

    /// Add a compiled expression and return its stable index.
    ///
    /// The index is guaranteed stable: once assigned it will not change even
    /// as more expressions are added. Safe to call concurrently with `eval_many`.
    pub fn add(&self, expr: Arc<Expression>) -> usize {
        let _lock = self.mu.lock();
        let old = self.exprs.load();
        let mut new_exprs = (**old).clone();
        let idx = new_exprs.len();
        new_exprs.push(Some(expr));
        self.exprs.store(Arc::new(new_exprs));
        idx
    }

    /// Compile a JSONata expression string, add it, and return its stable index.
    ///
    /// # Errors
    /// Returns parse or AST processing errors.
    pub fn compile(&self, src: &str) -> Result<usize, JsonataError> {
        let expr = Expression::compile(src)?;
        Ok(self.add(Arc::new(expr)))
    }

    /// Replace the expression at the given index.
    ///
    /// # Errors
    /// Returns an error if the index is out of range.
    pub fn replace(&self, idx: usize, expr: Arc<Expression>) -> Result<(), JsonataError> {
        let _lock = self.mu.lock();
        let old = self.exprs.load();
        if idx >= old.len() {
            return Err(JsonataError::new(
                "D0000",
                format!("expression index {idx} out of range [0, {})", old.len()),
            ));
        }
        let mut new_exprs = (**old).clone();
        new_exprs[idx] = Some(expr);
        self.exprs.store(Arc::new(new_exprs));
        Ok(())
    }

    /// Remove the expression at the given index. The index is NOT reused.
    ///
    /// # Errors
    /// Returns an error if the index is out of range.
    pub fn remove(&self, idx: usize) -> Result<(), JsonataError> {
        let _lock = self.mu.lock();
        let old = self.exprs.load();
        if idx >= old.len() {
            return Err(JsonataError::new(
                "D0000",
                format!("expression index {idx} out of range [0, {})", old.len()),
            ));
        }
        let mut new_exprs = (**old).clone();
        new_exprs[idx] = None;
        self.exprs.store(Arc::new(new_exprs));
        Ok(())
    }

    /// Remove all expressions.
    pub fn reset(&self) {
        let _lock = self.mu.lock();
        self.exprs.store(Arc::new(Vec::new()));
    }

    /// Number of expression slots (including removed).
    pub fn len(&self) -> usize {
        self.exprs.load().len()
    }

    /// Returns true if no expressions are registered.
    pub fn is_empty(&self) -> bool {
        self.exprs.load().is_empty()
    }

    /// Evaluate multiple expressions against one input.
    ///
    /// Returns `results[i]` for each `expr_indices[i]`. Removed or out-of-range
    /// indices produce `None`. Undefined results also produce `None`.
    ///
    /// # Errors
    /// Returns the first evaluation error encountered (short-circuits).
    pub fn eval_many(
        &self,
        input: &Value,
        expr_indices: &[usize],
    ) -> Result<Vec<Option<Value>>, JsonataError> {
        self.eval_many_inner(input, expr_indices, None)
    }

    /// Evaluate multiple expressions with a cancellation token.
    ///
    /// Setting the `AtomicBool` to `true` from another thread causes subsequent
    /// expression evaluations to return `D3001`. Already-completed results are
    /// discarded on cancellation (short-circuit).
    ///
    /// # Errors
    /// Returns `D3001` if cancelled, or the first evaluation error.
    pub fn eval_many_with_cancel(
        &self,
        input: &Value,
        expr_indices: &[usize],
        cancel: Arc<AtomicBool>,
    ) -> Result<Vec<Option<Value>>, JsonataError> {
        self.eval_many_inner(input, expr_indices, Some(cancel))
    }

    fn eval_many_inner(
        &self,
        input: &Value,
        expr_indices: &[usize],
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<Option<Value>>, JsonataError> {
        if expr_indices.is_empty() {
            return Ok(Vec::new());
        }
        // Load expression snapshot once — lock-free.
        let exprs = self.exprs.load();

        // Build shared env once per call if custom functions or cancel are set.
        let needs_env = !self.custom_funcs.is_empty() || cancel.is_some();
        let custom_env = if needs_env {
            let mut env = crate::evaluator::Environment::new();
            crate::stdlib::register_all(&mut env);
            for (name, func) in self.custom_funcs.iter() {
                let arc_fn = Arc::clone(func);
                let builtin: std::rc::Rc<crate::evaluator::BuiltinFn> =
                    std::rc::Rc::new(move |args: &[Value], focus: &Value| arc_fn(args, focus));
                env.bind(
                    name.clone(),
                    Value::Function(Box::new(crate::evaluator::FunctionValue::Builtin(builtin))),
                );
            }
            if let Some(cancel) = cancel {
                env.set_cancel(cancel);
            }
            if !input.is_undefined() {
                env.bind("$".into(), input.clone());
            }
            Some(std::rc::Rc::new(env))
        } else {
            None
        };

        let mut results = Vec::with_capacity(expr_indices.len());

        for &idx in expr_indices {
            if idx >= exprs.len() {
                results.push(None);
                continue;
            }
            let Some(ref expr) = exprs[idx] else {
                results.push(None);
                continue;
            };

            let start = self.metrics.as_ref().map(|_| Instant::now());
            let eval_result = match custom_env {
                Some(ref env) => expr.evaluate_with_env(input, env),
                None => expr.evaluate_value(input),
            };
            match eval_result {
                Ok(val) => {
                    if let (Some(hook), Some(start)) = (&self.metrics, start) {
                        hook.on_eval(idx, expr.is_fast_path(), start.elapsed(), None);
                    }
                    results.push(if val.is_undefined() { None } else { Some(val) });
                }
                Err(e) => {
                    if let (Some(hook), Some(start)) = (&self.metrics, start) {
                        hook.on_eval(idx, false, start.elapsed(), Some(&e));
                    }
                    return Err(e);
                }
            }
        }
        Ok(results)
    }

    /// Evaluate a single expression against input data.
    ///
    /// # Errors
    /// Returns evaluation errors, or `Value::Undefined` for removed/out-of-range indices.
    pub fn eval_one(&self, input: &Value, expr_index: usize) -> JsonataResult {
        let results = self.eval_many(input, &[expr_index])?;
        Ok(results
            .into_iter()
            .next()
            .flatten()
            .unwrap_or(Value::Undefined))
    }

    /// Evaluate a single expression with cancellation support.
    ///
    /// # Errors
    /// Returns `D3001` if cancelled, or other evaluation errors.
    pub fn eval_one_with_cancel(
        &self,
        input: &Value,
        expr_index: usize,
        cancel: Arc<AtomicBool>,
    ) -> JsonataResult {
        let results = self.eval_many_with_cancel(input, &[expr_index], cancel)?;
        Ok(results
            .into_iter()
            .next()
            .flatten()
            .unwrap_or(Value::Undefined))
    }

    /// Returns cache statistics.
    pub fn stats(&self) -> StreamStats {
        StreamStats {
            expressions: self.exprs.load().len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_and_eval_one() {
        let se = StreamEvaluator::new(Vec::new());
        let idx = se.compile("Account.Name").unwrap();
        let input = crate::value::Value::from_json_str(
            r#"{"Account": {"Name": "Firefly"}}"#,
        )
        .unwrap();
        let result = se.eval_one(&input, idx).unwrap();
        assert_eq!(result, Value::String("Firefly".into()));
    }

    #[test]
    fn eval_many_returns_results_per_index() {
        let se = StreamEvaluator::new(Vec::new());
        let i0 = se.compile("Account.Name").unwrap();
        let i1 = se.compile("Account.Order[0].OrderID").unwrap();
        let input = crate::value::Value::from_json_str(
            r#"{"Account": {"Name": "Firefly", "Order": [{"OrderID": "order103"}]}}"#,
        )
        .unwrap();
        let results = se.eval_many(&input, &[i0, i1]).unwrap();
        assert_eq!(results[0], Some(Value::String("Firefly".into())));
        assert_eq!(
            results[1],
            Some(Value::String("order103".into()))
        );
    }

    #[test]
    fn remove_produces_none() {
        let se = StreamEvaluator::new(Vec::new());
        let idx = se.compile("1+1").unwrap();
        se.remove(idx).unwrap();
        let input = Value::Undefined;
        let results = se.eval_many(&input, &[idx]).unwrap();
        assert_eq!(results[0], None);
    }

    #[test]
    fn replace_updates_expression() {
        let se = StreamEvaluator::new(Vec::new());
        let idx = se.compile("1+1").unwrap();
        let input = Value::Undefined;
        assert_eq!(se.eval_one(&input, idx).unwrap(), Value::Number(2.0));

        let new_expr = Arc::new(Expression::compile("2+2").unwrap());
        se.replace(idx, new_expr).unwrap();
        assert_eq!(se.eval_one(&input, idx).unwrap(), Value::Number(4.0));
    }

    #[test]
    fn reset_clears_all() {
        let se = StreamEvaluator::new(Vec::new());
        se.compile("1").unwrap();
        se.compile("2").unwrap();
        assert_eq!(se.len(), 2);
        se.reset();
        assert_eq!(se.len(), 0);
    }

    #[test]
    fn out_of_range_produces_none() {
        let se = StreamEvaluator::new(Vec::new());
        let results = se.eval_many(&Value::Undefined, &[999]).unwrap();
        assert_eq!(results[0], None);
    }

    #[test]
    fn concurrent_eval_is_safe() {
        let se = Arc::new(StreamEvaluator::new(Vec::new()));
        let idx = se.compile("Account.Name").unwrap();
        let json = r#"{"Account": {"Name": "Firefly"}}"#;

        let mut handles = Vec::new();
        for _ in 0..4 {
            let se = Arc::clone(&se);
            handles.push(std::thread::spawn(move || {
                let input = crate::value::Value::from_json_str(json).unwrap();
                for _ in 0..100 {
                    let result = se.eval_one(&input, idx).unwrap();
                    assert_eq!(result, Value::String("Firefly".into()));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    // ── Ported from Go stream_test.go ────────────────────────────────

    const STREAM_TEST_DATA: &str = r#"{
        "data": {"action": "grant-access", "user_type": 2},
        "metadata": {"is_admin": true}
    }"#;

    fn test_input() -> Value {
        Value::from_json_str(STREAM_TEST_DATA).unwrap()
    }

    #[test]
    fn compile_sequential_indices() {
        let se = StreamEvaluator::new(Vec::new());
        let exprs = [
            r#"data.action = "grant-access""#,
            "data.user_type = 2",
            "metadata.is_admin = true",
        ];
        for (i, src) in exprs.iter().enumerate() {
            let idx = se.compile(src).unwrap();
            assert_eq!(idx, i, "expr {i} should get index {i}");
        }
        assert_eq!(se.len(), 3);

        let indices: Vec<usize> = (0..3).collect();
        let results = se.eval_many(&test_input(), &indices).unwrap();
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r, &Some(Value::Bool(true)), "result[{i}]");
        }
    }

    #[test]
    fn add_precompiled_expressions() {
        let cases: &[(&str, Value)] = &[
            (r#"data.action = "grant-access""#, Value::Bool(true)),
            ("data.user_type = 2", Value::Bool(true)),
            ("metadata.is_admin = true", Value::Bool(true)),
            (r#"data.action != "other""#, Value::Bool(true)),
            ("data.user_type != 99", Value::Bool(true)),
            ("data.user_type", Value::Number(2.0)),
            ("data.user_type > 1", Value::Bool(true)),
            ("data.user_type = 2 and metadata.is_admin = true", Value::Bool(true)),
        ];
        let input = test_input();
        for (expr, want) in cases {
            let compiled = Arc::new(Expression::compile(expr).unwrap());
            let se = StreamEvaluator::new(Vec::new());
            let idx = se.add(compiled);
            assert_eq!(idx, 0);
            let got = se.eval_one(&input, idx).unwrap();
            assert_eq!(&got, want, "expr: {expr}");
        }
    }

    #[test]
    fn mixed_fast_path_and_full_eval() {
        let se = StreamEvaluator::new(Vec::new());
        let i0 = se.compile("data.user_type = 2").unwrap(); // comparison fast path
        let i1 = se.compile("data.user_type > 1").unwrap(); // full eval
        let i2 = se.compile("data.user_type").unwrap(); // pure-path fast path

        let results = se.eval_many(&test_input(), &[i0, i1, i2]).unwrap();
        assert_eq!(results[0], Some(Value::Bool(true)));
        assert_eq!(results[1], Some(Value::Bool(true)));
        assert_eq!(results[2], Some(Value::Number(2.0)));
    }

    #[test]
    fn index_stability_after_adds() {
        let se = StreamEvaluator::new(Vec::new());
        let i0 = se.compile(r#"data.action = "grant-access""#).unwrap();
        let i1 = se.compile("data.user_type = 2").unwrap();

        // Add 100 more expressions.
        for i in 0..100 {
            se.compile(&format!("data.user_type = {}", i + 1000)).unwrap();
        }
        assert_eq!(se.len(), 102);

        // Original indices still work correctly.
        let results = se.eval_many(&test_input(), &[i0, i1]).unwrap();
        assert_eq!(results[0], Some(Value::Bool(true)));
        assert_eq!(results[1], Some(Value::Bool(true)));
    }

    #[test]
    fn replace_swaps_expression() {
        let se = StreamEvaluator::new(Vec::new());
        let idx = se.compile("data.action").unwrap();
        let input = test_input();

        let got = se.eval_one(&input, idx).unwrap();
        assert_eq!(got, Value::String("grant-access".into()));

        let new_expr = Arc::new(Expression::compile("data.user_type").unwrap());
        se.replace(idx, new_expr).unwrap();

        let got = se.eval_one(&input, idx).unwrap();
        assert_eq!(got, Value::Number(2.0));
    }

    #[test]
    fn remove_returns_none_keeps_others() {
        let se = StreamEvaluator::new(Vec::new());
        let i0 = se.compile(r#"data.action = "grant-access""#).unwrap();
        let i1 = se.compile("data.user_type = 2").unwrap();

        se.remove(i0).unwrap();

        let results = se.eval_many(&test_input(), &[i0, i1]).unwrap();
        assert_eq!(results[0], None, "removed expr should be None");
        assert_eq!(results[1], Some(Value::Bool(true)), "kept expr should work");
    }

    #[test]
    fn reset_allows_reuse() {
        let se = StreamEvaluator::new(Vec::new());
        se.compile("data.action").unwrap();
        se.compile("data.user_type").unwrap();
        assert_eq!(se.len(), 2);

        se.reset();
        assert_eq!(se.len(), 0);

        // After reset, first compile gets index 0 again.
        let idx = se.compile("metadata.is_admin").unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn concurrent_add_and_eval() {
        let se = Arc::new(StreamEvaluator::new(Vec::new()));
        let idx = se.compile(r#"data.action = "grant-access""#).unwrap();

        let mut handles = Vec::new();

        // Readers: evaluate concurrently.
        for _ in 0..4 {
            let se = Arc::clone(&se);
            handles.push(std::thread::spawn(move || {
                let input = Value::from_json_str(STREAM_TEST_DATA).unwrap();
                for _ in 0..50 {
                    let r = se.eval_one(&input, idx).unwrap();
                    assert_eq!(r, Value::Bool(true));
                }
            }));
        }

        // Writer: add expressions concurrently with reads.
        {
            let se = Arc::clone(&se);
            handles.push(std::thread::spawn(move || {
                for i in 0..20 {
                    se.compile(&format!("data.user_type = {i}")).unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
        // Original expression still at index 0.
        let input = Value::from_json_str(STREAM_TEST_DATA).unwrap();
        assert_eq!(se.eval_one(&input, idx).unwrap(), Value::Bool(true));
    }

    // ── WithCustomFunctions tests ───────────────────────────────────

    #[test]
    fn custom_func_in_stream() {
        let double: CustomFunc = Arc::new(|args: &[Value], _| {
            let n = args.first().and_then(Value::as_f64).unwrap_or(0.0);
            Ok(Value::Number(n * 2.0))
        });
        let se = StreamEvaluator::new(Vec::new())
            .with_custom_functions(vec![("double".into(), double)]);
        let idx = se.compile("$double(data.user_type)").unwrap();
        let result = se.eval_one(&test_input(), idx).unwrap();
        assert_eq!(result, Value::Number(4.0));
    }

    #[test]
    fn custom_func_with_stdlib_in_stream() {
        let greet: CustomFunc = Arc::new(|args: &[Value], _| {
            let name = args.first().and_then(Value::as_str).unwrap_or("?");
            Ok(Value::String(format!("hi {name}").into()))
        });
        let se = StreamEvaluator::new(Vec::new())
            .with_custom_functions(vec![("greet".into(), greet)]);
        let idx = se.compile("$uppercase($greet(data.action))").unwrap();
        let result = se.eval_one(&test_input(), idx).unwrap();
        assert_eq!(result, Value::String("HI GRANT-ACCESS".into()));
    }

    #[test]
    fn multiple_custom_funcs_in_stream() {
        let add: CustomFunc = Arc::new(|args: &[Value], _| {
            let a = args.first().and_then(Value::as_f64).unwrap_or(0.0);
            let b = args.get(1).and_then(Value::as_f64).unwrap_or(0.0);
            Ok(Value::Number(a + b))
        });
        let mul: CustomFunc = Arc::new(|args: &[Value], _| {
            let a = args.first().and_then(Value::as_f64).unwrap_or(0.0);
            let b = args.get(1).and_then(Value::as_f64).unwrap_or(0.0);
            Ok(Value::Number(a * b))
        });
        let se = StreamEvaluator::new(Vec::new())
            .with_custom_functions(vec![("add".into(), add), ("mul".into(), mul)]);
        let i0 = se.compile("$add(data.user_type, 10)").unwrap();
        let i1 = se.compile("$mul(data.user_type, 3)").unwrap();
        let results = se.eval_many(&test_input(), &[i0, i1]).unwrap();
        assert_eq!(results[0], Some(Value::Number(12.0)));
        assert_eq!(results[1], Some(Value::Number(6.0)));
    }

    #[test]
    fn stream_no_custom_funcs_unchanged() {
        // Verify existing behavior is unaffected when no custom funcs registered
        let se = StreamEvaluator::new(Vec::new());
        let idx = se.compile("data.user_type + 1").unwrap();
        let result = se.eval_one(&test_input(), idx).unwrap();
        assert_eq!(result, Value::Number(3.0));
    }

    // ── Cancellation tests ──────────────────────────────────────────

    #[test]
    fn expression_cancel_returns_d3001() {
        use std::sync::atomic::AtomicBool;
        let cancel = Arc::new(AtomicBool::new(true)); // pre-cancelled
        // Use a recursive expression that will hit call_function's cancel check
        let expr = crate::expression::Expression::compile(
            "$reduce([1,2,3], function($a,$b){$a+$b}, 0)",
        )
        .unwrap();
        let err = expr
            .evaluate_with_cancel("", cancel)
            .unwrap_err();
        assert_eq!(err.code, "D3001");
    }

    #[test]
    fn stream_cancel_returns_d3001() {
        use std::sync::atomic::AtomicBool;
        let cancel = Arc::new(AtomicBool::new(true));
        let se = StreamEvaluator::new(Vec::new());
        let idx = se.compile("$reduce([1,2,3], function($a,$b){$a+$b}, 0)").unwrap();
        let err = se
            .eval_many_with_cancel(&test_input(), &[idx], cancel)
            .unwrap_err();
        assert_eq!(err.code, "D3001");
    }

    #[test]
    fn stream_cancel_not_set_works_normally() {
        use std::sync::atomic::AtomicBool;
        let cancel = Arc::new(AtomicBool::new(false)); // not cancelled
        let se = StreamEvaluator::new(Vec::new());
        let idx = se.compile("data.user_type + 1").unwrap();
        let result = se
            .eval_one_with_cancel(&test_input(), idx, cancel)
            .unwrap();
        assert_eq!(result, Value::Number(3.0));
    }
}
