#![no_main]

use libfuzzer_sys::fuzz_target;
use gnata::Expression;
use gnata::value::Value;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Compile + evaluate should never panic on any input.
        if let Ok(expr) = Expression::compile(s) {
            let _ = expr.evaluate_value(&Value::Undefined);
            let _ = expr.evaluate_value(&Value::Null);
            let _ = expr.evaluate_value(&Value::Number(42.0));
        }
    }
});
