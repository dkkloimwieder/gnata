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
            for item in arr.iter() {
                match item.as_f64() {
                    Some(n) => nums.push(n),
                    None => return None,
                }
            }
            Some(nums)
        }
        Value::Sequence(seq) => to_number_array(&seq.collapse()),
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

pub(crate) fn bankers_round(n: f64, scale: i32) -> f64 {
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

#[expect(clippy::unnecessary_wraps, reason = "must match the BuiltinFn signature")]
pub fn fn_random(_args: &[Value], _focus: &Value) -> JsonataResult {
    Ok(Value::Number(fastrand::f64()))
}

pub fn fn_sum(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new("T0410", "$sum: argument 1 is required"));
    }
    // Arity enforced by SignedBuiltin signature at call site.
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
    if args.is_empty() {
        return Err(JsonataError::new(
            "T0410",
            "$formatBase: requires at least 1 argument",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let n = args[0].as_f64().ok_or_else(|| {
        JsonataError::new("T0410", "$formatBase: first argument must be a number")
    })?;
    let radix = if args.len() >= 2 && !args[1].is_undefined() {
        args[1].as_f64().ok_or_else(|| {
            JsonataError::new("T0410", "$formatBase: second argument must be a number")
        })? as u32
    } else {
        10 // default base 10
    };
    if !(2..=36).contains(&radix) {
        return Err(JsonataError::new(
            "D3100",
            "$formatBase: radix must be between 2 and 36",
        ));
    }
    let int_val = n.round() as i64;
    let formatted = format_radix(int_val.unsigned_abs(), radix);
    if int_val < 0 {
        Ok(Value::String(format!("-{formatted}").into()))
    } else {
        Ok(Value::String(formatted.into()))
    }
}

fn format_radix(mut n: u64, radix: u32) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while n > 0 {
        let d = (n % u64::from(radix)) as u32;
        digits.push(char::from_digit(d, radix).unwrap_or('?'));
        n /= u64::from(radix);
    }
    digits.reverse();
    digits.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    const U: &Value = &Value::Undefined;

    fn n(x: f64) -> Value {
        Value::Number(x)
    }
    fn num(r: JsonataResult) -> f64 {
        match r {
            Ok(Value::Number(x)) => x,
            other => panic!("expected number, got {other:?}"),
        }
    }
    fn text(r: JsonataResult) -> String {
        match r {
            Ok(Value::String(s)) => s.to_string(),
            other => panic!("expected string, got {other:?}"),
        }
    }
    fn code(r: JsonataResult) -> &'static str {
        match r {
            Err(e) => e.code,
            other => panic!("expected error, got {other:?}"),
        }
    }

    /// $round is half-to-even (banker's), not half-away-from-zero
    /// (spec.md: JS Number semantics for $round).
    #[test]
    fn round_is_half_to_even() {
        assert_eq!(bankers_round(0.5, 0), 0.0);
        assert_eq!(bankers_round(1.5, 0), 2.0);
        assert_eq!(bankers_round(2.5, 0), 2.0);
        assert_eq!(bankers_round(-1.5, 0), -2.0);
        // Positive scale shifts the rule to that decimal place.
        assert_eq!(bankers_round(1.25, 1), 1.2);
        assert_eq!(bankers_round(1.75, 1), 1.8);
        // Negative scale rounds to tens.
        assert_eq!(bankers_round(125.0, -1), 120.0);
    }

    #[test]
    fn round_builtin_applies_scale_argument() {
        assert_eq!(num(fn_round(&[n(1.25), n(1.0)], U)), 1.2);
        assert_eq!(num(fn_round(&[n(2.5)], U)), 2.0);
    }

    #[test]
    fn power_rejects_non_finite_results() {
        assert_eq!(num(fn_power(&[n(2.0), n(10.0)], U)), 1024.0);
        assert_eq!(code(fn_power(&[n(0.0), n(-1.0)], U)), "D3061");
        assert_eq!(code(fn_power(&[n(-2.0), n(0.5)], U)), "D3061");
    }

    #[test]
    fn sqrt_of_negative_is_an_error() {
        assert_eq!(num(fn_sqrt(&[n(144.0)], U)), 12.0);
        assert_eq!(code(fn_sqrt(&[n(-1.0)], U)), "D3060");
    }

    #[test]
    fn format_base_covers_radix_range() {
        assert_eq!(text(fn_format_base(&[n(100.0), n(2.0)], U)), "1100100");
        assert_eq!(text(fn_format_base(&[n(255.0), n(16.0)], U)), "ff");
        assert_eq!(text(fn_format_base(&[n(-100.0), n(2.0)], U)), "-1100100");
        // Radix defaults to 10.
        assert_eq!(text(fn_format_base(&[n(12.0)], U)), "12");
        assert_eq!(code(fn_format_base(&[n(12.0), n(1.0)], U)), "D3100");
        assert_eq!(code(fn_format_base(&[n(12.0), n(37.0)], U)), "D3100");
    }

    #[test]
    fn aggregates_handle_boundaries() {
        let nums = Value::Array(Rc::from(vec![n(1.0), n(2.0), n(3.0), n(4.0)]));
        let nums = std::slice::from_ref(&nums);
        assert_eq!(num(fn_sum(nums, U)), 10.0);
        assert_eq!(num(fn_average(nums, U)), 2.5);
        assert_eq!(num(fn_max(nums, U)), 4.0);
        assert_eq!(num(fn_min(nums, U)), 1.0);
        // Empty arrays: $sum → 0, $max/$min/$average → undefined.
        let empty = Value::Array(Rc::from(Vec::<Value>::new()));
        let empty = std::slice::from_ref(&empty);
        assert_eq!(num(fn_sum(empty, U)), 0.0);
        assert!(matches!(fn_max(empty, U), Ok(Value::Undefined)));
        assert!(matches!(fn_min(empty, U), Ok(Value::Undefined)));
        assert!(matches!(fn_average(empty, U), Ok(Value::Undefined)));
        // Non-numeric element → T0412.
        let mixed = Value::Array(Rc::from(vec![n(1.0), Value::String("x".into())]));
        assert_eq!(code(fn_sum(&[mixed], U)), "T0412");
    }
}
