//! Command line interface definition.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use clap_color_help::default_styles;

/// Which tool performs installations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InstallerChoice {
    /// Use `python -m pip` if available, otherwise `uv`.
    Auto,
    /// Always use `python -m pip`.
    Pip,
    /// Always use `uv pip`.
    Uv,
}

/// Smart Python requirements checker: validates, installs and tidies your dependencies.
#[derive(Debug, Parser)]
#[command(
    name = "reqtidy",
    version,
    long_about = None,
    after_help = "EXIT CODES:\n  0  every requirement is satisfied\n  1  unsatisfied requirements remain (or a lookup failed)\n  2  usage or runtime error",
    disable_version_flag = true,
    styles = default_styles()
)]
pub struct Cli {
    /// Requirements file to check
    #[arg(value_name = "FILE", default_value = "requirements.txt")]
    pub file: PathBuf,

    /// Scan Python files recursively (used with --scan)
    #[arg(short = 'r', long)]
    pub recursive: bool,

    /// Retry failed installations automatically (up to 3 attempts)
    #[arg(short = 'f', long = "force-retry")]
    pub force_retry: bool,

    /// Install packages without asking for confirmation
    #[arg(short = 'F', long = "force-install")]
    pub force_install: bool,

    /// Show the summary table only (non-interactive, nothing is installed)
    #[arg(short = 's', long)]
    pub summary: bool,

    /// Same as --summary; handy in CI (exit code 1 when something is unsatisfied)
    #[arg(short = 'c', long)]
    pub check: bool,

    /// Compare an installed package with the latest release on pypi.org (repeatable)
    #[arg(short = 'i', long, value_name = "PACKAGE")]
    pub pypi: Vec<String>,

    /// Do not install anything
    #[arg(short = 'n', long = "no-install")]
    pub no_install: bool,

    /// Do not print the table
    #[arg(short = 'z', long = "no-show")]
    pub no_show: bool,

    /// Debug logging
    #[arg(short = 'd', long)]
    pub debug: bool,

    /// Python interpreter to inspect and install into (default: active venv, then python3/python)
    #[arg(long, value_name = "PATH")]
    pub python: Option<PathBuf>,

    /// Installer backend
    #[arg(long, value_enum, default_value_t = InstallerChoice::Auto)]
    pub installer: InstallerChoice,

    /// Find imported packages that are missing from the requirements file.
    /// Use `--scan=DIR` to pick a directory (default: current directory).
    #[arg(
        long,
        value_name = "DIR",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "."
    )]
    pub scan: Option<PathBuf>,

    /// Print a machine-readable JSON report instead of a table (never installs)
    #[arg(long)]
    pub json: bool,

    /// Send a desktop notification with the result (requires the `notify` build feature)
    #[arg(long)]
    pub notify: bool,
}

impl Cli {
    /// `true` when nothing may be installed.
    pub fn read_only(&self) -> bool {
        self.summary || self.check || self.no_install || self.json
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_flags() {
        let cli = Cli::parse_from(["reqtidy", "-F", "-f", "-i", "rich", "-i", "requests", "req.txt"]);
        assert!(cli.force_install && cli.force_retry);
        assert_eq!(cli.pypi, ["rich", "requests"]);
        assert_eq!(cli.file, PathBuf::from("req.txt"));
        assert!(!cli.read_only());
    }

    #[test]
    fn scan_takes_optional_value() {
        let cli = Cli::parse_from(["reqtidy", "--scan"]);
        assert_eq!(cli.scan, Some(PathBuf::from(".")));
        let cli = Cli::parse_from(["reqtidy", "--scan=src", "-r"]);
        assert_eq!(cli.scan, Some(PathBuf::from("src")));
        assert!(cli.recursive);
    }
}
