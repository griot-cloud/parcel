//! How the command line looks: one place for colours, symbols, tables and problem reports.
//!
//! Everything is written into a [`Out`] and printed through `anstream`, which drops the colours
//! when the output is not a terminal or `NO_COLOR` is set.

use std::fmt::{Display, Write};

use anstyle::{AnsiColor, Color, Style};
use parcel_core::Diagnostic;
use parcel_core::compile::{ReportEntry, Tier};
use parcel_core::lint::{Lint, Severity};
use parcel_core::structure::Lines;

const fn fg(c: AnsiColor) -> Style {
    Style::new().fg_color(Some(Color::Ansi(c)))
}

pub const BOLD: Style = Style::new().bold();
pub const DIM: Style = Style::new().dimmed();
pub const RED: Style = fg(AnsiColor::Red).bold();
pub const GREEN: Style = fg(AnsiColor::Green).bold();
pub const YELLOW: Style = fg(AnsiColor::Yellow).bold();
pub const CYAN: Style = fg(AnsiColor::Cyan);
pub const BLUE: Style = fg(AnsiColor::Blue).bold();

pub const OK: &str = "✓";
pub const FAIL: &str = "✗";
pub const WARN: &str = "!";
pub const INFO: &str = "·";

pub fn paint(style: Style, text: impl Display) -> String {
    format!("{style}{text}{style:#}")
}

/// Text to print, built up line by line.
#[derive(Default)]
pub struct Out(String);

impl Out {
    pub fn line(&mut self, s: impl Display) {
        let _ = writeln!(self.0, "{s}");
    }

    pub fn blank(&mut self) {
        if !self.0.is_empty() && !self.0.ends_with("\n\n") {
            self.0.push('\n');
        }
    }

    pub fn print(self) {
        anstream::print!("{}", self.0);
    }

    pub fn eprint(self) {
        anstream::eprint!("{}", self.0);
    }
}

/// A cell: its text and how it is painted. Widths are measured on the text alone.
pub struct Cell(pub String, pub Style);

impl Cell {
    pub fn plain(s: impl Into<String>) -> Cell {
        Cell(s.into(), Style::new())
    }
}

/// The terminal's width, or `None` when stdout is not a terminal (then nothing is wrapped).
pub fn width() -> Option<usize> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return None;
    }
    if let Some(c) = std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()) {
        return Some(c);
    }
    #[cfg(unix)]
    {
        // SAFETY: TIOCGWINSZ fills the zeroed winsize it is given and reads nothing else.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 {
            return Some(ws.ws_col as usize);
        }
    }
    Some(100)
}

/// `text` broken into lines of at most `max` characters, at spaces where possible.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(20);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let len = line.chars().count();
        if len > 0 && len + 1 + word.chars().count() > max {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Rows of cells in aligned columns. The last column is not padded, and wraps under itself
/// when the terminal is too narrow for it.
pub fn table(out: &mut Out, indent: usize, rows: Vec<Vec<Cell>>) {
    let n = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..n)
        .map(|i| {
            rows.iter()
                .filter_map(|r| r.get(i))
                .map(|c| c.0.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let term = width();
    for row in rows {
        let mut line = " ".repeat(indent);
        let last = row.len().saturating_sub(1);
        let mut used = indent;
        for (i, Cell(text, style)) in row.into_iter().enumerate() {
            if i < last {
                let _ = write!(
                    line,
                    "{}{}",
                    paint(style, &text),
                    " ".repeat(widths[i] - text.chars().count() + 2)
                );
                used += widths[i] + 2;
                continue;
            }
            match term.map(|w| w.saturating_sub(used)) {
                Some(room) if room >= 20 && text.chars().count() > room => {
                    let mut parts = wrap(&text, room).into_iter();
                    let first = parts.next().unwrap_or_default();
                    let _ = write!(line, "{}", paint(style, first));
                    for p in parts {
                        out.line(line.trim_end());
                        line = format!("{}{}", " ".repeat(used), paint(style, p));
                    }
                }
                _ => {
                    let _ = write!(line, "{}", paint(style, &text));
                }
            }
        }
        out.line(line.trim_end());
    }
}

/// `25000` as `25,000`.
pub fn count(n: impl Into<u64>) -> String {
    let s = n.into().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Where a problem is: the file, and the line when known.
pub struct Place<'a> {
    pub file: &'a str,
    pub source: Option<&'a str>,
}

/// One problem: its kind and title, where it is with the line itself, and what to do.
fn problem(
    out: &mut Out,
    kind: (&str, Style),
    code: &str,
    title: &str,
    place: &Place,
    line: Option<usize>,
    help: Option<&str>,
) {
    out.line(format!(
        "{}{} {}",
        paint(kind.1, kind.0),
        paint(DIM, format!("[{code}]")),
        paint(BOLD, title)
    ));
    let text = line.and_then(|l| place.source.and_then(|s| Lines::new(s).text(l)));
    let gutter = line.map(|l| l.to_string().len()).unwrap_or(1);
    let pad = " ".repeat(gutter);
    match line {
        Some(l) => out.line(format!("{pad}{} {}:{l}", paint(BLUE, "-->"), place.file)),
        None => out.line(format!("{pad}{} {}", paint(BLUE, "-->"), place.file)),
    }
    if let (Some(l), Some(t)) = (line, text) {
        out.line(format!("{pad} {}", paint(BLUE, "│")));
        out.line(format!(
            "{} {} {}",
            paint(BLUE, l),
            paint(BLUE, "│"),
            t.trim_end().replace('\t', "    ")
        ));
        out.line(format!("{pad} {}", paint(BLUE, "│")));
    }
    if let Some(h) = help {
        let lead = gutter + 9; // the width of "<pad> = help: "
        let lines = match width() {
            Some(w) => wrap(h, w.saturating_sub(lead)),
            None => vec![h.to_owned()],
        };
        for (i, l) in lines.iter().enumerate() {
            if i == 0 {
                out.line(format!(
                    "{pad} {} {} {l}",
                    paint(BLUE, "="),
                    paint(BOLD, "help:")
                ));
            } else {
                out.line(format!("{}{l}", " ".repeat(lead)));
            }
        }
    }
    out.blank();
}

/// The title of a diagnostic: its message, naming the rule when the message does not.
fn title(rule: Option<&str>, message: &str) -> String {
    match rule {
        Some(r) if !message.contains(&format!("`{r}`")) => format!("rule `{r}`: {message}"),
        _ => message.to_owned(),
    }
}

pub fn diagnostics(out: &mut Out, place: &Place, diags: &[Diagnostic]) {
    for d in diags {
        problem(
            out,
            ("error", RED),
            &format!("{:?}", d.code),
            &title(d.rule.as_deref(), &d.message),
            place,
            d.line,
            d.help.as_deref(),
        );
    }
}

pub fn lints(out: &mut Out, place: &Place, lints: &[Lint]) {
    for l in lints {
        let kind = match l.severity {
            Severity::Warning => ("warning", YELLOW),
            Severity::Note => ("note", CYAN),
        };
        problem(
            out,
            kind,
            l.name,
            &title(l.rule.as_deref(), &l.message),
            place,
            l.line,
            Some(&l.help),
        );
    }
}

/// The closing line after a list of problems.
pub fn verdict(out: &mut Out, ok: bool, text: impl Display) {
    out.blank();
    if ok {
        out.line(format!("{} {}", paint(GREEN, OK), paint(BOLD, text)));
    } else {
        out.line(format!("{} {}", paint(RED, FAIL), paint(BOLD, text)));
    }
}

/// `name v1` and the compilation hash, as the first line of a report.
pub fn title_line(out: &mut Out, name: &str, version: &parcel_core::Version, right: &str) {
    out.line(format!(
        "{} {}  {}",
        paint(BOLD, name),
        paint(BOLD, format!("v{version}")),
        paint(DIM, right)
    ));
}

/// A `label  value` line under a title.
pub fn field(out: &mut Out, label: &str, value: impl Display) {
    out.line(format!("  {}  {value}", paint(DIM, format!("{label:<8}"))));
}

pub fn tier(t: &Tier) -> &'static str {
    match t {
        Tier::PlanTime => "plan time",
        Tier::Prunes => "prunes",
        Tier::ScanFilter => "scan filter",
        Tier::WriteTime => "write time",
        Tier::Projection => "projection",
        Tier::Operator => "operator",
    }
}

/// Each rule's op and when it runs.
pub fn rules(out: &mut Out, report: &[ReportEntry]) {
    out.blank();
    let rows = report
        .iter()
        .map(|r| {
            vec![
                Cell::plain(&r.rule),
                Cell(r.op.clone(), CYAN),
                Cell::plain(tier(&r.tier)),
            ]
        })
        .collect();
    table(out, 2, rows);
}
