//! Python behavior the upstream CLI depends on: `textwrap` (as subclassed by
//! click 8.5), `click.wrap_text`, `difflib.get_close_matches`, and `str()` /
//! `repr()` of result values.

use icu_properties::props::{GeneralCategory, GeneralCategoryGroup};
use icu_properties::{CodePointMapData, CodePointMapDataBorrowed};

use crate::normalize::is_py_whitespace;

// ---------------------------------------------------------------------------
// Character classes (Python `re` with str patterns)

fn categories() -> CodePointMapDataBorrowed<'static, GeneralCategory> {
    CodePointMapData::<GeneralCategory>::new()
}

/// `\w`: `str.isalnum()` or `_` (letters L* and numbers N*).
fn is_word(ch: char) -> bool {
    if ch == '_' {
        return true;
    }
    let category = categories().get(ch);
    GeneralCategoryGroup::Letter.contains(category)
        || GeneralCategoryGroup::Number.contains(category)
}

/// `\d`: decimal digits (Nd).
fn is_digit(ch: char) -> bool {
    categories().get(ch) == GeneralCategory::DecimalNumber
}

/// textwrap `letter = r'[^\d\W]'`.
fn is_letter(ch: char) -> bool {
    is_word(ch) && !is_digit(ch)
}

/// textwrap `word_punct = r'[\w!"\'&.,?]'`.
fn is_word_punct(ch: char) -> bool {
    is_word(ch) || matches!(ch, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

/// textwrap `_whitespace = '\t\n\x0b\x0c\r '`.
fn is_tw_whitespace(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ')
}

/// click `_ansi_re = re.compile(r"\033\[[;?0-9]*[a-zA-Z]")`.
fn strip_ansi(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' && chars.get(i + 1) == Some(&'[') {
            let mut j = i + 2;
            while j < chars.len()
                && (chars[j] == ';' || chars[j] == '?' || chars[j].is_ascii_digit())
            {
                j += 1;
            }
            if j < chars.len() && chars[j].is_ascii_alphabetic() {
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// click `term_len`: length in code points without ANSI escapes.
pub(crate) fn term_len(text: &str) -> usize {
    strip_ansi(text).chars().count()
}

/// Python `str.strip() == ""`.
fn is_blank(text: &str) -> bool {
    text.chars().all(is_py_whitespace)
}

/// Python `str.expandtabs(tabsize)`.
pub(crate) fn expandtabs(text: &str, tabsize: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut column = 0usize;
    for ch in text.chars() {
        match ch {
            '\t' => {
                if tabsize > 0 {
                    let spaces = tabsize - (column % tabsize);
                    out.extend(std::iter::repeat_n(' ', spaces));
                    column += spaces;
                }
            }
            '\n' | '\r' => {
                out.push(ch);
                column = 0;
            }
            _ => {
                out.push(ch);
                column += 1;
            }
        }
    }
    out
}

/// Python `str.splitlines()` (without keepends).
pub(crate) fn splitlines(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let is_break = matches!(
            ch,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if is_break {
            lines.push(std::mem::take(&mut current));
            if ch == '\r' && chars.get(i + 1) == Some(&'\n') {
                i += 1;
            }
        } else {
            current.push(ch);
        }
        i += 1;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

// ---------------------------------------------------------------------------
// textwrap.TextWrapper (break_on_hyphens=True, break_long_words=True,
// drop_whitespace=True, replace_whitespace=False, max_lines=None) with
// click's `_wrap_chunks` and `_handle_long_word` overrides.

/// Port of `TextWrapper.wordsep_re.split` (chunks; empty ones dropped). The
/// regex uses look-behind/look-ahead, which the `regex` crate lacks, so the
/// alternation is evaluated by hand in the same order.
pub(crate) fn split_chunks(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let n = c.len();
    let at = |i: usize| c.get(i).copied();
    let letter_at = |i: Option<usize>| i.and_then(|i| c.get(i)).is_some_and(|ch| is_letter(*ch));
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < n {
        // 1. whitespace run
        if is_tw_whitespace(c[i]) {
            let mut j = i;
            while j < n && is_tw_whitespace(c[j]) {
                j += 1;
            }
            chunks.push(c[i..j].iter().collect());
            i = j;
            continue;
        }
        // 2. em-dash between words: (?<=wp) -{2,} (?=\w)
        if c[i] == '-' && i > 0 && is_word_punct(c[i - 1]) {
            let mut j = i;
            while j < n && c[j] == '-' {
                j += 1;
            }
            if j - i >= 2 && at(j).is_some_and(is_word) {
                chunks.push(c[i..j].iter().collect());
                i = j;
                continue;
            }
        }
        // 3. word, possibly hyphenated: nws+? followed by one alternative.
        let mut k = i + 1;
        let end = loop {
            // (a) a hyphen that splits a hyphenated word.
            if at(k) == Some('-') {
                let behind = (k >= 2 && letter_at(Some(k - 2)) && letter_at(Some(k - 1)))
                    || (k >= 3
                        && letter_at(Some(k - 3))
                        && at(k - 2) == Some('-')
                        && letter_at(Some(k - 1)));
                let ahead = letter_at(Some(k + 1))
                    && ((at(k + 2) == Some('-') && letter_at(Some(k + 3)))
                        || letter_at(Some(k + 2)));
                if behind && ahead {
                    break k + 1;
                }
            }
            // (b) end of word.
            if k >= n || is_tw_whitespace(c[k]) {
                break k;
            }
            // (c) before an em-dash: (?<=wp) (?=-{2,}\w)
            if is_word_punct(c[k - 1]) && c[k] == '-' {
                let mut j = k;
                while j < n && c[j] == '-' {
                    j += 1;
                }
                if j - k >= 2 && at(j).is_some_and(is_word) {
                    break k;
                }
            }
            k += 1;
        };
        chunks.push(c[i..end].iter().collect());
        i = end;
    }
    chunks.retain(|chunk: &String| !chunk.is_empty());
    chunks
}

/// The first `n` visible characters (click `_truncate_visible`).
fn truncate_visible(text: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut visible = 0;
    let mut i = 0;
    let mut cut = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' && chars.get(i + 1) == Some(&'[') {
            let mut j = i + 2;
            while j < chars.len()
                && (chars[j] == ';' || chars[j] == '?' || chars[j].is_ascii_digit())
            {
                j += 1;
            }
            if j < chars.len() && chars[j].is_ascii_alphabetic() {
                i = j + 1;
                continue;
            }
        }
        visible += 1;
        i += 1;
        cut = i;
        if visible >= n {
            break;
        }
    }
    chars[..cut].iter().collect()
}

/// click's `TextWrapper`.
#[derive(Clone, Debug)]
pub(crate) struct TextWrapper {
    pub width: usize,
    pub initial_indent: String,
    pub subsequent_indent: String,
}

impl TextWrapper {
    pub(crate) fn new(width: usize, initial_indent: &str, subsequent_indent: &str) -> Self {
        Self {
            width,
            initial_indent: initial_indent.to_owned(),
            subsequent_indent: subsequent_indent.to_owned(),
        }
    }

    /// `_wrap_chunks` (click 8.5 version, `max_lines=None`).
    fn wrap_chunks(&self, mut chunks: Vec<String>) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        chunks.reverse();
        while !chunks.is_empty() {
            let mut cur_line: Vec<String> = Vec::new();
            let mut cur_len: usize = 0;
            let indent = if lines.is_empty() {
                &self.initial_indent
            } else {
                &self.subsequent_indent
            };
            let width = self.width as isize - term_len(indent) as isize;
            if chunks.last().is_some_and(|c| is_blank(c)) && !lines.is_empty() {
                chunks.pop();
            }
            while let Some(last) = chunks.last() {
                let n = term_len(last);
                if (cur_len + n) as isize <= width {
                    cur_len += n;
                    cur_line.push(chunks.pop().unwrap_or_default());
                } else {
                    break;
                }
            }
            if chunks.last().is_some_and(|c| term_len(c) as isize > width) {
                // `_handle_long_word` with break_long_words=True.
                let space_left = (width - cur_len as isize).max(1) as usize;
                if let Some(last) = chunks.last_mut() {
                    let cut = truncate_visible(last, space_left);
                    let rest = last.chars().skip(cut.chars().count()).collect::<String>();
                    cur_line.push(cut);
                    *last = rest;
                }
                cur_len = cur_line.iter().map(|c| term_len(c)).sum();
            }
            if cur_line.last().is_some_and(|c| is_blank(c)) {
                cur_len -= term_len(cur_line.last().map(String::as_str).unwrap_or(""));
                cur_line.pop();
            }
            let _ = cur_len;
            if !cur_line.is_empty() {
                lines.push(format!("{indent}{}", cur_line.concat()));
            }
        }
        lines
    }

    /// `TextWrapper.wrap`.
    pub(crate) fn wrap(&self, text: &str) -> Vec<String> {
        let text = expandtabs(text, 8);
        self.wrap_chunks(split_chunks(&text))
    }

    /// `TextWrapper.fill`.
    pub(crate) fn fill(&self, text: &str) -> String {
        self.wrap(text).join("\n")
    }

    /// click `TextWrapper.indent_only`.
    fn indent_only(&self, text: &str) -> String {
        splitlines(text)
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                let indent = if idx == 0 {
                    &self.initial_indent
                } else {
                    &self.subsequent_indent
                };
                format!("{indent}{line}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// `click.wrap_text`.
pub(crate) fn wrap_text(
    text: &str,
    width: usize,
    initial_indent: &str,
    subsequent_indent: &str,
    preserve_paragraphs: bool,
) -> String {
    let text = expandtabs(text, 8);
    let wrapper = TextWrapper::new(width, initial_indent, subsequent_indent);
    if !preserve_paragraphs {
        return wrapper.fill(&text);
    }
    let mut paragraphs: Vec<(usize, bool, String)> = Vec::new();
    let mut buf: Vec<String> = Vec::new();
    let mut indent: Option<usize> = None;
    let flush =
        |buf: &mut Vec<String>, indent: Option<usize>, p: &mut Vec<(usize, bool, String)>| {
            if buf.is_empty() {
                return;
            }
            if buf[0].trim_matches(is_py_whitespace) == "\x08" {
                p.push((indent.unwrap_or(0), true, buf[1..].join("\n")));
            } else {
                p.push((indent.unwrap_or(0), false, buf.join(" ")));
            }
            buf.clear();
        };
    for line in splitlines(&text) {
        if line.is_empty() {
            flush(&mut buf, indent, &mut paragraphs);
            indent = None;
        } else {
            let mut line = line;
            if indent.is_none() {
                let orig_len = term_len(&line);
                line = line.trim_start_matches(is_py_whitespace).to_owned();
                indent = Some(orig_len - term_len(&line));
            }
            buf.push(line);
        }
    }
    flush(&mut buf, indent, &mut paragraphs);

    paragraphs
        .into_iter()
        .map(|(indent, raw, text)| {
            let extra = " ".repeat(indent);
            let wrapper = TextWrapper::new(
                width,
                &format!("{initial_indent}{extra}"),
                &format!("{subsequent_indent}{extra}"),
            );
            if raw {
                wrapper.indent_only(&text)
            } else {
                wrapper.fill(&text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ---------------------------------------------------------------------------
// difflib

/// `SequenceMatcher(None, a, b).ratio()` for short strings (no autojunk:
/// that only applies when `len(b) >= 200`).
fn matcher_ratio(a: &[char], b: &[char]) -> f64 {
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    2.0 * matching_chars(a, b) as f64 / total as f64
}

/// Sum of `get_matching_blocks()` sizes (recursive longest matches, ties to
/// the earliest in `a` then `b`, like `find_longest_match`).
fn matching_chars(a: &[char], b: &[char]) -> usize {
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    let mut total = 0;
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest_match(a, b, alo, ahi, blo, bhi);
        if k > 0 {
            total += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    total
}

// Index loops mirror difflib's `find_longest_match` (indices are the result).
#[allow(clippy::needless_range_loop)]
fn find_longest_match(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
    let mut j2len: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in alo..ahi {
        let mut newj2len = std::collections::HashMap::new();
        for j in blo..bhi {
            if b[j] != a[i] {
                continue;
            }
            let k = if j > 0 {
                j2len.get(&(j - 1)).copied().unwrap_or(0)
            } else {
                0
            } + 1;
            newj2len.insert(j, k);
            if k > bestsize {
                besti = i + 1 - k;
                bestj = j + 1 - k;
                bestsize = k;
            }
        }
        j2len = newj2len;
    }
    (besti, bestj, bestsize)
}

/// `difflib.get_close_matches(word, possibilities, n=3, cutoff=0.6)`.
pub(crate) fn get_close_matches(word: &str, possibilities: &[String]) -> Vec<String> {
    let a: Vec<char> = word.chars().collect();
    let mut scored: Vec<(f64, String)> = Vec::new();
    for x in possibilities {
        let b: Vec<char> = x.chars().collect();
        // real_quick_ratio and quick_ratio are upper bounds of ratio; the
        // final filter is ratio() >= cutoff, so computing ratio directly
        // selects the same set.
        let score = matcher_ratio(&b, &a);
        if score >= 0.6 {
            scored.push((score, x.clone()));
        }
    }
    // heapq.nlargest(n, result): by score, ties by the larger string.
    scored.sort_by(|x, y| {
        y.0.partial_cmp(&x.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| y.1.cmp(&x.1))
    });
    scored.into_iter().take(3).map(|(_, x)| x).collect()
}

// ---------------------------------------------------------------------------
// str() / repr() of JSON result values

/// Python `repr(float)`.
pub(crate) fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    // Shortest round-trip digits and the decimal exponent.
    let sci = format!("{value:e}");
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        let body = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point as usize >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
        } else {
            format!(
                "{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        };
        format!("{sign}{body}")
    } else {
        let mut mantissa = digits[..1].to_owned();
        if digits.len() > 1 {
            mantissa.push('.');
            mantissa.push_str(&digits[1..]);
        }
        let exp_sign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{mantissa}e{exp_sign}{:02}", exponent.abs())
    }
}

fn number_str(number: &serde_json::Number) -> String {
    if let Some(i) = number.as_i64() {
        i.to_string()
    } else if let Some(u) = number.as_u64() {
        u.to_string()
    } else {
        float_repr(number.as_f64().unwrap_or(f64::NAN))
    }
}

/// Python `repr()` of a JSON value.
pub(crate) fn py_repr_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => crate::engines::util::py_repr(s),
        serde_json::Value::Array(items) => {
            format!(
                "[{}]",
                items
                    .iter()
                    .map(py_repr_value)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        serde_json::Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", crate::engines::util::py_repr(k), py_repr_value(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => py_str_value(other),
    }
}

/// Python `str()` of a JSON value.
pub(crate) fn py_str_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_owned(),
        serde_json::Value::Bool(true) => "True".to_owned(),
        serde_json::Value::Bool(false) => "False".to_owned(),
        serde_json::Value::Number(number) => number_str(number),
        serde_json::Value::String(s) => s.clone(),
        other => py_repr_value(other),
    }
}

/// Python truthiness of a JSON value.
pub(crate) fn py_truthy(value: &serde_json::Value) -> bool {
    crate::engines::util::py_truthy(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_match_python_textwrap() {
        // textwrap.TextWrapper()._split("Look, goof-ball -- use the -b option!")
        assert_eq!(
            split_chunks("Look, goof-ball -- use the -b option!"),
            vec![
                "Look,", " ", "goof-", "ball", " ", "--", " ", "use", " ", "the", " ", "-b", " ",
                "option!"
            ]
        );
        assert_eq!(split_chunks("a--b"), vec!["a", "--", "b"]);
        assert_eq!(split_chunks("e-mail x-y"), vec!["e-mail", " ", "x-y"]);
    }

    #[test]
    fn wrap_text_like_click() {
        assert_eq!(
            wrap_text(
                "csv, json or filename.csv|json (save the results to a csv or json file)",
                46,
                "",
                "",
                true
            ),
            "csv, json or filename.csv|json (save the\nresults to a csv or json file)"
        );
        let long = "x".repeat(10);
        assert_eq!(wrap_text(&long, 4, "", "", false), "xxxx\nxxxx\nxx");
    }

    #[test]
    fn close_matches_like_difflib() {
        let options: Vec<String> = ["--query", "--page", "--proxy", "--help"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(get_close_matches("--nope", &options), vec!["--page"]);
        assert!(get_close_matches("--zzzzzz", &options).is_empty());
    }

    #[test]
    fn python_str_and_repr() {
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(0.00001), "1e-05");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(123.456), "123.456");
        let v: serde_json::Value =
            serde_json::json!({"a": "it's", "b": 1, "c": null, "d": [true, 1.5]});
        assert_eq!(
            py_str_value(&v),
            "{'a': \"it's\", 'b': 1, 'c': None, 'd': [True, 1.5]}"
        );
        assert_eq!(py_str_value(&serde_json::Value::Null), "None");
    }
}
