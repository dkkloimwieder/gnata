//! Type and misc functions: $type, $assert.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

pub fn fn_type_of(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$type: argument is required"));
    }
    let type_name = match &args[0] {
        Value::Undefined => return Ok(Value::Undefined),
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Function(_) => "function",
        Value::Sequence(_) | Value::TailCall(_) => "undefined",
    };
    Ok(Value::String(type_name.into()))
}

pub fn fn_assert(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$assert: argument is required"));
    }
    if args.len() > 2 {
        return Err(JsonataError::new(
            "T0410",
            "$assert: takes at most 2 arguments",
        ));
    }
    // First argument must be a boolean.
    match &args[0] {
        Value::Bool(b) => {
            if !b {
                let msg = args
                    .get(1)
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "assertion failed".into());
                return Err(JsonataError::new("D3141", msg));
            }
            Ok(Value::Undefined)
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$assert: first argument must be a boolean",
        )),
    }
}
