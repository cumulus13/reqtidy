//! Rendering of results: terminal tables, summaries and JSON.

use std::io::{self, Write};

use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use serde::Serialize;

use crate::check::{CheckResult, Status};

/// Writes a line to stdout, ignoring errors such as a closed pipe (`| head`).
pub fn emit(text: &str) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{text}");
}

/// Creates a table with the project's look and the given header cells.
pub fn new_table(header: &[&str]) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    let cells: Vec<Cell> = header
        .iter()
        .map(|h| Cell::new(*h).add_attribute(Attribute::Bold))
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
            Cell::new(&r.name),
            Cell::new(r.installed.as_deref().unwrap_or("none")),
            Cell::new(&r.required),
            Cell::new(r.status.emoji()),
            Cell::new(&r.detail).fg(color),
        ]);
    }
    emit("");
    emit("                Package Version Checker");
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

    /// One-line human readable summary.
    pub fn line(&self) -> String {
        format!(
            "{} ok, {} missing, {} mismatched, {} skipped, {} unverifiable",
            self.ok, self.missing, self.mismatch, self.skipped, self.unverifiable
        )
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
