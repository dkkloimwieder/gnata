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
    // TODO: second arg (boolean) for pretty-printing
    arg.stringify().map(Value::String)
}

pub fn fn_length(args: &[Value], focus: &Value) -> JsonataResult {
    let arg = if args.is_empty() { focus } else { &args[0] };
    if arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    match arg {
        Value::String(s) => Ok(Value::Number(s.chars().count() as f64)),
        _ => Err(JsonataError::new(
            "T0410",
            "$length: argument must be a string",
        )),
    }
}

pub fn fn_substring(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new(
            "T0410",
            "$substring: requires at least 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match &args[0] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$substring: first argument must be a string",
            ));
        }
    };
    let start = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;

    let actual_start = if start < 0 {
        (len + start).max(0) as usize
    } else {
        start.min(len) as usize
    };

    let result: String = if let Some(length_val) = args.get(2) {
        let length = length_val.as_f64().unwrap_or(0.0) as usize;
        chars[actual_start..].iter().take(length).collect()
    } else {
        chars[actual_start..].iter().collect()
    };
    Ok(Value::String(result))
}

pub fn fn_substring_before(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new(
            "T0410",
            "$substringBefore: requires 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match &args[0] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$substringBefore: first argument must be a string",
            ));
        }
    };
    let sep = match &args[1] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$substringBefore: second argument must be a string",
            ));
        }
    };
    match s.find(sep) {
        Some(idx) => Ok(Value::String(s[..idx].to_string())),
        None => Ok(Value::String(s.to_string())),
    }
}

pub fn fn_substring_after(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new(
            "T0410",
            "$substringAfter: requires 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match &args[0] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$substringAfter: first argument must be a string",
            ));
        }
    };
    let sep = match &args[1] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$substringAfter: second argument must be a string",
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
    let pad_char = if args.len() >= 3 {
        match &args[2] {
            Value::String(c) => c.chars().next().unwrap_or(' '),
            _ => ' ',
        }
    } else {
        ' '
    };

    let char_count = s.chars().count() as i64;
    let needed = width.unsigned_abs() as usize;
    if char_count >= needed as i64 {
        return Ok(Value::String(s));
    }
    let pad_count = needed - char_count as usize;
    let padding: String = std::iter::repeat_n(pad_char, pad_count).collect();

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
            // Regex object.
            if let Some(Value::String(pat)) = obj.get("pattern") {
                let re = regex::Regex::new(pat).map_err(|e| {
                    JsonataError::new("D3010", format!("$contains: invalid regex: {e}"))
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
    if args.len() < 2 {
        return Err(JsonataError::new(
            "T0410",
            "$split: requires at least 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let s = match &args[0] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$split: first argument must be a string",
            ));
        }
    };
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
                let re = regex::Regex::new(pat).map_err(|e| {
                    JsonataError::new("D3010", format!("$split: invalid regex: {e}"))
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
                "T0410",
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
                .replace("%5D", "]");
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
                .map_err(|e| JsonataError::new("D3010", format!("$decodeUrl: {e}")))?
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
                .map_err(|e| JsonataError::new("D3010", format!("$decodeUrlComponent: {e}")))?
                .to_string();
            Ok(Value::String(decoded))
        }
        _ => Err(JsonataError::new(
            "T0410",
            "$decodeUrlComponent: argument must be a string",
        )),
    }
}
