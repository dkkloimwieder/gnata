use std::rc::Rc;

use super::Value;

/// Internal multi-value container used during evaluation.
///
/// Sequences represent ordered collections that may collapse to a single value
/// or array depending on context and flags. Never exposed to users — always
/// collapsed before returning from evaluation.
///
/// Go equivalent: `*Sequence` in `internal/evaluator/value.go`.
#[derive(Debug, Clone)]
pub struct Sequence {
    pub values: Vec<Value>,
    /// Do NOT unwrap single-element sequences (set by `[]` suffix).
    pub keep_singleton: bool,
    /// Explicitly constructed via `[...]`; prevents flattening.
    pub cons_array: bool,
    /// Input was a JSON array; treated as a single document.
    pub outer_wrapper: bool,
    /// Contains tuple objects `{"@": value, varName: value}`.
    pub tuple_stream: bool,
}

impl Sequence {
    /// Create an empty sequence with default flags.
    pub fn new() -> Self {
        Self {
            values: Vec::with_capacity(4),
            keep_singleton: false,
            cons_array: false,
            outer_wrapper: false,
            tuple_stream: false,
        }
    }

    /// Create an empty sequence with pre-allocated capacity.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            values: Vec::with_capacity(cap),
            keep_singleton: false,
            cons_array: false,
            outer_wrapper: false,
            tuple_stream: false,
        }
    }

    /// Create a sequence pre-populated with items.
    pub fn with_items(items: Vec<Value>) -> Self {
        Self {
            values: items,
            keep_singleton: false,
            cons_array: false,
            outer_wrapper: false,
            tuple_stream: false,
        }
    }

    /// Append a value, flattening nested sequences but skipping Undefined.
    ///
    /// Go equivalent: `appendToSequence` in `eval_helpers.go`.
    pub fn append(&mut self, v: Value) {
        match v {
            Value::Undefined => {}
            Value::Sequence(inner) => {
                for item in inner.values {
                    self.append(item);
                }
            }
            other => self.values.push(other),
        }
    }

    /// Apply JSONata singleton-collapsing rules:
    /// - len 0 → Undefined
    /// - len 1 → element (unless KeepSingleton → wrapped in array)
    /// - len > 1 → Array
    ///
    /// Go equivalent: `CollapseSequence` in `value.go`.
    pub fn collapse(&self) -> Value {
        match self.values.len() {
            0 => Value::Undefined,
            1 => {
                if self.keep_singleton {
                    Value::Array(Rc::from(vec![self.values[0].clone()]))
                } else {
                    self.values[0].clone()
                }
            }
            _ => Value::Array(Rc::from(self.values.clone())),
        }
    }

    /// Collapse with KeepArray support for `[]` suffix on function calls.
    ///
    /// Go equivalent: `CollapseAndKeep` in `value.go`.
    pub fn collapse_and_keep(mut self, keep_array: bool) -> Value {
        if keep_array {
            self.keep_singleton = true;
        }
        let result = self.collapse();
        if keep_array {
            match result {
                Value::Array(_) => result,
                Value::Undefined => Value::Undefined,
                scalar => Value::Array(Rc::from(vec![scalar])),
            }
        } else {
            result
        }
    }

    /// Return values as a plain Vec (cloned).
    pub fn to_vec(&self) -> Vec<Value> {
        self.values.clone()
    }
}

impl Default for Sequence {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn collapse_empty_is_undefined() {
        let seq = Sequence::new();
        assert!(seq.collapse().is_undefined());
    }

    #[test]
    fn collapse_singleton_unwraps() {
        let seq = Sequence::with_items(vec![Value::Number(42.0)]);
        assert_eq!(seq.collapse(), Value::Number(42.0));
    }

    #[test]
    fn collapse_singleton_keeps_when_flagged() {
        let mut seq = Sequence::with_items(vec![Value::Number(42.0)]);
        seq.keep_singleton = true;
        let result = seq.collapse();
        assert_eq!(result, Value::Array(Rc::from(vec![Value::Number(42.0)])));
    }

    #[test]
    fn collapse_multiple_returns_array() {
        let seq = Sequence::with_items(vec![Value::Number(1.0), Value::Number(2.0)]);
        let result = seq.collapse();
        assert_eq!(
            result,
            Value::Array(Rc::from(vec![Value::Number(1.0), Value::Number(2.0)]))
        );
    }

    #[test]
    fn append_skips_undefined() {
        let mut seq = Sequence::new();
        seq.append(Value::Number(1.0));
        seq.append(Value::Undefined);
        seq.append(Value::Number(2.0));
        assert_eq!(seq.values.len(), 2);
    }

    #[test]
    fn append_flattens_nested_sequences() {
        let inner = Sequence::with_items(vec![Value::Number(1.0), Value::Number(2.0)]);
        let mut outer = Sequence::new();
        outer.append(Value::Number(0.0));
        outer.append(Value::Sequence(Box::new(inner)));
        outer.append(Value::Number(3.0));
        assert_eq!(outer.values.len(), 4);
    }

    #[test]
    fn collapse_and_keep_wraps_scalar() {
        let seq = Sequence::with_items(vec![Value::Number(42.0)]);
        let result = seq.collapse_and_keep(true);
        assert_eq!(result, Value::Array(Rc::from(vec![Value::Number(42.0)])));
    }

    #[test]
    fn collapse_and_keep_preserves_undefined() {
        let seq = Sequence::new();
        let result = seq.collapse_and_keep(true);
        assert!(result.is_undefined());
    }
}
