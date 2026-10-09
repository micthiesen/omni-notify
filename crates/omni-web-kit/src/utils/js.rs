//! JavaScript number, date and string semantics.
//!
//! On `wasm32` these call the browser's own implementations (`toFixed`,
//! `toLocaleString`, `Date.parse`, `localeCompare`) so formatting follows the
//! viewer's locale. Native builds (unit tests) use close Rust equivalents;
//! nothing persisted depends on them.

/// `Date.now()`.
pub fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    }
}

/// `Math.round`: halves round toward positive infinity.
pub fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// `Number.prototype.toFixed(digits)`.
pub fn to_fixed(value: f64, digits: u8) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Ok(text) = js_sys::Number::from(value).to_fixed(digits) {
            return String::from(text);
        }
    }
    format!("{value:.prec$}", prec = usize::from(digits))
}

/// `String(n)` for a JS number.
pub fn number_string(value: f64) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Ok(text) = js_sys::Number::from(value).to_string_with_radix(10) {
            return String::from(text);
        }
    }
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i64)
    } else if value.is_nan() {
        "NaN".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        format!("{value}")
    }
}

/// `n.toLocaleString()` in the browser's default locale.
pub fn locale_number(value: f64) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let number = wasm_bindgen::JsValue::from_f64(value);
        if let Some(text) = js_call0_string(&number, "toLocaleString") {
            return text;
        }
    }
    native_grouped(value)
}

fn native_grouped(value: f64) -> String {
    let negative = value < 0.0;
    let rounded = js_round(value.abs() * 1000.0) / 1000.0;
    let int = rounded.trunc() as u64;
    let digits = int.to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let frac = rounded.fract();
    if frac > 0.0 {
        let text = format!("{frac:.3}");
        let trimmed = text.trim_start_matches('0').trim_end_matches('0');
        grouped.push_str(trimmed);
    }
    if negative {
        format!("-{grouped}")
    } else {
        grouped
    }
}

#[cfg(target_arch = "wasm32")]
fn js_call0_string(target: &wasm_bindgen::JsValue, method: &str) -> Option<String> {
    use wasm_bindgen::JsCast as _;
    let function = js_sys::Reflect::get(target, &wasm_bindgen::JsValue::from_str(method))
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    function.call0(target).ok()?.as_string()
}

/// `Date.parse(text)`; `None` when the result is `NaN`.
pub fn parse_date_ms(text: &str) -> Option<f64> {
    #[cfg(target_arch = "wasm32")]
    {
        let ms = js_sys::Date::parse(text);
        if ms.is_nan() { None } else { Some(ms) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native_parse_iso(text)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn native_parse_iso(text: &str) -> Option<f64> {
    // `YYYY-MM-DDTHH:MM:SS(.sss)?Z` only; enough for tests.
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !text.ends_with('Z') {
        return None;
    }
    let num = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let millis = if bytes.get(19) == Some(&b'.') {
        num(20..23).unwrap_or(0)
    } else {
        0
    };
    let days = days_from_civil(year, month, day);
    Some((((days * 24 + hour) * 60 + minute) * 60 + second) as f64 * 1000.0 + millis as f64)
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `(y, m, d)` for days since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `new Date(ms).toISOString()`.
pub fn iso_string(ms: f64) -> String {
    let total = ms.floor() as i64;
    let millis = total.rem_euclid(1000);
    let secs = total.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Options for `Intl.DateTimeFormat` as `(key, value)` pairs.
pub type DateOptions<'a> = &'a [(&'a str, &'a str)];

#[cfg(target_arch = "wasm32")]
fn options_object(options: DateOptions<'_>) -> js_sys::Object {
    let object = js_sys::Object::new();
    for (key, value) in options {
        let _ = js_sys::Reflect::set(
            &object,
            &wasm_bindgen::JsValue::from_str(key),
            &wasm_bindgen::JsValue::from_str(value),
        );
    }
    object
}

/// `new Date(ms).toLocaleString("en-US", options)`.
pub fn date_locale_string(ms: f64, options: DateOptions<'_>) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
        String::from(date.to_locale_string("en-US", &options_object(options)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = options;
        iso_string(ms)
    }
}

/// `new Date(ms).toLocaleDateString("en-US", options)`.
pub fn date_locale_date_string(ms: f64, options: DateOptions<'_>) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
        String::from(date.to_locale_date_string("en-US", &options_object(options)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = options;
        iso_string(ms).chars().take(10).collect()
    }
}

/// `new Date(ms).toLocaleTimeString("en-US", options)`.
pub fn date_locale_time_string(ms: f64, options: DateOptions<'_>) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
        String::from(date.to_locale_time_string_with_options("en-US", &options_object(options)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = options;
        iso_string(ms).chars().skip(11).take(5).collect()
    }
}

/// Epoch ms of local midnight `year-month-day` (`new Date(y, m - 1, d)`).
pub fn local_date_ms(year: i32, month: i32, day: i32) -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new_with_year_month_day(year.max(0) as u32, month - 1, day);
        date.get_time()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        days_from_civil(i64::from(year), i64::from(month), i64::from(day)) as f64 * 86_400_000.0
    }
}

/// Local wall-clock parts of `ms`: `(hours, minutes, seconds, millis)`.
pub fn local_time_parts(ms: f64) -> (u32, u32, u32, u32) {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
        (
            date.get_hours(),
            date.get_minutes(),
            date.get_seconds(),
            date.get_milliseconds(),
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let total = ms.floor() as i64;
        let millis = total.rem_euclid(1000) as u32;
        let secs = total.div_euclid(1000).rem_euclid(86_400) as u32;
        (secs / 3600, (secs % 3600) / 60, secs % 60, millis)
    }
}

/// `a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" })`.
pub fn locale_compare_numeric(a: &str, b: &str) -> std::cmp::Ordering {
    #[cfg(target_arch = "wasm32")]
    {
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &"numeric".into(), &wasm_bindgen::JsValue::TRUE);
        let _ = js_sys::Reflect::set(&options, &"sensitivity".into(), &"base".into());
        js_sys::JsString::from(a)
            .locale_compare(b, &js_sys::Array::new(), &options)
            .cmp(&0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        a.to_lowercase().cmp(&b.to_lowercase())
    }
}

/// `a.localeCompare(b)` with default options.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::JsString::from(a)
            .locale_compare(b, &js_sys::Array::new(), &js_sys::Object::new())
            .cmp(&0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        a.cmp(b)
    }
}

/// `s.toLocaleLowerCase()`.
pub fn locale_lowercase(s: &str) -> String {
    s.to_lowercase()
}

/// UTF-16 length, as `String#length` counts it.
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.slice(0, n)` in UTF-16 units (a split surrogate pair is dropped).
pub fn utf16_slice(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let width = ch.len_utf16();
        if used + width > n {
            break;
        }
        used += width;
        out.push(ch);
    }
    out
}

/// Local calendar day of `ms` as days since 1970-01-01.
pub fn local_day_index(ms: f64) -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
        let offset_ms = date.get_timezone_offset() * 60_000.0;
        ((ms - offset_ms) / 86_400_000.0).floor() as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        (ms / 86_400_000.0).floor() as i64
    }
}

/// Monday of the week containing day index `day` (1970-01-01 was a Thursday).
pub fn week_start(day: i64) -> i64 {
    day - (day + 3).rem_euclid(7)
}

/// `decodeURIComponent`; `None` when it would throw.
pub fn decode_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn weeks_start_on_monday() {
        // 2026-10-05 is a Monday, 2026-10-11 a Sunday.
        let monday = super::days_from_civil(2026, 10, 5);
        assert_eq!(super::week_start(monday), monday);
        assert_eq!(super::week_start(monday + 6), monday);
        assert_eq!(super::week_start(monday + 7), monday + 7);
    }

    use super::*;

    #[test]
    fn iso_and_civil_round_trip() {
        assert_eq!(iso_string(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_string(1_700_000_000_123.0), "2023-11-14T22:13:20.123Z");
        assert_eq!(
            parse_date_ms("2023-11-14T22:13:20.123Z"),
            Some(1_700_000_000_123.0)
        );
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(native_grouped(1234567.0), "1,234,567");
        assert_eq!(number_string(3.0), "3");
        assert_eq!(utf16_slice("ab😀c", 3), "ab");
    }
}
