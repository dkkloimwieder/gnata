//! Number formatting matching JavaScript's `Number.toString()` behavior.
//!
//! Go equivalent: `FormatFloat` in `eval_helpers.go`, `FormatNumber` in `eval_helpers.go`.
//! Uses `ryu-js` crate for exact ECMAScript formatting.

/// Format an f64 to match Go's `FormatFloat` in `eval_helpers.go`.
///
/// Algorithm (mirrors Go exactly):
/// 1. `s = FormatFloat(n, 'g', 15, 64)` — 15 significant digits
/// 2. If `abs ∉ [5e-7, 1e21)` — scientific with shortest repr, cleaned exponent
/// 3. Else if `s` contains `e`/`E` — `FormatFloat(n, 'f', -1, 64)` full decimal
/// 4. Else — return `s`
///
/// - NaN/Inf → "null"
pub fn format_float(n: f64) -> String {
    if n.is_nan() || n.is_infinite() {
        return "null".into();
    }

    let abs = n.abs();

    // Step 1: 15 significant digits (Go's 'g', 15).
    // Rust's format!("{:.14e}") gives 15 sig digits in scientific form.
    let s = format_g15(n);

    // Step 2: very small or very large → scientific with shortest repr
    if abs != 0.0 && (abs < 5e-7 || abs >= 1e21) {
        // Use ryu-js for shortest representation (like Go's 'e', -1)
        let mut buf = ryu_js::Buffer::new();
        let ryu = buf.format(n).to_owned();
        if ryu.contains('e') || ryu.contains('E') {
            return clean_exponent(&ryu);
        }
        // Fallback: Rust scientific notation
        let sci = format!("{:e}", n);
        return clean_exponent(&sci);
    }

    // Step 3: if 'g',15 produced scientific, use full decimal (Go's 'f', -1)
    if s.contains('e') || s.contains('E') {
        // ryu-js may produce scientific notation. We need full decimal.
        // Use ryu-js first; if it's decimal, return it. Otherwise convert.
        let mut buf = ryu_js::Buffer::new();
        let ryu = buf.format(n).to_owned();
        if !ryu.contains('e') && !ryu.contains('E') {
            return ryu;
        }
        // ryu-js gave scientific; manually convert to decimal.
        return scientific_to_decimal(n);
    }

    // Step 4: return the 15-sig-digit result
    s
}

/// Format a number with 15 significant digits, equivalent to Go's
/// `strconv.FormatFloat(n, 'g', 15, 64)`.
///
/// Uses Rust's scientific formatting with 14 decimal places (= 15 sig digits),
/// then converts to the most compact non-scientific representation.
fn format_g15(n: f64) -> String {
    if n == 0.0 {
        return if n.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }

    // Format in scientific notation with 14 decimal places = 15 significant digits
    let sci = format!("{:.14e}", n);
    let (mantissa_str, exp_str) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp_str.parse().unwrap_or(0);

    let negative = mantissa_str.starts_with('-');
    let mant = if negative {
        &mantissa_str[1..]
    } else {
        mantissa_str
    };

    // Remove trailing zeros from mantissa
    let mant_trimmed = mant.trim_end_matches('0').trim_end_matches('.');

    // Extract digits (without decimal point)
    let digits: String = mant_trimmed.replace('.', "");
    let num_digits = digits.len() as i32;

    // Decide format: 'g' uses scientific if exp < -1 or exp >= precision
    // For precision 15: scientific if exp < -1 or exp >= 15
    let use_scientific = exp < -1 || exp >= 15;

    let result = if use_scientific {
        // Scientific notation: d.dddde±dd
        if num_digits <= 1 {
            format!("{}e{:+03}", &digits, exp)
        } else {
            format!("{}.{}e{:+03}", &digits[..1], &digits[1..], exp)
        }
    } else if exp + 1 <= 0 {
        // Needs leading zeros: 0.000...digits
        let zeros = (-(exp + 1)) as usize + 1;
        let mut r = String::from("0.");
        for _ in 1..zeros {
            r.push('0');
        }
        r.push_str(&digits);
        r
    } else {
        let decimal_pos = (exp + 1) as usize;
        if decimal_pos >= num_digits as usize {
            // All digits before decimal, pad with zeros
            let mut r = digits.clone();
            for _ in 0..(decimal_pos - num_digits as usize) {
                r.push('0');
            }
            r
        } else {
            // Decimal point within digits
            format!("{}.{}", &digits[..decimal_pos], &digits[decimal_pos..])
        }
    };

    if negative {
        format!("-{result}")
    } else {
        result
    }
}

/// Convert a number in the range [5e-7, 1e21) to full decimal representation.
/// Equivalent to Go's `strconv.FormatFloat(n, 'f', -1, 64)`.
fn scientific_to_decimal(n: f64) -> String {
    // Use format! with enough precision to get exact representation
    // Then trim trailing zeros after decimal point
    let s = format!("{:.20}", n);
    let s = s.trim_end_matches('0');
    let s = s.trim_end_matches('.');
    s.to_owned()
}

/// Clean up a scientific notation string: remove leading zeros from exponent,
/// ensure sign is present.
fn clean_exponent(s: &str) -> String {
    let (mantissa, exp) = if let Some(pos) = s.find('e') {
        (&s[..pos], &s[pos + 1..])
    } else if let Some(pos) = s.find('E') {
        (&s[..pos], &s[pos + 1..])
    } else {
        return s.to_owned();
    };

    let (sign, digits) = if exp.starts_with('+') || exp.starts_with('-') {
        (&exp[..1], &exp[1..])
    } else {
        ("+", exp)
    };

    let trimmed = digits.trim_start_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };

    format!("{mantissa}e{sign}{trimmed}")
}

/// Format a JSON number string to its canonical form.
///
/// If the string contains scientific notation (e/E), convert through f64
/// and format via `format_float` to normalize. Otherwise return verbatim
/// to preserve precision for integers beyond 2^53.
///
/// Go equivalent: `FormatNumber` in `eval_helpers.go`.
pub fn format_number(s: &str) -> String {
    if !s.contains('e') && !s.contains('E') {
        return s.to_owned();
    }
    match s.parse::<f64>() {
        Ok(f) => format_float(f),
        Err(_) => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_integers() {
        assert_eq!(format_float(0.0), "0");
        assert_eq!(format_float(1.0), "1");
        assert_eq!(format_float(42.0), "42");
        assert_eq!(format_float(-1.0), "-1");
    }

    #[test]
    fn format_decimals() {
        assert_eq!(format_float(0.5), "0.5");
        assert_eq!(format_float(3.14), "3.14");
    }

    #[test]
    fn format_nan_inf_as_null() {
        assert_eq!(format_float(f64::NAN), "null");
        assert_eq!(format_float(f64::INFINITY), "null");
        assert_eq!(format_float(f64::NEG_INFINITY), "null");
    }

    #[test]
    fn format_scientific_large() {
        // 1e21 and above should use scientific notation
        assert_eq!(format_float(1e21), "1e+21");
        assert_eq!(format_float(1e25), "1e+25");
    }

    #[test]
    fn format_scientific_small() {
        // Below 5e-7 should use scientific notation
        assert_eq!(format_float(1e-7), "1e-7");
        assert_eq!(format_float(5e-8), "5e-8");
    }

    #[test]
    fn format_decimal_range() {
        // Between 5e-7 and 1e21 should use decimal
        assert_eq!(format_float(0.000001), "0.000001");
        assert_eq!(
            format_float(999999999999999900000.0),
            "999999999999999900000"
        );
    }

    #[test]
    fn format_number_preserves_precision() {
        // Plain integers returned verbatim
        assert_eq!(format_number("12345678901234567"), "12345678901234567");
        // Scientific notation normalized through f64
        assert_eq!(format_number("1e3"), "1000");
    }

    #[test]
    fn format_matches_go_reference() {
        // case001: $string(22/7) — 15 significant digits
        assert_eq!(format_float(22.0_f64 / 7.0), "3.14285714285714");
        // case008: sum results
        assert_eq!(format_float(90.57), "90.57");
        assert_eq!(format_float(245.79), "245.79");
        // case018: 78.8 / 2
        assert_eq!(format_float(39.4), "39.4");
    }
}
