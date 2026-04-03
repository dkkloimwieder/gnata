//! `$formatInteger(number, picture)` — XPath 3.1 integer formatting.
//!
//! Port of Go `functions/string_format_integer.go`, function `fnFormatInteger`.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

pub fn fn_format_integer(args: &[Value], _focus: &Value) -> JsonataResult {
    // i64::MAX is 9223372036854775807. When cast to f64 it rounds up to 9.223372036854776e18
    // (which is 2^63), so any f64 >= that value would overflow i64 on cast.
    // i64::MIN is -9223372036854775808 = -2^63, which is exactly representable as f64,
    // so truncated == i64::MIN as f64 is still valid.
    const MAX_I64_F64: f64 = 9.223_372_036_854_776e18; // == i64::MAX as f64 (rounds up to 2^63)

    if args.len() < 2 {
        return Err(JsonataError::new(
            "D3006",
            "$formatInteger: requires 2 arguments",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let n = args[0]
        .as_f64()
        .ok_or_else(|| JsonataError::new("T0410", "$formatInteger: argument 1 must be a number"))?;
    let picture = match &args[1] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$formatInteger: argument 2 must be a string",
            ));
        }
    };

    let truncated = n.trunc();

    if !(-MAX_I64_F64..MAX_I64_F64).contains(&truncated) {
        let (format_token, modifier) = split_picture_modifier(picture);
        if format_token == "w" || format_token == "W" || format_token == "Ww" {
            return Ok(Value::String(format_big_float_words(
                truncated,
                format_token,
                modifier,
            )));
        }
        return Err(JsonataError::new(
            "D3137",
            "$formatInteger: number too large for integer formatting",
        ));
    }

    let result = format_integer_with_picture(truncated as i64, picture)?;
    Ok(Value::String(result))
}

fn format_integer_with_picture(n: i64, picture: &str) -> Result<String, JsonataError> {
    let (format_token, modifier) = split_picture_modifier(picture);
    let negative = n < 0;
    let abs_n = n.unsigned_abs() as i64; // safe: we checked range in caller

    let mut result = match format_token {
        "w" => {
            let words = int_to_words(abs_n);
            if modifier == "o" {
                apply_ordinal_word(&words)
            } else {
                words
            }
        }
        "W" => {
            if modifier == "o" {
                apply_ordinal_word(&int_to_words(abs_n)).to_uppercase()
            } else {
                int_to_words(abs_n).to_uppercase()
            }
        }
        "Ww" => {
            if modifier == "o" {
                to_title_case(&apply_ordinal_word(&int_to_words(abs_n)))
            } else {
                to_title_case(&int_to_words(abs_n))
            }
        }
        "i" => to_roman(abs_n, false),
        "I" => to_roman(abs_n, true),
        _ => {
            let chars: Vec<char> = format_token.chars().collect();
            if chars.len() == 1 {
                let ch = chars[0];
                if ch.is_ascii_lowercase() {
                    let mut r = to_alphabetic(abs_n, 'a');
                    if negative {
                        r.insert(0, '-');
                    }
                    return Ok(r);
                }
                if ch.is_ascii_uppercase() && ch != 'W' && ch != 'I' {
                    let mut r = to_alphabetic(abs_n, 'A');
                    if negative {
                        r.insert(0, '-');
                    }
                    return Ok(r);
                }
            }
            // Must contain at least one digit placeholder
            if !chars
                .iter()
                .any(|&c| c == '#' || c.is_ascii_digit() || unicode_digit_zero(c) != '\0')
            {
                return Err(JsonataError::new(
                    "D3130",
                    format!(
                        "$formatInteger: unsupported picture string {format_token:?}"
                    ),
                ));
            }
            let mut r = format_integer_decimal(abs_n, format_token)?;
            if modifier == "o" {
                r.push_str(ordinal_suffix(abs_n));
            }
            if negative {
                r.insert(0, '-');
            }
            return Ok(r);
        }
    };

    if negative {
        result.insert(0, '-');
    }
    Ok(result)
}

fn split_picture_modifier(picture: &str) -> (&str, &str) {
    if let Some(idx) = picture.find(';') {
        (&picture[..idx], &picture[idx + 1..])
    } else {
        (picture, "c")
    }
}

// ── Decimal formatting ───────────────────────────────────────────────────────

fn format_integer_decimal(n: i64, picture: &str) -> Result<String, JsonataError> {
    let runes: Vec<char> = picture.chars().collect();

    // Determine digit family (zero rune)
    let mut zero_rune = '0';
    let mut found_family = false;
    for &c in &runes {
        if c.is_ascii_digit() {
            if found_family && zero_rune != '0' {
                return Err(JsonataError::new(
                    "D3131",
                    "$formatInteger: mixed digit families in picture",
                ));
            }
            zero_rune = '0';
            found_family = true;
            continue;
        }
        let z = unicode_digit_zero(c);
        if z != '\0' {
            if found_family && zero_rune != z {
                return Err(JsonataError::new(
                    "D3131",
                    "$formatInteger: mixed digit families in picture",
                ));
            }
            if found_family && zero_rune == '0' {
                return Err(JsonataError::new(
                    "D3131",
                    "$formatInteger: mixed digit families in picture",
                ));
            }
            zero_rune = z;
            found_family = true;
        }
    }

    // Count mandatory and total digit positions
    let mut mandatory_count = 0usize;
    let mut total_digits = 0usize;
    for &c in &runes {
        if c == '#' {
            total_digits += 1;
        } else if c.is_ascii_digit() || is_unicode_digit(c) {
            mandatory_count += 1;
            total_digits += 1;
        }
    }
    if total_digits == 0 {
        return Err(JsonataError::new(
            "D3131",
            "$formatInteger: no digit placeholders in picture",
        ));
    }

    // Collect grouping info (separator char + position from right)
    let mut grp_infos: Vec<(char, usize)> = Vec::new();
    let mut digit_from_right = 0usize;
    for &c in runes.iter().rev() {
        if c == '#' || c.is_ascii_digit() || is_unicode_digit(c) {
            digit_from_right += 1;
        } else if digit_from_right > 0 {
            grp_infos.push((c, digit_from_right));
        }
    }

    // Build digit string with zero-padding
    let mut digits = format!("{n}");
    while digits.len() < mandatory_count {
        digits.insert(0, '0');
    }

    // Apply grouping
    if !grp_infos.is_empty() && !digits.is_empty() {
        digits = apply_integer_grouping(&digits, &grp_infos);
    }

    // Apply digit family translation
    if zero_rune != '0' {
        digits = apply_digit_family_rune(&digits, zero_rune);
    }

    Ok(digits)
}

/// Apply grouping separators to a digit string.
/// `grps` is a list of (separator_char, position_from_right).
fn apply_integer_grouping(digits: &str, grps: &[(char, usize)]) -> String {
    if grps.is_empty() {
        return digits.to_string();
    }

    let mut pos_set = std::collections::HashMap::new();
    for &(sep, pos_r) in grps {
        pos_set.insert(pos_r, sep);
    }

    let all_same_sep = grps.iter().all(|g| g.0 == grps[0].0);

    let is_regular = if grps.len() > 1 {
        let gap0 = grps[0].1;
        grps.windows(2).all(|w| w[1].1 - w[0].1 == gap0)
    } else {
        true
    };

    if all_same_sep && is_regular {
        let interval = grps[0].1;
        let rightmost_sep = grps[0].0;
        let max_len = digits.len() + 1;
        let mut pos = interval;
        while pos <= max_len {
            pos_set.entry(pos).or_insert(rightmost_sep);
            pos += interval;
        }
    }

    let runes: Vec<char> = digits.chars().collect();
    let mut result = Vec::new();
    for (i, &ch) in runes.iter().enumerate() {
        let pos_from_right = runes.len() - i;
        if i > 0
            && let Some(&sep) = pos_set.get(&pos_from_right) {
                result.push(sep);
            }
        result.push(ch);
    }
    result.into_iter().collect()
}

fn apply_digit_family_rune(s: &str, zero: char) -> String {
    let mut result = String::with_capacity(s.len() * 4);
    for c in s.chars() {
        if c.is_ascii_digit() {
            result.push(char::from_u32(zero as u32 + c as u32 - '0' as u32).unwrap_or(c));
        } else {
            result.push(c);
        }
    }
    result
}

// ── Unicode digit support ────────────────────────────────────────────────────

const UNICODE_ZEROS: &[char] = &[
    '\u{0660}', '\u{06F0}', '\u{07C0}', '\u{0966}', '\u{09E6}', '\u{0A66}', '\u{0AE6}', '\u{0B66}',
    '\u{0BE6}', '\u{0C66}', '\u{0CE6}', '\u{0D66}', '\u{0DE6}', '\u{0E50}', '\u{0ED0}', '\u{0F20}',
    '\u{1040}', '\u{1090}', '\u{17E0}', '\u{1810}', '\u{1946}', '\u{19D0}', '\u{1A80}', '\u{1A90}',
    '\u{1B50}', '\u{1BB0}', '\u{1C40}', '\u{1C50}', '\u{A620}', '\u{A8D0}', '\u{A900}', '\u{A9D0}',
    '\u{A9F0}', '\u{AA50}', '\u{ABF0}', '\u{FF10}',
];

fn unicode_digit_zero(c: char) -> char {
    for &z in UNICODE_ZEROS {
        let z_u32 = z as u32;
        let c_u32 = c as u32;
        if c_u32 >= z_u32 && c_u32 <= z_u32 + 9 {
            return z;
        }
    }
    '\0'
}

fn is_unicode_digit(c: char) -> bool {
    unicode_digit_zero(c) != '\0'
}

// ── Ordinal suffix ───────────────────────────────────────────────────────────

fn ordinal_suffix(n: i64) -> &'static str {
    let abs = n.unsigned_abs();
    let mod100 = abs % 100;
    let mod10 = abs % 10;
    if (11..=13).contains(&mod100) {
        return "th";
    }
    match mod10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

// ── Ordinal words ────────────────────────────────────────────────────────────

fn apply_ordinal_word(word: &str) -> String {
    let ordinals: &[(&str, &str)] = &[
        ("one", "first"),
        ("two", "second"),
        ("three", "third"),
        ("four", "fourth"),
        ("five", "fifth"),
        ("six", "sixth"),
        ("seven", "seventh"),
        ("eight", "eighth"),
        ("nine", "ninth"),
        ("ten", "tenth"),
        ("eleven", "eleventh"),
        ("twelve", "twelfth"),
        ("thirteen", "thirteenth"),
        ("fourteen", "fourteenth"),
        ("fifteen", "fifteenth"),
        ("sixteen", "sixteenth"),
        ("seventeen", "seventeenth"),
        ("eighteen", "eighteenth"),
        ("nineteen", "nineteenth"),
        ("twenty", "twentieth"),
        ("thirty", "thirtieth"),
        ("forty", "fortieth"),
        ("fifty", "fiftieth"),
        ("sixty", "sixtieth"),
        ("seventy", "seventieth"),
        ("eighty", "eightieth"),
        ("ninety", "ninetieth"),
        ("hundred", "hundredth"),
        ("thousand", "thousandth"),
        ("million", "millionth"),
        ("billion", "billionth"),
        ("trillion", "trillionth"),
    ];

    // Find last word and its separator
    let (prefix, sep, last) = {
        let mut last_idx = None;
        let mut sep_char = ' ';
        for (i, c) in word.char_indices().rev() {
            if c == ' ' || c == '-' {
                last_idx = Some(i);
                sep_char = c;
                break;
            }
        }
        match last_idx {
            Some(idx) => {
                let prefix = &word[..idx];
                let last = &word[idx + sep_char.len_utf8()..];
                (prefix, sep_char.to_string(), last)
            }
            None => ("", String::new(), word),
        }
    };

    // Check ordinal map
    for &(cardinal, ordinal) in ordinals {
        if last == cardinal {
            return format!("{prefix}{sep}{ordinal}");
        }
    }

    // Ends in "y" -> "ieth"
    if let Some(stem) = last.strip_suffix('y') {
        return format!("{prefix}{sep}{stem}ieth");
    }

    // Default: append "th"
    format!("{prefix}{sep}{last}th")
}

// ── Number to words ──────────────────────────────────────────────────────────

fn int_to_words(n: i64) -> String {
    if n == 0 {
        return "zero".to_string();
    }
    if n < 0 {
        return format!("minus {}", int_to_words(-n));
    }
    to_words(n)
}

fn below_thousand(n: i64) -> String {
    const ONES: &[&str] = &[
        "",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: &[&str] = &[
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];

    if n == 0 {
        return String::new();
    }
    if n < 20 {
        return ONES[n as usize].to_string();
    }
    if n < 100 {
        if n % 10 == 0 {
            return TENS[(n / 10) as usize].to_string();
        }
        return format!("{}-{}", TENS[(n / 10) as usize], ONES[(n % 10) as usize]);
    }
    // n < 1000
    let rem = n % 100;
    if rem == 0 {
        return format!("{} hundred", ONES[(n / 100) as usize]);
    }
    format!(
        "{} hundred and {}",
        ONES[(n / 100) as usize],
        below_thousand(rem)
    )
}

const SCALES: &[(&str, i64)] = &[
    ("trillion", 1_000_000_000_000),
    ("billion", 1_000_000_000),
    ("million", 1_000_000),
    ("thousand", 1_000),
];

fn to_words(n: i64) -> String {
    if n == 0 {
        return String::new();
    }
    if n < 1000 {
        return below_thousand(n);
    }
    for &(name, val) in SCALES {
        if n < val {
            continue;
        }
        let q = n / val;
        let rem = n % val;
        let q_word = to_words(q);
        let mut result = format!("{q_word} {name}");
        if rem > 0 {
            let rem_word = to_words(rem);
            if rem < 100 {
                result.push_str(" and ");
                result.push_str(&rem_word);
            } else {
                result.push_str(", ");
                result.push_str(&rem_word);
            }
        }
        return result;
    }
    below_thousand(n)
}

// ── Float to words (for very large numbers) ──────────────────────────────────

fn float_to_words(f: f64) -> String {
    if f == 0.0 {
        return "zero".to_string();
    }
    let mut f = f;
    let trillion: f64 = 1e12;
    let mut trillion_count = 0usize;
    while f >= trillion {
        f /= trillion;
        trillion_count += 1;
    }
    let mut result = int_to_words(f.round() as i64);
    for _ in 0..trillion_count {
        result.push_str(" trillion");
    }
    result
}

fn format_big_float_words(f: f64, format_token: &str, modifier: &str) -> String {
    let negative = f < 0.0;
    let f = f.abs();
    let mut words = match format_token {
        "w" => {
            if modifier == "o" {
                apply_ordinal_word(&float_to_words(f))
            } else {
                float_to_words(f)
            }
        }
        "W" => {
            if modifier == "o" {
                apply_ordinal_word(&float_to_words(f)).to_uppercase()
            } else {
                float_to_words(f).to_uppercase()
            }
        }
        "Ww" => {
            if modifier == "o" {
                to_title_case(&apply_ordinal_word(&float_to_words(f)))
            } else {
                to_title_case(&float_to_words(f))
            }
        }
        _ => float_to_words(f),
    };
    if negative {
        words.insert_str(0, "minus ");
    }
    words
}

// ── Title case ───────────────────────────────────────────────────────────────

fn to_title_case(s: &str) -> String {
    let lowercase: &[&str] = &["and", "or", "of", "the"];
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    let mut word_buf = String::new();

    let flush = |word_buf: &mut String, result: &mut String, capitalize_next: &mut bool| {
        if word_buf.is_empty() {
            return;
        }
        let lower = word_buf.to_lowercase();
        if *capitalize_next || !lowercase.contains(&lower.as_str()) {
            if !lower.is_empty() {
                let mut chars = lower.chars();
                if let Some(first) = chars.next() {
                    result.extend(first.to_uppercase());
                    result.push_str(chars.as_str());
                }
            }
        } else {
            result.push_str(&lower);
        }
        *capitalize_next = false;
        word_buf.clear();
    };

    for c in s.chars() {
        if c == ' ' || c == ',' || c == '-' {
            flush(&mut word_buf, &mut result, &mut capitalize_next);
            result.push(c);
            capitalize_next = c == '-';
        } else {
            word_buf.push(c);
        }
    }
    flush(&mut word_buf, &mut result, &mut capitalize_next);
    result
}

// ── Roman numerals ───────────────────────────────────────────────────────────

fn to_roman(mut n: i64, upper: bool) -> String {
    if n <= 0 {
        return String::new();
    }
    let vals: &[(i64, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut result = String::new();
    for &(v, sym) in vals {
        while n >= v {
            result.push_str(sym);
            n -= v;
        }
    }
    if upper { result } else { result.to_lowercase() }
}

// ── Alphabetic sequences ─────────────────────────────────────────────────────

fn to_alphabetic(mut n: i64, base: char) -> String {
    if n <= 0 {
        return String::new();
    }
    let mut result: Vec<char> = Vec::new();
    while n > 0 {
        n -= 1;
        result.push(char::from_u32(base as u32 + (n % 26) as u32).expect("valid ASCII letter offset"));
        n /= 26;
    }
    result.reverse();
    result.into_iter().collect()
}
