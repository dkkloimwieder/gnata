//! Higher-order functions: $map, $filter, $reduce, $each, $sift, $sort, $single.

use std::rc::Rc;

use crate::error::{JsonataError, JsonataResult};
use crate::evaluator::{Environment, FunctionValue, call_function};
use crate::parser::AstArena;
use crate::value::{Sequence, Value};

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
        other => Rc::new(vec![other.clone()]),
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
    let mut seq = Sequence::new();
    for (i, item) in arr.iter().enumerate() {
        let call_args = vec![
            item.clone(),
            Value::Number(i as f64),
            Value::Array(arr.clone()),
        ];
        let val = call_function(&func, &call_args, item, env, arena)?;
        if !val.is_undefined() {
            seq.values.push(val);
        }
    }
    // Return as Sequence — caller handles collapse with keep_array support.
    Ok(Value::Sequence(seq))
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
        other => Rc::new(vec![other.clone()]),
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
        return Ok(result.swap_remove(0));
    }
    Ok(Value::Array(Rc::new(result)))
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
        other => Rc::new(vec![other.clone()]),
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
    // Check that the function accepts at least 2 parameters.
    if let crate::evaluator::functions::FunctionValue::Lambda(lambda) = &func
        && lambda.params.len() < 2 {
            return Err(JsonataError::new(
                "D3050",
                "$reduce: function argument must accept at least 2 parameters",
            ));
        }
    let init = args.get(2).cloned();
    if arr.is_empty() {
        return Ok(init.unwrap_or(Value::Undefined));
    }
    let (mut acc, start) = match init {
        Some(v) => (v, 0),
        None => (arr[0].clone(), 1),
    };
    // Determine arity for passing index/array like Go does.
    let param_count = if let FunctionValue::Lambda(ref lam) = func {
        lam.params.len()
    } else {
        2 // default: (acc, item)
    };
    let arr_val = Value::Array(arr.clone());
    for (idx, item) in arr[start..].iter().enumerate() {
        let call_args = match param_count {
            0 | 1 => vec![acc],
            2 => vec![acc, item.clone()],
            3 => vec![acc, item.clone(), Value::Number((start + idx) as f64)],
            _ => vec![
                acc,
                item.clone(),
                Value::Number((start + idx) as f64),
                arr_val.clone(),
            ],
        };
        acc = call_function(&func, &call_args, item, env, arena)?;
    }
    Ok(acc)
}

pub fn fn_each(
    args: &[Value],
    focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    // When called with 1 arg (function), use focus as the object.
    let (obj_arg, func_arg) = if args.len() >= 2 {
        (&args[0], &args[1])
    } else if args.len() == 1 && args[0].is_function() {
        (focus, &args[0])
    } else if args.len() == 1 {
        (&args[0], focus)
    } else {
        return Err(JsonataError::new("T0410", "$each: requires 2 arguments"));
    };
    if obj_arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    let Value::Object(obj) = obj_arg else {
        return Err(JsonataError::new(
            "T0410",
            "$each: first argument must be an object",
        ));
    };
    let func = match func_arg {
        Value::Function(f) => f.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$each: second argument must be a function",
            ));
        }
    };
    let mut seq = Sequence::new();
    for (key, val) in obj.iter() {
        let call_args = vec![val.clone(), Value::String(key.as_str().into())];
        let r = call_function(&func, &call_args, val, env, arena)?;
        if !r.is_undefined() {
            seq.values.push(r);
        }
    }
    Ok(Value::Sequence(seq))
}

pub fn fn_sift(
    args: &[Value],
    focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    // When called with 1 arg (function), use focus as the object.
    let (obj_arg, func_arg) = if args.len() >= 2 {
        (&args[0], &args[1])
    } else if args.len() == 1 && args[0].is_function() {
        (focus, &args[0])
    } else {
        return Err(JsonataError::new("T0410", "$sift: requires 2 arguments"));
    };
    if obj_arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    let func = match func_arg {
        Value::Function(f) => f.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$sift: second argument must be a function",
            ));
        }
    };
    // If the argument is an array, map $sift over each element.
    if let Value::Array(arr) = obj_arg {
        let mut results = Vec::new();
        for item in arr.iter() {
            if let Value::Object(obj) = item {
                let sifted = sift_object(obj, &func, item, env, arena)?;
                if !sifted.is_undefined() {
                    results.push(sifted);
                }
            }
        }
        if results.is_empty() {
            return Ok(Value::Undefined);
        }
        return Ok(Value::Array(Rc::new(results)));
    }
    let Value::Object(obj) = obj_arg else {
        return Err(JsonataError::new(
            "T0410",
            "$sift: first argument must be an object",
        ));
    };
    sift_object(obj, &func, obj_arg, env, arena)
}

pub fn fn_sort(
    args: &[Value],
    focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$sort: argument is required"));
    }
    // Resolve array and comparator, mirroring Go's makeFnSort logic:
    // - 1 arg that's a function → use focus as array, arg as comparator
    // - 1 arg that's not a function → arg is the array, no comparator
    // - 2+ args → args[0] is array, args[1] is comparator
    let (arr_val, comparator) = if args.len() == 1 && args[0].is_function() {
        let f = match &args[0] {
            Value::Function(f) => Some(f.clone()),
            _ => None,
        };
        (focus, f)
    } else {
        let f = args.get(1).and_then(|v| match v {
            Value::Function(f) => Some(f.clone()),
            _ => None,
        });
        (&args[0], f)
    };
    if arr_val.is_undefined() {
        return Ok(Value::Undefined);
    }
    let mut arr = match arr_val {
        Value::Array(a) => (**a).clone(),
        other => vec![other.clone()],
    };
    if arr.len() <= 1 {
        return Ok(Value::Array(Rc::new(arr)));
    }
    // Sort with optional comparator.
    let mut error: Option<JsonataError> = None;
    arr.sort_by(|a, b| {
        if error.is_some() {
            return std::cmp::Ordering::Equal;
        }
        match &comparator {
            Some(func) => {
                // Match Go: call fn(b, a) (swapped) and map true→Less, false→Equal.
                // JSONata comparator fn(a,b) returns true when a should sort AFTER b.
                // By calling fn(b,a): true means b sorts after a → a < b → Less.
                // false means equal or a sorts after b → preserve order → Equal.
                match call_function(func, &[b.clone(), a.clone()], a, env, arena) {
                    Ok(val) => {
                        if val.to_boolean() {
                            std::cmp::Ordering::Less
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    }
                    Err(e) => {
                        error = Some(e);
                        std::cmp::Ordering::Equal
                    }
                }
            }
            None => {
                // Default: compare by value.
                match a.compare_order(b) {
                    Ok(n) => match n.cmp(&0) {
                        std::cmp::Ordering::Less => std::cmp::Ordering::Less,
                        std::cmp::Ordering::Equal => std::cmp::Ordering::Equal,
                        std::cmp::Ordering::Greater => std::cmp::Ordering::Greater,
                    },
                    Err(e) => {
                        // Remap T2008 to D3070 for $sort function context.
                        let mapped = if e.code == "T2008" {
                            JsonataError::new("D3070", e.message.clone())
                        } else {
                            e
                        };
                        error = Some(mapped);
                        std::cmp::Ordering::Equal
                    }
                }
            }
        }
    });
    if let Some(e) = error {
        return Err(e);
    }
    Ok(Value::Array(Rc::new(arr)))
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
        other => Rc::new(vec![other.clone()]),
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
            "D3139",
            "$single: expected 1 match, found 0",
        )),
        _ => Ok(matches.swap_remove(0)),
    }
}

/// Helper: sift a single object, passing (value, key, object) to the predicate.
fn sift_object(
    obj: &Rc<crate::value::FxIndexMap<String, Value>>,
    func: &crate::evaluator::functions::FunctionValue,
    obj_val: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    let mut result = crate::value::FxIndexMap::default();
    for (key, val) in obj.iter() {
        let call_args = vec![val.clone(), Value::String(key.as_str().into()), obj_val.clone()];
        let keep = call_function(func, &call_args, val, env, arena)?;
        if keep.to_boolean() {
            result.insert(key.clone(), val.clone());
        }
    }
    if result.is_empty() {
        return Ok(Value::Undefined);
    }
    Ok(Value::Object(Rc::new(result)))
}
