//! Result-field normalizers — a port of Python `ddgs`'s `utils.py`.
//!
//! Upstream applies these in `BaseResult.__setattr__` on every non-empty
//! assignment, keyed by field name:
//!
//! - `title`, `body`, `author`, `publisher`, `info` → [`normalize_text`]
//! - `href`, `url`, `thumbnail`, `image` → [`normalize_url`]
//! - `date` → `_normalize_date` (an integer unix timestamp becomes an ISO-8601
//!   UTC string via [`timestamp_to_iso`]; strings pass through unchanged)
//!
//! The result structs in [`crate::results`] call these from their constructors
//! (`normalized()`), so every engine gets the same treatment.

use std::borrow::Cow;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use icu_normalizer::ComposingNormalizerBorrowed;
use icu_properties::CodePointMapData;
use icu_properties::props::{GeneralCategory, GeneralCategoryGroup};
use regex::{Captures, Regex};

/// `_REGEX_STRIP_TAGS = re.compile("<.*?>")`. As in Python, `.` does not match
/// `\n`, so a tag that spans lines is left in place.
static STRIP_TAGS: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new("<.*?>").ok());

/// `html._charref` from CPython's `html/__init__.py`.
static CHARREF: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?)").ok());

/// Port of `ddgs.utils._normalize_text`.
///
/// Strip HTML tags, unescape HTML entities, Unicode NFC, drop every character
/// whose general category is `C*` (Cc, Cf, Cs, Co, Cn), then collapse
/// whitespace (`" ".join(text.split())`).
pub(crate) fn normalize_text(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let stripped = match STRIP_TAGS.as_ref() {
        Some(re) => re.replace_all(raw, ""),
        None => Cow::Borrowed(raw),
    };
    let unescaped = html_unescape(&stripped);
    let composed = ComposingNormalizerBorrowed::new_nfc().normalize(&unescaped);
    let categories = CodePointMapData::<GeneralCategory>::new();
    let visible: String = composed
        .chars()
        .filter(|ch| !GeneralCategoryGroup::Other.contains(categories.get(*ch)))
        .collect();
    collapse_whitespace(&visible)
}

/// Whitespace as Python's `str.split()` / `str.isspace()` define it: Rust's
/// `char::is_whitespace` (Unicode `White_Space`) plus U+001C..U+001F, which
/// Python counts as separators (bidi class B/S). Verified exhaustively against
/// CPython 3 (Unicode 17): those four are the only difference.
pub(crate) fn is_py_whitespace(ch: char) -> bool {
    ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch)
}

/// `" ".join(text.split())` with Python's whitespace definition.
pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split(is_py_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Python `int(text)` for a string of digits matched by a `\d+` regex group.
/// Like CPython it accepts any Unicode `Nd` digit (e.g. `٣` → 3): `Nd`
/// characters are encoded in aligned runs of ten (0..9), so a digit's value is
/// its offset from the start of its contiguous `Nd` block, modulo 10. Returns
/// `None` on a non-digit or on `i64` overflow (callers treat overflow as the
/// `OverflowError` upstream raises when the number reaches `timedelta`).
pub(crate) fn python_int(digits: &str) -> Option<i64> {
    let categories = CodePointMapData::<GeneralCategory>::new();
    let is_nd = |ch: char| categories.get(ch) == GeneralCategory::DecimalNumber;
    if digits.is_empty() {
        return None;
    }
    let mut value: i64 = 0;
    for ch in digits.chars() {
        let digit = match ch.to_digit(10) {
            Some(digit) => i64::from(digit),
            None if is_nd(ch) => {
                let mut start = u32::from(ch);
                while let Some(prev) = start.checked_sub(1).and_then(char::from_u32) {
                    if !is_nd(prev) {
                        break;
                    }
                    start -= 1;
                }
                i64::from((u32::from(ch) - start) % 10)
            }
            None => return None,
        };
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

/// Port of `ddgs.utils._normalize_url`: `unquote(url).replace(" ", "+")`.
pub(crate) fn normalize_url(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    unquote(raw).replace(' ', "+")
}

/// Port of `urllib.parse.unquote` (UTF-8, `errors="replace"`).
///
/// Every valid `%XX` escape is decoded to its byte; malformed escapes are kept
/// literally; `+` is left alone. Invalid UTF-8 in the decoded bytes becomes
/// U+FFFD per maximal invalid subsequence, which is how both CPython's and
/// Rust's lossy UTF-8 decoders behave.
pub(crate) fn unquote(input: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    if !input.contains('%') {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Port of `urllib.parse.quote(text)` with the default `safe="/"`.
///
/// ASCII letters, digits, `_.-~` and `/` stay as they are; every other UTF-8
/// byte becomes an uppercase `%XX` escape (a space becomes `%20`, not `+`).
pub(crate) fn quote(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'~' | b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Port of `urllib.parse.unquote_plus`: `+` → space, then [`unquote`].
pub(crate) fn unquote_plus(input: &str) -> String {
    unquote(&input.replace('+', " "))
}

/// Port of `datetime.fromtimestamp(ts, timezone.utc).isoformat()` for an
/// integer timestamp, e.g. `1710000000` → `"2024-03-09T16:00:00+00:00"`.
///
/// Returns `None` outside Python's `datetime` range (years 1..=9999), where
/// upstream raises.
pub(crate) fn timestamp_to_iso(ts: i64) -> Option<String> {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(1..=9999).contains(&year) {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    ))
}

/// Current unix time in whole seconds (Python's `.replace(microsecond=0)`).
pub(crate) fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// ISO-8601 UTC string for "now minus `seconds`", as upstream's relative-date
/// helpers produce (`(datetime.now(utc) - delta).replace(microsecond=0)`).
pub(crate) fn iso_seconds_ago(seconds: i64) -> Option<String> {
    timestamp_to_iso(now_unix_secs().checked_sub(seconds)?)
}

/// Days in `month` of `year` (proleptic Gregorian), `None` for a bad month.
pub(crate) fn days_in_month(year: i64, month: u32) -> Option<u32> {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if leap { 29 } else { 28 }),
        _ => None,
    }
}

/// Howard Hinnant's `days_from_civil`: (y, m, d) → days since 1970-01-01.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400); // [0, 399]
    let month = i64::from(month);
    let mp = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// CPython's `max_fold_seconds` (`_datetimemodule.c`).
const MAX_FOLD_SECONDS: i64 = 24 * 3600;

/// Port of CPython's `local_to_seconds` (`_datetimemodule.c`): solve
/// `local(u) == t` for the instant `u` of naive local time `t`. Both are
/// seconds since the Unix epoch, `t` read as if it were UTC. `local(u)` is the
/// local wall-clock time at instant `u` in the same form, or `None` where
/// CPython's `localtime` call fails.
fn local_to_seconds(t: i64, fold: bool, local: &impl Fn(i64) -> Option<i64>) -> Option<i64> {
    let a = local(t)? - t;
    let u1 = t - a;
    let t1 = local(u1)?;
    let b = if t1 == t {
        // One solution found; look for an earlier (fold 0) or later (fold 1) one.
        let u2 = if fold {
            u1 + MAX_FOLD_SECONDS
        } else {
            u1 - MAX_FOLD_SECONDS
        };
        let b = local(u2)? - u2;
        if a == b {
            return Some(u1);
        }
        b
    } else {
        t1 - u1
    };
    let u2 = t - b;
    let t2 = local(u2)?;
    if t2 == t {
        return Some(u2);
    }
    if t1 == t {
        return Some(u1);
    }
    // Neither offset solves it: `t` is in a gap.
    Some(if fold { u1.min(u2) } else { u1.max(u2) })
}

/// `naive.astimezone(timezone.utc)` for a naive datetime `t` (fold 0), as
/// CPython does it: `local_timezone_from_local` solves for the instant with
/// both folds and, in a gap, keeps the earlier one; the zone offset is the
/// host's offset *at that instant*; the result is `t - offset`. Returns Unix
/// seconds, or `None` where CPython raises.
pub(crate) fn naive_local_to_utc(t: i64, local: &impl Fn(i64) -> Option<i64>) -> Option<i64> {
    let mut seconds = local_to_seconds(t, false, local)?;
    let seconds2 = local_to_seconds(t, true, local)?;
    // "Detect gap": `(seconds2 > seconds) == fold` with fold = 0.
    if seconds2 != seconds && seconds2 < seconds {
        seconds = seconds2;
    }
    let offset = local(seconds)? - seconds;
    t.checked_sub(offset)
}

/// The host's local wall-clock time at Unix instant `u`, as "as if UTC"
/// seconds: CPython's `local()` helper, which calls `localtime_r` (POSIX) /
/// `localtime_s` (Windows) and reads the broken-down fields, so the host's
/// `TZ`/zoneinfo applies exactly as it does for Python.
pub(crate) fn host_local(u: i64) -> Option<i64> {
    let tm = host_localtime(u)?;
    let month = u32::try_from(tm.tm_mon + 1).ok()?;
    let day = u32::try_from(tm.tm_mday).ok()?;
    let days = days_from_civil(i64::from(tm.tm_year) + 1900, month, day);
    Some(
        days * 86_400
            + i64::from(tm.tm_hour) * 3600
            + i64::from(tm.tm_min) * 60
            + i64::from(tm.tm_sec),
    )
}

#[cfg(unix)]
fn host_localtime(u: i64) -> Option<libc::tm> {
    unsafe extern "C" {
        fn tzset();
    }
    // CPython calls `tzset` once, when the `time` module is initialized, and
    // not before each `localtime_r`; so a `TZ` change after start-up is not
    // seen. Match that with a one-time call.
    static TZSET: std::sync::Once = std::sync::Once::new();
    // SAFETY: `tzset` reads the `TZ` environment variable and writes the
    // libc time-zone globals. A data race is possible only with a concurrent
    // environment write. In Rust 2024 `std::env::set_var`/`remove_var` are
    // `unsafe` and their callers must guarantee no concurrent environment
    // access, so this read is sound under that contract. `Once` makes sure
    // that this crate does not race with itself.
    TZSET.call_once(|| unsafe { tzset() });
    let time = libc::time_t::try_from(u).ok()?;
    // SAFETY: `localtime_r` writes only into the caller-owned `tm`, a plain C
    // struct for which all-zero bytes are a valid value. It reads the
    // time-zone state that `tzset` set up above.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return None;
        }
        Some(tm)
    }
}

#[cfg(windows)]
fn host_localtime(u: i64) -> Option<libc::tm> {
    let time = libc::time_t::try_from(u).ok()?;
    // SAFETY: `localtime_s` writes only into the caller-owned `tm`, a plain C
    // struct for which all-zero bytes are a valid value.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_s(&mut tm, &time) != 0 {
            return None;
        }
        Some(tm)
    }
}

/// Broken-down time for targets without a C `localtime` (e.g. wasm), where
/// the local zone is taken to be UTC.
#[cfg(not(any(unix, windows)))]
struct Tm {
    tm_year: i32,
    tm_mon: i32,
    tm_mday: i32,
    tm_hour: i32,
    tm_min: i32,
    tm_sec: i32,
}

#[cfg(not(any(unix, windows)))]
fn host_localtime(u: i64) -> Option<Tm> {
    let (year, month, day) = civil_from_days(u.div_euclid(86_400));
    let secs = u.rem_euclid(86_400);
    Some(Tm {
        tm_year: i32::try_from(year - 1900).ok()?,
        tm_mon: i32::try_from(month).ok()? - 1,
        tm_mday: i32::try_from(day).ok()?,
        tm_hour: i32::try_from(secs / 3600).ok()?,
        tm_min: i32::try_from(secs % 3600 / 60).ok()?,
        tm_sec: i32::try_from(secs % 60).ok()?,
    })
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 → (y, m, d).
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = yoe + era * 400 + i64::from(month <= 2);
    // Both values are range-bounded by construction.
    (year, month as u32, day as u32)
}

/// Port of CPython's `html.unescape` (HTML5 named + numeric references,
/// including legacy semicolon-less names and longest-prefix matching).
pub(crate) fn html_unescape(input: &str) -> Cow<'_, str> {
    if !input.contains('&') {
        return Cow::Borrowed(input);
    }
    match CHARREF.as_ref() {
        Some(re) => re.replace_all(input, |caps: &Captures<'_>| {
            replace_charref(caps.get(1).map_or("", |m| m.as_str()))
        }),
        None => Cow::Borrowed(input),
    }
}

/// `html._replace_charref`, given the text after `&`.
fn replace_charref(s: &str) -> String {
    if let Some(numeric) = s.strip_prefix('#') {
        let (digits, radix) = match numeric.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16),
            None => (numeric, 10),
        };
        // The regex guarantees valid digits, so a parse failure is an overflow,
        // i.e. a number above 0x10FFFF.
        let num = u32::from_str_radix(digits.trim_end_matches(';'), radix).unwrap_or(u32::MAX);
        if let Some(replacement) = invalid_charref(num) {
            return replacement.to_string();
        }
        if (0xD800..=0xDFFF).contains(&num) || num > 0x10FFFF {
            return '\u{FFFD}'.to_string();
        }
        if is_invalid_codepoint(num) {
            return String::new();
        }
        return char::from_u32(num).map(String::from).unwrap_or_default();
    }

    if let Some(value) = named_entity(s) {
        return value;
    }
    // Longest matching prefix, as `range(len(s)-1, 1, -1)` in CPython.
    let boundaries: Vec<usize> = s.char_indices().map(|(index, _)| index).collect();
    for count in (2..boundaries.len()).rev() {
        let (prefix, rest) = s.split_at(boundaries[count]);
        if let Some(value) = named_entity(prefix) {
            return value + rest;
        }
    }
    format!("&{s}")
}

/// Look up an HTML5 named reference (`"amp;"`, legacy `"amp"`). The
/// markup5ever table also stores every prefix mapped to `(0, 0)`; those are
/// not entities.
fn named_entity(name: &str) -> Option<String> {
    let &(first, second) = markup5ever::data::NAMED_ENTITIES.get(name)?;
    if first == 0 {
        return None;
    }
    let mut value = String::new();
    value.push(char::from_u32(first)?);
    if second != 0 {
        value.push(char::from_u32(second)?);
    }
    Some(value)
}

/// `html._invalid_charrefs`.
fn invalid_charref(num: u32) -> Option<char> {
    Some(match num {
        0x00 => '\u{FFFD}',
        0x0d => '\r',
        0x80 => '\u{20AC}',
        0x81 => '\u{81}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8a => '\u{0160}',
        0x8b => '\u{2039}',
        0x8c => '\u{0152}',
        0x8d => '\u{8D}',
        0x8e => '\u{017D}',
        0x8f => '\u{8F}',
        0x90 => '\u{90}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9a => '\u{0161}',
        0x9b => '\u{203A}',
        0x9c => '\u{0153}',
        0x9d => '\u{9D}',
        0x9e => '\u{017E}',
        0x9f => '\u{0178}',
        _ => return None,
    })
}

/// `html._invalid_codepoints`.
fn is_invalid_codepoint(num: u32) -> bool {
    matches!(num, 0x1..=0x8 | 0xb | 0xe..=0x1f | 0x7f..=0x9f | 0xfdd0..=0xfdef)
        || (num <= 0x10FFFF && (num & 0xFFFF) >= 0xFFFE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        assert!(STRIP_TAGS.is_some());
        assert!(CHARREF.is_some());
    }

    #[test]
    fn text_strips_tags() {
        assert_eq!(normalize_text("The <b>Rust</b> book"), "The Rust book");
        assert_eq!(normalize_text("<span class=\"x\">a</span><br/>b"), "ab");
        // `.` does not cross newlines, so a multi-line tag survives (as
        // upstream); the `\n` itself is then dropped as category Cc.
        assert_eq!(normalize_text("a<b\n>c"), "a<b>c");
    }

    #[test]
    fn text_unescapes_entities() {
        assert_eq!(normalize_text("Tom &amp; Jerry"), "Tom & Jerry");
        assert_eq!(
            normalize_text("&lt;tag&gt; &quot;q&quot; &#39;s&#x27;"),
            "<tag> \"q\" 's'"
        );
        // Entities are decoded after tag stripping, so escaped tags remain.
        assert_eq!(normalize_text("&lt;b&gt;x&lt;/b&gt;"), "<b>x</b>");
        // Legacy no-semicolon names and longest-prefix matching.
        assert_eq!(normalize_text("&copy2024 &notit;"), "©2024 ¬it;");
        assert_eq!(normalize_text("&ampx"), "&x");
        // Unknown names are kept verbatim.
        assert_eq!(normalize_text("&bogus; a&b"), "&bogus; a&b");
        // Multi-codepoint entity.
        assert_eq!(html_unescape("&NotEqualTilde;"), "\u{2242}\u{338}");
    }

    #[test]
    fn numeric_charrefs_follow_cpython() {
        assert_eq!(html_unescape("&#65;&#x42;&#X43"), "ABC");
        assert_eq!(html_unescape("&#0;"), "\u{FFFD}");
        assert_eq!(html_unescape("&#x80;"), "\u{20AC}");
        assert_eq!(html_unescape("&#xD800;"), "\u{FFFD}");
        assert_eq!(html_unescape("&#99999999999;"), "\u{FFFD}");
        assert_eq!(html_unescape("a&#1;b&#xFFFF;c"), "abc");
        assert_eq!(html_unescape("&#;"), "&#;");
    }

    #[test]
    fn text_applies_nfc() {
        // "e" + combining acute → precomposed "é".
        assert_eq!(normalize_text("caf\u{65}\u{301}"), "caf\u{e9}");
    }

    #[test]
    fn text_removes_category_c() {
        // Cc (NUL, BEL, DEL), Cf (ZWJ, soft hyphen, BOM), Co (private use).
        assert_eq!(
            normalize_text("a\u{0}b\u{7}c\u{7f}d\u{200d}e\u{ad}f\u{feff}g\u{e000}h"),
            "abcdefgh"
        );
        // Tabs/newlines are Cc too, so they vanish rather than becoming spaces.
        assert_eq!(normalize_text("a\tb\nc"), "abc");
    }

    #[test]
    fn text_collapses_whitespace() {
        assert_eq!(normalize_text("  a   b \u{a0} c\u{3000}d  "), "a b c d");
        assert_eq!(normalize_text(""), "");
        assert_eq!(normalize_text("   "), "");
    }

    #[test]
    fn url_unquotes_and_plus_encodes_spaces() {
        assert_eq!(
            normalize_url("https://x.com/a%20b?q=c%2Bd+e"),
            "https://x.com/a+b?q=c+d+e"
        );
        assert_eq!(normalize_url("https://x.com/a b"), "https://x.com/a+b");
        assert_eq!(normalize_url("https://x.com/%E2%82%AC"), "https://x.com/€");
        // Malformed escapes pass through.
        assert_eq!(normalize_url("100%zz%4"), "100%zz%4");
        assert_eq!(normalize_url(""), "");
    }

    #[test]
    fn url_replaces_invalid_utf8() {
        assert_eq!(unquote("a%FFb"), "a\u{FFFD}b");
        // Truncated 3-byte sequence is one maximal subpart → one U+FFFD.
        assert_eq!(unquote("%E2%82x"), "\u{FFFD}x");
        assert_eq!(unquote("%E2%E2%82%AC"), "\u{FFFD}€");
    }

    #[test]
    fn unquote_plus_decodes_plus() {
        assert_eq!(unquote_plus("a+b%2Bc"), "a b+c");
    }

    #[test]
    fn quote_matches_python_default_safe() {
        // urllib.parse.quote("a b/c?d=é&~_.-+") == "a%20b/c%3Fd%3D%C3%A9%26~_.-%2B"
        assert_eq!(quote("a b/c?d=é&~_.-+"), "a%20b/c%3Fd%3D%C3%A9%26~_.-%2B");
    }

    #[test]
    fn timestamp_formats_like_python_isoformat() {
        assert_eq!(
            timestamp_to_iso(0).as_deref(),
            Some("1970-01-01T00:00:00+00:00")
        );
        // datetime.fromtimestamp(1710000000, timezone.utc).isoformat()
        assert_eq!(
            timestamp_to_iso(1_710_000_000).as_deref(),
            Some("2024-03-09T16:00:00+00:00")
        );
        assert_eq!(
            timestamp_to_iso(951_782_400).as_deref(),
            Some("2000-02-29T00:00:00+00:00")
        );
        assert_eq!(
            timestamp_to_iso(-1).as_deref(),
            Some("1969-12-31T23:59:59+00:00")
        );
        assert_eq!(
            timestamp_to_iso(253_402_300_799).as_deref(),
            Some("9999-12-31T23:59:59+00:00")
        );
        assert_eq!(timestamp_to_iso(253_402_300_800), None);
    }

    #[test]
    fn whitespace_matches_python_str_split() {
        // U+001C..U+001F are whitespace to Python but not to Rust.
        assert_eq!(collapse_whitespace("a\u{1c}b\u{1f}c"), "a b c");
        assert_eq!(collapse_whitespace(" a \u{a0}\u{2028} b "), "a b");
        assert_eq!(collapse_whitespace("a\u{200b}b"), "a\u{200b}b");
    }

    #[test]
    fn python_int_accepts_unicode_digits() {
        assert_eq!(python_int("042"), Some(42));
        assert_eq!(python_int("\u{663}"), Some(3)); // ARABIC-INDIC DIGIT THREE
        assert_eq!(python_int("\u{1d7d3}"), Some(5)); // MATHEMATICAL BOLD DIGIT FIVE
        assert_eq!(python_int("1\u{967}"), Some(11)); // DEVANAGARI DIGIT ONE
        assert_eq!(python_int("99999999999999999999"), None);
        assert_eq!(python_int("1a"), None);
        assert_eq!(python_int(""), None);
    }

    #[test]
    fn days_from_civil_round_trips() {
        for days in [-719_468, -1, 0, 11_016, 19_791, 2_932_896] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
        assert_eq!(days_from_civil(2024, 3, 9), 19_791);
    }

    #[test]
    fn naive_local_to_utc_fixed_offsets() {
        let t = days_from_civil(2026, 11, 4) * 86_400;
        let plus_two = |u: i64| Some(u + 7200);
        assert_eq!(naive_local_to_utc(t, &plus_two), Some(t - 7200));
        let minus_five = |u: i64| Some(u - 5 * 3600);
        assert_eq!(naive_local_to_utc(t, &minus_five), Some(t + 5 * 3600));
        let utc = |u: i64| Some(u);
        assert_eq!(naive_local_to_utc(t, &utc), Some(t));
        let failing = |_: i64| None;
        assert_eq!(naive_local_to_utc(t, &failing), None);
    }

    #[test]
    fn naive_local_to_utc_dst_gap_and_fold() {
        let day = days_from_civil(2026, 3, 29) * 86_400;
        // UTC+1, springing forward to UTC+2 at 01:00 UTC (local 02:00 → 03:00).
        let switch = day + 3600;
        let spring = move |u: i64| Some(if u < switch { u + 3600 } else { u + 7200 });
        assert_eq!(naive_local_to_utc(day, &spring), Some(day - 3600));
        assert_eq!(
            naive_local_to_utc(day + 4 * 3600, &spring),
            Some(day + 2 * 3600)
        );
        // Gap: local 02:30 takes the pre-transition offset (+01:00) → 01:30 UTC,
        // as CPython's `astimezone` does for fold = 0.
        assert_eq!(
            naive_local_to_utc(day + 2 * 3600 + 1800, &spring),
            Some(day + 3600 + 1800)
        );
        // UTC+2 falling back to UTC+1 at 01:00 UTC (local 03:00 → 02:00).
        let fall = move |u: i64| Some(if u < switch { u + 7200 } else { u + 3600 });
        // Fold: local 02:30 happens twice; fold 0 is the first (+02:00).
        assert_eq!(
            naive_local_to_utc(day + 2 * 3600 + 1800, &fall),
            Some(day + 1800)
        );
    }

    #[test]
    fn host_local_is_consistent() {
        // Whatever the host zone, local time is within ±26h of UTC.
        let now = now_unix_secs();
        let local = host_local(now).expect("localtime");
        assert!((local - now).abs() <= 26 * 3600);
    }

    #[test]
    fn month_lengths() {
        assert_eq!(days_in_month(2024, 2), Some(29));
        assert_eq!(days_in_month(1900, 2), Some(28));
        assert_eq!(days_in_month(2000, 2), Some(29));
        assert_eq!(days_in_month(2026, 13), None);
    }
}
