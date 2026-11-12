//! Installing and upgrading packages through `pip` or `uv`, including the
//! interactive confirmation flow and the retry logic.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::check::{CheckResult, Status};
use crate::cli::InstallerChoice;

/// Attempts made per package when retrying automatically.
const MAX_ATTEMPTS: u32 = 3;

/// A concrete installer backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installer {
    /// `python -m pip install`
    Pip,
    /// `uv pip install --python <python>`
    Uv,
}

fn probe(mut command: Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn pip_available(python: &Path) -> bool {
    let mut command = Command::new(python);
    command.args(["-m", "pip", "--version"]);
    probe(command)
}

fn uv_available() -> bool {
    let mut command = Command::new("uv");
    command.arg("--version");
    probe(command)
}

impl Installer {
    /// Resolves the user's choice into a usable backend.
    pub fn resolve(choice: InstallerChoice, python: &Path) -> Result<Self> {
        match choice {
            InstallerChoice::Pip => {
                if pip_available(python) {
                    Ok(Installer::Pip)
                } else {
                    bail!("`{} -m pip` is not available; try --installer uv", python.display())
                }
            }
            InstallerChoice::Uv => {
                if uv_available() {
                    Ok(Installer::Uv)
                } else {
                    bail!("`uv` was not found on PATH; install it or use --installer pip")
                }
            }
            InstallerChoice::Auto => {
                if pip_available(python) {
                    Ok(Installer::Pip)
                } else if uv_available() {
                    Ok(Installer::Uv)
                } else {
                    bail!(
                        "neither pip (for {}) nor uv is available, so nothing can be installed",
                        python.display()
                    )
                }
            }
        }
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Installer::Pip => "pip",
            Installer::Uv => "uv",
        }
    }

    fn command(self, python: &Path, specs: &[String]) -> Command {
        match self {
            Installer::Pip => {
                let mut command = Command::new(python);
                command.args(["-m", "pip", "install"]).args(specs);
                command
            }
            Installer::Uv => {
                let mut command = Command::new("uv");
                command
                    .args(["pip", "install", "--python"])
                    .arg(python)
                    .args(specs);
                command
            }
        }
    }

    /// Runs the installer with inherited stdio so progress stays visible.
    fn run(self, python: &Path, specs: &[String]) -> bool {
        let mut command = self.command(python, specs);
        log::debug!("exec: {command:?}");
        match command.status() {
            Ok(status) => status.success(),
            Err(e) => {
                eprintln!("error: could not launch {}: {e}", self.name());
                false
            }
        }
    }
}

/// Answer to an interactive question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    No,
    All,
    Quit,
}

fn ask(prompt: &str, allow_all: bool) -> Answer {
    let choices = if allow_all { "[y/N/a/q]" } else { "[y/N]" };
    loop {
        eprint!("{prompt} {choices} ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        match io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => return Answer::Quit,
            Ok(_) => {}
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Answer::Yes,
            "" | "n" | "no" => return Answer::No,
            "a" | "all" if allow_all => return Answer::All,
            "q" | "quit" if allow_all => return Answer::Quit,
            _ => eprintln!("Please answer with one of {choices}."),
        }
    }
}

/// Options controlling the install flow.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Install without asking.
    pub force_install: bool,
    /// Retry failures automatically.
    pub force_retry: bool,
    /// Whether stdin/stdout are terminals (prompts are only possible then).
    pub interactive: bool,
}

/// Which problem results will be installed and which were declined.
#[derive(Debug)]
pub struct Plan<'a> {
    /// Results to install.
    pub selected: Vec<&'a CheckResult>,
    /// Results the user declined, or that could not be asked about.
    pub declined: Vec<&'a CheckResult>,
}

fn question(result: &CheckResult) -> String {
    match result.status {
        Status::Missing => format!(
            "{} {} is not installed. Install `{}`?",
            result.status.emoji(),
            result.name,
            result.install_spec
        ),
        _ => format!(
            "{} {} {} does not satisfy {}. Install `{}`?",
            result.status.emoji(),
            result.name,
            result.installed.as_deref().unwrap_or("?"),
            result.required,
            result.install_spec
        ),
    }
}

/// Decides what to install, asking the user when appropriate.
pub fn plan<'a>(problems: &[&'a CheckResult], options: &Options) -> Plan<'a> {
    let mut selected = Vec::new();
    let mut declined = Vec::new();

    if options.force_install {
        selected.extend_from_slice(problems);
        return Plan { selected, declined };
    }
    if !options.interactive {
        declined.extend_from_slice(problems);
        return Plan { selected, declined };
    }

    let mut accept_all = false;
    let mut quit = false;
    for &result in problems {
        if quit {
            declined.push(result);
        } else if accept_all {
            selected.push(result);
        } else {
            match ask(&question(result), true) {
                Answer::Yes => selected.push(result),
                Answer::No => declined.push(result),
                Answer::All => {
                    accept_all = true;
                    selected.push(result);
                }
                Answer::Quit => {
                    quit = true;
                    declined.push(result);
                }
            }
        }
    }
    Plan { selected, declined }
}

/// What happened during installation.
#[derive(Debug, Default)]
pub struct InstallSummary {
    /// Specs that were installed successfully.
    pub succeeded: Vec<String>,
    /// Specs that could not be installed.
    pub failed: Vec<String>,
}

fn install_one(installer: Installer, python: &Path, spec: &str, options: &Options) -> bool {
    let single = [spec.to_string()];
    let mut attempt = 1;
    loop {
        eprintln!("→ Installing {spec} with {} (attempt {attempt})", installer.name());
        if installer.run(python, &single) {
            return true;
        }
        if options.force_retry {
            if attempt >= MAX_ATTEMPTS {
                return false;
            }
            attempt += 1;
            thread::sleep(Duration::from_secs(u64::from(attempt)));
        } else if options.interactive
            && ask(&format!("Installing {spec} failed. Retry?"), false) == Answer::Yes
        {
            attempt += 1;
        } else {
            return false;
        }
    }
}

/// Installs all `specs`: one batch first, then package by package if the batch fails.
pub fn execute(
    installer: Installer,
    python: &Path,
    specs: &[String],
    options: &Options,
) -> InstallSummary {
    let mut summary = InstallSummary::default();
    if specs.is_empty() {
        return summary;
    }
    if specs.len() > 1 {
        eprintln!("→ Installing {} packages with {}…", specs.len(), installer.name());
        if installer.run(python, specs) {
            summary.succeeded.extend_from_slice(specs);
            return summary;
        }
        eprintln!("The batch install failed; retrying package by package to isolate the problem…");
    }
    for spec in specs {
        if install_one(installer, python, spec, options) {
            summary.succeeded.push(spec.clone());
        } else {
            summary.failed.push(spec.clone());
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(name: &str, status: Status) -> CheckResult {
        CheckResult {
            name: name.to_string(),
            installed: None,
            required: ">=1".to_string(),
            status,
            detail: String::new(),
            install_spec: format!("{name}>=1"),
        }
    }

    #[test]
    fn force_install_selects_everything() {
        let a = problem("a", Status::Missing);
        let b = problem("b", Status::Mismatch);
        let options = Options {
            force_install: true,
            force_retry: false,
            interactive: false,
        };
        let plan = plan(&[&a, &b], &options);
        assert_eq!(plan.selected.len(), 2);
        assert!(plan.declined.is_empty());
    }

    #[test]
    fn non_interactive_without_force_installs_nothing() {
        let a = problem("a", Status::Missing);
        let options = Options {
            force_install: false,
            force_retry: false,
            interactive: false,
        };
        let plan = plan(&[&a], &options);
        assert!(plan.selected.is_empty());
        assert_eq!(plan.declined.len(), 1);
    }

    #[test]
    fn builds_pip_and_uv_commands() {
        let specs = vec!["numpy>=1.20".to_string()];
        let pip = Installer::Pip.command(Path::new("python3"), &specs);
        let args: Vec<_> = pip.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["-m", "pip", "install", "numpy>=1.20"]);

        let uv = Installer::Uv.command(Path::new("/venv/bin/python"), &specs);
        assert_eq!(uv.get_program(), "uv");
        let args: Vec<_> = uv.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["pip", "install", "--python", "/venv/bin/python", "numpy>=1.20"]);
    }
}
