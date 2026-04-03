//! Datetime functions: $now, $millis, $fromMillis, $toMillis.
//!
//! Implements XPath/JSONata picture-format datetime formatting and parsing.
//! Port of Go `functions/datetime_funcs.go`, `datetime_format.go`, `datetime_parse.go`.

use crate::error::{JsonataError, JsonataResult};
use crate::value::Value;

// ── Public entry points ─────────────────────────────────────────────────────

#[allow(clippy::missing_errors_doc)]
pub fn fn_now(args: &[Value], _focus: &Value) -> JsonataResult {
    let now = jiff::Zoned::now();
    if !args.is_empty() && !args[0].is_undefined() {
        let picture = match &args[0] {
            Value::String(s) => s.clone(),
            _ => {
                return Err(JsonataError::new(
                    "T0410",
                    "$now: picture argument must be a string",
                ));
            }
        };
        let tz_offset = if args.len() >= 2 && !args[1].is_undefined() {
            match &args[1] {
                Value::String(s) => parse_tz(s)?,
                _ => return Err(JsonataError::new("T0410", "timezone must be a string")),
            }
        } else {
            0
        };
        let ts = now.timestamp();
        let millis = ts.as_millisecond();
        let s = format_with_picture(millis, &picture, tz_offset)?;
        return Ok(Value::String(s));
    }
    // Default: ISO 8601 with milliseconds
    let ts = now.timestamp();
    let millis = ts.as_millisecond();
    Ok(Value::String(format_default_iso(millis, 0)))
}

#[allow(clippy::missing_errors_doc)]
pub fn fn_millis(_args: &[Value], _focus: &Value) -> JsonataResult {
    let now = jiff::Timestamp::now();
    Ok(Value::Number(now.as_millisecond() as f64))
}

#[allow(clippy::missing_errors_doc)]
pub fn fn_from_millis(args: &[Value], focus: &Value) -> JsonataResult {
    // With no args, use focus as millis argument.
    let effective_args: &[Value];
    let tmp;
    if args.is_empty() {
        if focus.is_undefined() {
            return Ok(Value::Undefined);
        }
        tmp = std::slice::from_ref(focus);
        effective_args = tmp;
    } else {
        effective_args = args;
    }

    if effective_args[0].is_undefined() {
        return Ok(Value::Undefined);
    }

    let ms = match value_to_f64(&effective_args[0]) {
        Some(v) => v as i64,
        None => {
            return Err(JsonataError::new(
                "T0410",
                "$fromMillis: argument must be a number",
            ));
        }
    };

    // Resolve timezone offset (arg index 2).
    let tz_offset = if effective_args.len() >= 3 && !effective_args[2].is_undefined() {
        match &effective_args[2] {
            Value::String(s) => parse_tz(s)?,
            _ => return Err(JsonataError::new("T0410", "timezone must be a string")),
        }
    } else {
        0
    };

    if effective_args.len() >= 2 && !effective_args[1].is_undefined() {
        let picture = match &effective_args[1] {
            Value::String(s) => s.clone(),
            _ => {
                return Err(JsonataError::new(
                    "T0410",
                    "$fromMillis: picture argument must be a string",
                ));
            }
        };
        let s = format_with_picture(ms, &picture, tz_offset)?;
        return Ok(Value::String(s));
    }

    // No picture: ISO 8601 default.
    Ok(Value::String(format_default_iso(ms, tz_offset)))
}

#[allow(clippy::missing_errors_doc)]
pub fn fn_to_millis(args: &[Value], _focus: &Value) -> JsonataResult {
    if args.is_empty() || args[0].is_undefined() {
        return Ok(Value::Undefined);
    }

    let s = match &args[0] {
        Value::String(s) => s.clone(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$toMillis: argument must be a string",
            ));
        }
    };

    if args.len() >= 2 && !args[1].is_undefined() {
        let picture = match &args[1] {
            Value::String(p) => p.clone(),
            _ => {
                return Err(JsonataError::new(
                    "T0410",
                    "$toMillis: picture argument must be a string",
                ));
            }
        };
        let result = parse_with_picture(&s, &picture)?;
        return match result {
            Some(ms) => Ok(Value::Number(ms as f64)),
            None => Ok(Value::Undefined),
        };
    }

    // ISO 8601 / RFC 3339 parsing.
    parse_iso_to_millis(&s)
}

// ── ISO 8601 parsing (no picture) ───────────────────────────────────────────

fn parse_iso_to_millis(s: &str) -> JsonataResult {
    // Try jiff's timestamp parsing first (handles RFC 3339 / ISO 8601 with Z / offset).
    if let Ok(ts) = s.parse::<jiff::Timestamp>() {
        return Ok(Value::Number(ts.as_millisecond() as f64));
    }

    // Try date-only "YYYY-MM-DD".
    if let Some(ms) = try_parse_date_only(s) {
        return Ok(Value::Number(ms as f64));
    }

    // Try year-only "YYYY".
    if let Some(ms) = try_parse_year_only(s) {
        return Ok(Value::Number(ms as f64));
    }

    // Try datetime without timezone "YYYY-MM-DDTHH:MM:SS" or with sub-seconds.
    if let Some(ms) = try_parse_datetime_no_tz(s) {
        return Ok(Value::Number(ms as f64));
    }

    Err(JsonataError::new(
        "D3110",
        format!(
            "$toMillis: the value '{s}' does not match the standard datetime format"
        ),
    ))
}

fn try_parse_date_only(s: &str) -> Option<i64> {
    // "YYYY-MM-DD"
    if s.len() == 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-' {
        let y: i16 = s[0..4].parse().ok()?;
        let m: i8 = s[5..7].parse().ok()?;
        let d: i8 = s[8..10].parse().ok()?;
        let date = jiff::civil::date(y, m, d);
        let ts = date.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp();
        return Some(ts.as_millisecond());
    }
    None
}

fn try_parse_year_only(s: &str) -> Option<i64> {
    if s.len() == 4 {
        let y: i16 = s.parse().ok()?;
        let date = jiff::civil::date(y, 1, 1);
        let ts = date.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp();
        return Some(ts.as_millisecond());
    }
    None
}

fn try_parse_datetime_no_tz(s: &str) -> Option<i64> {
    // "YYYY-MM-DDTHH:MM:SS" or "YYYY-MM-DDTHH:MM:SS.sss"
    if s.len() >= 19 && s.as_bytes()[4] == b'-' && s.as_bytes()[10] == b'T' {
        let date_part = &s[0..10];
        let time_part = &s[11..];
        let y: i16 = date_part[0..4].parse().ok()?;
        let m: i8 = date_part[5..7].parse().ok()?;
        let d: i8 = date_part[8..10].parse().ok()?;

        let h: i8 = time_part[0..2].parse().ok()?;
        let mi: i8 = time_part[3..5].parse().ok()?;
        let sec: i8 = time_part[6..8].parse().ok()?;
        let ms: i32 = if time_part.len() > 9 && time_part.as_bytes()[8] == b'.' {
            let frac = &time_part[9..];
            let frac = &frac[..frac.len().min(3)];
            let v: i32 = frac.parse().ok()?;
            // Pad to ms
            match frac.len() {
                1 => v * 100,
                2 => v * 10,
                _ => v,
            }
        } else {
            0
        };

        let dt = jiff::civil::datetime(y, m, d, h, mi, sec, ms * 1_000_000);
        let ts = dt.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp();
        return Some(ts.as_millisecond());
    }
    None
}

// ── Default ISO 8601 formatting ──────────────────────────────────────────────

fn format_default_iso(ms: i64, tz_offset_secs: i32) -> String {
    // Apply timezone offset to get local time.
    let local_ms = ms + i64::from(tz_offset_secs) * 1000;
    let secs = local_ms.div_euclid(1000);
    let millis_part = local_ms.rem_euclid(1000);

    let (y, mo, d, h, mi, s) = secs_to_ymd_hms(secs);

    let base = format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis_part:03}"
    );

    if tz_offset_secs == 0 {
        return base + "Z";
    }
    let sign = if tz_offset_secs >= 0 { '+' } else { '-' };
    let abs_offset = tz_offset_secs.unsigned_abs();
    let oh = abs_offset / 3600;
    let om = (abs_offset % 3600) / 60;
    format!("{base}{sign}{oh:02}:{om:02}")
}

// ── Picture-format formatting ────────────────────────────────────────────────

/// Format epoch milliseconds using an XPath picture string.
/// Returns an error for invalid picture strings.
#[allow(clippy::missing_errors_doc)]
pub fn format_with_picture(
    ms: i64,
    picture: &str,
    tz_offset_secs: i32,
) -> Result<String, JsonataError> {
    // Apply TZ offset.
    let local_ms = ms + i64::from(tz_offset_secs) * 1000;
    let secs = local_ms.div_euclid(1000);
    let ms_frac = local_ms.rem_euclid(1000) as u32;
    let (year, month, day, hour, minute, second) = secs_to_ymd_hms(secs);
    let weekday = day_of_week(year, month, day); // 0=Sun..6=Sat

    // Pre-scan for unclosed brackets.
    let runes: Vec<char> = picture.chars().collect();
    let mut i = 0;
    while i < runes.len() {
        if runes[i] == '[' {
            if i + 1 < runes.len() && runes[i + 1] == '[' {
                i += 2;
                continue;
            }
            let mut j = i + 1;
            while j < runes.len() && runes[j] != ']' {
                j += 1;
            }
            if j >= runes.len() {
                return Err(JsonataError::new(
                    "D3135",
                    "the picture string has an unclosed variable marker '[...'",
                ));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }

    let mut result = String::new();
    let mut i = 0;
    while i < runes.len() {
        let ch = runes[i];
        if ch == '[' {
            if i + 1 < runes.len() && runes[i + 1] == '[' {
                result.push('[');
                i += 2;
                continue;
            }
            let mut j = i + 1;
            while j < runes.len() && runes[j] != ']' {
                j += 1;
            }
            // Strip whitespace from token.
            let token: String = runes[i + 1..j]
                .iter()
                .filter(|&&c| c != ' ' && c != '\n' && c != '\r' && c != '\t')
                .collect();
            let s = format_token(
                &token,
                year,
                month,
                day,
                hour,
                minute,
                second,
                ms_frac as i32,
                weekday,
                tz_offset_secs,
            )?;
            result.push_str(&s);
            i = j + 1;
            continue;
        }
        if ch == ']' && i + 1 < runes.len() && runes[i + 1] == ']' {
            result.push(']');
            i += 2;
            continue;
        }
        result.push(ch);
        i += 1;
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn format_token(
    token: &str,
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    ms_frac: i32,
    weekday: u8, // 0=Sun..6=Sat
    tz_offset_secs: i32,
) -> Result<String, JsonataError> {
    if token.is_empty() {
        return Ok(String::new());
    }
    let mut chars = token.chars();
    let component = chars.next().expect("token is non-empty");
    let modifier: String = chars.collect();

    match component {
        'Y' => format_year_component(year, &modifier),
        'M' => format_month_token(i64::from(month), &modifier),
        'D' => format_day_component(i64::from(day), &modifier),
        'H' => format_integer_token(i64::from(hour), &modifier),
        'h' => {
            let h12 = i64::from((hour + 11) % 12 + 1);
            format_integer_token(h12, &modifier)
        }
        'm' => {
            let m = if modifier.is_empty() { "01" } else { &modifier };
            format_integer_token(i64::from(minute), m)
        }
        's' => {
            let m = if modifier.is_empty() { "01" } else { &modifier };
            format_integer_token(i64::from(second), m)
        }
        'f' => Ok(format_frac_second(ms_frac, &modifier)),
        'F' => Ok(format_weekday_token(weekday, &modifier)),
        'Z' | 'z' => format_timezone(component, &modifier, tz_offset_secs),
        'P' => Ok(format_ampm(hour, &modifier)),
        'E' | 'C' => Ok("ISO".to_string()),
        'd' => {
            let doy = i64::from(day_of_year(year, month, day));
            format_day_of_year_token(doy, &modifier)
        }
        'W' => {
            let (_, w) = iso_week(year, month, day);
            format_integer_token(i64::from(w), &modifier)
        }
        'X' => {
            let (iso_y, _) = iso_week(year, month, day);
            format_year_token(iso_y, &modifier)
        }
        'w' => {
            // Week of month (ISO week Thursday method).
            let (thy, thm, thd) = iso_week_thursday(year, month, day);
            let wom = week_of_month(thy, thm, thd);
            format_integer_token(i64::from(wom), &modifier)
        }
        'x' => {
            // Month of the ISO week (Thursday-based month).
            let (_thy, thm, _thd) = iso_week_thursday(year, month, day);
            format_iso_week_month(thm, &modifier)
        }
        _ => Ok(format!("[{token}]")),
    }
}

fn format_year_component(y: i32, modifier: &str) -> Result<String, JsonataError> {
    match modifier {
        "I" => Ok(to_roman(y.into(), true)),
        "i" => Ok(to_roman(y.into(), false)),
        "w" => Ok(int_to_words(y.into())),
        "W" => Ok(int_to_words(y.into()).to_uppercase()),
        "a" => Ok(to_alphabetic(y.into(), 'a')),
        "A" => Ok(to_alphabetic(y.into(), 'A')),
        "N" => Err(JsonataError::new(
            "D3133",
            format!(
                "the picture string is not valid: unsupported modifier in [Y{modifier}]"
            ),
        )),
        _ => format_year_token(y, modifier),
    }
}

#[allow(clippy::unnecessary_wraps)]
fn format_year_token(y: i32, modifier: &str) -> Result<String, JsonataError> {
    if let Some(rest) = modifier.strip_prefix(',') {
        return Ok(truncate_year(y, rest));
    }
    if let Some(comma_pos) = modifier.find(',') {
        let prefix = &modifier[..comma_pos];
        let suffix = &modifier[comma_pos + 1..];
        // Check for max-width truncation: prefix + "-N".
        if let Some(dash_pos) = suffix.find('-') {
            let max_str = &suffix[dash_pos + 1..];
            if let Ok(max_width) = max_str
                .trim_matches(|c: char| c == '#' || c == '*' || c == ' ')
                .parse::<usize>()
                && max_width > 0
            {
                let s = format_integer_mod(i64::from(y), prefix);
                return Ok(if s.len() > max_width {
                    s[s.len() - max_width..].to_string()
                } else {
                    s
                });
            }
        }
        // "9,999,*" style grouping.
        if prefix.contains('9') || suffix.contains('9') {
            return Ok(format_integer_with_grouping(y));
        }
        return Ok(format_integer_mod(i64::from(y), prefix));
    }
    Ok(format_integer_mod(i64::from(y), modifier))
}

fn truncate_year(y: i32, width_spec: &str) -> String {
    let width: usize = if let Some(dash) = width_spec.find('-') {
        width_spec[..dash].parse().unwrap_or(0)
    } else {
        width_spec.parse().unwrap_or(0)
    };
    if width == 0 {
        return y.to_string();
    }
    let s = y.to_string();
    if s.len() > width {
        s[s.len() - width..].to_string()
    } else {
        s
    }
}

fn format_integer_with_grouping(v: i32) -> String {
    let s = v.to_string();
    if s.len() <= 3 {
        return s;
    }
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(c);
    }
    result
}

#[allow(clippy::unnecessary_wraps)]
fn format_month_token(m: i64, modifier: &str) -> Result<String, JsonataError> {
    let month_names = MONTH_NAMES;
    let idx = (m - 1) as usize;
    match modifier {
        mod_ if mod_.starts_with("Nn") => {
            let name = month_names[idx];
            if let Some(suffix) = mod_.strip_prefix("Nn").and_then(|s| s.strip_prefix(',')) {
                let width = parse_width_from_suffix(suffix);
                if width > 0 && name.len() > width {
                    return Ok(name[..width].to_string());
                }
            }
            Ok(name.to_string())
        }
        "N" => Ok(month_names[idx].to_uppercase()),
        "a" => Ok(to_alphabetic(m, 'a')),
        "A" => Ok(to_alphabetic(m, 'A')),
        "I" => Ok(to_roman(m, true)),
        "i" => Ok(to_roman(m, false)),
        _ => Ok(format_numeric_with_min_width(m as i32, modifier)),
    }
}

fn parse_width_from_suffix(suffix: &str) -> usize {
    if let Some(dash) = suffix.find('-') {
        suffix[..dash].parse().unwrap_or(0)
    } else {
        suffix.parse().unwrap_or(0)
    }
}

fn format_numeric_with_min_width(v: i32, modifier: &str) -> String {
    let (primary, min_width) = if let Some(comma) = modifier.find(',') {
        let suffix = &modifier[comma + 1..];
        let w = parse_width_from_suffix(suffix);
        (&modifier[..comma], w)
    } else {
        (modifier, 0)
    };
    let s = format_integer_mod(i64::from(v), primary);
    if min_width > 0 && s.len() < min_width {
        format!("{s:0>min_width$}")
    } else {
        s
    }
}

#[allow(clippy::unnecessary_wraps)]
fn format_day_component(day: i64, modifier: &str) -> Result<String, JsonataError> {
    match modifier {
        "I" => Ok(to_roman(day, true)),
        "i" => Ok(to_roman(day, false)),
        "a" => Ok(to_alphabetic(day, 'a')),
        "A" => Ok(to_alphabetic(day, 'A')),
        "wo" => Ok(int_to_words_ordinal(day)),
        "Wo" => Ok(int_to_words_ordinal(day).to_uppercase()),
        "w" => Ok(int_to_words(day)),
        "W" => Ok(int_to_words(day).to_uppercase()),
        _ => Ok(format_day_token(day, modifier)),
    }
}

fn format_day_token(d: i64, modifier: &str) -> String {
    let is_ordinal = modifier.ends_with('o');
    let base_modifier = if is_ordinal {
        &modifier[..modifier.len() - 1]
    } else {
        modifier
    };
    let s = if base_modifier.contains(',') {
        format_numeric_with_min_width(d as i32, base_modifier)
    } else {
        format_integer_mod(d, base_modifier)
    };
    if is_ordinal { s + ordinal_suffix(d) } else { s }
}

#[allow(clippy::unnecessary_wraps)]
fn format_day_of_year_token(doy: i64, modifier: &str) -> Result<String, JsonataError> {
    match modifier {
        "wo" => Ok(int_to_words_ordinal(doy)),
        "Wo" => Ok(int_to_words_ordinal(doy).to_uppercase()),
        "w" => Ok(int_to_words(doy)),
        "W" => Ok(int_to_words(doy).to_uppercase()),
        _ => {
            if let Some(base) = modifier.strip_suffix('o') {
                Ok(format_integer_mod(doy, base) + ordinal_suffix(doy))
            } else {
                Ok(format_integer_mod(doy, modifier))
            }
        }
    }
}

fn format_weekday_token(wd: u8, modifier: &str) -> String {
    // wd: 0=Sun, 1=Mon, ..., 6=Sat
    let names = WEEKDAY_NAMES;
    match modifier {
        "" | "n" => names[wd as usize].to_lowercase(),
        m if m.starts_with("Nn") => {
            let name = names[wd as usize];
            if let Some(suffix) = m.strip_prefix("Nn").and_then(|s| s.strip_prefix(',')) {
                let width = parse_width_from_suffix(suffix);
                if width > 0 && name.len() > width {
                    return name[..width].to_string();
                }
            }
            name.to_string()
        }
        "N" => names[wd as usize].to_uppercase(),
        _ => {
            // Numeric: ISO weekday (Mon=1, ..., Sun=7)
            let iso = (i32::from(wd) + 6) % 7 + 1;
            format_integer_mod(i64::from(iso), modifier)
        }
    }
}

fn format_ampm(hour: u8, modifier: &str) -> String {
    let s = if hour < 12 { "am" } else { "pm" };
    if modifier == "N" {
        s.to_uppercase()
    } else {
        s.to_string()
    }
}

fn format_frac_second(ns_millis: i32, modifier: &str) -> String {
    let width = if modifier.is_empty() {
        3
    } else {
        modifier.len()
    };
    let _s = format!("{:09}", i64::from(ns_millis) * 1_000_000); // ns_millis is really ms
    // Actually ms_frac is milliseconds (0-999); pad to 9 digits as nanoseconds.
    let ms_as_ns = i64::from(ns_millis) * 1_000_000;
    let full = format!("{ms_as_ns:09}");
    if width <= 9 {
        full[..width].to_string()
    } else {
        full + &"0".repeat(width - 9)
    }
}

#[allow(clippy::unnecessary_wraps)]
fn format_iso_week_month(month: u8, modifier: &str) -> Result<String, JsonataError> {
    let m = month as usize;
    match modifier {
        "Nn" | "n" => Ok(MONTH_NAMES[m - 1].to_string()),
        "N" => Ok(MONTH_NAMES[m - 1].to_uppercase()),
        _ => Ok(m.to_string()),
    }
}

fn format_timezone(
    component: char,
    modifier: &str,
    tz_offset_secs: i32,
) -> Result<String, JsonataError> {
    let use_z = modifier.ends_with('t');
    let mod_ = if use_z {
        &modifier[..modifier.len() - 1]
    } else {
        modifier
    };

    let prefix = if component == 'z' { "GMT" } else { "" };

    if tz_offset_secs == 0 && use_z {
        return Ok(format!("{prefix}Z"));
    }

    let sign = if tz_offset_secs >= 0 { '+' } else { '-' };
    let abs_secs = tz_offset_secs.unsigned_abs() as i32;
    let hours = abs_secs / 3600;
    let mins = (abs_secs % 3600) / 60;

    let s = match mod_ {
        "0" => {
            if mins == 0 {
                format!("{prefix}{sign}{hours}")
            } else {
                format!("{prefix}{sign}{hours:}:{mins:02}")
            }
        }
        "0101" => format!("{prefix}{sign}{hours:02}{mins:02}"),
        "01:01" | "" | "Z" => format!("{prefix}{sign}{hours:02}:{mins:02}"),
        "010101" | "01:01:01" => {
            return Err(JsonataError::new(
                "D3134",
                format!("invalid picture component: [{component}{modifier}]"),
            ));
        }
        _ => format!("{prefix}{sign}{hours:02}:{mins:02}"),
    };
    Ok(s)
}

/// Format an integer using a modifier string (picture pattern like "", "1", "01", "001", "#", "9,999,*").
fn format_integer_mod(v: i64, modifier: &str) -> String {
    let modifier = modifier.trim();

    if modifier.is_empty() || modifier == "1" {
        return v.to_string();
    }

    // Grouping separator (contains comma).
    if modifier.contains(',') {
        let s = v.to_string();
        let neg = s.starts_with('-');
        let digits = if neg { &s[1..] } else { &s };
        if digits.len() > 3 {
            let chars: Vec<char> = digits.chars().collect();
            let n = chars.len();
            let mut result = String::new();
            for (i, &c) in chars.iter().enumerate() {
                if i > 0 && (n - i).is_multiple_of(3) {
                    result.push(',');
                }
                result.push(c);
            }
            return if neg { format!("-{result}") } else { result };
        }
        return s;
    }

    // Optional "#" prefix means plain numeric, no padding.
    if modifier.starts_with('#') {
        return v.to_string();
    }

    // Count leading zeros to determine padding width.
    if modifier.starts_with('0') {
        let digit_count = modifier.chars().take_while(char::is_ascii_digit).count();
        if digit_count > 0 {
            if v < 0 {
                return format!("-{:0>width$}", -v, width = digit_count);
            }
            return format!("{v:0>digit_count$}");
        }
    }

    v.to_string()
}

#[allow(clippy::unnecessary_wraps)]
fn format_integer_token(v: i64, modifier: &str) -> Result<String, JsonataError> {
    Ok(format_integer_mod(v, modifier))
}

// ── Picture-format parsing ───────────────────────────────────────────────────

#[derive(Debug)]
struct PicturePart {
    is_token: bool,
    component: char,
    modifier: String,
    literal: String,
}

#[allow(clippy::too_many_lines)]
fn parse_with_picture(input: &str, picture: &str) -> Result<Option<i64>, JsonataError> {
    let runes: Vec<char> = picture.chars().collect();
    let mut parts: Vec<PicturePart> = Vec::new();
    let mut i = 0;

    while i < runes.len() {
        if runes[i] == '[' {
            if i + 1 < runes.len() && runes[i + 1] == '[' {
                parts.push(PicturePart {
                    is_token: false,
                    component: '\0',
                    modifier: String::new(),
                    literal: "[".into(),
                });
                i += 2;
                continue;
            }
            let mut j = i + 1;
            while j < runes.len() && runes[j] != ']' {
                j += 1;
            }
            if j >= runes.len() {
                return Ok(None); // Unclosed bracket → undefined
            }
            let tok: String = runes[i + 1..j]
                .iter()
                .filter(|&&c| c != ' ' && c != '\n' && c != '\r' && c != '\t')
                .collect();
            if tok.is_empty() {
                i = j + 1;
                continue;
            }
            let comp = tok.chars().next().expect("tok is non-empty");
            if !VALID_COMPONENTS.contains(&comp) {
                return Err(JsonataError::new(
                    "D3132",
                    format!("$toMillis: unknown picture component '{comp}'"),
                ));
            }
            let mod_: String = tok.chars().skip(1).collect();
            // [YN] is invalid.
            if comp == 'Y' && mod_ == "N" {
                return Err(JsonataError::new(
                    "D3133",
                    "$toMillis: the picture string is not valid: unsupported modifier [YN]",
                ));
            }
            parts.push(PicturePart {
                is_token: true,
                component: comp,
                modifier: mod_,
                literal: String::new(),
            });
            i = j + 1;
        } else if runes[i] == ']' && i + 1 < runes.len() && runes[i + 1] == ']' {
            parts.push(PicturePart {
                is_token: false,
                component: '\0',
                modifier: String::new(),
                literal: "]".into(),
            });
            i += 2;
        } else {
            parts.push(PicturePart {
                is_token: false,
                component: '\0',
                modifier: String::new(),
                literal: runes[i].to_string(),
            });
            i += 1;
        }
    }

    // Track which components appear.
    let mut has_cal_y = false;
    let mut has_week_y = false;
    let mut has_m = false;
    let mut has_d = false;
    let mut has_doy = false;
    let mut has_h = false;
    let mut has_hour = false;
    let mut has_min = false;
    let mut has_sec = false;

    for p in &parts {
        if !p.is_token {
            continue;
        }
        match p.component {
            'Y' => has_cal_y = true,
            'X' => has_week_y = true,
            'M' => has_m = true,
            'D' => has_d = true,
            'd' => has_doy = true,
            'H' => {
                has_h = true;
                has_hour = true;
            }
            'h' => has_hour = true,
            'm' => has_min = true,
            's' => has_sec = true,
            _ => {}
        }
    }
    let _ = has_sec; // used implicitly

    // If the picture has no datetime components at all, return undefined.
    let has_any_token = parts.iter().any(|p| p.is_token);
    if !has_any_token {
        return Ok(None);
    }

    // Validation.
    if has_d && !has_m && !has_doy {
        return Err(JsonataError::new(
            "D3136",
            "$toMillis: the date/time picture is underspecified; missing month component",
        ));
    }
    if (has_min || has_sec) && !has_hour && !has_h {
        return Err(JsonataError::new(
            "D3136",
            "$toMillis: the date/time picture is underspecified; missing hour component",
        ));
    }
    if has_week_y && !has_cal_y {
        return Err(JsonataError::new(
            "D3136",
            "$toMillis: the date/time picture is underspecified; week-based year requires full calendar date",
        ));
    }

    // Parse the input using parts.
    let input_runes: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let mut year = 0i32;
    let mut month = 0u8;
    let mut day = 0u8;
    let mut hour = 0u8;
    let mut minute = 0u8;
    let mut second = 0u8;
    let mut millisec = 0i32;
    let mut day_of_year = 0u32;
    let mut tz_offset = 0i32;
    let mut is_pm = false;
    let mut is_12h = false;
    let mut has_tz = false;
    let mut has_year = false;

    for part in &parts {
        if !part.is_token {
            // Consume literal.
            for lr in part.literal.chars() {
                if pos >= input_runes.len() {
                    return Ok(None);
                }
                if input_runes[pos].to_lowercase().next() != lr.to_lowercase().next() {
                    return Ok(None);
                }
                pos += 1;
            }
            continue;
        }

        match part.component {
            'Y' | 'X' => {
                has_year = true;
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                year = v as i32;
                pos += n as usize;
            }
            'M' => {
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                month = v as u8;
                pos += n as usize;
            }
            'D' => {
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                day = v as u8;
                pos += n as usize;
            }
            'd' => {
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                day_of_year = v as u32;
                pos += n as usize;
            }
            'H' => {
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                hour = v as u8;
                pos += n as usize;
            }
            'h' => {
                is_12h = true;
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                hour = v as u8;
                pos += n as usize;
            }
            'm' => {
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                minute = v as u8;
                pos += n as usize;
            }
            's' => {
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                second = v as u8;
                pos += n as usize;
            }
            'f' => {
                let (v, n) = parse_token_value(&input_runes[pos..], &part.modifier);
                if n < 0 {
                    return Ok(None);
                }
                // Normalize to milliseconds.
                let ms_val = normalize_frac_to_ms(v, n as usize);
                millisec = ms_val;
                pos += n as usize;
            }
            'P' => {
                if pos + 2 > input_runes.len() {
                    return Ok(None);
                }
                let s2: String = input_runes[pos..pos + 2].iter().collect();
                match s2.to_lowercase().as_str() {
                    "am" => {
                        is_pm = false;
                        pos += 2;
                    }
                    "pm" => {
                        is_pm = true;
                        pos += 2;
                    }
                    _ => return Ok(None),
                }
            }
            'F' => {
                let n = consume_name_or_number(&input_runes[pos..], &part.modifier);
                if n > 0 {
                    pos += n;
                }
            }
            'Z' | 'z' => {
                let (offset, n) = parse_tz_from_input(&input_runes[pos..], part.component);
                if n > 0 {
                    tz_offset = offset;
                    has_tz = true;
                    pos += n;
                }
            }
            'W' | 'w' | 'x' => {
                let n = consume_name_or_number(&input_runes[pos..], &part.modifier);
                if n > 0 {
                    pos += n;
                }
            }
            _ => {}
        }
    }

    // Adjust 12-hour clock.
    if is_12h {
        if is_pm && hour != 12 {
            hour += 12;
        } else if !is_pm && hour == 12 {
            hour = 0;
        }
    }

    // If no year seen, use today for time-only pictures.
    if !has_year && year == 0 && (has_hour || has_min) {
        let now = jiff::Zoned::now();
        let ts = now.timestamp().as_millisecond();
        let (y, mo, d, _, _, _) = secs_to_ymd_hms(ts / 1000);
        year = y;
        month = mo;
        day = d;
    }

    if day_of_year > 0 {
        let ms = date_to_ms_with_doy(year, day_of_year, hour, minute, second, millisec)?;
        let ms_utc = ms - i64::from(tz_offset) * 1000;
        return Ok(Some(ms_utc));
    }

    if month == 0 {
        month = 1;
    }
    if day == 0 {
        day = 1;
    }

    let ms = calendar_to_ms(year, month, day, hour, minute, second, millisec)?;
    let ms_utc = if has_tz {
        ms - i64::from(tz_offset) * 1000
    } else {
        ms
    };
    Ok(Some(ms_utc))
}

fn normalize_frac_to_ms(v: i64, n: usize) -> i32 {
    let mut val = v;
    if n < 3 {
        for _ in n..3 {
            val *= 10;
        }
    } else if n > 3 {
        for _ in 3..n {
            val /= 10;
        }
    }
    val as i32
}

fn calendar_to_ms(
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    ms: i32,
) -> Result<i64, JsonataError> {
    let y = year as i16;
    let dt = jiff::civil::datetime(
        y,
        month as i8,
        day as i8,
        hour as i8,
        minute as i8,
        second as i8,
        ms * 1_000_000,
    );
    let ts = dt
        .to_zoned(jiff::tz::TimeZone::UTC)
        .map_err(|e| JsonataError::new("D3137", format!("invalid date: {e}")))?
        .timestamp();
    Ok(ts.as_millisecond())
}

fn date_to_ms_with_doy(
    year: i32,
    doy: u32,
    hour: u8,
    minute: u8,
    second: u8,
    ms: i32,
) -> Result<i64, JsonataError> {
    // Start from Jan 1 of year, add (doy - 1) days.
    let y = year as i16;
    let dt = jiff::civil::datetime(
        y,
        1i8,
        1i8,
        hour as i8,
        minute as i8,
        second as i8,
        ms * 1_000_000,
    );
    let ts = dt
        .to_zoned(jiff::tz::TimeZone::UTC)
        .map_err(|e| JsonataError::new("D3137", format!("invalid date: {e}")))?;
    // Add (doy - 1) days.
    let final_ts = ts
        .checked_add(jiff::Span::new().days(i64::from(doy - 1)))
        .map_err(|e| JsonataError::new("D3137", format!("date overflow: {e}")))?;
    Ok(final_ts.timestamp().as_millisecond())
}

// ── Token value parsing ──────────────────────────────────────────────────────

fn parse_token_value(runes: &[char], modifier: &str) -> (i64, i64) {
    if runes.is_empty() {
        return (-1, -1);
    }
    if modifier == "I" || modifier == "i" {
        return parse_roman(runes);
    }
    if modifier == "a" || modifier == "A" {
        return parse_alphabetic(runes, modifier);
    }
    if modifier == "N"
        || modifier == "n"
        || modifier == "Nn"
        || modifier.starts_with("Nn")
        || modifier.starts_with('N')
    {
        return parse_month_name(runes, modifier);
    }
    if modifier == "w"
        || modifier == "W"
        || modifier.starts_with("wo")
        || modifier.starts_with("Wo")
        || modifier.starts_with("Ww")
        || modifier.starts_with("ww")
    {
        return parse_word_number(runes);
    }
    if modifier.ends_with('o') {
        return parse_ordinal_number(runes);
    }
    parse_numeric_value(runes, modifier)
}

fn parse_numeric_value(runes: &[char], modifier: &str) -> (i64, i64) {
    let mut i = 0;
    let sign: i64 = if !runes.is_empty() && runes[0] == '-' {
        i += 1;
        -1
    } else {
        1
    };
    let start = i;
    let max_w = modifier_field_width(modifier);
    while i < runes.len() && runes[i].is_ascii_digit() {
        if max_w > 0 && (i - start) >= max_w {
            break;
        }
        i += 1;
    }
    if i == start {
        return (-1, -1);
    }
    let s: String = runes[start..i].iter().collect();
    match s.parse::<i64>() {
        Ok(n) => (sign * n, i as i64),
        Err(_) => (-1, -1),
    }
}

fn modifier_field_width(modifier: &str) -> usize {
    if modifier.is_empty() {
        return 0;
    }
    if modifier.starts_with(',') {
        if let Some(dash) = modifier.rfind('-') {
            let part = &modifier[dash + 1..];
            if let Ok(n) = part.parse::<usize>()
                && n > 0
            {
                return n;
            }
        }
        return 0;
    }
    if modifier.len() < 2 {
        return 0;
    }
    if modifier.chars().all(|c| c == '0' || c == '1') {
        return modifier.len();
    }
    0
}

fn parse_ordinal_number(runes: &[char]) -> (i64, i64) {
    let mut i = 0;
    while i < runes.len() && runes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return (-1, -1);
    }
    let s: String = runes[..i].iter().collect();
    let n: i64 = match s.parse() {
        Ok(v) => v,
        Err(_) => return (-1, -1),
    };
    // Consume optional ordinal suffix (st/nd/rd/th).
    if i + 2 <= runes.len() {
        let suffix: String = runes[i..i + 2].iter().collect();
        match suffix.to_lowercase().as_str() {
            "st" | "nd" | "rd" | "th" => {
                i += 2;
            }
            _ => {}
        }
    }
    (n, i as i64)
}

fn parse_roman(runes: &[char]) -> (i64, i64) {
    let roman_val = |c: char| -> Option<i64> {
        match c.to_uppercase().next().unwrap_or(c) {
            'I' => Some(1),
            'V' => Some(5),
            'X' => Some(10),
            'L' => Some(50),
            'C' => Some(100),
            'D' => Some(500),
            'M' => Some(1000),
            _ => None,
        }
    };
    let mut i = 0;
    while i < runes.len() && roman_val(runes[i]).is_some() {
        i += 1;
    }
    if i == 0 {
        return (-1, -1);
    }
    let mut total: i64 = 0;
    let mut prev: i64 = 0;
    for j in (0..i).rev() {
        let v = roman_val(runes[j]).expect("loop only covers validated roman chars");
        if v < prev {
            total -= v;
        } else {
            total += v;
            prev = v;
        }
    }
    (total, i as i64)
}

fn parse_alphabetic(runes: &[char], modifier: &str) -> (i64, i64) {
    let base = if modifier == "A" { 'A' } else { 'a' };
    let mut i = 0;
    let mut result: i64 = 0;
    while i < runes.len() && runes[i].is_ascii_alphabetic() {
        let c = runes[i].to_lowercase().next().expect("to_lowercase always yields at least one char");
        let digit = (c as i64) - ('a' as i64) + 1;
        result = result * 26 + digit;
        i += 1;
    }
    if i == 0 {
        return (-1, -1);
    }
    let _ = base;
    (result, i as i64)
}

fn parse_month_name(runes: &[char], modifier: &str) -> (i64, i64) {
    let max_len = parse_name_max_len(modifier);
    for (mi, &name) in MONTH_NAMES.iter().enumerate() {
        if max_len > 0 && max_len < name.chars().count() {
            let abbr: String = name.chars().take(max_len).collect();
            if runes.len() >= max_len {
                let s: String = runes[..max_len].iter().collect();
                if s.to_lowercase() == abbr.to_lowercase() {
                    return ((mi + 1) as i64, max_len as i64);
                }
            }
        } else {
            let name_len = name.chars().count();
            if runes.len() >= name_len {
                let s: String = runes[..name_len].iter().collect();
                if s.to_lowercase() == name.to_lowercase() {
                    return ((mi + 1) as i64, name_len as i64);
                }
            }
        }
    }
    (-1, -1)
}

fn parse_name_max_len(modifier: &str) -> usize {
    if modifier.contains(',') {
        let parts: Vec<&str> = modifier.splitn(2, ',').collect();
        if parts.len() == 2 {
            let range_part = parts[1];
            let range_parts: Vec<&str> = range_part.split('-').collect();
            if let Some(last) = range_parts.last()
                && let Ok(v) = last.parse::<usize>()
            {
                return v;
            }
            if let Ok(v) = range_parts[0].parse::<usize>() {
                return v;
            }
        }
    }
    0
}

fn parse_word_number(runes: &[char]) -> (i64, i64) {
    let s: String = runes.iter().collect();
    match parse_word_number_from_string(&s) {
        (consumed, val) if consumed > 0 => (val, consumed as i64),
        _ => (-1, -1),
    }
}

fn consume_name_or_number(runes: &[char], modifier: &str) -> usize {
    if runes.is_empty() {
        return 0;
    }
    // Try weekday name match.
    for name in WEEKDAY_NAMES {
        if modifier == "N"
            || modifier == "n"
            || modifier == "Nn"
            || modifier.starts_with("Nn")
            || modifier.starts_with('N')
        {
            let max_len = parse_name_max_len(modifier);
            let name_runes: Vec<char> = name.chars().collect();
            if max_len > 0 && max_len < name_runes.len() {
                let abbr: String = name_runes[..max_len].iter().collect();
                if runes.len() >= max_len {
                    let s: String = runes[..max_len].iter().collect();
                    if s.to_lowercase() == abbr.to_lowercase() {
                        return max_len;
                    }
                }
            } else if runes.len() >= name_runes.len() {
                let s: String = runes[..name_runes.len()].iter().collect();
                if s.to_lowercase() == name.to_lowercase() {
                    return name_runes.len();
                }
            }
        }
    }
    // Numeric fallback.
    let mut i = 0;
    while i < runes.len() && runes[i].is_ascii_digit() {
        i += 1;
    }
    i
}

fn parse_tz_from_input(runes: &[char], component: char) -> (i32, usize) {
    if runes.is_empty() {
        return (0, 0);
    }
    let s: String = runes.iter().collect();

    // Handle GMT prefix (z component).
    if (component == 'z' || s.starts_with("GMT")) && s.starts_with("GMT") {
        let rest: Vec<char> = runes[3..].to_vec();
        let (offset, n) = parse_tz_from_input(&rest, 'Z');
        return (offset, 3 + n);
    }

    if runes[0] == 'Z' {
        return (0, 1);
    }

    let sign: i32 = match runes[0] {
        '+' => 1,
        '-' => -1,
        _ => return (0, 0),
    };
    let mut i = 1;

    let h_start = i;
    while i < runes.len() && runes[i].is_ascii_digit() && i - h_start < 2 {
        i += 1;
    }
    if i == h_start {
        return (0, 0);
    }
    let hours: i32 = runes[h_start..i]
        .iter()
        .collect::<String>()
        .parse()
        .unwrap_or(0);

    // Optional colon.
    if i < runes.len() && runes[i] == ':' {
        i += 1;
    }

    let m_start = i;
    while i < runes.len() && runes[i].is_ascii_digit() && i - m_start < 2 {
        i += 1;
    }
    let mins: i32 = if i > m_start {
        runes[m_start..i]
            .iter()
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    } else {
        0
    };

    (sign * (hours * 3600 + mins * 60), i)
}

// ── Word number parsing ──────────────────────────────────────────────────────

fn parse_word_number_from_string(s: &str) -> (usize, i64) {
    let s_lower = s.to_lowercase();
    let ones = &[
        "zero",
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
    let tens = &[
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    let (consumed, val) = parse_complex_number(&s_lower, ones, tens);
    (consumed, val)
}

struct WordParser<'a> {
    s: &'a str,
    pos: usize,
    ones: &'a [&'a str; 20],
    tens: &'a [&'a str; 10],
}

impl WordParser<'_> {
    fn skip_sep(&mut self) {
        while self.pos < self.s.len() {
            let c = self.s[self.pos..].chars().next();
            if c == Some(' ') || c == Some(',') {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.s[self.pos..].starts_with("and ") {
            self.pos += 4;
        }
    }

    fn try_word_or_ordinal(&mut self, word: &str, ordinal: &str) -> bool {
        let save = self.pos;
        self.skip_sep();
        for candidate in &[word, ordinal] {
            if candidate.is_empty() {
                continue;
            }
            if !self.s[self.pos..].starts_with(candidate) {
                continue;
            }
            let after = &self.s[self.pos + candidate.len()..];
            if after.is_empty()
                || !after
                    .chars()
                    .next()
                    .is_some_and(char::is_alphabetic)
            {
                self.pos += candidate.len();
                return true;
            }
        }
        self.pos = save;
        false
    }

    fn try_word(&mut self, word: &str) -> bool {
        self.try_word_or_ordinal(word, "")
    }

    fn parse_sub100(&mut self) -> Option<i64> {
        let save = self.pos;
        // Teens/ones (19 down to 10).
        let ones_ordinals = [
            "zeroth",
            "first",
            "second",
            "third",
            "fourth",
            "fifth",
            "sixth",
            "seventh",
            "eighth",
            "ninth",
            "tenth",
            "eleventh",
            "twelfth",
            "thirteenth",
            "fourteenth",
            "fifteenth",
            "sixteenth",
            "seventeenth",
            "eighteenth",
            "nineteenth",
        ];
        for i in (10..=19usize).rev() {
            let ord = if i < ones_ordinals.len() {
                ones_ordinals[i]
            } else {
                ""
            };
            if self.try_word_or_ordinal(self.ones[i], ord) {
                return Some(i as i64);
            }
        }
        // Tens (ninety down to twenty).
        let tens_ordinals = [
            "",
            "",
            "twentieth",
            "thirtieth",
            "fortieth",
            "fiftieth",
            "sixtieth",
            "seventieth",
            "eightieth",
            "ninetieth",
        ];
        for i in (2..=9usize).rev() {
            let ord = if i < tens_ordinals.len() {
                tens_ordinals[i]
            } else {
                ""
            };
            if self.try_word_or_ordinal(self.tens[i], ord) {
                let v = i as i64 * 10;
                let dash_save = self.pos;
                if self.pos < self.s.len() && self.s[self.pos..].starts_with('-') {
                    self.pos += 1;
                }
                let ones_ordinals2 = [
                    "", "first", "second", "third", "fourth", "fifth", "sixth", "seventh",
                    "eighth", "ninth",
                ];
                for j in (1..=9usize).rev() {
                    let o2 = if j < ones_ordinals2.len() {
                        ones_ordinals2[j]
                    } else {
                        ""
                    };
                    if self.try_word_or_ordinal(self.ones[j], o2) {
                        return Some(v + j as i64);
                    }
                }
                self.pos = dash_save;
                return Some(v);
            }
        }
        // Ones (nine down to one).
        for i in (1..=9usize).rev() {
            let ord = if i < ones_ordinals.len() {
                ones_ordinals[i]
            } else {
                ""
            };
            if self.try_word_or_ordinal(self.ones[i], ord) {
                return Some(i as i64);
            }
        }
        self.pos = save;
        None
    }

    fn parse_sub1000(&mut self) -> Option<i64> {
        let save = self.pos;
        for i in (1..=9usize).rev() {
            if self.try_word(self.ones[i]) {
                if self.try_word_or_ordinal("hundred", "hundredth") {
                    let v = i as i64 * 100;
                    let rem = self.parse_sub100().unwrap_or(0);
                    return Some(v + rem);
                }
                self.pos = save;
                break;
            }
        }
        self.parse_sub100()
    }
}

fn parse_complex_number(s: &str, ones: &[&str; 20], tens: &[&str; 10]) -> (usize, i64) {
    let mut p = WordParser {
        s,
        pos: 0,
        ones,
        tens,
    };
    let save = p.pos;

    // Try "nineteen hundred" style.
    for i in (1..=19usize).rev() {
        if p.try_word(p.ones[i]) {
            if p.try_word_or_ordinal("hundred", "hundredth") {
                let total = i as i64 * 100;
                let rem = p.parse_sub100().unwrap_or(0);
                return (p.pos, total + rem);
            }
            p.pos = save;
            break;
        }
    }

    // Try "X thousand, Y hundred and Z".
    if let Some(thousand_part) = p.parse_sub1000() {
        if p.try_word_or_ordinal("thousand", "thousandth") {
            let mut total = thousand_part * 1000;
            if let Some(rest) = p.parse_sub1000() {
                total += rest;
            }
            return (p.pos, total);
        }
        return (p.pos, thousand_part);
    }

    (0, 0)
}

// ── Number formatting helpers ────────────────────────────────────────────────

const WEEKDAY_NAMES: &[&str] = &[
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTH_NAMES: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const VALID_COMPONENTS: &[char] = &[
    'Y', 'M', 'D', 'd', 'H', 'h', 'm', 's', 'f', 'F', 'Z', 'z', 'P', 'C', 'E', 'W', 'w', 'X', 'x',
];

fn to_roman(n: i64, upper: bool) -> String {
    if n <= 0 {
        return String::new();
    }
    let vals: &[i64] = &[1000, 900, 500, 400, 100, 90, 50, 40, 10, 9, 5, 4, 1];
    let syms: &[&str] = &[
        "M", "CM", "D", "CD", "C", "XC", "L", "XL", "X", "IX", "V", "IV", "I",
    ];
    let mut n = n;
    let mut s = String::new();
    for (i, &v) in vals.iter().enumerate() {
        while n >= v {
            s.push_str(syms[i]);
            n -= v;
        }
    }
    if upper { s } else { s.to_lowercase() }
}

fn to_alphabetic(n: i64, base: char) -> String {
    if n <= 0 {
        return String::new();
    }
    let mut n = n;
    let mut result: Vec<char> = Vec::new();
    while n > 0 {
        n -= 1;
        result.insert(
            0,
            char::from_u32(base as u32 + (n % 26) as u32).unwrap_or('?'),
        );
        n /= 26;
    }
    result.iter().collect()
}

fn int_to_words(n: i64) -> String {
    fn below_thousand(n: i64, ones: &[&str], tens: &[&str]) -> String {
        if n == 0 {
            return String::new();
        }
        if n < 20 {
            return ones[n as usize].to_string();
        }
        if n < 100 {
            return if n % 10 == 0 {
                tens[(n / 10) as usize].to_string()
            } else {
                format!("{}-{}", tens[(n / 10) as usize], ones[(n % 10) as usize])
            };
        }
        let rem = n % 100;
        if rem == 0 {
            format!("{} hundred", ones[(n / 100) as usize])
        } else {
            format!(
                "{} hundred and {}",
                ones[(n / 100) as usize],
                below_thousand(rem, ones, tens)
            )
        }
    }

    fn to_words(n: i64, ones: &[&str], tens: &[&str]) -> String {
        use std::fmt::Write;
        if n == 0 {
            return String::new();
        }
        if n < 1000 {
            return below_thousand(n, ones, tens);
        }
        let scales: &[(i64, &str)] = &[
            (1_000_000_000_000, "trillion"),
            (1_000_000_000, "billion"),
            (1_000_000, "million"),
            (1_000, "thousand"),
        ];
        for &(scale, name) in scales {
            if n >= scale {
                let q = n / scale;
                let rem = n % scale;
                let q_word = to_words(q, ones, tens);
                let mut result = format!("{q_word} {name}");
                if rem > 0 {
                    let rem_word = to_words(rem, ones, tens);
                    if rem < 100 {
                        let _ = write!(result, " and {rem_word}");
                    } else {
                        let _ = write!(result, ", {rem_word}");
                    }
                }
                return result;
            }
        }
        below_thousand(n, ones, tens)
    }

    if n == 0 {
        return "zero".to_string();
    }
    if n < 0 {
        return format!("minus {}", int_to_words(-n));
    }
    let ones = [
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
    let tens = [
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];

    to_words(n, &ones, &tens)
}

fn int_to_words_ordinal(n: i64) -> String {
    apply_ordinal_word(&int_to_words(n))
}

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
    // Find last word.
    let (prefix, sep, last) = if let Some(pos) = word.rfind([' ', '-']) {
        let sep = &word[pos..=pos];
        (&word[..pos], sep, &word[pos + 1..])
    } else {
        ("", "", word)
    };
    for &(from, to) in ordinals {
        if last == from {
            return format!("{prefix}{sep}{to}");
        }
    }
    if let Some(stem) = last.strip_suffix('y') {
        return format!("{prefix}{sep}{stem}ieth");
    }
    format!("{prefix}{sep}{last}th")
}

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

// ── Calendar math (pure arithmetic, no external crate) ──────────────────────

/// Convert Unix seconds to (year, month, day, hour, minute, second).
fn secs_to_ymd_hms(secs: i64) -> (i32, u8, u8, u8, u8, u8) {
    let (date_days, time_secs) = if secs >= 0 {
        (secs / 86400, secs % 86400)
    } else {
        let d = (secs + 1) / 86400 - 1;
        let t = secs - d * 86400;
        (d, t)
    };

    let h = (time_secs / 3600) as u8;
    let mi = ((time_secs % 3600) / 60) as u8;
    let s = (time_secs % 60) as u8;

    let (y, mo, d) = days_to_ymd(date_days);
    (y, mo, d, h, mi, s)
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: i64) -> (i32, u8, u8) {
    // Algorithm: civil date from epoch days.
    // Based on Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u8, d as u8)
}

fn is_leap_year(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

#[allow(dead_code)]
fn days_in_month(y: i32, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn day_of_year(y: i32, mo: u8, d: u8) -> u32 {
    let months: &[u8] = &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut doy = u32::from(d);
    for (m, &days) in months[..mo as usize - 1].iter().enumerate() {
        doy += u32::from(days);
        if m == 1 && is_leap_year(y) {
            doy += 1;
        }
    }
    doy
}

/// Day of week: 0=Sunday, 1=Monday, ..., 6=Saturday.
fn day_of_week(y: i32, m: u8, d: u8) -> u8 {
    // Tomohiko Sakamoto's algorithm.
    let t: &[i32] = &[0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + t[m as usize - 1] + i32::from(d)).rem_euclid(7)) as u8
}

/// ISO week number: returns (iso_year, iso_week).
fn iso_week(y: i32, m: u8, d: u8) -> (i32, u32) {
    // ISO 8601 week date: weeks start on Monday, week 1 contains the year's first Thursday.
    // Formula: week = (ordinalDay - isoDow + 10) / 7
    //   where isoDow: Mon=1..Sun=7
    let doy = day_of_year(y, m, d) as i32;
    let dow = i32::from(day_of_week(y, m, d)); // 0=Sun..6=Sat
    let dow_iso1 = (dow + 6) % 7 + 1; // Mon=1..Sun=7
    let week = (doy - dow_iso1 + 10) / 7;
    if week < 1 {
        // Belongs to last week of previous year.
        let prev_y = y - 1;
        return (prev_y, iso_weeks_in_year(prev_y));
    }
    let max_week = iso_weeks_in_year(y);
    if week > max_week as i32 {
        return (y + 1, 1);
    }
    (y, week as u32)
}

/// Returns the number of ISO weeks in a given year (52 or 53).
fn iso_weeks_in_year(y: i32) -> u32 {
    // A year has 53 weeks if and only if Dec 31 is a Thursday,
    // or Dec 30 is a Thursday (which happens in leap years).
    // Equivalently: Jan 1 is Thursday, or Dec 31 is Thursday.
    let jan1_dow = day_of_week(y, 1, 1); // 0=Sun..6=Sat
    let dec31_dow = day_of_week(y, 12, 31);
    // Thursday = 4 in our system (0=Sun)
    if jan1_dow == 4 || dec31_dow == 4 {
        53
    } else {
        52
    }
}

/// Returns the Thursday of the ISO week for a given date.
fn iso_week_thursday(y: i32, m: u8, d: u8) -> (i32, u8, u8) {
    // Thursday is dow_iso = 3 (Mon=0..Sun=6).
    let dow_iso = (i32::from(day_of_week(y, m, d)) + 6) % 7;
    let offset = 3 - dow_iso; // days to add to reach Thursday
    add_days(y, m, d, offset)
}

fn add_days(y: i32, m: u8, d: u8, delta: i32) -> (i32, u8, u8) {
    let total_days = ymd_to_epoch_days(y, m, d) + i64::from(delta);
    let (ny, nm, nd) = days_to_ymd(total_days);
    (ny, nm, nd)
}

fn ymd_to_epoch_days(y: i32, m: u8, d: u8) -> i64 {
    // Inverse of days_to_ymd (Howard Hinnant).
    let m = i32::from(m);
    let d = i32::from(d);
    let y = if m <= 2 { i64::from(y) - 1 } else { i64::from(y) };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as u64 + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn week_of_month(_thy: i32, _thm: u8, thd: u8) -> u32 {
    u32::from(thd).div_ceil(7)
}

// ── Timezone parsing ─────────────────────────────────────────────────────────

/// Parse timezone string to offset in seconds.
/// Accepts: "UTC", "America/New_York", "+05:30", "-05:00", "+0530", "-0500".
fn parse_tz(s: &str) -> Result<i32, JsonataError> {
    // Try named timezone via jiff.
    if let Ok(tz) = jiff::tz::TimeZone::get(s) {
        // For fixed-name TZs, get the offset at epoch 0 as approximation.
        // For proper named TZs we convert a known timestamp.
        let ts = jiff::Timestamp::new(0, 0).expect("epoch 0 is always valid");
        let offset = tz.to_fixed_offset().map_or(0, jiff::tz::Offset::seconds);
        let _ = ts;
        return Ok(offset);
    }
    // Try numeric offset.
    parse_numeric_tz(s).map_err(|_| JsonataError::new("D3137", format!("unknown timezone {s:?}")))
}

fn parse_numeric_tz(s: &str) -> Result<i32, String> {
    if s.is_empty() {
        return Err("empty".into());
    }
    let (sign, rest) = match s.as_bytes()[0] {
        b'+' => (1i32, &s[1..]),
        b'-' => (-1i32, &s[1..]),
        _ => {
            // Try "0000" (treat as positive).
            if s.chars().all(|c| c.is_ascii_digit() || c == ':') {
                (1i32, s)
            } else {
                return Err(format!("bad tz: {s}"));
            }
        }
    };
    let rest = rest.replace(':', "");
    if rest.len() != 4 {
        return Err(format!("bad tz len: {rest}"));
    }
    let h: i32 = rest[..2].parse().map_err(|_| "bad h".to_string())?;
    let m: i32 = rest[2..].parse().map_err(|_| "bad m".to_string())?;
    Ok(sign * (h * 3600 + m * 60))
}

// ── Value extraction helper ──────────────────────────────────────────────────

fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn millis_val(ms: i64) -> Value {
        Value::Number(ms as f64)
    }
    fn str_val(s: &str) -> Value {
        Value::String(s.into())
    }

    fn from_millis_1arg(ms: i64) -> String {
        match fn_from_millis(&[millis_val(ms)], &Value::Undefined) {
            Ok(Value::String(s)) => s.to_string(),
            other => panic!("expected string, got {:?}", other),
        }
    }

    fn from_millis_picture(ms: i64, picture: &str) -> String {
        match fn_from_millis(&[millis_val(ms), str_val(picture)], &Value::Undefined) {
            Ok(Value::String(s)) => s.to_string(),
            other => panic!("expected string, got {:?}", other),
        }
    }

    fn from_millis_picture_tz(ms: i64, picture: &str, tz: &str) -> String {
        match fn_from_millis(
            &[millis_val(ms), str_val(picture), str_val(tz)],
            &Value::Undefined,
        ) {
            Ok(Value::String(s)) => s.to_string(),
            other => panic!("expected string, got {:?}", other),
        }
    }

    fn to_millis_1arg(s: &str) -> i64 {
        match fn_to_millis(&[str_val(s)], &Value::Undefined) {
            Ok(Value::Number(n)) => n as i64,
            other => panic!("expected number, got {:?}", other),
        }
    }

    fn to_millis_picture(s: &str, picture: &str) -> Option<i64> {
        match fn_to_millis(&[str_val(s), str_val(picture)], &Value::Undefined) {
            Ok(Value::Number(n)) => Some(n as i64),
            Ok(Value::Undefined) => None,
            other => panic!("expected number or undefined, got {:?}", other),
        }
    }

    // ── $fromMillis basic ────────────────────────────────────────────

    #[test]
    fn test_from_millis_epoch_plus_1() {
        assert_eq!(from_millis_1arg(1), "1970-01-01T00:00:00.001Z");
    }

    #[test]
    fn test_from_millis_known_timestamp() {
        assert_eq!(from_millis_1arg(1509380732935), "2017-10-30T16:25:32.935Z");
    }

    #[test]
    fn test_from_millis_undefined_input() {
        let result = fn_from_millis(&[Value::Undefined], &Value::Undefined);
        assert!(matches!(result, Ok(Value::Undefined)));
    }

    #[test]
    fn test_from_millis_picture_year() {
        assert_eq!(
            from_millis_picture(1521801216617, "Year: [Y0001]"),
            "Year: 2018"
        );
    }

    #[test]
    fn test_from_millis_picture_date() {
        assert_eq!(
            from_millis_picture(1521801216617, "[Y0001]-[M01]-[D01]"),
            "2018-03-23"
        );
    }

    #[test]
    fn test_from_millis_picture_datetime_us() {
        assert_eq!(
            from_millis_picture(1521801216617, "[M01]/[D01]/[Y0001] at [H01]:[m01]:[s01]"),
            "03/23/2018 at 10:33:36"
        );
    }

    #[test]
    fn test_from_millis_picture_iso_with_frac_and_tz() {
        assert_eq!(
            from_millis_picture(
                1521801216617,
                "[Y]-[M01]-[D01]T[H01]:[m]:[s].[f001][Z01:01t]"
            ),
            "2018-03-23T10:33:36.617Z"
        );
    }

    #[test]
    fn test_from_millis_picture_tz_bst() {
        assert_eq!(
            from_millis_picture_tz(
                1521801216617,
                "[Y]-[M01]-[D01]T[H01]:[m]:[s].[f001][Z0101t]",
                "+0100"
            ),
            "2018-03-23T11:33:36.617+0100"
        );
    }

    #[test]
    fn test_from_millis_picture_tz_minus5() {
        assert_eq!(
            from_millis_picture_tz(1531310400000, "[Y]-[M01]-[D01]T[H01]:[m]:[s][Z]", "-0500"),
            "2018-07-11T07:00:00-05:00"
        );
    }

    #[test]
    fn test_from_millis_picture_tz_z_modifier() {
        assert_eq!(
            from_millis_picture(1531310400000, "[Y]-[M01]-[D01]T[H01]:[m]:[s][Z01:01t]"),
            "2018-07-11T12:00:00Z"
        );
    }

    #[test]
    fn test_from_millis_picture_roman_year() {
        assert_eq!(
            from_millis_picture(1521801216617, "[D1] [M01] [YI]"),
            "23 03 MMXVIII"
        );
    }

    #[test]
    fn test_from_millis_picture_ordinal() {
        assert_eq!(
            from_millis_picture(1521801216617, "[D1o] [M01] [Y]"),
            "23rd 03 2018"
        );
    }

    #[test]
    fn test_from_millis_picture_year_words() {
        assert_eq!(
            from_millis_picture(1521801216617, "[Yw]"),
            "two thousand and eighteen"
        );
    }

    #[test]
    fn test_from_millis_picture_month_name() {
        assert_eq!(
            from_millis_picture(1521801216617, "[D1o] [MNn] [Y]"),
            "23rd March 2018"
        );
    }

    #[test]
    fn test_from_millis_picture_weekday_name() {
        assert_eq!(
            from_millis_picture(1521801216617, "[FNn], [D1o] [MNn] [Y]"),
            "Friday, 23rd March 2018"
        );
    }

    #[test]
    fn test_from_millis_picture_default_modifiers() {
        assert_eq!(
            from_millis_picture(1521801216617, "[F], [D]/[M]/[Y] [h]:[m]:[s] [P]"),
            "friday, 23/3/2018 10:33:36 am"
        );
    }

    #[test]
    fn test_from_millis_picture_ampm_uppercase() {
        assert_eq!(
            from_millis_picture(1521801216617, "[F], [D]/[M]/[Y] [h]:[m]:[s] [PN]"),
            "friday, 23/3/2018 10:33:36 AM"
        );
    }

    #[test]
    fn test_from_millis_picture_day_of_year() {
        assert_eq!(
            from_millis_picture(1514808000000, "[dwo] day of the year"),
            "first day of the year"
        );
    }

    #[test]
    fn test_from_millis_picture_week_of_year() {
        assert_eq!(from_millis_picture(1514808000000, "Week: [W]"), "Week: 1");
    }

    #[test]
    fn test_from_millis_picture_week_of_month() {
        assert_eq!(
            from_millis_picture(1359460800000, "Week: [w] of [xNn]"),
            "Week: 5 of January"
        );
    }

    #[test]
    fn test_from_millis_error_unclosed_bracket() {
        let result = fn_from_millis(
            &[millis_val(1419940800000), str_val("[YN]-[M")],
            &Value::Undefined,
        );
        assert!(
            matches!(result, Err(ref e) if e.code == "D3135"),
            "got {:?}",
            result
        );
    }

    #[test]
    fn test_from_millis_error_named_year() {
        let result = fn_from_millis(
            &[millis_val(1419940800000), str_val("[YN]-[M]-[D]")],
            &Value::Undefined,
        );
        assert!(
            matches!(result, Err(ref e) if e.code == "D3133"),
            "got {:?}",
            result
        );
    }

    #[test]
    fn test_from_millis_picture_tz_6digit_error() {
        let result = fn_from_millis(
            &[
                millis_val(1230757500000),
                str_val("[Y]-[M01]-[D01]T[H01]:[m]:[s].[f001][Z010101t]"),
                str_val("+0530"),
            ],
            &Value::Undefined,
        );
        assert!(
            matches!(result, Err(ref e) if e.code == "D3134"),
            "got {:?}",
            result
        );
    }

    // ── $toMillis basic ──────────────────────────────────────────────

    #[test]
    fn test_to_millis_iso8601() {
        assert_eq!(to_millis_1arg("1970-01-01T00:00:00.001Z"), 1);
    }

    #[test]
    fn test_to_millis_known_timestamp() {
        assert_eq!(to_millis_1arg("2017-10-30T16:25:32.935Z"), 1509380732935);
    }

    #[test]
    fn test_to_millis_undefined_input() {
        let result = fn_to_millis(&[Value::Undefined], &Value::Undefined);
        assert!(matches!(result, Ok(Value::Undefined)));
    }

    #[test]
    fn test_to_millis_picture_year() {
        assert_eq!(to_millis_picture("2018", "[Y1]"), Some(1514764800000));
    }

    #[test]
    fn test_to_millis_picture_ymd() {
        assert_eq!(
            to_millis_picture("2018-03-27", "[Y1]-[M01]-[D01]"),
            Some(1522108800000)
        );
    }

    #[test]
    fn test_to_millis_picture_iso_format() {
        assert_eq!(
            to_millis_picture(
                "2018-03-27T14:03:00.123Z",
                "[Y0001]-[M01]-[D01]T[H01]:[m01]:[s01].[f001]Z"
            ),
            Some(1522159380123)
        );
    }

    #[test]
    fn test_to_millis_picture_ordinal() {
        assert_eq!(
            to_millis_picture("27th 3 1976", "[D1o] [M#1] [Y0001]"),
            Some(196732800000)
        );
    }

    #[test]
    fn test_to_millis_picture_roman_year() {
        assert_eq!(to_millis_picture("MCMLXXXIV", "[YI]"), Some(441763200000));
    }

    #[test]
    fn test_to_millis_picture_month_name() {
        assert_eq!(
            to_millis_picture("27th April 2008", "[D1o] [MNn] [Y0001]"),
            Some(1209254400000)
        );
    }

    #[test]
    fn test_to_millis_picture_words() {
        assert_eq!(
            to_millis_picture("one thousand, nine hundred and eighty-four", "[Yw]"),
            Some(441763200000)
        );
    }

    #[test]
    fn test_to_millis_picture_12h_am() {
        assert_eq!(
            to_millis_picture("4/4/2018 12:06 am", "[D1]/[M1]/[Y0001] [h]:[m] [P]"),
            Some(1522800360000)
        );
    }

    #[test]
    fn test_to_millis_picture_day_of_year() {
        assert_eq!(
            to_millis_picture("2018-094", "[Y0001]-[d001]"),
            Some(1522800000000)
        );
    }

    #[test]
    fn test_to_millis_picture_timezone() {
        let result = to_millis_picture(
            "2020-09-09 08:00:00 +02:00",
            "[Y0001]-[M01]-[D01] [H01]:[m01]:[s01] [Z]",
        );
        // 2020-09-09 08:00:00 +02:00 = 2020-09-09 06:00:00 UTC
        let ts = fn_from_millis(&[Value::Number(result.unwrap() as f64)], &Value::Undefined);
        assert!(
            matches!(ts, Ok(Value::String(ref s)) if s.starts_with("2020-09-09T06:00:00")),
            "got {:?}",
            ts
        );
    }

    #[test]
    fn test_to_millis_picture_error_unknown_component() {
        let result = fn_to_millis(
            &[str_val("2018-05-22"), str_val("[Y]-[M]-[q]")],
            &Value::Undefined,
        );
        assert!(
            matches!(result, Err(ref e) if e.code == "D3132"),
            "got {:?}",
            result
        );
    }

    #[test]
    fn test_to_millis_picture_error_named_year() {
        let result = fn_to_millis(
            &[str_val("2018-05-22"), str_val("[YN]-[M]-[D]")],
            &Value::Undefined,
        );
        assert!(
            matches!(result, Err(ref e) if e.code == "D3133"),
            "got {:?}",
            result
        );
    }

    #[test]
    fn test_to_millis_picture_error_no_month() {
        let result = fn_to_millis(&[str_val("2018-22"), str_val("[Y]-[D]")], &Value::Undefined);
        assert!(
            matches!(result, Err(ref e) if e.code == "D3136"),
            "got {:?}",
            result
        );
    }

    #[test]
    fn test_to_millis_picture_no_match() {
        let result = to_millis_picture("irrelevant string", "[Y]-[M]-[D]");
        assert_eq!(result, None);
    }

    // ── Calendar helpers ─────────────────────────────────────────────

    #[test]
    fn test_secs_to_ymd_hms_epoch() {
        assert_eq!(secs_to_ymd_hms(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn test_secs_to_ymd_hms_known() {
        // 2018-03-23T10:33:36
        assert_eq!(secs_to_ymd_hms(1521801216), (2018, 3, 23, 10, 33, 36));
    }

    #[test]
    fn test_secs_to_ymd_hms_negative() {
        // 1969-12-31T23:59:59 = epoch - 1 second
        assert_eq!(secs_to_ymd_hms(-1), (1969, 12, 31, 23, 59, 59));
    }

    #[test]
    fn test_day_of_week_friday() {
        // 2018-03-23 is a Friday (5 in our encoding 0=Sun,5=Fri)
        assert_eq!(day_of_week(2018, 3, 23), 5);
    }

    #[test]
    fn test_iso_week_known() {
        // 2018-01-01 is Monday, week 1
        assert_eq!(iso_week(2018, 1, 1), (2018, 1));
        // 2018-12-31 is Monday, week 1 of 2019
        assert_eq!(iso_week(2018, 12, 31), (2019, 1));
    }

    #[test]
    fn test_iso_week_all_edge_cases() {
        // From isoWeekDate.json test suite
        assert_eq!(iso_week(2005, 1, 1), (2004, 53)); // Sat 1 Jan 2005
        assert_eq!(iso_week(2005, 1, 2), (2004, 53)); // Sun 2 Jan 2005
        assert_eq!(iso_week(2005, 12, 31), (2005, 52)); // Sat 31 Dec 2005
        assert_eq!(iso_week(2006, 1, 1), (2005, 52)); // Sun 1 Jan 2006
        assert_eq!(iso_week(2006, 1, 2), (2006, 1)); // Mon 2 Jan 2006
        assert_eq!(iso_week(2006, 12, 31), (2006, 52)); // Sun 31 Dec 2006
        assert_eq!(iso_week(2007, 1, 1), (2007, 1)); // Mon 1 Jan 2007
        assert_eq!(iso_week(2007, 12, 30), (2007, 52)); // Sun 30 Dec 2007
        assert_eq!(iso_week(2007, 12, 31), (2008, 1)); // Mon 31 Dec 2007
        assert_eq!(iso_week(2008, 1, 1), (2008, 1)); // Tue 1 Jan 2008
        assert_eq!(iso_week(2008, 12, 28), (2008, 52)); // Sun 28 Dec 2008
        assert_eq!(iso_week(2008, 12, 29), (2009, 1)); // Mon 29 Dec 2008
        assert_eq!(iso_week(2008, 12, 30), (2009, 1)); // Tue 30 Dec 2008
        assert_eq!(iso_week(2008, 12, 31), (2009, 1)); // Wed 31 Dec 2008
        assert_eq!(iso_week(2009, 1, 1), (2009, 1)); // Thu 1 Jan 2009
        assert_eq!(iso_week(2009, 12, 31), (2009, 53)); // Thu 31 Dec 2009
        assert_eq!(iso_week(2010, 1, 1), (2009, 53)); // Fri 1 Jan 2010
        assert_eq!(iso_week(2010, 1, 2), (2009, 53)); // Sat 2 Jan 2010
        assert_eq!(iso_week(2010, 1, 3), (2009, 53)); // Sun 3 Jan 2010
        // From formatDateTime.json W tests
        assert_eq!(iso_week(2014, 12, 23), (2014, 52)); // Tue
        assert_eq!(iso_week(2014, 12, 28), (2014, 52)); // Sun
        assert_eq!(iso_week(2014, 12, 29), (2015, 1)); // Mon
        assert_eq!(iso_week(2015, 1, 1), (2015, 1)); // Thu
        assert_eq!(iso_week(2015, 1, 5), (2015, 2)); // Mon
        assert_eq!(iso_week(2015, 12, 28), (2015, 53)); // Mon
        assert_eq!(iso_week(2015, 12, 31), (2015, 53)); // Thu
        assert_eq!(iso_week(2016, 1, 2), (2015, 53)); // Sat
    }

    #[test]
    fn test_int_to_words() {
        assert_eq!(int_to_words(0), "zero");
        assert_eq!(int_to_words(1), "one");
        assert_eq!(int_to_words(18), "eighteen");
        assert_eq!(int_to_words(42), "forty-two");
        assert_eq!(
            int_to_words(1984),
            "one thousand, nine hundred and eighty-four"
        );
        assert_eq!(int_to_words(2018), "two thousand and eighteen");
    }

    #[test]
    fn test_to_roman() {
        assert_eq!(to_roman(2018, true), "MMXVIII");
        assert_eq!(to_roman(1984, true), "MCMLXXXIV");
        assert_eq!(to_roman(3, false), "iii");
    }

    #[test]
    fn test_format_default_iso() {
        assert_eq!(format_default_iso(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_default_iso(1, 0), "1970-01-01T00:00:00.001Z");
        assert_eq!(
            format_default_iso(1509380732935, 0),
            "2017-10-30T16:25:32.935Z"
        );
    }
}
