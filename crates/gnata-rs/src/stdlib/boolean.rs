//! Boolean functions: $boolean, $not, $exists.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

pub fn fn_boolean(args: &[Value], focus: &Value) -> JsonataResult {
    // Note: don't enforce arity — HOF callbacks like $filter($boolean)
    // pass 3 args (value, index, array). Only use the first arg.
    let arg = if args.is_empty() { focus } else { &args[0] };
    if arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    Ok(Value::Bool(arg.to_boolean()))
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
    if args.len() > 1 {
        return Err(JsonataError::new(
            "T0410",
            "$exists: takes at most 1 argument",
        ));
    }
    Ok(Value::Bool(!args[0].is_undefined()))
}
