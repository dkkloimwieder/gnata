mod format;
mod sequence;

pub use format::{format_float, format_number};
pub use sequence::Sequence;

use indexmap::IndexMap;
use serde_json::Number;

use crate::error::{JsonataError, JsonataResult};

/// Core value type for JSONata evaluation.
///
/// Uses owned types throughout — no lifetimes in the public API.
/// `Undefined` and `Null` are distinct enum variants preserving JSONata semantics.
/// `Sequence` is internal-only and never exposed to users.
#[derive(Debug, Clone)]
pub enum Value {
    /// JSONata undefined — missing value, no representation in JSON.
    /// Go equivalent: `nil`.
    Undefined,
    /// JSON null — explicit null value.
    /// Go equivalent: `jsonNullType{}` sentinel.
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(IndexMap<String, Value>),
    /// Internal sequence used during evaluation. Never returned to users.
    Sequence(Sequence),
    /// Function value (built-in, lambda, partial application).
    /// Internal — collapsed before returning to users.
    Function(crate::evaluator::FunctionValue),
    /// Tail-call sentinel for TCO trampoline. Internal only.
    TailCall(Box<crate::evaluator::TailCall>),
}

impl Value {
    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Value::Number(_))
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Value::Bool(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Value::Array(_))
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    pub fn is_sequence(&self) -> bool {
        matches!(self, Value::Sequence(_))
    }

    pub fn is_function(&self) -> bool {
        matches!(self, Value::Function(_))
    }

    /// Returns true if the value is a finite number (not NaN or Inf).
    pub fn is_numeric(&self) -> bool {
        match self {
            Value::Number(n) => n.is_finite(),
            _ => false,
        }
    }

    /// Extract as f64 if this is a Number variant.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&IndexMap<String, Value>> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Validates that this is a finite number. Returns D1001 for Inf/NaN.
    pub fn check_numeric(&self) -> Result<(), JsonataError> {
        if let Value::Number(n) = self
            && (n.is_infinite() || n.is_nan())
        {
            return Err(JsonataError::with_code("D1001").with_value(format_float(*n)));
        }
        Ok(())
    }

    // ── Boolean coercion ─────────────────────────────────────────────

    /// Implements JSONata boolean casting rules.
    ///
    /// - Undefined/Null → false
    /// - Bool → value
    /// - String → non-empty
    /// - Number → non-zero
    /// - Object → non-empty
    /// - Array: len 0 → false, len 1 → recurse, len > 1 → any truthy
    /// - Sequence → collapse then recurse
    pub fn to_boolean(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::String(s) => !s.is_empty(),
            Value::Number(n) => *n != 0.0,
            Value::Object(m) => !m.is_empty(),
            Value::Array(arr) => match arr.len() {
                0 => false,
                1 => arr[0].to_boolean(),
                _ => arr.iter().any(|v| v.to_boolean()),
            },
            Value::Sequence(seq) => seq.collapse().to_boolean(),
            Value::Function(_) => true,
            Value::TailCall(_) => true,
        }
    }

    // ── Equality ─────────────────────────────────────────────────────

    /// Implements JSONata structural equality.
    ///
    /// Critical invariant: `undefined = undefined` returns `false`.
    /// `null = null` returns `true`.
    pub fn deep_equal(&self, other: &Value) -> bool {
        // Undefined on either side → false (even undefined = undefined)
        if self.is_undefined() || other.is_undefined() {
            return false;
        }
        // Null matches only null
        if self.is_null() || other.is_null() {
            return self.is_null() && other.is_null();
        }
        match (self, other) {
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.deep_equal(y))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, va)| b.get(k).is_some_and(|vb| va.deep_equal(vb)))
            }
            _ => false,
        }
    }

    // ── Comparison ───────────────────────────────────────────────────

    /// Compares two values for ordering. Returns -1, 0, or 1.
    /// Undefined sorts after non-undefined.
    pub fn compare_order(&self, other: &Value) -> JsonataResult<i8> {
        match (self, other) {
            (Value::Undefined, Value::Undefined) => Ok(0),
            (Value::Undefined, _) => Ok(1),
            (_, Value::Undefined) => Ok(-1),
            (Value::Number(a), Value::Number(b)) => Ok(a.partial_cmp(b).map_or(0, |o| o as i8)),
            (Value::String(a), Value::String(b)) => Ok(a.cmp(b) as i8),
            (Value::Number(_), Value::String(_)) | (Value::String(_), Value::Number(_)) => Err(
                JsonataError::new("T2007", "cannot compare string and number values"),
            ),
            _ => Err(JsonataError::new(
                "T2008",
                "cannot compare values of incompatible types".to_string(),
            )),
        }
    }

    /// Relational comparison (<, <=, >, >=).
    /// Returns Undefined if either operand is undefined.
    pub fn compare(&self, other: &Value, op: &str) -> JsonataResult {
        // Validate left operand type
        if !self.is_undefined() && !self.is_number() && !self.is_string() {
            return Err(JsonataError::new(
                "T2010",
                format!("the operands of the \"{op}\" operator must be numbers or strings"),
            ));
        }
        // Undefined propagation
        if self.is_undefined() || other.is_undefined() {
            return Ok(Value::Undefined);
        }
        match (self, other) {
            (Value::Number(a), Value::Number(b)) => {
                let result = match op {
                    "<" => a < b,
                    "<=" => a <= b,
                    ">" => a > b,
                    ">=" => a >= b,
                    _ => unreachable!(),
                };
                Ok(Value::Bool(result))
            }
            (Value::Number(_), Value::String(_)) | (Value::String(_), Value::Number(_)) => {
                Err(JsonataError::new(
                    "T2009",
                    format!(
                        "the operands of the \"{op}\" operator must be both numbers or both strings"
                    ),
                ))
            }
            (Value::String(a), Value::String(b)) => {
                let result = match op {
                    "<" => a < b,
                    "<=" => a <= b,
                    ">" => a > b,
                    ">=" => a >= b,
                    _ => unreachable!(),
                };
                Ok(Value::Bool(result))
            }
            _ => Err(JsonataError::new(
                "T2010",
                format!("the operands of the \"{op}\" operator must be numbers or strings"),
            )),
        }
    }

    // ── Stringify ────────────────────────────────────────────────────

    /// Convert a value to its string representation.
    pub fn stringify(&self) -> JsonataResult<String> {
        match self {
            Value::Undefined => Ok(String::new()),
            Value::String(s) => Ok(s.clone()),
            Value::Number(n) => Ok(format_float(*n)),
            Value::Bool(true) => Ok("true".into()),
            Value::Bool(false) => Ok("false".into()),
            other => {
                let json = serde_json::to_string(&other.to_json())
                    .map_err(|e| JsonataError::new("", format!("cannot stringify value: {e}")))?;
                Ok(json)
            }
        }
    }

    /// Check if a value is "contained in" another (for `in` operator).
    pub fn contained_in(&self, arr: &Value) -> bool {
        match arr {
            Value::Array(items) => items.iter().any(|item| self.deep_equal(item)),
            Value::Sequence(seq) => seq.values.iter().any(|item| self.deep_equal(item)),
            other => self.deep_equal(other),
        }
    }

    // ── JSON conversion ──────────────────────────────────────────────

    /// Convert from serde_json::Value, preserving number precision where possible.
    pub fn from_json(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(b),
            serde_json::Value::Number(n) => {
                // With arbitrary_precision, n.as_f64() parses the string repr
                Value::Number(n.as_f64().unwrap_or(f64::NAN))
            }
            serde_json::Value::String(s) => Value::String(s),
            serde_json::Value::Array(arr) => {
                Value::Array(arr.into_iter().map(Value::from_json).collect())
            }
            serde_json::Value::Object(obj) => {
                // serde_json with preserve_order uses IndexMap internally
                Value::Object(
                    obj.into_iter()
                        .map(|(k, v)| (k, Value::from_json(v)))
                        .collect(),
                )
            }
        }
    }

    /// Convert to serde_json::Value for serialization.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Undefined | Value::Null => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Number(n) => {
                if n.is_nan() || n.is_infinite() {
                    // NaN/Inf → null in JSON (matches JS behavior)
                    serde_json::Value::Null
                } else {
                    // Use ryu-js formatting to get JS-compatible string, then parse as Number
                    let s = format_float(*n);
                    Number::from_string_unchecked(s).into()
                }
            }
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(|v| v.to_json()).collect())
            }
            Value::Object(obj) => serde_json::Value::Object(
                obj.iter().map(|(k, v)| (k.clone(), v.to_json())).collect(),
            ),
            Value::Sequence(seq) => seq.collapse().to_json(),
            // Functions and tail-calls are not JSON-representable.
            Value::Function(_) | Value::TailCall(_) => serde_json::Value::Null,
        }
    }

    /// Decode a JSON byte slice into a Value, preserving object key order.
    pub fn from_json_bytes(b: &[u8]) -> Result<Self, serde_json::Error> {
        let v: serde_json::Value = serde_json::from_slice(b)?;
        Ok(Value::from_json(v))
    }

    /// Decode a JSON string into a Value, preserving object key order.
    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        let v: serde_json::Value = serde_json::from_str(s)?;
        Ok(Value::from_json(v))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.deep_equal(other)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Number(n as f64)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_owned())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Undefined/Null distinction ───────────────────────────────────

    #[test]
    fn undefined_equals_undefined_is_false() {
        // Critical invariant: undefined = undefined → false
        assert!(!Value::Undefined.deep_equal(&Value::Undefined));
    }

    #[test]
    fn null_equals_null_is_true() {
        assert!(Value::Null.deep_equal(&Value::Null));
    }

    #[test]
    fn null_not_equal_undefined() {
        assert!(!Value::Null.deep_equal(&Value::Undefined));
        assert!(!Value::Undefined.deep_equal(&Value::Null));
    }

    // ── Boolean coercion ─────────────────────────────────────────────

    #[test]
    fn boolean_coercion() {
        assert!(!Value::Undefined.to_boolean());
        assert!(!Value::Null.to_boolean());
        assert!(Value::Bool(true).to_boolean());
        assert!(!Value::Bool(false).to_boolean());
        assert!(Value::String("hello".into()).to_boolean());
        assert!(!Value::String("".into()).to_boolean());
        // "0" is truthy, "" is falsy, "false" is truthy
        assert!(Value::String("0".into()).to_boolean());
        assert!(Value::String("false".into()).to_boolean());
        assert!(Value::Number(1.0).to_boolean());
        assert!(!Value::Number(0.0).to_boolean());
    }

    #[test]
    fn boolean_array_coercion() {
        // Empty array → false
        assert!(!Value::Array(vec![]).to_boolean());
        // Single element → recurse
        assert!(Value::Array(vec![Value::Bool(true)]).to_boolean());
        assert!(!Value::Array(vec![Value::Bool(false)]).to_boolean());
        // Multiple → any truthy
        assert!(Value::Array(vec![Value::Bool(false), Value::Bool(true)]).to_boolean());
        assert!(!Value::Array(vec![Value::Bool(false), Value::Bool(false)]).to_boolean());
    }

    // ── Deep equality ────────────────────────────────────────────────

    #[test]
    fn deep_equal_numbers() {
        assert!(Value::Number(42.0).deep_equal(&Value::Number(42.0)));
        assert!(!Value::Number(42.0).deep_equal(&Value::Number(43.0)));
    }

    #[test]
    fn deep_equal_arrays() {
        let a = Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]);
        let b = Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]);
        let c = Value::Array(vec![Value::Number(1.0), Value::Number(3.0)]);
        assert!(a.deep_equal(&b));
        assert!(!a.deep_equal(&c));
    }

    #[test]
    fn deep_equal_objects() {
        let mut a = IndexMap::new();
        a.insert("x".into(), Value::Number(1.0));
        a.insert("y".into(), Value::Number(2.0));

        let mut b = IndexMap::new();
        b.insert("y".into(), Value::Number(2.0));
        b.insert("x".into(), Value::Number(1.0));

        // Order-independent comparison
        assert!(Value::Object(a).deep_equal(&Value::Object(b)));
    }

    #[test]
    fn deep_equal_type_mismatch() {
        assert!(!Value::Number(1.0).deep_equal(&Value::String("1".into())));
        assert!(!Value::Bool(true).deep_equal(&Value::Number(1.0)));
    }

    // ── JSON round-trip ──────────────────────────────────────────────

    #[test]
    fn json_round_trip() {
        let json = r#"{"name":"test","values":[1,2,3],"active":true,"data":null}"#;
        let val = Value::from_json_str(json).unwrap();
        assert!(val.is_object());
        let obj = val.as_object().unwrap();
        assert_eq!(obj.get("name"), Some(&Value::String("test".into())));
        assert!(obj.get("data").unwrap().is_null());
    }

    #[test]
    fn json_preserves_key_order() {
        let json = r#"{"z":1,"a":2,"m":3}"#;
        let val = Value::from_json_str(json).unwrap();
        let obj = val.as_object().unwrap();
        let keys: Vec<&String> = obj.keys().collect();
        assert_eq!(keys, vec!["z", "a", "m"]);
    }

    // ── Comparison ───────────────────────────────────────────────────

    #[test]
    fn compare_numbers() {
        let r = Value::Number(1.0)
            .compare(&Value::Number(2.0), "<")
            .unwrap();
        assert_eq!(r, Value::Bool(true));
    }

    #[test]
    fn compare_undefined_propagates() {
        let r = Value::Number(1.0).compare(&Value::Undefined, "<").unwrap();
        assert!(r.is_undefined());
    }

    #[test]
    fn compare_type_mismatch_error() {
        let r = Value::Number(1.0).compare(&Value::String("a".into()), "<");
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code, "T2009");
    }

    // ── Stringify ────────────────────────────────────────────────────

    #[test]
    fn stringify_values() {
        assert_eq!(Value::Undefined.stringify().unwrap(), "");
        assert_eq!(Value::Number(42.0).stringify().unwrap(), "42");
        assert_eq!(Value::Bool(true).stringify().unwrap(), "true");
        assert_eq!(Value::String("hi".into()).stringify().unwrap(), "hi");
    }
}
