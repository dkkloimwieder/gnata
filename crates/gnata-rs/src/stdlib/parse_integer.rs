//! `$parseInteger(string, picture)` — parse a formatted integer string back to a number.
//!
//! Port of Go `functions/string_format_integer.go` `fnParseInteger`.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

pub fn fn_parse_integer(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.len() < 2 {
        return Err(JsonataError::new(
            "D3006",
            "$parseInteger: requires 2 arguments",
        ));
    }

    if matches!(args[0], Value::Undefined) {
        return Ok(Value::Undefined);
    }

    let s: &str = match &args[0] {
        Value::String(s) => s,
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$parseInteger: argument 1 must be a string",
            ));
        }
    };

    let picture: &str = match &args[1] {
        Value::String(s) => s,
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$parseInteger: argument 2 must be a string",
            ));
        }
    };

    match parse_integer_with_picture(s, picture) {
        Ok(n) => Ok(Value::Number(n as f64)),
        Err(e) if e.code == "D3137_FLOAT" => {
            // The message contains the float string
            if let Ok(f) = e.message.parse::<f64>() {
                Ok(Value::Number(f))
            } else {
                Err(e)
            }
        }
        Err(e) => Err(e),
    }
}

fn split_picture_modifier(picture: &str) -> (&str, &str) {
    if let Some(idx) = picture.find(';') {
        (&picture[..idx], &picture[idx + 1..])
    } else {
        (picture, "c")
    }
}

static UNICODE_ZEROS: &[char] = &[
    '\u{0660}', '\u{06F0}', '\u{07C0}', '\u{0966}', '\u{09E6}', '\u{0A66}', '\u{0AE6}', '\u{0B66}',
    '\u{0BE6}', '\u{0C66}', '\u{0CE6}', '\u{0D66}', '\u{0DE6}', '\u{0E50}', '\u{0ED0}', '\u{0F20}',
    '\u{1040}', '\u{1090}', '\u{17E0}', '\u{1810}', '\u{1946}', '\u{19D0}', '\u{1A80}', '\u{1A90}',
    '\u{1B50}', '\u{1BB0}', '\u{1C40}', '\u{1C50}', '\u{A620}', '\u{A8D0}', '\u{A900}', '\u{A9D0}',
    '\u{A9F0}', '\u{AA50}', '\u{ABF0}', '\u{FF10}',
];

fn unicode_digit_zero(c: char) -> Option<char> {
    UNICODE_ZEROS.iter().find(|&&z| c >= z && c <= char::from_u32(z as u32 + 9).unwrap_or(z)).copied()
}

fn parse_integer_with_picture(s: &str, picture: &str) -> Result<i64, JsonataError> {
    let (format_token, _modifier) = split_picture_modifier(picture);

    match format_token {
        "w" | "W" | "Ww" => return words_to_int(&s.to_lowercase()),
        "i" | "I" => return from_roman(&s.to_uppercase()),
        _ => {
            let chars: Vec<char> = format_token.chars().collect();
            if chars.len() == 1 {
                let ch = chars[0];
                if ch.is_ascii_alphabetic() {
                    return from_alphabetic(&s.to_lowercase());
                }
            }
        }
    }

    // Decimal digit pattern
    let mut zero_rune = '0';
    let mut has_mandatory = false;
    for c in picture.chars() {
        if c.is_ascii_digit() {
            zero_rune = '0';
            has_mandatory = true;
        } else if let Some(z) = unicode_digit_zero(c) {
            zero_rune = z;
            has_mandatory = true;
        }
    }

    if !has_mandatory {
        return Err(JsonataError::new(
            "D3130",
            "$parseInteger: picture string must contain at least one mandatory digit placeholder",
        ));
    }

    let mut digits = String::new();
    for c in s.chars() {
        if c == '-' || c.is_ascii_digit() {
            digits.push(c);
        } else if zero_rune != '0' {
            let z_u32 = zero_rune as u32;
            let c_u32 = c as u32;
            if c_u32 >= z_u32 && c_u32 <= z_u32 + 9 {
                // Map to ASCII digit
                if let Some(ascii) = char::from_u32('0' as u32 + (c_u32 - z_u32)) {
                    digits.push(ascii);
                }
            }
        }
    }

    let cleaned = digits.trim();
    cleaned.parse::<i64>().map_err(|_| {
        JsonataError::new(
            "D3137",
            format!("$parseInteger: cannot parse {s:?} as integer"),
        )
    })
}

// ── De-ordinalise ────────────────────────────────────────────────────────────

fn de_ordinalise(s: &str) -> String {
    let irregulars: &[(&str, &str)] = &[
        ("first", "one"),
        ("second", "two"),
        ("third", "three"),
        ("fourth", "four"),
        ("fifth", "five"),
        ("sixth", "six"),
        ("seventh", "seven"),
        ("eighth", "eight"),
        ("ninth", "nine"),
        ("tenth", "ten"),
        ("eleventh", "eleven"),
        ("twelfth", "twelve"),
        ("thirteenth", "thirteen"),
        ("fourteenth", "fourteen"),
        ("fifteenth", "fifteen"),
        ("sixteenth", "sixteen"),
        ("seventeenth", "seventeen"),
        ("eighteenth", "eighteen"),
        ("nineteenth", "nineteen"),
        ("twentieth", "twenty"),
        ("thirtieth", "thirty"),
        ("fortieth", "forty"),
        ("fiftieth", "fifty"),
        ("sixtieth", "sixty"),
        ("seventieth", "seventy"),
        ("eightieth", "eighty"),
        ("ninetieth", "ninety"),
        ("hundredth", "hundred"),
        ("thousandth", "thousand"),
        ("millionth", "million"),
        ("billionth", "billion"),
        ("trillionth", "trillion"),
        ("zeroth", "zero"),
    ];

    // Find last separator (space or hyphen)
    let bytes = s.as_bytes();
    let mut last_idx: Option<usize> = None;
    let mut last_sep: u8 = b' ';
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b' ' || bytes[i] == b'-' {
            last_idx = Some(i);
            last_sep = bytes[i];
            break;
        }
    }

    let (prefix, last_word) = if let Some(idx) = last_idx {
        (&s[..idx], &s[idx + 1..])
    } else {
        ("", s)
    };

    for &(ordinal, cardinal) in irregulars {
        if last_word == ordinal {
            if last_idx.is_some() {
                return format!("{}{}{}", prefix, last_sep as char, cardinal);
            }
            return cardinal.to_string();
        }
    }

    s.to_string()
}

// ── Words to number ──────────────────────────────────────────────────────────

fn words_to_float(s: &str) -> Result<f64, JsonataError> {
    let s = de_ordinalise(s);

    let word_vals: &[(&str, f64)] = &[
        ("zero", 0.0),
        ("one", 1.0),
        ("two", 2.0),
        ("three", 3.0),
        ("four", 4.0),
        ("five", 5.0),
        ("six", 6.0),
        ("seven", 7.0),
        ("eight", 8.0),
        ("nine", 9.0),
        ("ten", 10.0),
        ("eleven", 11.0),
        ("twelve", 12.0),
        ("thirteen", 13.0),
        ("fourteen", 14.0),
        ("fifteen", 15.0),
        ("sixteen", 16.0),
        ("seventeen", 17.0),
        ("eighteen", 18.0),
        ("nineteen", 19.0),
        ("twenty", 20.0),
        ("thirty", 30.0),
        ("forty", 40.0),
        ("fifty", 50.0),
        ("sixty", 60.0),
        ("seventy", 70.0),
        ("eighty", 80.0),
        ("ninety", 90.0),
        ("hundred", 100.0),
        ("thousand", 1e3),
        ("million", 1e6),
        ("billion", 1e9),
        ("trillion", 1e12),
    ];

    let s = s.replace(['-', ','], " ");
    let words: Vec<&str> = s.split_whitespace().collect();

    let mut total: f64 = 0.0;
    let mut current: f64 = 0.0;

    for w in &words {
        if *w == "and" {
            continue;
        }
        let val = word_vals
            .iter()
            .find(|(name, _)| name == w)
            .map(|(_, v)| *v);

        let Some(val) = val else {
            return Err(JsonataError::new(
                "D3137",
                format!("$parseInteger: unknown word {w:?}"),
            ));
        };

        if val == 100.0 {
            if current == 0.0 {
                current = 1.0;
            }
            current *= 100.0;
        } else if val >= 1000.0 {
            if current == 0.0 {
                if total == 0.0 {
                    total = 1.0;
                }
                total *= val;
            } else {
                total += current * val;
                current = 0.0;
            }
        } else {
            current += val;
        }
    }

    Ok(total + current)
}

fn words_to_int(s: &str) -> Result<i64, JsonataError> {
    const MAX_SAFE: f64 = (1_i64 << 63) as f64 - 1024.0;
    let f = words_to_float(s)?;
    if f > MAX_SAFE || f < -MAX_SAFE {
        return Err(JsonataError::new("D3137_FLOAT", format!("{f}")));
    }
    Ok(f as i64)
}

// ── Roman numerals ───────────────────────────────────────────────────────────

fn from_roman(s: &str) -> Result<i64, JsonataError> {
    fn roman_val(c: char) -> Option<i64> {
        match c {
            'I' => Some(1),
            'V' => Some(5),
            'X' => Some(10),
            'L' => Some(50),
            'C' => Some(100),
            'D' => Some(500),
            'M' => Some(1000),
            _ => None,
        }
    }

    let chars: Vec<char> = s.chars().collect();
    let mut total: i64 = 0;

    for (i, &c) in chars.iter().enumerate() {
        let v = roman_val(c).ok_or_else(|| {
            JsonataError::new(
                "D3137",
                format!("$parseInteger: invalid Roman numeral {c:?}"),
            )
        })?;

        if i + 1 < chars.len()
            && let Some(next) = roman_val(chars[i + 1])
                && next > v {
                    total -= v;
                    continue;
                }
        total += v;
    }

    Ok(total)
}

// ── Alphabetic (spreadsheet column) ──────────────────────────────────────────

fn from_alphabetic(s: &str) -> Result<i64, JsonataError> {
    let mut result: i64 = 0;
    for c in s.chars() {
        if !c.is_ascii_lowercase() {
            return Err(JsonataError::new(
                "D3137",
                format!("$parseInteger: invalid alphabetic character {c:?}"),
            ));
        }
        result = result * 26 + (c as i64 - 'a' as i64 + 1);
    }
    Ok(result)
}
