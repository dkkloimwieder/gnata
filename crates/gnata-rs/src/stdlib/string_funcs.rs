//! String functions: $string, $length, $substring, $substringBefore, $substringAfter,
//! $uppercase, $lowercase, $trim, $pad, $contains, $split, $join,
//! $base64encode, $base64decode.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;
use base64::Engine;

pub fn fn_string(args: &[Value], focus: &Value) -> JsonataResult {
    let arg = if args.is_empty() { focus } else { &args[0] };
    if arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    // Check for Inf/NaN numbers → D3001.
    if let Value::Number(n) = arg {
        if n.is_infinite() || n.is_nan() {
            return Err(JsonataError::new("D3001", "Number out of range"));
        }
    }
    // Arity enforced by SignedBuiltin signature at call site.
    let prettify = if args.len() >= 2 {
        match &args[1] {
            Value::Bool(b) => *b,
            Value::Function(_) => {
                return Err(JsonataError::new(
                    "D3011",
                    "$string: second argument cannot be a function",
                ));
            }
            Value::Undefined => false,
            _ => {
                return Err(JsonataError::new(
                    "T0410",
                    "$string: second argument must be a boolean",
                ));
            }
        }
    } else {
        false
    };
    arg.stringify(prettify).map(Value::String)
}

pub fn fn_length(args: &[Value], focus: &Value) -> JsonataResult {
    if args.len() > 1 {
        return Err(JsonataError::new("T0410", "$length: expects 1 argument"));
    }
    let from_focus = args.is_empty();
    let arg = if from_focus { focus } else { &args[0] };
    if arg.is_undefined() {
        if from_focus {
            return Err(JsonataError::new("T0411", "$length: argument is required"));
        }
        return Ok(Value::Undefined);
    }
    match arg {
        Value::String(s) => Ok(Value::Number(s.chars().count() as f64)),
        _ => {
            let code = if from_focus { "T0411" } else { "T0410" };
            Err(JsonataError::new(
                code,
                "$length: argument must be a string",
            ))
        }
    }
}

pub fn fn_substring(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new(
            "T0410",
            "$substring: requires at least 2 arguments",
        ));
    }
    if args.len() > 3 {
        return Err(JsonataError::new("T0410", "$substring: too many arguments"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match &args[0] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$substring: argument 1 must be a string",
            ));
        }
    };
    // arg2 (start) must be a number if provided.
    let start = if let Some(arg2) = args.get(1) {
        match arg2.as_f64() {
            Some(n) => n as i64,
            None => {
                return Err(JsonataError::new(
                    "T0410",
                    "$substring: argument 2 must be a number",
                ));
            }
        }
    } else {
        0
    };

    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;

    let actual_start = if start < 0 {
        (len + start).max(0) as usize
    } else {
        start.min(len) as usize
    };

    // arg3 (length) must be a number if provided.
    let result: String = if let Some(length_val) = args.get(2) {
        match length_val.as_f64() {
            Some(n) => {
                let length = n as usize;
                chars[actual_start..].iter().take(length).collect()
            }
            None => {
                return Err(JsonataError::new(
                    "T0410",
                    "$substring: argument 3 must be a number",
                ));
            }
        }
    } else {
        chars[actual_start..].iter().collect()
    };
    Ok(Value::String(result))
}

pub fn fn_substring_before(args: &[Value], focus: &Value) -> JsonataResult {
    // Go semantics: len(args)==0 → T0411; len(args)==1 → use focus as str, arg as sep;
    // len(args)==2 → str=args[0], sep=args[1]; len(args)>2 → T0410.
    if args.len() > 2 {
        return Err(JsonataError::new(
            "T0410",
            "$substringBefore: too many arguments",
        ));
    }
    let (str_arg, sep_arg, from_context) = if args.len() == 2 {
        (&args[0], &args[1], false)
    } else if args.len() == 1 {
        (focus, &args[0], true)
    } else {
        return Err(JsonataError::new(
            "T0411",
            "$substringBefore: requires 2 arguments",
        ));
    };
    if str_arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match str_arg {
        Value::String(s) => s.as_str(),
        _ => {
            // When using focus as context and it's not a string → T0411
            let code = if from_context { "T0411" } else { "T0410" };
            return Err(JsonataError::new(
                code,
                "$substringBefore: argument 1 must be a string",
            ));
        }
    };
    let sep = match sep_arg {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$substringBefore: argument 2 must be a string",
            ));
        }
    };
    match s.find(sep) {
        Some(idx) => Ok(Value::String(s[..idx].to_string())),
        None => Ok(Value::String(s.to_string())),
    }
}

pub fn fn_substring_after(args: &[Value], focus: &Value) -> JsonataResult {
    // Go semantics: len(args)==0 → T0411; len(args)==1 → use focus as str, arg as sep;
    // len(args)==2 → str=args[0], sep=args[1]; len(args)>2 → T0410.
    if args.len() > 2 {
        return Err(JsonataError::new(
            "T0410",
            "$substringAfter: too many arguments",
        ));
    }
    let (str_arg, sep_arg, from_context) = if args.len() == 2 {
        (&args[0], &args[1], false)
    } else if args.len() == 1 {
        (focus, &args[0], true)
    } else {
        return Err(JsonataError::new(
            "T0411",
            "$substringAfter: requires 2 arguments",
        ));
    };
    if str_arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match str_arg {
        Value::String(s) => s.as_str(),
        _ => {
            let code = if from_context { "T0411" } else { "T0410" };
            return Err(JsonataError::new(
                code,
                "$substringAfter: argument 1 must be a string",
            ));
        }
    };
    let sep = match sep_arg {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$substringAfter: argument 2 must be a string",
            ));
        }
    };
    match s.find(sep) {
        Some(idx) => Ok(Value::String(s[idx + sep.len()..].to_string())),
        None => Ok(Value::String(s.to_string())),
    }
}

pub fn fn_uppercase(args: &[Value], focus: &Value) -> JsonataResult {
    let arg = if args.is_empty() { focus } else { &args[0] };
    if arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    match arg {
        Value::String(s) => Ok(Value::String(s.to_uppercase())),
        _ => Err(JsonataError::new(
            "T0410",
            "$uppercase: argument must be a string",
        )),
    }
}

pub fn fn_lowercase(args: &[Value], focus: &Value) -> JsonataResult {
    let arg = if args.is_empty() { focus } else { &args[0] };
    if arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    match arg {
        Value::String(s) => Ok(Value::String(s.to_lowercase())),
        _ => Err(JsonataError::new(
            "T0410",
            "$lowercase: argument must be a string",
        )),
    }
}

pub fn fn_trim(args: &[Value], focus: &Value) -> JsonataResult {
    let arg = if args.is_empty() { focus } else { &args[0] };
    if arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    match arg {
        Value::String(s) => {
            // JSONata $trim: strip leading/trailing whitespace AND collapse internal whitespace.
            let trimmed = s.trim();
            let mut result = String::with_capacity(trimmed.len());
            let mut prev_space = false;
            for c in trimmed.chars() {
                if c.is_whitespace() {
                    if !prev_space {
                        result.push(' ');
                    }
                    prev_space = true;
                } else {
                    result.push(c);
                    prev_space = false;
                }
            }
            Ok(Value::String(result))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$trim: argument must be a string",
        )),
    }
}

pub fn fn_pad(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new(
            "T0410",
            "$pad: requires at least 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match &args[0] {
        Value::String(s) => s.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$pad: first argument must be a string",
            ));
        }
    };
    let width = args[1]
        .as_f64()
        .ok_or_else(|| JsonataError::new("T0410", "$pad: width must be a number"))?
        as i64;
    let pad_str = if args.len() >= 3 {
        match &args[2] {
            Value::String(c) if !c.is_empty() => c.clone(),
            _ => " ".to_string(),
        }
    } else {
        " ".to_string()
    };

    let char_count = s.chars().count() as i64;
    let needed = width.unsigned_abs() as usize;
    if char_count >= needed as i64 {
        return Ok(Value::String(s));
    }
    let pad_count = needed - char_count as usize;
    let padding: String = pad_str.chars().cycle().take(pad_count).collect();

    if width > 0 {
        Ok(Value::String(format!("{s}{padding}")))
    } else {
        Ok(Value::String(format!("{padding}{s}")))
    }
}

pub fn fn_contains(args: &[Value], focus: &Value) -> JsonataResult {
    // When called with 1 arg in path context, use focus as the string.
    let (str_arg, pattern_arg) = if args.len() >= 2 {
        (&args[0], &args[1])
    } else if args.len() == 1 {
        (focus, &args[0])
    } else {
        return Err(JsonataError::new(
            "T0410",
            "$contains: requires 2 arguments",
        ));
    };
    if str_arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match str_arg {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$contains: first argument must be a string",
            ));
        }
    };
    match pattern_arg {
        Value::String(sub) => Ok(Value::Bool(s.contains(sub.as_str()))),
        Value::Object(obj) if obj.contains_key("pattern") => {
            // Regex object — use compile_regex to properly handle flags.
            if let Some(Value::String(pat)) = obj.get("pattern") {
                let flags = match obj.get("flags") {
                    Some(Value::String(f)) => f.as_str(),
                    _ => "",
                };
                let re = crate::stdlib::regex::compile_regex(pat, flags).map_err(|e| {
                    JsonataError::new("D3010", format!("$contains: invalid regex: {}", e.message))
                })?;
                Ok(Value::Bool(re.is_match(s)))
            } else {
                Ok(Value::Bool(false))
            }
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$contains: second argument must be a string or regex",
        )),
    }
}

pub fn fn_split(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new(
            "T0410",
            "$split: requires at least 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    // Non-string first arg → undefined
    let s = match &args[0] {
        Value::String(s) => s.as_str(),
        _ => return Ok(Value::Undefined),
    };
    if args.len() < 2 {
        return Err(JsonataError::new(
            "T0410",
            "$split: requires at least 2 arguments",
        ));
    }
    // Check limit arg before using it
    if let Some(limit_arg) = args.get(2) {
        if !limit_arg.is_undefined() {
            match limit_arg.as_f64() {
                Some(n) if n < 0.0 => {
                    return Err(JsonataError::new(
                        "D3020",
                        "$split: third argument must not be negative",
                    ));
                }
                Some(_) => {} // valid number
                None => {
                    return Err(JsonataError::new(
                        "T0410",
                        "$split: third argument must be a number",
                    ));
                }
            }
        }
    }
    let limit = args.get(2).and_then(|v| v.as_f64()).map(|n| n as usize);

    let parts: Vec<Value> = match &args[1] {
        Value::String(sep) => {
            let splits: Vec<&str> = if sep.is_empty() {
                // Empty separator: split into individual characters.
                s.char_indices()
                    .map(|(i, c)| &s[i..i + c.len_utf8()])
                    .collect()
            } else {
                s.split(sep.as_str()).collect()
            };
            let mut result: Vec<Value> = splits
                .into_iter()
                .map(|p| Value::String(p.into()))
                .collect();
            // Apply limit: return at most N items.
            if let Some(lim) = limit {
                result.truncate(lim);
            }
            result
        }
        Value::Object(obj) if obj.contains_key("pattern") => {
            if let Some(Value::String(pat)) = obj.get("pattern") {
                let flags = match obj.get("flags") {
                    Some(Value::String(f)) => f.as_str(),
                    _ => "",
                };
                let re = crate::stdlib::regex::compile_regex(pat, flags).map_err(|e| {
                    JsonataError::new("D3010", format!("$split: invalid regex: {}", e.message))
                })?;
                let splits: Vec<&str> = re.split(s).collect();
                let mut result: Vec<Value> = splits
                    .into_iter()
                    .map(|p| Value::String(p.into()))
                    .collect();
                if let Some(lim) = limit {
                    result.truncate(lim);
                }
                result
            } else {
                vec![Value::String(s.into())]
            }
        }
        Value::Function(_) => {
            return Err(JsonataError::new(
                "T1010",
                "$split: second argument must be a string or regex",
            ));
        }
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$split: separator must be a string or regex",
            ));
        }
    };
    Ok(Value::Array(parts))
}

pub fn fn_join(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$join: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let arr = match &args[0] {
        Value::Array(a) => a,
        Value::String(s) => return Ok(Value::String(s.clone())),
        _ => {
            return Err(JsonataError::new(
                "T0412",
                "$join: argument must be an array of strings",
            ));
        }
    };
    let sep = match args.get(1) {
        Some(Value::String(s)) => s.as_str(),
        None | Some(Value::Undefined) => "",
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$join: separator must be a string",
            ));
        }
    };
    let strings: Result<Vec<&str>, _> = arr
        .iter()
        .map(|v| match v {
            Value::String(s) => Ok(s.as_str()),
            _ => Err(JsonataError::new(
                "T0412",
                "$join: array must contain only strings",
            )),
        })
        .collect();
    Ok(Value::String(strings?.join(sep)))
}

pub fn fn_base64_encode(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    match &args[0] {
        Value::String(s) => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
            Ok(Value::String(encoded))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$base64encode: argument must be a string",
        )),
    }
}

pub fn fn_base64_decode(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    match &args[0] {
        Value::String(s) => {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .map_err(|e| JsonataError::new("D3010", format!("$base64decode: {e}")))?;
            let result = String::from_utf8(decoded)
                .map_err(|e| JsonataError::new("D3010", format!("$base64decode: {e}")))?;
            Ok(Value::String(result))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$base64decode: argument must be a string",
        )),
    }
}

pub fn fn_encode_url(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    match &args[0] {
        Value::String(s) => {
            // encodeUrl preserves URI-safe characters.
            let encoded =
                percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC)
                    .to_string();
            // Restore URI-safe chars that shouldn't be encoded.
            let encoded = encoded
                .replace("%2F", "/")
                .replace("%3A", ":")
                .replace("%40", "@")
                .replace("%21", "!")
                .replace("%24", "$")
                .replace("%26", "&")
                .replace("%27", "'")
                .replace("%28", "(")
                .replace("%29", ")")
                .replace("%2A", "*")
                .replace("%2B", "+")
                .replace("%2C", ",")
                .replace("%3B", ";")
                .replace("%3D", "=")
                .replace("%3F", "?")
                .replace("%23", "#")
                .replace("%5B", "[")
                .replace("%5D", "]")
                .replace("%2D", "-")
                .replace("%2E", ".")
                .replace("%5F", "_")
                .replace("%7E", "~");
            Ok(Value::String(encoded))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$encodeUrl: argument must be a string",
        )),
    }
}

pub fn fn_encode_url_component(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    match &args[0] {
        Value::String(s) => {
            let encoded =
                percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC)
                    .to_string()
                    .replace("%21", "!")
                    .replace("%27", "'")
                    .replace("%28", "(")
                    .replace("%29", ")")
                    .replace("%2A", "*")
                    .replace("%2D", "-")
                    .replace("%2E", ".")
                    .replace("%5F", "_")
                    .replace("%7E", "~");
            Ok(Value::String(encoded))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$encodeUrlComponent: argument must be a string",
        )),
    }
}

pub fn fn_decode_url(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    match &args[0] {
        Value::String(s) => {
            let decoded = percent_encoding::percent_decode_str(s)
                .decode_utf8()
                .map_err(|e| JsonataError::new("D3140", format!("$decodeUrl: {e}")))?
                .to_string();
            Ok(Value::String(decoded))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$decodeUrl: argument must be a string",
        )),
    }
}

pub fn fn_decode_url_component(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    match &args[0] {
        Value::String(s) => {
            let decoded = percent_encoding::percent_decode_str(s)
                .decode_utf8()
                .map_err(|e| JsonataError::new("D3140", format!("$decodeUrlComponent: {e}")))?
                .to_string();
            Ok(Value::String(decoded))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$decodeUrlComponent: argument must be a string",
        )),
    }
}
