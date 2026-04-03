//! Numeric functions: $number, $abs, $floor, $ceil, $round, $power, $sqrt, $random,
//! $sum, $max, $min, $average, $formatBase.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

pub fn fn_number(args: &[Value], focus: &Value) -> JsonataResult {
    let arg = match args.len() {
        0 => focus,
        1 => &args[0],
        _ => return Err(JsonataError::new("T0410", "$number: too many arguments")),
    };
    if arg.is_undefined() {
        return Ok(Value::Undefined);
    }
    match arg {
        Value::Null => Err(JsonataError::new(
            "T0410",
            "$number: cannot cast null to number",
        )),
        Value::Number(n) => Ok(Value::Number(*n)),
        Value::Bool(b) => Ok(Value::Number(if *b { 1.0 } else { 0.0 })),
        Value::String(s) => {
            let s = s.trim();
            // Support hex/binary/octal prefixes.
            if s.len() >= 2 && s.starts_with('0') {
                let prefix = s.as_bytes()[1];
                let (radix, offset) = match prefix {
                    b'x' | b'X' => (16, 2),
                    b'b' | b'B' => (2, 2),
                    b'o' | b'O' => (8, 2),
                    _ => (0, 0),
                };
                if radix > 0 {
                    return match i64::from_str_radix(&s[offset..], radix) {
                        Ok(n) => Ok(Value::Number(n as f64)),
                        Err(_) => Err(JsonataError::new(
                            "D3030",
                            format!("$number: unable to cast \"{s}\" to a number"),
                        )),
                    };
                }
            }
            match s.parse::<f64>() {
                Ok(f) if f.is_finite() => Ok(Value::Number(f)),
                _ => Err(JsonataError::new(
                    "D3030",
                    format!("$number: unable to cast \"{s}\" to a number"),
                )),
            }
        }
        Value::Array(_) => Err(JsonataError::new(
            "T0410",
            "$number: cannot cast array to number",
        )),
        Value::Object(_) => Err(JsonataError::new(
            "T0410",
            "$number: cannot cast object to number",
        )),
        _ => Err(JsonataError::new("T0410", "$number: unsupported type")),
    }
}

fn require_number(args: &[Value], name: &str) -> Result<Option<f64>, JsonataError> {
    if args.is_empty() {
        return Err(JsonataError::new(
            "T0410",
            format!("{name}: argument is required"),
        ));
    }
    if args[0].is_undefined() {
        return Ok(None);
    }
    match args[0].as_f64() {
        Some(n) => Ok(Some(n)),
        None => Err(JsonataError::new(
            "T0410",
            format!("{name}: argument must be a number"),
        )),
    }
}

fn to_number_array(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Number(n) => Some(vec![*n]),
        Value::Array(arr) => {
            let mut nums = Vec::with_capacity(arr.len());
            for item in arr {
                match item.as_f64() {
                    Some(n) => nums.push(n),
                    None => return None,
                }
            }
            Some(nums)
        }
        _ => None,
    }
}

pub fn fn_abs(args: &[Value], _focus: &Value) -> JsonataResult {
    match require_number(args, "$abs")? {
        Some(n) => Ok(Value::Number(n.abs())),
        None => Ok(Value::Undefined),
    }
}

pub fn fn_floor(args: &[Value], _focus: &Value) -> JsonataResult {
    match require_number(args, "$floor")? {
        Some(n) => Ok(Value::Number(n.floor())),
        None => Ok(Value::Undefined),
    }
}

pub fn fn_ceil(args: &[Value], _focus: &Value) -> JsonataResult {
    match require_number(args, "$ceil")? {
        Some(n) => Ok(Value::Number(n.ceil())),
        None => Ok(Value::Undefined),
    }
}

pub fn fn_round(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$round: argument is required"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let n = args[0]
        .as_f64()
        .ok_or_else(|| JsonataError::new("T0410", "$round: argument must be a number"))?;
    let scale = if args.len() >= 2 && !args[1].is_undefined() {
        args[1]
            .as_f64()
            .ok_or_else(|| JsonataError::new("T0410", "$round: scale must be a number"))?
            as i32
    } else {
        0
    };
    Ok(Value::Number(bankers_round(n, scale)))
}

fn bankers_round(n: f64, scale: i32) -> f64 {
    if !n.is_finite() {
        return n;
    }
    let mult = 10f64.powi(scale);
    let shifted = n * mult;
    let truncated = shifted.trunc();
    let remainder = (shifted - truncated).abs();

    let rounded = if (remainder - 0.5).abs() < 1e-10 {
        // Exactly 0.5: round to even.
        if truncated as i64 % 2 == 0 {
            truncated
        } else {
            truncated + shifted.signum()
        }
    } else if remainder > 0.5 {
        truncated + shifted.signum()
    } else {
        truncated
    };
    rounded / mult
}

pub fn fn_power(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new("T0410", "$power: requires 2 arguments"));
    }
    if args[0].is_undefined() || args[1].is_undefined() {
        return Ok(Value::Undefined);
    }
    let base = args[0]
        .as_f64()
        .ok_or_else(|| JsonataError::new("T0410", "$power: arguments must be numbers"))?;
    let exp = args[1]
        .as_f64()
        .ok_or_else(|| JsonataError::new("T0410", "$power: arguments must be numbers"))?;
    let result = base.powf(exp);
    if !result.is_finite() {
        return Err(JsonataError::new("D3061", "$power: result is non-finite"));
    }
    Ok(Value::Number(result))
}

pub fn fn_sqrt(args: &[Value], _focus: &Value) -> JsonataResult {
    match require_number(args, "$sqrt")? {
        Some(n) if n < 0.0 => Err(JsonataError::new(
            "D3060",
            "$sqrt: square root of a negative number",
        )),
        Some(n) => Ok(Value::Number(n.sqrt())),
        None => Ok(Value::Undefined),
    }
}

pub fn fn_random(_args: &[Value], _focus: &Value) -> JsonataResult {
    Ok(Value::Number(fastrand::f64()))
}

pub fn fn_sum(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$sum: argument 1 is required"));
    }
    if args.len() > 1 {
        return Err(JsonataError::new("T0410", "$sum: expects 1 argument"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let nums = to_number_array(&args[0])
        .ok_or_else(|| JsonataError::new("T0412", "$sum: argument must be an array of numbers"))?;
    Ok(Value::Number(nums.iter().sum()))
}

pub fn fn_max(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$max: argument 1 is required"));
    }
    if args.len() > 1 {
        return Err(JsonataError::new("T0410", "$max: expects 1 argument"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let nums = to_number_array(&args[0])
        .ok_or_else(|| JsonataError::new("T0412", "$max: argument must be an array of numbers"))?;
    if nums.is_empty() {
        return Ok(Value::Undefined);
    }
    Ok(Value::Number(
        nums.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ))
}

pub fn fn_min(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$min: argument 1 is required"));
    }
    if args.len() > 1 {
        return Err(JsonataError::new("T0410", "$min: expects 1 argument"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let nums = to_number_array(&args[0])
        .ok_or_else(|| JsonataError::new("T0412", "$min: argument must be an array of numbers"))?;
    if nums.is_empty() {
        return Ok(Value::Undefined);
    }
    Ok(Value::Number(
        nums.iter().copied().fold(f64::INFINITY, f64::min),
    ))
}

pub fn fn_average(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new(
            "T0410",
            "$average: argument 1 is required",
        ));
    }
    if args.len() > 1 {
        return Err(JsonataError::new("T0410", "$average: expects 1 argument"));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let nums = to_number_array(&args[0]).ok_or_else(|| {
        JsonataError::new("T0412", "$average: argument must be an array of numbers")
    })?;
    if nums.is_empty() {
        return Ok(Value::Undefined);
    }
    let sum: f64 = nums.iter().sum();
    Ok(Value::Number(sum / nums.len() as f64))
}

pub fn fn_format_base(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new(
            "T0410",
            "$formatBase: requires 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let n = args[0].as_f64().ok_or_else(|| {
        JsonataError::new("T0410", "$formatBase: first argument must be a number")
    })?;
    let radix = args[1].as_f64().ok_or_else(|| {
        JsonataError::new("T0410", "$formatBase: second argument must be a number")
    })? as u32;
    if !(2..=36).contains(&radix) {
        return Err(JsonataError::new(
            "D3010",
            "$formatBase: radix must be between 2 and 36",
        ));
    }
    let int_val = n.trunc() as i64;
    let formatted = format_radix(int_val.unsigned_abs(), radix);
    if int_val < 0 {
        Ok(Value::String(format!("-{formatted}")))
    } else {
        Ok(Value::String(formatted))
    }
}

fn format_radix(mut n: u64, radix: u32) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while n > 0 {
        let d = (n % radix as u64) as u32;
        digits.push(char::from_digit(d, radix).unwrap_or('?'));
        n /= radix as u64;
    }
    digits.reverse();
    digits.into_iter().collect()
}
