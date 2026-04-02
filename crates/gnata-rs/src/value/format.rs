//! Number formatting matching JavaScript's `Number.toString()` behavior.
//!
//! Go equivalent: `FormatFloat` in `eval_helpers.go`, `FormatNumber` in `eval_helpers.go`.
//! Uses `ryu-js` crate for exact ECMAScript formatting.

/// Format an f64 to match JavaScript's `Number.toString()`.
///
/// - NaN/Inf → "null"
/// - Numbers in [5e-7, 1e21) use decimal notation
/// - Numbers outside that range use scientific notation with cleaned exponents
pub fn format_float(n: f64) -> String {
    if n.is_nan() || n.is_infinite() {
        return "null".into();
    }
    // ryu-js implements the exact ECMAScript Number::toString algorithm
    let mut buf = ryu_js::Buffer::new();
    buf.format(n).to_owned()
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
}
