//! Array functions: $count, $append, $reverse, $shuffle, $distinct, $flatten, $zip.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

pub fn fn_count(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$count: argument is required"));
    }
    if args.len() > 1 {
        return Err(JsonataError::new("T0410", "$count: expects 1 argument"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Number(0.0));
    }
    match &args[0] {
        Value::Array(a) => Ok(Value::Number(a.len() as f64)),
        _ => Ok(Value::Number(1.0)), // scalar counts as 1
    }
}

pub fn fn_append(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new("T0410", "$append: requires 2 arguments"));
    }
    let a = &args[0];
    let b = &args[1];
    // If either is undefined, return the other unchanged.
    if a.is_undefined() {
        return Ok(b.clone());
    }
    if b.is_undefined() {
        return Ok(a.clone());
    }
    let mut result = match a {
        Value::Array(arr) => arr.clone(),
        other => vec![other.clone()],
    };
    match b {
        Value::Array(arr) => result.extend(arr.iter().cloned()),
        other => result.push(other.clone()),
    }
    Ok(Value::Array(result))
}

pub fn fn_reverse(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$reverse: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let mut arr = match &args[0] {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    arr.reverse();
    Ok(Value::Array(arr))
}

pub fn fn_shuffle(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$shuffle: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let mut arr = match &args[0] {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    // Fisher-Yates shuffle.
    for i in (1..arr.len()).rev() {
        let j = fastrand::usize(..=i);
        arr.swap(i, j);
    }
    Ok(Value::Array(arr))
}

pub fn fn_distinct(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new(
            "T0410",
            "$distinct: argument is required",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let arr = match &args[0] {
        Value::Array(a) => a,
        other => return Ok(other.clone()),
    };
    let mut result = Vec::new();
    for item in arr {
        if !result
            .iter()
            .any(|existing: &Value| existing.deep_equal(item))
        {
            result.push(item.clone());
        }
    }
    Ok(Value::Array(result))
}

pub fn fn_flatten(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$flatten: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let arr = match &args[0] {
        Value::Array(a) => a.clone(),
        other => return Ok(other.clone()),
    };
    let depth = args
        .get(1)
        .and_then(|v| v.as_f64())
        .map(|n| n as usize)
        .unwrap_or(usize::MAX);
    let result = flatten_recursive(&arr, depth);
    Ok(Value::Array(result))
}

fn flatten_recursive(arr: &[Value], depth: usize) -> Vec<Value> {
    let mut result = Vec::new();
    for item in arr {
        if depth > 0
            && let Value::Array(inner) = item
        {
            result.extend(flatten_recursive(inner, depth - 1));
            continue;
        }
        result.push(item.clone());
    }
    result
}

pub fn fn_zip(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new(
            "T0410",
            "$zip: requires at least 1 argument",
        ));
    }
    // If any argument is undefined, return empty array.
    if args.iter().any(|a| a.is_undefined()) {
        return Ok(Value::Array(vec![]));
    }
    // Wrap non-array args as singleton arrays.
    let arrays: Vec<Vec<Value>> = args
        .iter()
        .map(|a| match a {
            Value::Array(arr) => arr.clone(),
            other => vec![other.clone()],
        })
        .collect();
    if arrays.is_empty() {
        return Ok(Value::Array(vec![]));
    }
    // Use minimum length across all arrays.
    let min_len = arrays.iter().map(|a| a.len()).min().unwrap_or(0);
    let mut result = Vec::with_capacity(min_len);
    for i in 0..min_len {
        let tuple: Vec<Value> = arrays.iter().map(|a| a[i].clone()).collect();
        result.push(Value::Array(tuple));
    }
    Ok(Value::Array(result))
}
