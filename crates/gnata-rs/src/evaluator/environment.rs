//! Evaluation environment: lexical scoping chain with shared call counter.
//!
//! Port of Go `internal/evaluator/env.go`.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::JsonataError;
use crate::value::Value;

/// Maximum recursive call depth before U1001.
/// Matches the JSONata reference implementation's default.
pub const DEFAULT_MAX_CALL_DEPTH: u32 = 100;

/// Shared recursive call depth counter across all child environments.
#[derive(Debug)]
pub struct CallCounter {
    pub depth: Cell<u32>,
    pub eval_depth: Cell<u32>,
    pub max: u32,
}

impl Default for CallCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl CallCounter {
    pub fn new() -> Self {
        Self {
            depth: Cell::new(0),
            eval_depth: Cell::new(0),
            max: DEFAULT_MAX_CALL_DEPTH,
        }
    }
}

/// Evaluation environment holding variable bindings with lexical scoping.
///
/// Forms a linked chain via `parent`. All environments in a chain share
/// the same `CallCounter` (via Rc) and cancellation token (via Arc).
#[derive(Debug)]
pub struct Environment {
    parent: Option<Rc<Environment>>,
    bindings: HashMap<String, Value>,
    calls: Rc<CallCounter>,
    cancel: Option<Arc<AtomicBool>>,
}

impl Environment {
    /// Create a root environment with no bindings.
    pub fn new() -> Self {
        Self {
            parent: None,
            bindings: HashMap::new(),
            calls: Rc::new(CallCounter::new()),
            cancel: None,
        }
    }

    /// Create a child scope inheriting from parent.
    pub fn new_child(parent: Rc<Environment>) -> Self {
        let calls = Rc::clone(&parent.calls);
        let cancel = parent.cancel.clone();
        Self {
            parent: Some(parent),
            bindings: HashMap::new(),
            calls,
            cancel,
        }
    }

    /// Set a variable in this environment.
    pub fn bind(&mut self, name: String, value: Value) {
        self.bindings.insert(name, value);
    }

    /// Look up a variable, walking the parent chain.
    pub fn lookup(&self, name: &str) -> Option<&Value> {
        if let Some(v) = self.bindings.get(name) {
            return Some(v);
        }
        if let Some(ref parent) = self.parent {
            return parent.lookup(name);
        }
        None
    }

    /// Look up a variable and return both the value and the environment
    /// in which the binding was found. Used by the parent operator (%).
    pub fn lookup_with_env(&self, name: &str) -> Option<(&Value, &Environment)> {
        if let Some(v) = self.bindings.get(name) {
            return Some((v, self));
        }
        if let Some(ref parent) = self.parent {
            return parent.lookup_with_env(name);
        }
        None
    }

    /// Check only the direct bindings (no parent chain walk).
    pub fn lookup_direct(&self, name: &str) -> Option<&Value> {
        self.bindings.get(name)
    }

    /// Get the parent environment.
    pub fn parent(&self) -> Option<&Rc<Environment>> {
        self.parent.as_ref()
    }

    /// Get a reference to the shared call counter.
    pub fn call_counter(&self) -> &Rc<CallCounter> {
        &self.calls
    }

    /// Install a fresh call counter, decoupling from parent.
    /// Used for per-$eval isolation.
    pub fn reset_call_counter(&mut self) {
        self.calls = Rc::new(CallCounter::new());
    }

    /// Increment the $eval nesting counter.
    pub fn incr_eval_depth(&self, max_depth: u32) -> Result<(), JsonataError> {
        let d = self.calls.eval_depth.get() + 1;
        if d > max_depth {
            return Err(JsonataError::new(
                "D3121",
                "$eval: maximum nesting depth exceeded",
            ));
        }
        self.calls.eval_depth.set(d);
        Ok(())
    }

    /// Decrement the $eval nesting counter.
    pub fn decr_eval_depth(&self) {
        let d = self.calls.eval_depth.get();
        if d > 0 {
            self.calls.eval_depth.set(d - 1);
        }
    }

    /// Set the cancellation token.
    pub fn set_cancel(&mut self, cancel: Arc<AtomicBool>) {
        self.cancel = Some(cancel);
    }

    /// Check if evaluation has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    /// Clone this environment (shallow copy of bindings, shared parent/calls).
    pub fn shallow_clone(&self) -> Self {
        Self {
            parent: self.parent.clone(),
            bindings: self.bindings.clone(),
            calls: Rc::clone(&self.calls),
            cancel: self.cancel.clone(),
        }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_lookup_missing() {
        let env = Environment::new();
        assert!(env.lookup("x").is_none());
    }

    #[test]
    fn bind_and_lookup() {
        let mut env = Environment::new();
        env.bind("x".into(), Value::Number(42.0));
        assert_eq!(env.lookup("x"), Some(&Value::Number(42.0)));
    }

    #[test]
    fn child_inherits_parent() {
        let mut root = Environment::new();
        root.bind("x".into(), Value::Number(1.0));
        let parent = Rc::new(root);
        let child = Environment::new_child(Rc::clone(&parent));
        assert_eq!(child.lookup("x"), Some(&Value::Number(1.0)));
    }

    #[test]
    fn child_shadows_parent() {
        let mut root = Environment::new();
        root.bind("x".into(), Value::Number(1.0));
        let parent = Rc::new(root);
        let mut child = Environment::new_child(Rc::clone(&parent));
        child.bind("x".into(), Value::Number(2.0));
        assert_eq!(child.lookup("x"), Some(&Value::Number(2.0)));
    }

    #[test]
    fn shared_call_counter() {
        let root = Environment::new();
        let parent = Rc::new(root);
        let child = Environment::new_child(Rc::clone(&parent));
        parent.calls.depth.set(5);
        assert_eq!(child.calls.depth.get(), 5);
    }

    #[test]
    fn lookup_direct_no_parent() {
        let mut root = Environment::new();
        root.bind("x".into(), Value::Number(1.0));
        let parent = Rc::new(root);
        let child = Environment::new_child(parent);
        assert!(child.lookup_direct("x").is_none());
    }

    #[test]
    fn cancellation() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut env = Environment::new();
        env.set_cancel(Arc::clone(&cancel));
        assert!(!env.is_cancelled());
        cancel.store(true, Ordering::Relaxed);
        assert!(env.is_cancelled());
    }
}
