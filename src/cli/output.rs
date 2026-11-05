//! Ports of the upstream CLI output helpers (`ddgs/cli.py`): `_print_data`,
//! `_save_data`/`_save_json`/`_save_csv`, `_sanitize_query`,
//! `_download_results`/`_download_file`, plus `click.style`, `click.echo` and
//! `click.progressbar` as they use them.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use super::PyError;
use super::py::{py_str_value, py_truthy, wrap_text};
use crate::http::{HttpClient, HttpConfig, Verify};

/// One result as a Python dict (field order kept).
pub(crate) type Row = Map<String, Value>;

/// `COLORS` in `ddgs/cli.py`.
const COLORS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "white",
    "bright_white",
];

fn ansi_color(name: &str) -> u8 {
    match name {
        "black" => 30,
        "red" => 31,
        "green" => 32,
        "yellow" => 33,
        "blue" => 34,
        "magenta" => 35,
        "cyan" => 36,
        "white" => 37,
        "bright_black" => 90,
        "bright_red" => 91,
        "bright_green" => 92,
        "bright_yellow" => 93,
        "bright_blue" => 94,
        "bright_magenta" => 95,
        "bright_cyan" => 96,
        "bright_white" => 97,
        _ => 39,
    }
}

/// `click.style(text, fg=fg, bg=bg, overline=overline)`.
fn style(text: &str, fg: &str, bg: &str, overline: bool) -> String {
    let mut out = format!("\x1b[{}m\x1b[{}m", ansi_color(fg), ansi_color(bg) + 10);
    if overline {
        out.push_str("\x1b[53m");
    }
    out.push_str(text);
    out.push_str("\x1b[0m");
    out
}

/// `click.echo(message)` to stdout: ANSI codes are stripped when stdout is
/// not a terminal.
pub(crate) fn echo(message: &str) {
    let mut stdout = std::io::stdout().lock();
    let text = if stdout.is_terminal() {
        message.to_owned()
    } else {
        strip(message)
    };
    let _ = writeln!(stdout, "{text}");
    let _ = stdout.flush();
}

/// `click.echo(message, err=True)`.
pub(crate) fn echo_err(message: &str) {
    let mut stderr = std::io::stderr().lock();
    let text = if stderr.is_terminal() {
        message.to_owned()
    } else {
        strip(message)
    };
    let _ = writeln!(stderr, "{text}");
}

fn strip(text: &str) -> String {
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

/// `f"{title:<12}"`.
fn pad12(title: &str) -> String {
    let len = title.chars().count();
    if len >= 12 {
        title.to_owned()
    } else {
        format!("{title}{}", " ".repeat(12 - len))
    }
}

/// Upstream `_print_data` on stdout. In a terminal it waits for Enter
/// (`input()`) after each result; an end of input raises `EOFError` there,
/// and here.
pub(crate) fn print_data(data: &[Row], no_color: bool) -> Result<(), PyError> {
    let is_tty = std::io::stdout().is_terminal();
    let mut stdout = std::io::stdout();
    let mut stdin = std::io::stdin().lock();
    print_data_to(&mut stdout, is_tty, &mut stdin, data, no_color)
}

/// `_print_data` with an explicit output, terminal flag and input.
pub(crate) fn print_data_to(
    out: &mut dyn Write,
    is_tty: bool,
    input: &mut dyn BufRead,
    data: &[Row],
    no_color: bool,
) -> Result<(), PyError> {
    let no_color = no_color || !is_tty;
    // click.echo: ANSI codes are stripped when the output is not a terminal.
    let mut echo = |line: String| {
        let text = if is_tty { line } else { strip(&line) };
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    };
    for (i, row) in data.iter().enumerate() {
        let i = i + 1;
        let sep = if is_tty {
            format!("{i}.\t    {}", "=".repeat(78))
        } else {
            format!("{i}.")
        };
        echo(style(&sep, "white", "black", false));
        for (j, (key, value)) in row.iter().enumerate() {
            let j = j + 1;
            let (title, text) = if py_truthy(value) {
                let width = if matches!(
                    key.as_str(),
                    "content" | "href" | "image" | "source" | "thumbnail" | "url"
                ) {
                    300
                } else {
                    78
                };
                let title = if key == "detected_language" {
                    "language"
                } else {
                    key.as_str()
                };
                let text = wrap_text(&py_str_value(value), width, "", &" ".repeat(12), true);
                (title.to_owned(), text)
            } else {
                (key.clone(), py_str_value(value))
            };
            let fg = if no_color {
                "white"
            } else {
                COLORS
                    .get(j)
                    .copied()
                    .ok_or_else(|| PyError::new("KeyError", &j.to_string()))?
            };
            echo(style(
                &format!("{}{text}", pad12(&title)),
                fg,
                "black",
                true,
            ));
        }
        if is_tty {
            let mut line = String::new();
            let read = input.read_line(&mut line).unwrap_or(0);
            if read == 0 {
                return Err(PyError::bare("EOFError"));
            }
        }
    }
    Ok(())
}

/// Upstream `_sanitize_query`.
pub(crate) fn sanitize_query(query: &str) -> String {
    query
        .replace("filetype", "")
        .replace(':', "")
        .replace('"', "'")
        .replace("site", "")
        .replace([' ', '/', '\\'], "_")
    // Upstream ends with `.replace(" ", "")`, a no-op after the line above.
}

/// `f"{datetime.now(tz=timezone.utc):%Y%m%d_%H%M%S}"`.
fn utc_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = crate::normalize::civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}_{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Upstream `_save_data`.
pub(crate) fn save_data(
    query: &str,
    data: &[Row],
    function_name: &str,
    filename: Option<&str>,
) -> Result<(), PyError> {
    let (name, ext): (Option<String>, Option<String>) = match filename {
        Some(f) if f.ends_with(".csv") || f.ends_with(".json") => match f.rsplit_once('.') {
            Some((name, ext)) => (Some(name.to_owned()), Some(ext.to_owned())),
            None => (None, Some(f.to_owned())),
        },
        other => (None, other.map(str::to_owned)),
    };
    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{function_name}_{query}_{}", utc_stamp()));
    match ext.as_deref() {
        Some("csv") => save_csv(&format!("{name}.csv"), data),
        Some("json") => save_json(&format!("{name}.json"), data),
        _ => Ok(()),
    }
}

/// Upstream `_save_json`: `json.dumps(data, ensure_ascii=False, indent=2)`.
pub(crate) fn save_json(path: &str, data: &[Row]) -> Result<(), PyError> {
    let text = serde_json::to_string_pretty(data)
        .map_err(|e| PyError::new("TypeError", &e.to_string()))?;
    std::fs::write(path, text).map_err(|e| PyError::os(&e, path))
}

/// Python `csv` field: `None` → `""`, `str` as is, other values → `str(v)`.
fn csv_field(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        other => py_str_value(other),
    }
}

/// `csv.writer` with the default dialect and `QUOTE_MINIMAL`.
fn csv_quote(field: &str) -> String {
    if field.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// Upstream `_save_csv`.
pub(crate) fn save_csv(path: &str, data: &[Row]) -> Result<(), PyError> {
    let mut out = String::new();
    if let Some(first) = data.first() {
        let headers: Vec<&String> = first.keys().collect();
        out.push_str(
            &headers
                .iter()
                .map(|h| csv_quote(h))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("\r\n");
        for row in data {
            let fields: Vec<String> = headers
                .iter()
                .map(|h| csv_quote(&row.get(h.as_str()).map(csv_field).unwrap_or_default()))
                .collect();
            out.push_str(&fields.join(","));
            out.push_str("\r\n");
        }
    }
    std::fs::write(path, out).map_err(|e| PyError::os(&e, path))
}

/// click's `ProgressBar` with `label="Downloading", show_percent=True,
/// show_pos=True, width=50` (template `"%(label)s  [%(bar)s]  %(info)s"`).
struct ProgressBar {
    length: usize,
    pos: usize,
    is_atty: bool,
    finished: bool,
    start: Instant,
    last_eta: Instant,
    avg: Vec<f64>,
    eta_known: bool,
    max_width: Option<usize>,
    last_line: Option<String>,
}

impl ProgressBar {
    const LABEL: &'static str = "Downloading";
    const WIDTH: usize = 50;

    fn new(length: usize) -> Self {
        let now = Instant::now();
        Self {
            length,
            pos: 0,
            is_atty: std::io::stdout().is_terminal(),
            finished: false,
            start: now,
            last_eta: now,
            avg: Vec::new(),
            eta_known: false,
            max_width: None,
            last_line: None,
        }
    }

    fn pct(&self) -> f64 {
        if self.finished {
            return 1.0;
        }
        (self.pos as f64 / (self.length.max(1) as f64)).min(1.0)
    }

    fn eta(&self) -> f64 {
        if self.finished || self.avg.is_empty() {
            return 0.0;
        }
        let per = self.avg.iter().sum::<f64>() / self.avg.len() as f64;
        per * (self.length.saturating_sub(self.pos)) as f64
    }

    fn format_eta(&self) -> String {
        if !self.eta_known {
            return String::new();
        }
        let mut t = self.eta() as u64;
        let seconds = t % 60;
        t /= 60;
        let minutes = t % 60;
        t /= 60;
        let hours = t % 24;
        t /= 24;
        if t > 0 {
            format!("{t}d {hours:02}:{minutes:02}:{seconds:02}")
        } else {
            format!("{hours:02}:{minutes:02}:{seconds:02}")
        }
    }

    fn line(&self) -> String {
        let filled = (self.pct() * Self::WIDTH as f64) as usize;
        let bar = format!("{}{}", "#".repeat(filled), "-".repeat(Self::WIDTH - filled));
        let mut info = vec![
            format!("{}/{}", self.pos, self.length),
            format!("{:>4}%", (self.pct() * 100.0) as u64)[1..].to_owned(),
        ];
        if self.eta_known && !self.finished {
            info.push(self.format_eta());
        }
        format!("{}  [{bar}]  {}", Self::LABEL, info.join("  "))
            .trim_end()
            .to_owned()
    }

    fn render(&mut self) {
        let mut stdout = std::io::stdout().lock();
        if !self.is_atty {
            if self.last_line.as_deref() != Some(Self::LABEL) {
                self.last_line = Some(Self::LABEL.to_owned());
                let _ = writeln!(stdout, "{}", Self::LABEL);
            }
            return;
        }
        let clear_width = self.max_width.unwrap_or(Self::WIDTH);
        let line = self.line();
        let line_len = line.chars().count();
        if self.max_width.is_none_or(|w| w < line_len) {
            self.max_width = Some(line_len);
        }
        let full = format!(
            "\r{line}{}",
            " ".repeat(clear_width.saturating_sub(line_len))
        );
        if self.last_line.as_deref() != Some(full.as_str()) {
            let _ = write!(stdout, "{full}");
            let _ = stdout.flush();
            self.last_line = Some(full);
        }
    }

    fn update(&mut self) {
        self.pos += 1;
        if self.pos >= self.length {
            self.finished = true;
        }
        if self.last_eta.elapsed() >= Duration::from_secs(1) {
            self.last_eta = Instant::now();
            let elapsed = self.start.elapsed().as_secs_f64();
            let step = if self.pos > 0 {
                elapsed / self.pos as f64
            } else {
                elapsed
            };
            let keep = self.avg.len().saturating_sub(6);
            self.avg.drain(..keep);
            self.avg.push(step);
            self.eta_known = true;
        }
        self.render();
    }

    fn finish(&mut self) {
        if self.is_atty {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout);
        }
    }
}

/// Upstream `_download_file`: a fresh impersonating client (timeout 10 s);
/// the body is written only for status 200; every error is swallowed.
async fn download_file(
    url: String,
    dir: PathBuf,
    filename: String,
    proxy: Option<String>,
    verify: bool,
) {
    let config = HttpConfig {
        timeout: Some(Duration::from_secs(10)),
        proxy,
        verify: if verify {
            Verify::Enabled
        } else {
            Verify::Disabled
        },
        max_retries: 0,
        retry_delay: Duration::ZERO,
    };
    let Ok(client) = HttpClient::new(&config) else {
        return;
    };
    let Ok(response) = client.fetch_raw(&url, &[]).await else {
        return;
    };
    if response.status == 200 {
        let name: String = filename.chars().take(200).collect();
        let _ = std::fs::write(dir.join(name), response.body);
    }
}

/// Upstream `_download_results`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_results(
    query: &str,
    results: &[Row],
    function_name: &str,
    proxy: Option<&str>,
    threads: i64,
    pathname: Option<&str>,
    verify: bool,
) -> Result<(), PyError> {
    let path = pathname
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{function_name}_{query}_{}", utc_stamp()));
    std::fs::create_dir_all(&path).map_err(|e| PyError::os(&e, &path))?;
    if threads <= 0 {
        return Err(PyError::new(
            "ValueError",
            "max_workers must be greater than 0",
        ));
    }
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(threads as usize));
    let mut tasks = tokio::task::JoinSet::new();
    let mut failure = None;
    for (i, row) in results.iter().enumerate() {
        let key = if function_name == "images" {
            "image"
        } else {
            "href"
        };
        let url = match row.get(key) {
            Some(Value::String(url)) => url.clone(),
            Some(Value::Null) => {
                failure = Some(PyError::new(
                    "AttributeError",
                    "'NoneType' object has no attribute 'split'",
                ));
                break;
            }
            _ => {
                failure = Some(PyError::new(
                    "KeyError",
                    &crate::engines::util::py_repr(key),
                ));
                break;
            }
        };
        let last = url.split('/').next_back().unwrap_or("");
        let filename = crate::normalize::unquote(last.split('?').next().unwrap_or(""));
        let permit = semaphore.clone();
        let dir = Path::new(&path).to_path_buf();
        let proxy = proxy.map(str::to_owned);
        tasks.spawn(async move {
            let _permit = permit.acquire_owned().await;
            download_file(url, dir, format!("{}_{filename}", i + 1), proxy, verify).await;
        });
    }
    if let Some(failure) = failure {
        while tasks.join_next().await.is_some() {}
        return Err(failure);
    }
    let mut bar = ProgressBar::new(tasks.len());
    bar.render();
    while tasks.join_next().await.is_some() {
        bar.update();
    }
    bar.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Printed text, CSV and JSON equal upstream's `_print_data`,
    /// `_save_csv` and `_save_json` for the same rows
    /// (`fixtures/cli/output_reference.json`, from ddgs 9.16.1).
    #[test]
    fn output_matches_upstream_reference() {
        let reference: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../fixtures/cli/output_reference.json")).unwrap();
        let dir = std::env::temp_dir().join(format!("websearch-output-ref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for case in &reference {
            let name = case["name"].as_str().unwrap();
            let rows: Vec<Row> = case["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();

            let mut printed = Vec::new();
            let mut no_input: &[u8] = &[];
            print_data_to(&mut printed, false, &mut no_input, &rows, false).unwrap();
            assert_eq!(
                String::from_utf8(printed).unwrap(),
                case["print"].as_str().unwrap(),
                "{name}: print"
            );

            let csv_path = dir.join(format!("{name}.csv"));
            save_csv(csv_path.to_str().unwrap(), &rows).unwrap();
            assert_eq!(
                std::fs::read_to_string(&csv_path).unwrap(),
                case["csv"].as_str().unwrap(),
                "{name}: csv"
            );

            let json_path = dir.join(format!("{name}.json"));
            save_json(json_path.to_str().unwrap(), &rows).unwrap();
            assert_eq!(
                std::fs::read_to_string(&json_path).unwrap(),
                case["json"].as_str().unwrap(),
                "{name}: json"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_like_upstream() {
        assert_eq!(
            sanitize_query("site:example.com filetype:pdf \"a b\""),
            "example.com_pdf_'a_b'"
        );
    }

    #[test]
    fn csv_quoting_like_python() {
        assert_eq!(csv_quote("a,b"), "\"a,b\"");
        assert_eq!(csv_quote("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_quote("plain"), "plain");
        assert_eq!(csv_field(&Value::Null), "");
        assert_eq!(csv_field(&serde_json::json!({"a": 1})), "{'a': 1}");
    }

    #[test]
    fn style_codes_like_click() {
        assert_eq!(
            style("x", "red", "black", true),
            "\x1b[31m\x1b[40m\x1b[53mx\x1b[0m"
        );
    }
}
