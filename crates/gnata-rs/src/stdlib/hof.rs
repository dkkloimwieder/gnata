//! Higher-order functions: $map, $filter, $reduce, $each, $sift, $sort, $single.

use std::rc::Rc;

use crate::error::{JsonataError, JsonataResult};
use crate::evaluator::{Environment, call_function};
use crate::parser::AstArena;
use crate::value::Value;

pub fn fn_map(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new("T0410", "$map: requires 2 arguments"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let arr = match &args[0] {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    let func = match &args[1] {
        Value::Function(f) => f.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$map: second argument must be a function",
            ));
        }
    };
    let mut result = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let call_args = vec![
            item.clone(),
            Value::Number(i as f64),
            Value::Array(arr.clone()),
        ];
        let val = call_function(&func, &call_args, item, env, arena)?;
        if !val.is_undefined() {
            result.push(val);
        }
    }
    Ok(Value::Array(result))
}

pub fn fn_filter(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new("T0410", "$filter: requires 2 arguments"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let arr = match &args[0] {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    let func = match &args[1] {
        Value::Function(f) => f.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$filter: second argument must be a function",
            ));
        }
    };
    let mut result = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let call_args = vec![
            item.clone(),
            Value::Number(i as f64),
            Value::Array(arr.clone()),
        ];
        let val = call_function(&func, &call_args, item, env, arena)?;
        if val.to_boolean() {
            result.push(item.clone());
        }
    }
    if result.is_empty() {
        return Ok(Value::Undefined);
    }
    if result.len() == 1 {
        return Ok(result.into_iter().next().unwrap());
    }
    Ok(Value::Array(result))
}

pub fn fn_reduce(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new("T0410", "$reduce: requires 2 arguments"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let arr = match &args[0] {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    let func = match &args[1] {
        Value::Function(f) => f.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$reduce: second argument must be a function",
            ));
        }
    };
    let init = args.get(2).cloned();
    if arr.is_empty() {
        return Ok(init.unwrap_or(Value::Undefined));
    }
    let (mut acc, start) = match init {
        Some(v) => (v, 0),
        None => (arr[0].clone(), 1),
    };
    for item in &arr[start..] {
        acc = call_function(&func, &[acc, item.clone()], item, env, arena)?;
    }
    Ok(acc)
}

pub fn fn_each(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new("T0410", "$each: requires 2 arguments"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let obj = match &args[0] {
        Value::Object(o) => o,
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$each: first argument must be an object",
            ));
        }
    };
    let func = match &args[1] {
        Value::Function(f) => f.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$each: second argument must be a function",
            ));
        }
    };
    let mut result = Vec::new();
    for (key, val) in obj {
        let call_args = vec![val.clone(), Value::String(key.clone())];
        let r = call_function(&func, &call_args, val, env, arena)?;
        if !r.is_undefined() {
            result.push(r);
        }
    }
    if result.is_empty() {
        return Ok(Value::Undefined);
    }
    if result.len() == 1 {
        return Ok(result.into_iter().next().unwrap());
    }
    Ok(Value::Array(result))
}

pub fn fn_sift(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new("T0410", "$sift: requires 2 arguments"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let obj = match &args[0] {
        Value::Object(o) => o,
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$sift: first argument must be an object",
            ));
        }
    };
    let func = match &args[1] {
        Value::Function(f) => f.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$sift: second argument must be a function",
            ));
        }
    };
    let mut result = indexmap::IndexMap::new();
    for (key, val) in obj {
        let call_args = vec![val.clone(), Value::String(key.clone())];
        let keep = call_function(&func, &call_args, val, env, arena)?;
        if keep.to_boolean() {
            result.insert(key.clone(), val.clone());
        }
    }
    if result.is_empty() {
        return Ok(Value::Undefined);
    }
    Ok(Value::Object(result))
}

pub fn fn_sort(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$sort: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let mut arr = match &args[0] {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    if arr.len() <= 1 {
        return Ok(Value::Array(arr));
    }
    let comparator = args.get(1).and_then(|v| match v {
        Value::Function(f) => Some(f.clone()),
        _ => None,
    });
    // Sort with optional comparator.
    let mut error: Option<JsonataError> = None;
    arr.sort_by(|a, b| {
        if error.is_some() {
            return std::cmp::Ordering::Equal;
        }
        match &comparator {
            Some(func) => match call_function(func, &[a.clone(), b.clone()], a, env, arena) {
                Ok(val) => {
                    if let Some(n) = val.as_f64() {
                        n.partial_cmp(&0.0)
                            .map(|c| c.reverse())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    } else if val.to_boolean() {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Less
                    }
                }
                Err(e) => {
                    error = Some(e);
                    std::cmp::Ordering::Equal
                }
            },
            None => {
                // Default: compare by value.
                match a.compare_order(b) {
                    Ok(n) => match n.cmp(&0) {
                        std::cmp::Ordering::Less => std::cmp::Ordering::Less,
                        std::cmp::Ordering::Equal => std::cmp::Ordering::Equal,
                        std::cmp::Ordering::Greater => std::cmp::Ordering::Greater,
                    },
                    Err(e) => {
                        error = Some(e);
                        std::cmp::Ordering::Equal
                    }
                }
            }
        }
    });
    if let Some(e) = error {
        return Err(e);
    }
    Ok(Value::Array(arr))
}

pub fn fn_single(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$single: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let arr = match &args[0] {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    let func = args.get(1).and_then(|v| match v {
        Value::Function(f) => Some(f.clone()),
        _ => None,
    });
    let mut matches = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let keep = match &func {
            Some(f) => {
                let call_args = vec![
                    item.clone(),
                    Value::Number(i as f64),
                    Value::Array(arr.clone()),
                ];
                call_function(f, &call_args, item, env, arena)?.to_boolean()
            }
            None => true,
        };
        if keep {
            matches.push(item.clone());
            if matches.len() > 1 {
                return Err(JsonataError::new(
                    "D3138",
                    "$single: expected 1 match, found multiple",
                ));
            }
        }
    }
    match matches.len() {
        0 => Err(JsonataError::new(
            "D3138",
            "$single: expected 1 match, found 0",
        )),
        _ => Ok(matches.into_iter().next().unwrap()),
    }
}
