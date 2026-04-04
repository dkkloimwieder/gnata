//! Object functions: $keys, $values, $spread, $merge, $lookup, $error.

use std::rc::Rc;

use indexmap::IndexMap;

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

pub fn fn_keys(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$keys: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    match &args[0] {
        Value::Object(obj) => {
            if obj.is_empty() {
                return Ok(Value::Undefined);
            }
            let keys: Vec<Value> = obj.keys().map(|k| Value::String(k.as_str().into())).collect();
            if keys.len() == 1 {
                return Ok(keys.into_iter().next().unwrap_or(Value::Undefined));
            }
            Ok(Value::Array(Rc::new(keys)))
        }
        Value::Array(arr) => {
            // Collect all keys from array of objects.
            let mut all_keys = Vec::new();
            for item in arr.iter() {
                if let Value::Object(obj) = item {
                    for k in obj.keys() {
                        if !all_keys
                            .iter()
                            .any(|existing: &Value| matches!(existing, Value::String(s) if s.as_ref() == k))
                        {
                            all_keys.push(Value::String(k.as_str().into()));
                        }
                    }
                }
            }
            if all_keys.is_empty() {
                return Ok(Value::Undefined);
            }
            if all_keys.len() == 1 {
                return Ok(all_keys.into_iter().next().unwrap_or(Value::Undefined));
            }
            Ok(Value::Array(Rc::new(all_keys)))
        }
        _ => Ok(Value::Undefined),
    }
}

pub fn fn_values(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$values: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    match &args[0] {
        Value::Object(obj) => {
            let vals: Vec<Value> = obj.values().cloned().collect();
            Ok(Value::Array(Rc::new(vals)))
        }
        _ => Ok(Value::Undefined),
    }
}

pub fn fn_spread(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$spread: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let spread_one = |obj: &IndexMap<String, Value>| -> Vec<Value> {
        obj.iter()
            .map(|(k, v)| {
                let mut m = IndexMap::new();
                m.insert(k.clone(), v.clone());
                Value::Object(Rc::new(m))
            })
            .collect()
    };
    match &args[0] {
        Value::Object(obj) => {
            let items = spread_one(obj);
            Ok(Value::Sequence(crate::value::Sequence::with_items(items)))
        }
        Value::Array(arr) => {
            let mut result = Vec::new();
            for item in arr.iter() {
                if let Value::Object(obj) = item {
                    result.extend(spread_one(obj));
                }
            }
            Ok(Value::Array(Rc::new(result)))
        }
        _ => Ok(args[0].clone()),
    }
}

pub fn fn_merge(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$merge: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let mut merged = IndexMap::new();
    let merge_obj = |merged: &mut IndexMap<String, Value>, obj: &IndexMap<String, Value>| {
        for (k, v) in obj {
            merged.insert(k.clone(), v.clone());
        }
    };
    match &args[0] {
        Value::Array(arr) => {
            for item in arr.iter() {
                if let Value::Object(obj) = item {
                    merge_obj(&mut merged, obj);
                }
            }
        }
        Value::Object(obj) => merge_obj(&mut merged, obj),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$merge: argument must be an array of objects",
            ));
        }
    }
    Ok(Value::Object(Rc::new(merged)))
}

pub fn fn_lookup(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new("T0410", "$lookup: requires 2 arguments"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let key: &str = match &args[1] {
        Value::String(s) => s,
        _ => return Err(JsonataError::new("T0410", "$lookup: key must be a string")),
    };
    match &args[0] {
        Value::Object(obj) => Ok(obj.get(key).cloned().unwrap_or(Value::Undefined)),
        Value::Array(arr) => {
            // Lookup across array of objects.
            let mut result = Vec::new();
            for item in arr.iter() {
                if let Value::Object(obj) = item
                    && let Some(v) = obj.get(key)
                {
                    result.push(v.clone());
                }
            }
            match result.len() {
                0 => Ok(Value::Undefined),
                1 => Ok(result.into_iter().next().unwrap_or(Value::Undefined)),
                _ => Ok(Value::Array(Rc::new(result))),
            }
        }
        _ => Ok(Value::Undefined),
    }
}

pub fn fn_error(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Err(JsonataError::new("D3137", "$error() function evaluated"));
    }
    match &args[0] {
        Value::String(s) => Err(JsonataError::new("D3137", s.to_string())),
        _ => Err(JsonataError::new(
            "T0410",
            "$error: argument must be a string",
        )),
    }
}
