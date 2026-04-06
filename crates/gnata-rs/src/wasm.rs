//! WebAssembly entry points via wasm-bindgen.
//!
//! Exports match the Go WASM API:
//!   gnataEval(expr, jsonData) → string (JSON) or throws
//!   gnataCompile(expr) → handle (number) or throws
//!   gnataEvalHandle(handle, jsonData) → string (JSON) or throws
//!   gnataReleaseHandle(handle)

use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::prelude::*;

use crate::expression::Expression;
use crate::value::Value;

thread_local! {
    static COMPILED: RefCell<HashMap<u32, Expression>> = RefCell::new(HashMap::new());
    static EXPR_CACHE: RefCell<HashMap<String, Expression>> = RefCell::new(HashMap::new());
    static NEXT_HANDLE: RefCell<u32> = const { RefCell::new(0) };
}

/// Compile and evaluate a JSONata expression against JSON data.
/// Returns the result as a JSON string, or empty string for undefined.
#[wasm_bindgen(js_name = gnataEval)]
pub fn eval(expr: &str, json_data: &str) -> Result<String, JsError> {
    // Check cache first
    let cached_result = EXPR_CACHE.with(|cache| {
        let cache = cache.borrow();
        cache.get(expr).map(|e| eval_expression(e, json_data))
    });

    if let Some(result) = cached_result {
        return result;
    }

    let compiled = Expression::compile(expr).map_err(|e| JsError::new(&e.to_string()))?;
    let result = eval_expression(&compiled, json_data);

    EXPR_CACHE.with(|cache| {
        cache.borrow_mut().insert(expr.to_string(), compiled);
    });

    result
}

/// Compile a JSONata expression and return a numeric handle.
#[wasm_bindgen(js_name = gnataCompile)]
pub fn compile(expr: &str) -> Result<u32, JsError> {
    let compiled = Expression::compile(expr).map_err(|e| JsError::new(&e.to_string()))?;

    let handle = NEXT_HANDLE.with(|h| {
        let mut h = h.borrow_mut();
        *h = h.wrapping_add(1);
        *h
    });

    COMPILED.with(|cache| {
        cache.borrow_mut().insert(handle, compiled);
    });

    Ok(handle)
}

/// Evaluate a previously compiled expression (by handle) against JSON data.
#[wasm_bindgen(js_name = gnataEvalHandle)]
pub fn eval_handle(handle: u32, json_data: &str) -> Result<String, JsError> {
    COMPILED.with(|cache| {
        let cache = cache.borrow();
        let expr = cache
            .get(&handle)
            .ok_or_else(|| JsError::new(&format!("unknown handle {handle}")))?;
        eval_expression(expr, json_data)
    })
}

/// Release a compiled expression handle.
#[wasm_bindgen(js_name = gnataReleaseHandle)]
pub fn release_handle(handle: u32) {
    COMPILED.with(|cache| {
        cache.borrow_mut().remove(&handle);
    });
}

fn eval_expression(expr: &Expression, json_data: &str) -> Result<String, JsError> {
    let input = if json_data.is_empty() || json_data == "null" {
        Value::Undefined
    } else {
        Value::from_json_str(json_data).map_err(|e| JsError::new(&e.to_string()))?
    };

    let result = expr
        .evaluate(&input)
        .map_err(|e| JsError::new(&e.to_string()))?;

    if result.is_undefined() {
        return Ok(String::new());
    }

    let json = result.to_json();
    serde_json::to_string(&json).map_err(|e| JsError::new(&e.to_string()))
}
