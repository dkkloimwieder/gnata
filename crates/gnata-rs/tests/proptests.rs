//! Property-based tests using proptest.

use std::rc::Rc;

use compact_str::CompactString;
use proptest::prelude::*;

use gnata::value::{ObjectMap, Value};

// ── Value strategy ──────────────────────────────────────────────────────────

/// Generate arbitrary JSON-representable Values (no Sequence/Function/TailCall).
fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        // Finite f64 only — NaN/Inf serialize to null, breaking roundtrip.
        any::<f64>()
            .prop_filter("must be finite and not -0", |n| n.is_finite() && !(*n == 0.0 && n.is_sign_negative()))
            .prop_map(Value::Number),
        "[a-zA-Z0-9_ ]{0,50}".prop_map(|s| Value::String(CompactString::from(s))),
    ];

    leaf.prop_recursive(
        3,   // max depth
        64,  // max nodes
        4,   // items per collection
        |inner| {
            prop_oneof![
                // Arrays
                prop::collection::vec(inner.clone(), 0..5)
                    .prop_map(|v| Value::Array(Rc::from(v))),
                // Objects
                prop::collection::vec(
                    ("[a-zA-Z_][a-zA-Z0-9_]{0,10}", inner),
                    0..5,
                )
                .prop_map(|pairs| {
                    let mut map = ObjectMap::new();
                    for (k, v) in pairs {
                        map.insert(CompactString::from(k), v);
                    }
                    Value::Object(Rc::from(map))
                }),
            ]
        },
    )
}

// ── Property: Value JSON roundtrip ──────────────────────────────────────────

proptest! {
    #[test]
    fn value_json_roundtrip(val in arb_value()) {
        let json_val = val.to_json();
        let json_str = serde_json::to_string(&json_val).unwrap();
        // Use serde_json for roundtrip (simd-json rejects some ryu-js formatted numbers).
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(json_val, parsed);
    }
}

// ── Property: deep_equal reflexivity ────────────────────────────────────────

proptest! {
    #[test]
    fn deep_equal_reflexive(val in arb_value()) {
        // deep_equal(undefined, undefined) is false by JSONata spec.
        if !val.is_undefined() {
            prop_assert!(val.deep_equal(&val), "deep_equal should be reflexive for {:?}", val);
        }
    }
}

// ── Property: deep_equal symmetry ───────────────────────────────────────────

proptest! {
    #[test]
    fn deep_equal_symmetric(a in arb_value(), b in arb_value()) {
        prop_assert_eq!(
            a.deep_equal(&b),
            b.deep_equal(&a),
            "deep_equal should be symmetric for {:?} and {:?}", a, b,
        );
    }
}

// ── Property: parse_signature never panics ──────────────────────────────────

proptest! {
    #[test]
    fn parse_signature_no_panic(s in "[ -~]{0,30}") {
        // Should return Ok or Err, never panic.
        let _ = gnata::evaluator::parse_signature(&s);
    }
}

// ── Property: Parser::parse never panics ────────────────────────────────────

proptest! {
    #[test]
    fn parser_parse_no_panic(s in "[ -~]{0,50}") {
        let _ = gnata::parser::Parser::parse(&s);
    }
}
