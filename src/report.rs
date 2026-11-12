//! Rendering of results: terminal tables, colors, summaries and JSON.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicU8, Ordering};

use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table, TableComponent};
use serde::Serialize;

use crate::check::{CheckResult, Status};
use crate::cli::ColorMode;

static COLOR_MODE: AtomicU8 = AtomicU8::new(0);

/// Sets the global color policy (`--color`).
pub fn set_color_mode(mode: ColorMode) {
    let value = match mode {
        ColorMode::Auto => 0,
        ColorMode::Always => 1,
        ColorMode::Never => 2,
    };
    COLOR_MODE.store(value, Ordering::Relaxed);
}

/// Decides whether to emit colors for a stream.
///
/// In `auto` mode colors are used only for terminals, when `NO_COLOR` is unset
/// and `TERM` is not `dumb`.
pub fn color_enabled(stream_is_terminal: bool) -> bool {
    match COLOR_MODE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            stream_is_terminal
                && std::env::var_os("NO_COLOR").is_none()
                && !matches!(std::env::var("TERM").as_deref(), Ok("dumb"))
        }
    }
}

/// Whether stdout should be colored.
pub fn stdout_color() -> bool {
    color_enabled(io::stdout().is_terminal())
}

/// Whether stderr should be colored.
pub fn stderr_color() -> bool {
    color_enabled(io::stderr().is_terminal())
}

/// A text style for [`paint`].
#[derive(Debug, Clone, Copy)]
pub enum Style {
    /// Bold.
    Bold,
    /// Dim.
    Dim,
    /// Red.
    Red,
    /// Green.
    Green,
    /// Yellow.
    Yellow,
    /// Cyan.
    Cyan,
}

impl Style {
    fn code(self) -> &'static str {
        match self {
            Style::Bold => "1",
            Style::Dim => "2",
            Style::Red => "31",
            Style::Green => "32",
            Style::Yellow => "33",
            Style::Cyan => "36",
        }
    }
}

/// Wraps `text` in ANSI escapes when `enabled`.
pub fn paint(text: &str, style: Style, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{}m{}\x1b[0m", style.code(), text)
    } else {
        text.to_string()
    }
}

/// Prints `warning: message` to stderr.
pub fn warn(message: &str) {
    eprintln!("{} {message}", paint("warning:", Style::Yellow, stderr_color()));
}

/// Prints `error: message` to stderr.
pub fn print_error(message: &str) {
    eprintln!("{} {message}", paint("error:", Style::Red, stderr_color()));
}

/// Writes a line to stdout, ignoring errors such as a closed pipe (`| head`).
pub fn emit(text: &str) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{text}");
}

/// Creates a table with the project's look and the given header cells.
pub fn new_table(header: &[&str]) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    // The condensed preset has no lines between rows; make the column lines solid.
    table.set_style(TableComponent::VerticalLines, '│');
    if stdout_color() {
        table.enforce_styling();
    } else {
        table.force_no_tty();
    }
    let cells: Vec<Cell> = header
        .iter()
        .map(|h| {
            Cell::new(*h)
                .add_attribute(Attribute::Bold)
                .fg(Color::Cyan)
        })
        .collect();
    table.set_header(cells);
    table
}

/// Colour associated with a status.
pub fn status_color(status: Status) -> Color {
    match status {
        Status::Ok => Color::Green,
        Status::Missing => Color::Red,
        Status::Mismatch => Color::Yellow,
        Status::Skipped | Status::Unverifiable => Color::Cyan,
    }
}

/// Prints the main "Package Version Checker" table.
pub fn print_table(results: &[CheckResult]) {
    let mut table = new_table(&["Package", "Installed", "Required", "", "Status"]);
    for r in results {
        let color = status_color(r.status);
        table.add_row(vec![
            Cell::new(&r.name).add_attribute(Attribute::Bold),
            Cell::new(r.installed.as_deref().unwrap_or("none")).fg(color),
            Cell::new(&r.required),
            Cell::new(r.status.emoji()),
            Cell::new(&r.detail).fg(color),
        ]);
    }
    emit("");
    emit(&format!(
        "                {}",
        paint("Package Version Checker", Style::Bold, stdout_color())
    ));
    emit(&table.to_string());
}

/// Counts per status.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct Summary {
    /// Satisfied requirements.
    pub ok: usize,
    /// Missing packages.
    pub missing: usize,
    /// Version mismatches.
    pub mismatch: usize,
    /// Requirements skipped because of markers.
    pub skipped: usize,
    /// Requirements whose installed version could not be compared.
    pub unverifiable: usize,
}

impl Summary {
    /// Tallies a result list.
    pub fn of(results: &[CheckResult]) -> Self {
        let mut s = Summary::default();
        for r in results {
            match r.status {
                Status::Ok => s.ok += 1,
                Status::Missing => s.missing += 1,
                Status::Mismatch => s.mismatch += 1,
                Status::Skipped => s.skipped += 1,
                Status::Unverifiable => s.unverifiable += 1,
            }
        }
        s
    }

    /// Number of unsatisfied requirements.
    pub fn problems(&self) -> usize {
        self.missing + self.mismatch
    }

    /// One-line human readable summary, optionally colored.
    pub fn line(&self, colored: bool) -> String {
        let part = |count: usize, label: &str, style: Style| {
            let text = format!("{count} {label}");
            if count == 0 {
                paint(&text, Style::Dim, colored)
            } else {
                paint(&text, style, colored)
            }
        };
        [
            part(self.ok, "ok", Style::Green),
            part(self.missing, "missing", Style::Red),
            part(self.mismatch, "mismatched", Style::Yellow),
            part(self.skipped, "skipped", Style::Cyan),
            part(self.unverifiable, "unverifiable", Style::Cyan),
        ]
        .join(", ")
    }
}

/// Machine readable report (`--json`).
#[derive(Debug, Serialize)]
pub struct JsonReport<'a> {
    /// The requirements file that was checked.
    pub requirements_file: String,
    /// The interpreter used.
    pub python: String,
    /// Python version of the interpreter.
    pub python_version: &'a str,
    /// Counts per status.
    pub summary: Summary,
    /// Per-requirement results.
    pub results: &'a [CheckResult],
}
