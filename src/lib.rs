//! # reqtidy
//!
//! A fast Python requirements checker that validates, installs and tidies your
//! project dependencies. It is a Rust rewrite of
//! [`cumulus13/pipr`](https://github.com/cumulus13/pipr).
//!
//! The crate is primarily a command line tool (`reqtidy`), but its building
//! blocks are public so they can be reused and tested in isolation:
//!
//! * [`pep440`] – versions and version specifiers
//! * [`marker`] – PEP 508 environment markers
//! * [`requirement`] – requirement lines and `requirements.txt` files
//! * [`python`] – interpreter discovery and introspection
//! * [`check`] – comparing requirements with what is installed
//! * [`installer`] – installing through `pip` or `uv`
//! * [`scan`] – finding imports that are missing from the requirements
//! * [`pypi`] – looking up the latest release on pypi.org

pub mod check;
pub mod cli;
pub mod installer;
pub mod marker;
pub mod notify;
pub mod pep440;
pub mod pypi;
pub mod python;
pub mod report;
pub mod requirement;
pub mod scan;

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::str::FromStr;

use anyhow::{bail, Result};
use comfy_table::{Cell, Color};

use check::CheckResult;
use cli::Cli;
use installer::Installer;
use pep440::Version;
use python::Interpreter;
use report::{JsonReport, Summary};

fn exit_code(failed: bool) -> ExitCode {
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn init_logging(debug: bool) {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(if debug {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Warn
    });
    builder.parse_default_env();
    builder.format_timestamp(None);
    // Ignore the error: a logger may already be installed (e.g. in tests).
    let _ = builder.try_init();
}

fn notify_if_requested(cli: &Cli, title: &str, body: &str) {
    if cli.notify {
        notify::send(title, body);
    }
}

/// Runs the command line tool and returns the process exit code.
///
/// Errors (bad usage, missing files, no Python interpreter, ...) are returned
/// as `Err`; the binary prints them and exits with status 2.
pub fn run(cli: &Cli) -> Result<ExitCode> {
    report::set_color_mode(cli.color);
    init_logging(cli.debug);
    if !cli.pypi.is_empty() {
        return run_pypi(cli);
    }
    if let Some(dir) = &cli.scan {
        return run_scan(cli, dir);
    }
    run_check(cli)
}

fn run_check(cli: &Cli) -> Result<ExitCode> {
    if !cli.file.is_file() {
        bail!(
            "requirements file not found: {} \
             (create one, e.g. `pip freeze > requirements.txt`, or pass a path)",
            cli.file.display()
        );
    }
    let parsed = requirement::parse_file(&cli.file)?;
    for warning in &parsed.warnings {
        report::warn(warning);
    }
    if parsed.requirements.is_empty() && !cli.json {
        eprintln!("No requirements found in {}", cli.file.display());
        return Ok(ExitCode::SUCCESS);
    }

    let interpreter = Interpreter::discover(cli.python.as_deref())?;
    log::debug!(
        "python {} at {}",
        interpreter.version(),
        interpreter.path.display()
    );
    let installed = interpreter.installed_packages()?;
    let mut results = check::check_all(&parsed.requirements, &interpreter.markers, &installed);
    let summary = Summary::of(&results);

    if cli.json {
        let json = JsonReport {
            requirements_file: cli.file.display().to_string(),
            python: interpreter.path.display().to_string(),
            python_version: interpreter.version(),
            summary,
            results: &results,
        };
        report::emit(&serde_json::to_string_pretty(&json)?);
        return Ok(exit_code(summary.problems() > 0));
    }

    if !cli.no_show {
        report::print_table(&results);
    }
    report::emit(&format!(
        "Summary: {}",
        summary.line(report::stdout_color())
    ));

    if summary.problems() == 0 {
        notify_if_requested(cli, "reqtidy", "All requirements are satisfied");
        return Ok(ExitCode::SUCCESS);
    }
    if cli.read_only() {
        notify_if_requested(
            cli,
            "reqtidy",
            &format!(
                "{} unsatisfied requirement(s)",
                summary.problems()
            ),
        );
        return Ok(exit_code(true));
    }

    let options = installer::Options {
        force_install: cli.force_install,
        force_retry: cli.force_retry,
        interactive: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
    };
    let specs: Vec<String> = {
        let problems: Vec<&CheckResult> =
            results.iter().filter(|r| r.status.is_problem()).collect();
        let plan = installer::plan(&problems, &options);
        plan.selected
            .iter()
            .map(|r| r.install_spec.clone())
            .collect()
    };
    if specs.is_empty() {
        if !options.interactive && !options.force_install {
            eprintln!(
                "Not running in a terminal, so nothing was installed. \
                 Use -F/--force-install to install without prompting, or --check for a read-only report."
            );
        } else {
            eprintln!("Nothing was installed.");
        }
        return Ok(exit_code(true));
    }

    let backend = Installer::resolve(cli.installer, &interpreter.path)?;
    let outcome = installer::execute(backend, &interpreter.path, &specs, &options);
    for spec in &outcome.failed {
        report::warn(&format!("failed to install {spec}"));
    }

    let installed = interpreter.installed_packages()?;
    results = check::check_all(&parsed.requirements, &interpreter.markers, &installed);
    let after = Summary::of(&results);
    if !cli.no_show {
        report::print_table(&results);
    }
    report::emit(&format!(
        "Summary: {}",
        after.line(report::stdout_color())
    ));

    let body = format!(
        "{} installed, {} failed, {} still unsatisfied",
        outcome.succeeded.len(),
        outcome.failed.len(),
        after.problems()
    );
    notify_if_requested(cli, "reqtidy", &body);
    Ok(exit_code(after.problems() > 0))
}

fn run_pypi(cli: &Cli) -> Result<ExitCode> {
    let interpreter = Interpreter::discover(cli.python.as_deref())?;
    let installed = interpreter.installed_packages()?;
    let mut table = report::new_table(&["Package", "Installed", "Latest on PyPI", "", "Status"]);
    let mut failures = 0usize;

    for name in &cli.pypi {
        let current = installed
            .versions
            .get(&requirement::normalize_name(name))
            .map(String::as_str);
        let (latest, emoji, detail, color) = match pypi::fetch_latest(name) {
            Ok(Some(info)) => {
                let latest = info.version;
                let (emoji, detail, color) = match current {
                    None => ("❌", "Not installed".to_string(), Color::Red),
                    Some(cur) => match (Version::from_str(cur), Version::from_str(&latest)) {
                        (Ok(c), Ok(l)) if c >= l => {
                            ("✅", "Up to date".to_string(), Color::Green)
                        }
                        (Ok(_), Ok(_)) => (
                            "⚠️",
                            format!("Update available: {cur} → {latest}"),
                            Color::Yellow,
                        ),
                        _ => ("ℹ️", "Versions are not comparable".to_string(), Color::Cyan),
                    },
                };
                (latest, emoji, detail, color)
            }
            Ok(None) => {
                failures += 1;
                (
                    "-".to_string(),
                    "❌",
                    "Not found on PyPI".to_string(),
                    Color::Red,
                )
            }
            Err(e) => {
                failures += 1;
                (
                    "-".to_string(),
                    "❌",
                    format!("Lookup failed: {e:#}"),
                    Color::Red,
                )
            }
        };
        table.add_row(vec![
            Cell::new(name),
            Cell::new(current.unwrap_or("none")),
            Cell::new(&latest),
            Cell::new(emoji),
            Cell::new(&detail).fg(color),
        ]);
    }

    report::emit(&table.to_string());
    Ok(exit_code(failures > 0))
}

fn run_scan(cli: &Cli, dir: &Path) -> Result<ExitCode> {
    if !dir.is_dir() {
        bail!("scan directory not found: {}", dir.display());
    }
    let interpreter = Interpreter::discover(cli.python.as_deref())?;
    let requirements = if cli.file.is_file() {
        let parsed = requirement::parse_file(&cli.file)?;
        for warning in &parsed.warnings {
            report::warn(warning);
        }
        parsed.requirements
    } else {
        eprintln!(
            "note: {} not found; every third-party import will be reported as missing",
            cli.file.display()
        );
        Vec::new()
    };

    let found = scan::run(&interpreter, dir, cli.recursive, &requirements)?;
    if cli.json {
        report::emit(&serde_json::to_string_pretty(&found)?);
    } else {
        print_scan(&found, &cli.file);
    }
    Ok(exit_code(!found.missing.is_empty()))
}

fn print_scan(found: &scan::ScanReport, requirements_file: &Path) {
    if found.stdlib_detection_unavailable {
        eprintln!(
            "warning: this Python (< 3.10) cannot list standard-library modules; \
             stdlib imports may appear under \"not installed\""
        );
    }
    if !found.missing.is_empty() {
        let mut table = report::new_table(&["Import", "Package to add", "Used in"]);
        for item in &found.missing {
            table.add_row(vec![
                Cell::new(&item.module),
                Cell::new(item.packages.join(", ")).fg(Color::Yellow),
                Cell::new(item.files.join(", ")),
            ]);
        }
        report::emit("");
        report::emit(&format!(
            "Imports not covered by {}:",
            requirements_file.display()
        ));
        report::emit(&table.to_string());
    }
    if !found.unresolved.is_empty() {
        report::emit("");
        report::emit("Imports that are not installed (no package could be suggested):");
        for item in &found.unresolved {
            report::emit(&format!("  ℹ️  {} ({})", item.module, item.files.join(", ")));
        }
    }
    report::emit("");
    report::emit(&format!(
        "Scanned {} file(s): {} covered, {} missing, {} unresolved, {} stdlib/local",
        found.files_scanned,
        found.covered,
        found.missing.len(),
        found.unresolved.len(),
        found.ignored
    ));
}
