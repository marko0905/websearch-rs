//! A small port of the parts of click 8.5 that the upstream `ddgs` CLI uses:
//! option parsing (`click.parser`), value conversion (`TEXT`, `INTEGER`,
//! `BOOLEAN`, `Choice`), eager `--help`, chained group dispatch, help pages
//! (`HelpFormatter`) and usage errors (`UsageError.show`).

use std::collections::HashMap;

use super::py::{get_close_matches, term_len, wrap_text};

/// A click parameter type.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ParamType {
    Text,
    Int,
    Bool,
    Choice(&'static [&'static str]),
}

impl ParamType {
    fn metavar(&self) -> String {
        match self {
            Self::Text => "TEXT".to_owned(),
            Self::Int => "INTEGER".to_owned(),
            Self::Bool => "BOOLEAN".to_owned(),
            Self::Choice(choices) => format!("[{}]", choices.join("|")),
        }
    }
}

/// A converted option value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    None,
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

impl Value {
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
    pub(crate) fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub(crate) fn as_bool(&self) -> bool {
        matches!(self, Self::Bool(true))
    }
}

/// The default of an option.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Default {
    None,
    Str(&'static str),
    Int(i64),
    Bool(bool),
    List(&'static [&'static str]),
}

/// A `@click.option(...)`.
#[derive(Clone, Debug)]
pub(crate) struct OptSpec {
    pub name: &'static str,
    pub decls: &'static [&'static str],
    pub ty: ParamType,
    pub is_flag: bool,
    pub multiple: bool,
    pub required: bool,
    pub default: Default,
    pub help: &'static str,
}

impl OptSpec {
    pub(crate) const fn value(
        name: &'static str,
        decls: &'static [&'static str],
        ty: ParamType,
        default: Default,
        help: &'static str,
    ) -> Self {
        Self {
            name,
            decls,
            ty,
            is_flag: false,
            multiple: false,
            required: false,
            default,
            help,
        }
    }
    pub(crate) const fn flag(
        name: &'static str,
        decls: &'static [&'static str],
        help: &'static str,
    ) -> Self {
        Self {
            name,
            decls,
            ty: ParamType::Bool,
            is_flag: true,
            multiple: false,
            required: false,
            default: Default::Bool(false),
            help,
        }
    }
    pub(crate) const fn multiple(mut self) -> Self {
        self.multiple = true;
        self
    }
    pub(crate) const fn required(mut self) -> Self {
        self.required = true;
        self
    }
}

/// A `@cli.command()`.
#[derive(Clone, Debug)]
pub(crate) struct CommandSpec {
    pub name: &'static str,
    /// `inspect.cleandoc(docstring)`.
    pub help: &'static str,
    pub options: Vec<OptSpec>,
}

const HELP_HELP: &str = "Show this message and exit.";

/// What a usage error prints (`UsageError.show`): with a context it prints
/// the usage line and a hint first; without one only `Error: ...`.
#[derive(Debug)]
pub(crate) struct UsageError {
    pub message: String,
    /// `(usage line, help hint command)`, when the error has a context.
    pub ctx: Option<(String, String)>,
}

impl UsageError {
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        if let Some((usage, command)) = &self.ctx {
            out.push_str(usage);
            out.push_str(&format!("Try '{command} --help' for help.\n\n"));
        }
        out.push_str(&format!("Error: {}\n", self.message));
        out
    }
}

/// The outcome of parsing one command's arguments.
pub(crate) enum Parsed {
    /// `--help` was given: print this page to stdout and exit 0.
    Help(String),
    /// Converted values by parameter name, and the arguments left for the
    /// next chained command.
    Values(HashMap<&'static str, Value>, Vec<String>),
}

/// Terminal width as click's `HelpFormatter` computes it:
/// `max(min(columns, 80) - 2, 50)`.
pub(crate) fn help_width() -> usize {
    let columns = terminal_columns();
    (columns.min(80).saturating_sub(2)).max(50)
}

/// `shutil.get_terminal_size().columns`: `COLUMNS`, else the terminal, else 80.
pub(crate) fn terminal_columns() -> usize {
    if let Some(columns) = std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.trim().parse::<usize>().ok())
        && columns > 0
    {
        return columns;
    }
    terminal_size_from_os().unwrap_or(80)
}

#[cfg(unix)]
fn terminal_size_from_os() -> Option<usize> {
    // SAFETY: ioctl(TIOCGWINSZ) writes a winsize into a zeroed, owned struct.
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) == 0 && size.ws_col > 0 {
            return Some(usize::from(size.ws_col));
        }
    }
    None
}

#[cfg(not(unix))]
fn terminal_size_from_os() -> Option<usize> {
    None
}

/// click `HelpFormatter`.
struct Formatter {
    width: usize,
    indent: usize,
    buffer: String,
}

impl Formatter {
    fn new() -> Self {
        Self {
            width: help_width(),
            indent: 0,
            buffer: String::new(),
        }
    }

    fn write_usage(&mut self, prog: &str, args: &str) {
        let prefix = "Usage: ";
        let usage_prefix = format!("{:>width$}{prog} ", prefix, width = self.indent);
        let text_width = self.width - self.indent;
        if args.is_empty() {
            self.buffer.push_str(usage_prefix.trim_end_matches(' '));
            self.buffer.push('\n');
            return;
        }
        if text_width >= term_len(&usage_prefix) + 20 {
            let indent = " ".repeat(term_len(&usage_prefix));
            self.buffer
                .push_str(&wrap_text(args, text_width, &usage_prefix, &indent, false));
        } else {
            self.buffer.push_str(&usage_prefix);
            self.buffer.push('\n');
            let indent = " ".repeat(self.indent.max(term_len(prefix)) + 4);
            self.buffer
                .push_str(&wrap_text(args, text_width, &indent, &indent, false));
        }
        self.buffer.push('\n');
    }

    fn write_paragraph(&mut self) {
        if !self.buffer.is_empty() {
            self.buffer.push('\n');
        }
    }

    fn write_heading(&mut self, heading: &str) {
        self.buffer
            .push_str(&format!("{:>width$}{heading}:\n", "", width = self.indent));
    }

    fn write_text(&mut self, text: &str) {
        let indent = " ".repeat(self.indent);
        self.buffer
            .push_str(&wrap_text(text, self.width, &indent, &indent, true));
        self.buffer.push('\n');
    }

    fn write_dl(&mut self, rows: &[(String, String)]) {
        let col_max = 30;
        let col_spacing = 2;
        let first_width = rows.iter().map(|(a, _)| term_len(a)).max().unwrap_or(0);
        let first_col = first_width.min(col_max) + col_spacing;
        for (first, second) in rows {
            self.buffer
                .push_str(&format!("{:>width$}{first}", "", width = self.indent));
            if second.is_empty() {
                self.buffer.push('\n');
                continue;
            }
            if term_len(first) <= first_col - col_spacing {
                self.buffer
                    .push_str(&" ".repeat(first_col - term_len(first)));
            } else {
                self.buffer.push('\n');
                self.buffer.push_str(&" ".repeat(first_col + self.indent));
            }
            let text_width = (self.width as isize - first_col as isize - 2).max(10) as usize;
            let wrapped = wrap_text(second, text_width, "", "", true);
            let lines = super::py::splitlines(&wrapped);
            if let Some((head, tail)) = lines.split_first() {
                self.buffer.push_str(&format!("{head}\n"));
                for line in tail {
                    self.buffer.push_str(&format!(
                        "{:>width$}{line}\n",
                        "",
                        width = first_col + self.indent
                    ));
                }
            } else {
                self.buffer.push('\n');
            }
        }
    }

    fn section(&mut self, name: &str, rows: &[(String, String)]) {
        self.write_paragraph();
        self.write_heading(name);
        self.indent += 2;
        self.write_dl(rows);
        self.indent -= 2;
    }
}

/// `join_options`: declarations sorted by prefix length (stable), joined.
fn join_options(decls: &[&str]) -> String {
    let mut sorted: Vec<&str> = decls.to_vec();
    sorted.sort_by_key(|decl| if decl.starts_with("--") { 2 } else { 1 });
    sorted.join(", ")
}

fn option_record(spec: &OptSpec) -> (String, String) {
    let mut first = join_options(spec.decls);
    if !spec.is_flag {
        first.push(' ');
        first.push_str(&spec.ty.metavar());
    }
    let mut help = spec.help.to_owned();
    if spec.required {
        help = if help.is_empty() {
            "[required]".to_owned()
        } else {
            format!("{help}  [required]")
        };
    }
    (first, help)
}

/// The help page of one command.
pub(crate) fn command_help(prog: &str, command: &CommandSpec) -> String {
    let mut f = Formatter::new();
    f.write_usage(&format!("{prog} {}", command.name), "[OPTIONS]");
    if !command.help.is_empty() {
        f.write_paragraph();
        f.indent += 2;
        f.write_text(command.help);
        f.indent -= 2;
    }
    let mut rows: Vec<(String, String)> = command.options.iter().map(option_record).collect();
    rows.push(("--help".to_owned(), HELP_HELP.to_owned()));
    f.section("Options", &rows);
    f.buffer
}

/// The usage line of a command (as a usage error prints it).
fn command_usage(prog: &str, command: &CommandSpec) -> String {
    let mut f = Formatter::new();
    f.write_usage(&format!("{prog} {}", command.name), "[OPTIONS]");
    f.buffer
}

const GROUP_ARGS: &str = "[OPTIONS] COMMAND1 [ARGS]... [COMMAND2 [ARGS]...]...";

/// The group help page (commands sorted by name, with their short help).
pub(crate) fn group_help(prog: &str, help: &str, commands: &[CommandSpec]) -> String {
    let mut f = Formatter::new();
    f.write_usage(prog, GROUP_ARGS);
    f.write_paragraph();
    f.indent += 2;
    f.write_text(help);
    f.indent -= 2;
    f.section("Options", &[("--help".to_owned(), HELP_HELP.to_owned())]);
    let mut sorted: Vec<&CommandSpec> = commands.iter().collect();
    sorted.sort_by_key(|c| c.name);
    let longest = sorted.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let limit = f.width.saturating_sub(6 + longest);
    let rows: Vec<(String, String)> = sorted
        .iter()
        .map(|c| (c.name.to_owned(), short_help(c.help, limit)))
        .collect();
    f.section("Commands", &rows);
    f.buffer
}

fn group_usage(prog: &str) -> String {
    let mut f = Formatter::new();
    f.write_usage(prog, GROUP_ARGS);
    f.buffer
}

/// click `_make_default_short_help(help, max_length)`, then `.strip()`.
fn short_help(help: &str, max_length: usize) -> String {
    let help = match help.find("\n\n") {
        Some(end) => &help[..end],
        None => help,
    };
    let mut words: Vec<&str> = help
        .split(crate::normalize::is_py_whitespace)
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return String::new();
    }
    if words[0] == "\x08" {
        words.remove(0);
    }
    let len = |w: &str| w.chars().count();
    let mut total_length = 0usize;
    let last_index = words.len().saturating_sub(1);
    let mut stop = None;
    for (i, word) in words.iter().enumerate() {
        total_length += len(word) + usize::from(i > 0);
        if total_length > max_length {
            stop = Some(i);
            break;
        }
        if word.ends_with('.') {
            return words[..=i].join(" ");
        }
        if total_length == max_length && i != last_index {
            stop = Some(i);
            break;
        }
    }
    let Some(mut i) = stop else {
        return words.join(" ");
    };
    total_length += 3;
    while i > 0 {
        total_length -= len(words[i]) + usize::from(i > 0);
        if total_length <= max_length {
            break;
        }
        i -= 1;
    }
    format!("{}...", words[..i].join(" "))
}

/// Python `int(value)` for a `str`.
fn python_int(value: &str) -> Option<i64> {
    let trimmed = value.trim_matches(crate::normalize::is_py_whitespace);
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let without: String = digits.chars().filter(|c| *c != '_').collect();
    let n = crate::normalize::python_int(&without)?;
    Some(if negative { -n } else { n })
}

fn display_name(spec: &OptSpec) -> String {
    spec.decls
        .iter()
        .map(|d| format!("'{d}'"))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn py_repr(value: &str) -> String {
    crate::engines::util::py_repr(value)
}

fn convert(spec: &OptSpec, raw: &str) -> Result<Value, String> {
    match spec.ty {
        ParamType::Text => Ok(Value::Str(raw.to_owned())),
        ParamType::Int => python_int(raw)
            .map(Value::Int)
            .ok_or_else(|| format!("{} is not a valid integer.", py_repr(raw))),
        ParamType::Bool => {
            let normalized = raw
                .trim_matches(crate::normalize::is_py_whitespace)
                .to_lowercase();
            match normalized.as_str() {
                "1" | "yes" | "true" | "on" | "t" | "y" => Ok(Value::Bool(true)),
                "0" | "no" | "false" | "off" | "f" | "n" | "" => Ok(Value::Bool(false)),
                _ => Err(format!(
                    "{} is not a valid boolean. Recognized values: , 0, 1, f, false, n, no, off, on, t, true, y, yes",
                    py_repr(raw)
                )),
            }
        }
        ParamType::Choice(choices) => {
            if choices.contains(&raw) {
                Ok(Value::Str(raw.to_owned()))
            } else {
                let list = choices
                    .iter()
                    .map(|c| py_repr(c))
                    .collect::<Vec<_>>()
                    .join(", ");
                if choices.len() == 1 {
                    Err(format!("{} is not {list}.", py_repr(raw)))
                } else {
                    Err(format!("{} is not one of {list}.", py_repr(raw)))
                }
            }
        }
    }
}

fn default_value(spec: &OptSpec) -> Value {
    match spec.default {
        Default::None => Value::None,
        Default::Str(s) => Value::Str(s.to_owned()),
        Default::Int(i) => Value::Int(i),
        Default::Bool(b) => Value::Bool(b),
        Default::List(items) => Value::List(items.iter().map(|s| (*s).to_owned()).collect()),
    }
}

/// `split_opt`: a decl is short when it is one prefix char plus one char.
fn is_short(decl: &str) -> bool {
    let mut chars = decl.chars();
    chars.next() == Some('-') && chars.next().is_some_and(|c| c != '-') && chars.next().is_none()
}

/// Parse one command's arguments (`Command.make_context` in a chain:
/// `allow_interspersed_args=False`, `allow_extra_args=True`).
pub(crate) fn parse_command(
    prog: &str,
    command: &CommandSpec,
    args: &[String],
) -> Result<Parsed, UsageError> {
    let ctx = || {
        Some((
            command_usage(prog, command),
            format!("{prog} {}", command.name),
        ))
    };
    let mut long: Vec<(&'static str, usize)> = Vec::new(); // decl → option index (usize::MAX = --help)
    let mut short: Vec<(&'static str, usize)> = Vec::new();
    for (index, spec) in command.options.iter().enumerate() {
        for decl in spec.decls {
            if is_short(decl) {
                short.push((decl, index));
            } else {
                long.push((decl, index));
            }
        }
    }
    long.push(("--help", usize::MAX));
    let find = |table: &[(&'static str, usize)], name: &str| {
        table.iter().find(|(d, _)| *d == name).map(|(_, i)| *i)
    };

    // Raw occurrences in invocation order: (option index, raw value).
    let mut seen: Vec<(usize, Option<String>)> = Vec::new();
    let mut rargs: Vec<String> = args.to_vec();
    let mut i = 0;
    while i < rargs.len() {
        let arg = rargs[i].clone();
        if arg == "--" {
            i += 1;
            break;
        }
        if !(arg.starts_with('-') && arg.chars().count() > 1) {
            break; // interspersed args are not allowed: the rest is for the next command.
        }
        i += 1;
        let (opt, explicit) = match arg.split_once('=') {
            Some((o, v)) => (o.to_owned(), Some(v.to_owned())),
            None => (arg.clone(), None),
        };
        if let Some(index) = find(&long, &opt) {
            let takes_value = index != usize::MAX && !command.options[index].is_flag;
            if takes_value {
                let value = match explicit {
                    Some(v) => v,
                    None => {
                        if i >= rargs.len() {
                            return Err(UsageError {
                                message: format!("Option {} requires an argument.", py_repr(&opt)),
                                ctx: None,
                            });
                        }
                        i += 1;
                        rargs[i - 1].clone()
                    }
                };
                seen.push((index, Some(value)));
            } else if explicit.is_some() {
                return Err(UsageError {
                    message: format!("Option {} does not take a value.", py_repr(&opt)),
                    ctx: None,
                });
            } else {
                seen.push((index, None));
            }
            continue;
        }
        if arg.starts_with("--") {
            let possibilities: Vec<String> = long.iter().map(|(d, _)| (*d).to_owned()).collect();
            return Err(no_such_option(&opt, &possibilities, ctx()));
        }
        // `_match_short_opt` over each character.
        let chars: Vec<char> = arg.chars().collect();
        let mut pos = 1;
        while pos < chars.len() {
            let name = format!("-{}", chars[pos]);
            pos += 1;
            let Some(index) = find(&short, &name) else {
                return Err(UsageError {
                    message: format!("No such option {}.", py_repr(&name)),
                    ctx: ctx(),
                });
            };
            if command.options[index].is_flag {
                seen.push((index, None));
                continue;
            }
            let value = if pos < chars.len() {
                let rest: String = chars[pos..].iter().collect();
                pos = chars.len();
                rest
            } else {
                if i >= rargs.len() {
                    return Err(UsageError {
                        message: format!("Option {} requires an argument.", py_repr(&name)),
                        ctx: None,
                    });
                }
                i += 1;
                rargs[i - 1].clone()
            };
            seen.push((index, Some(value)));
        }
    }
    let rest: Vec<String> = rargs.split_off(i);

    // Eager `--help` first.
    if seen.iter().any(|(index, _)| *index == usize::MAX) {
        return Ok(Parsed::Help(command_help(prog, command)));
    }

    // Processing order: invocation order, then the rest in declaration order.
    let mut order: Vec<usize> = Vec::new();
    for (index, _) in &seen {
        if !order.contains(index) {
            order.push(*index);
        }
    }
    for index in 0..command.options.len() {
        if !order.contains(&index) {
            order.push(index);
        }
    }

    let mut values: HashMap<&'static str, Value> = HashMap::new();
    for index in order {
        let spec = &command.options[index];
        let raws: Vec<&Option<String>> = seen
            .iter()
            .filter(|(i, _)| *i == index)
            .map(|(_, v)| v)
            .collect();
        let value = if raws.is_empty() {
            if spec.required {
                return Err(UsageError {
                    message: format!("Missing option {}.", display_name(spec)),
                    ctx: ctx(),
                });
            }
            default_value(spec)
        } else if spec.is_flag {
            Value::Bool(true)
        } else if spec.multiple {
            let mut items = Vec::new();
            for raw in &raws {
                let raw = raw.as_deref().unwrap_or("");
                match convert(spec, raw) {
                    Ok(Value::Str(s)) => items.push(s),
                    Ok(_) => {}
                    Err(message) => return Err(bad_parameter(spec, &message, ctx())),
                }
            }
            Value::List(items)
        } else {
            // The last occurrence wins.
            let raw = raws.last().and_then(|r| r.as_deref()).unwrap_or("");
            match convert(spec, raw) {
                Ok(value) => value,
                Err(message) => return Err(bad_parameter(spec, &message, ctx())),
            }
        };
        values.insert(spec.name, value);
    }
    Ok(Parsed::Values(values, rest))
}

fn bad_parameter(spec: &OptSpec, message: &str, ctx: Option<(String, String)>) -> UsageError {
    UsageError {
        message: format!("Invalid value for {}: {message}", display_name(spec)),
        ctx,
    }
}

fn format_possibilities(possibilities: &[String]) -> String {
    let mut sorted = possibilities.to_vec();
    sorted.sort();
    let list = sorted
        .iter()
        .map(|p| py_repr(p))
        .collect::<Vec<_>>()
        .join(", ");
    if sorted.len() == 1 {
        format!("Did you mean {list}?")
    } else {
        format!("(Did you mean one of: {list}?)")
    }
}

fn no_such_option(
    name: &str,
    possibilities: &[String],
    ctx: Option<(String, String)>,
) -> UsageError {
    let matches = get_close_matches(name, possibilities);
    let mut message = format!("No such option {}.", py_repr(name));
    if !matches.is_empty() {
        message = format!("{message} {}", format_possibilities(&matches));
    }
    UsageError { message, ctx }
}

/// The result of parsing the whole command line.
pub(crate) enum Dispatch {
    /// Print to stdout and exit 0.
    Help(String),
    /// Print to stderr and exit 2 (no arguments: `no_args_is_help`).
    HelpError(String),
    /// Run these commands in order.
    Run(Vec<(&'static str, HashMap<&'static str, Value>)>),
}

/// `MultiCommand.main` for a `@click.group(chain=True)`.
pub(crate) fn dispatch(
    prog: &str,
    group_help_text: &str,
    commands: &[CommandSpec],
    args: &[String],
) -> Result<Dispatch, UsageError> {
    let group_ctx = || Some((group_usage(prog), prog.to_owned()));
    if args.is_empty() {
        return Ok(Dispatch::HelpError(group_help(
            prog,
            group_help_text,
            commands,
        )));
    }
    // Group options: only `--help`; parsing stops at the first non-option.
    let mut i = 0;
    if let Some(arg) = args.first() {
        if arg == "--" {
            i = 1;
        } else if arg.starts_with('-') && arg.chars().count() > 1 {
            let (opt, explicit) = match arg.split_once('=') {
                Some((o, v)) => (o, Some(v)),
                None => (arg.as_str(), None),
            };
            if opt == "--help" {
                if explicit.is_some() {
                    return Err(UsageError {
                        message: "Option '--help' does not take a value.".to_owned(),
                        ctx: None,
                    });
                }
                return Ok(Dispatch::Help(group_help(prog, group_help_text, commands)));
            }
            if arg.starts_with("--") {
                return Err(no_such_option(opt, &["--help".to_owned()], group_ctx()));
            }
            let first: String = arg.chars().take(2).collect();
            return Err(UsageError {
                message: format!("No such option {}.", py_repr(&first)),
                ctx: group_ctx(),
            });
        }
    }
    let mut rest: Vec<String> = args[i..].to_vec();
    if rest.is_empty() {
        return Err(UsageError {
            message: "Missing command.".to_owned(),
            ctx: group_ctx(),
        });
    }
    let mut runs = Vec::new();
    while !rest.is_empty() {
        let name = rest.remove(0);
        let Some(command) = commands.iter().find(|c| c.name == name) else {
            let names: Vec<String> = commands.iter().map(|c| c.name.to_owned()).collect();
            let matches = get_close_matches(&name, &names);
            let mut message = format!("No such command {}.", py_repr(&name));
            if !matches.is_empty() {
                message = format!("{message} {}", format_possibilities(&matches));
            }
            return Err(UsageError {
                message,
                ctx: group_ctx(),
            });
        };
        match parse_command(prog, command, &rest)? {
            Parsed::Help(page) => return Ok(Dispatch::Help(page)),
            Parsed::Values(values, remaining) => {
                runs.push((command.name, values));
                rest = remaining;
            }
        }
    }
    Ok(Dispatch::Run(runs))
}
