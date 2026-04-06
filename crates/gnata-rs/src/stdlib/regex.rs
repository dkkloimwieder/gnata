//! Regex support: $match, $replace, and regex compilation helpers.
//!
//! Port of Go `functions/string_match_replace.go` and `internal/evaluator/eval_regex.go`.

use std::rc::Rc;

use regex::Regex;

use crate::error::{JsonataError, JsonataResult};
use crate::evaluator::{Environment, FunctionValue, call_function};
use crate::parser::AstArena;
use crate::value::Value;

/// Compile a regex from a pattern string and flags.
#[allow(clippy::missing_errors_doc)]
pub fn compile_regex(pattern: &str, flags: &str) -> Result<Regex, JsonataError> {
    let mut inline = String::new();
    if flags.contains('i') {
        inline.push('i');
    }
    if flags.contains('m') {
        inline.push('m');
    }
    if flags.contains('s') {
        inline.push('s');
    }
    let full = if inline.is_empty() {
        pattern.to_string()
    } else {
        format!("(?{inline}){pattern}")
    };
    Regex::new(&full).map_err(|e| JsonataError::new("D3137", format!("invalid regex: {e}")))
}

/// Compile a regex from a Value (string or regex object {pattern, flags}).
fn compile_regex_arg(v: &Value) -> Result<Regex, JsonataError> {
    match v {
        Value::String(s) => {
            let escaped = regex::escape(s);
            Regex::new(&escaped)
                .map_err(|e| JsonataError::new("D3137", format!("regex error: {e}")))
        }
        Value::Object(obj) => {
            let pattern: &str = match obj.get("pattern") {
                Some(Value::String(s)) => s,
                _ => "",
            };
            let flags: &str = match obj.get("flags") {
                Some(Value::String(s)) => s,
                _ => "",
            };
            compile_regex(pattern, flags)
        }
        _ => Err(JsonataError::new(
            "T0410",
            "expected a string or regex pattern",
        )),
    }
}

/// Build a match result object from a regex match.
fn build_match_object(s: &str, caps: &regex::Captures, m: &regex::Match) -> Value {
    let match_str: compact_str::CompactString = m.as_str().into();
    let start = s[..m.start()].chars().count() as f64;
    let end = s[..m.end()].chars().count() as f64;

    let mut groups = Vec::new();
    for i in 1..caps.len() {
        match caps.get(i) {
            Some(g) => groups.push(Value::String(g.as_str().into())),
            None => groups.push(Value::String("".into())),
        }
    }

    let mut obj = crate::value::ObjectMap::new();
    obj.insert("match".into(), Value::String(match_str));
    obj.insert("start".into(), Value::Number(start));
    obj.insert("end".into(), Value::Number(end));
    obj.insert("groups".into(), Value::Array(Rc::new(groups)));
    Value::Object(Rc::new(obj))
}

/// $match(str, pattern, limit?)
#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
pub fn fn_match(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new(
            "D3006",
            "$match: requires at least 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let s: &str = match &args[0] {
        Value::String(s) => s,
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$match: argument 1 must be a string",
            ));
        }
    };

    let limit: Option<usize> = args.get(2).and_then(super::super::value::Value::as_f64).map(|n| n as usize);

    // If the second argument is a function, use custom matcher protocol.
    if let Value::Function(func) = &args[1] {
        return match_with_custom_matcher(s, func, limit, env, arena);
    }

    let re = compile_regex_arg(&args[1])?;

    let mut result = Vec::new();
    for caps in re.captures_iter(s) {
        if let Some(lim) = limit
            && result.len() >= lim
        {
            break;
        }
        if let Some(m) = caps.get(0) {
            result.push(build_match_object(s, &caps, &m));
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

/// Custom matcher: call a function that returns {match, start, groups, next} objects.
fn match_with_custom_matcher(
    s: &str,
    matcher_fn: &FunctionValue,
    limit: Option<usize>,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    let mut result = Vec::new();

    // Initial call: matcher_fn(str, 0)
    let mut res = call_function(
        matcher_fn,
        &[Value::String(s.into()), Value::Number(0.0)],
        &Value::Undefined,
        env,
        arena,
    )?;

    while let Value::Object(obj) = &res {

        let match_val = obj.get("match").cloned().unwrap_or(Value::Undefined);
        let start_val = obj.get("start").cloned().unwrap_or(Value::Undefined);
        let groups_val = obj.get("groups").cloned().unwrap_or(Value::Array(Rc::new(vec![])));

        let mut match_obj = crate::value::ObjectMap::new();
        match_obj.insert("match".into(), match_val);
        match_obj.insert("index".into(), start_val);
        match_obj.insert("groups".into(), groups_val);
        result.push(Value::Object(Rc::new(match_obj)));

        if let Some(lim) = limit
            && result.len() >= lim {
                break;
            }

        // Get the next function and call it.
        let next_fn = match obj.get("next") {
            Some(Value::Function(f)) => f.clone(),
            _ => break,
        };
        res = call_function(&next_fn, &[], &Value::Undefined, env, arena)?;
    }

    if result.is_empty() {
        return Ok(Value::Undefined);
    }
    if result.len() == 1 {
        return Ok(result.swap_remove(0));
    }
    Ok(Value::Array(Rc::new(result)))
}

/// $replace(str, pattern, replacement, limit?)
#[allow(clippy::missing_errors_doc)]
pub fn fn_replace(
    args: &[Value],
    _focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let s: compact_str::CompactString = match &args[0] {
        Value::String(s) => s.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$replace: argument 1 must be a string",
            ));
        }
    };
    if args.len() < 3 {
        return Err(JsonataError::new(
            "T0410",
            "$replace: argument 3 (replacement) is required",
        ));
    }

    if let Some(v) = args.get(3)
        && v.is_null() {
            return Err(JsonataError::new(
                "T0410",
                "$replace: fourth argument must be a number",
            ));
        }
    let limit: Option<usize> = args.get(3).and_then(|v| {
        v.as_f64().map(|n| {
            if n < 0.0 {
                usize::MAX // will be caught below
            } else {
                n as usize
            }
        })
    });
    if let Some(v) = args.get(3)
        && let Some(n) = v.as_f64()
        && n < 0.0
    {
        return Err(JsonataError::new(
            "D3011",
            "$replace: fourth argument must not be negative",
        ));
    }

    // String pattern with string replacement — simple case.
    if let (Value::String(pattern), Value::String(replacement)) = (&args[1], &args[2]) {
        if pattern.is_empty() {
            return Err(JsonataError::new(
                "D3010",
                "$replace: pattern cannot be an empty string",
            ));
        }
        return Ok(Value::String(replace_n_literal(
            &s,
            pattern,
            replacement,
            limit,
        ).into()));
    }

    // Regex pattern.
    let re = compile_regex_arg(&args[1])?;

    match &args[2] {
        Value::String(replacement) => {
            replace_regex_string(&s, &re, replacement, limit).map(|s| Value::String(s.into()))
        }
        Value::Function(func) => {
            replace_with_fn(&s, &re, func, limit, env, arena).map(|s| Value::String(s.into()))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$replace: argument 3 must be a string or function",
        )),
    }
}

fn replace_n_literal(s: &str, old: &str, replacement: &str, limit: Option<usize>) -> String {
    match limit {
        None => s.replace(old, replacement),
        Some(0) => s.to_string(),
        Some(lim) => {
            let mut result = String::new();
            let mut remaining = s;
            let mut count = 0;
            while count < lim {
                if let Some(idx) = remaining.find(old) {
                    result.push_str(&remaining[..idx]);
                    result.push_str(replacement);
                    remaining = &remaining[idx + old.len()..];
                    count += 1;
                } else {
                    break;
                }
            }
            result.push_str(remaining);
            result
        }
    }
}

fn replace_regex_string(
    s: &str,
    re: &Regex,
    repl: &str,
    limit: Option<usize>,
) -> Result<String, JsonataError> {
    let mut result = String::new();
    let mut prev = 0;
    for (count, caps) in re.captures_iter(s).enumerate() {
        if let Some(lim) = limit
            && count >= lim
        {
            break;
        }
        let m = caps.get(0).ok_or_else(|| {
            JsonataError::new("D1004", "$replace: failed to get regex match")
        })?;
        if m.as_str().is_empty() {
            return Err(JsonataError::new(
                "D1004",
                "$replace: the regex matched a zero-length string",
            ));
        }
        result.push_str(&s[prev..m.start()]);

        // Expand replacement template with back-references.
        let groups: Vec<&str> = (1..caps.len())
            .map(|i| caps.get(i).map_or("", |g| g.as_str()))
            .collect();
        result.push_str(&expand_replacement(repl, m.as_str(), &groups));

        prev = m.end();
    }
    result.push_str(&s[prev..]);
    Ok(result)
}

fn replace_with_fn(
    s: &str,
    re: &Regex,
    func: &FunctionValue,
    limit: Option<usize>,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> Result<String, JsonataError> {
    let mut result = String::new();
    let mut prev = 0;

    for (count, caps) in re.captures_iter(s).enumerate() {
        if let Some(lim) = limit
            && count >= lim
        {
            break;
        }
        let m = caps.get(0).ok_or_else(|| {
            JsonataError::new("D1004", "$replace: failed to get regex match")
        })?;
        if m.as_str().is_empty() {
            return Err(JsonataError::new(
                "D1004",
                "$replace: the regex matched a zero-length string",
            ));
        }
        result.push_str(&s[prev..m.start()]);

        let match_obj = build_match_object(s, &caps, &m);
        let val = call_function(func, &[match_obj], &Value::Undefined, env, arena)?;
        match val {
            Value::String(sv) => result.push_str(&sv),
            _ => {
                return Err(JsonataError::new(
                    "D3012",
                    "$replace: replacement function must return a string",
                ));
            }
        }

        prev = m.end();
    }
    result.push_str(&s[prev..]);
    Ok(result)
}

/// Expand a JSONata replacement template with back-references ($0, $1, etc.)
fn expand_replacement(repl: &str, full_match: &str, groups: &[&str]) -> String {
    let mut result = String::new();
    let bytes = repl.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'$' {
            result.push(bytes[i] as char);
            i += 1;
            continue;
        }
        i += 1; // skip $
        if i >= bytes.len() {
            result.push('$');
            break;
        }
        if bytes[i] == b'$' {
            result.push('$');
            i += 1;
            continue;
        }
        if !bytes[i].is_ascii_digit() {
            result.push('$');
            result.push(bytes[i] as char);
            i += 1;
            continue;
        }
        // Collect digit run.
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let num_str = &repl[start..i];
        let n: usize = num_str.parse().unwrap_or(0);
        if n == 0 {
            result.push_str(full_match);
        } else if n <= groups.len() {
            result.push_str(groups[n - 1]);
        } else {
            // Try shorter prefixes.
            let mut found = false;
            for plen in (1..num_str.len()).rev() {
                let p: usize = num_str[..plen].parse().unwrap_or(0);
                if p == 0 {
                    result.push_str(full_match);
                    result.push_str(&num_str[plen..]);
                    found = true;
                    break;
                }
                if p <= groups.len() {
                    result.push_str(groups[p - 1]);
                    result.push_str(&num_str[plen..]);
                    found = true;
                    break;
                }
            }
            if !found && num_str.len() > 1 {
                result.push_str(&num_str[1..]);
            }
        }
    }
    result
}
