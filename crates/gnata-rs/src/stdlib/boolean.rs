//! Boolean functions: $boolean, $not, $exists.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

pub fn fn_boolean(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("D3006", "$boolean: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    Ok(Value::Bool(args[0].to_boolean()))
}

pub fn fn_not(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("D3006", "$not: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    Ok(Value::Bool(!args[0].to_boolean()))
}

pub fn fn_exists(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$exists: argument is required"));
    }
    Ok(Value::Bool(!args[0].is_undefined()))
}
