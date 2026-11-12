//! Talking to a Python interpreter: locating one, reading its marker
//! environment and listing the distributions installed in it.
//!
//! Everything is done by running small, embedded scripts with `python -c`, so
//! no `pip` is required just to *inspect* an environment.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::requirement::{normalize_name, normalize_url};

/// Prints the PEP 508 marker environment as JSON (reference implementation
/// from the PEP).
pub const MARKER_SCRIPT: &str = r#"
import json, os, platform, sys

def format_full_version(info):
    version = "{0.major}.{0.minor}.{0.micro}".format(info)
    kind = info.releaselevel
    if kind != "final":
        version += kind[0] + str(info.serial)
    return version

if hasattr(sys, "implementation"):
    implementation_version = format_full_version(sys.implementation.version)
    implementation_name = sys.implementation.name
else:
    implementation_version = "0"
    implementation_name = ""

print(json.dumps({
    "implementation_name": implementation_name,
    "implementation_version": implementation_version,
    "os_name": os.name,
    "platform_machine": platform.machine(),
    "platform_release": platform.release(),
    "platform_system": platform.system(),
    "platform_version": platform.version(),
    "python_full_version": platform.python_version(),
    "platform_python_implementation": platform.python_implementation(),
    "python_version": ".".join(platform.python_version_tuple()[:2]),
    "sys_platform": sys.platform,
}))
"#;

/// Prints `[[name, version, direct_url_or_null], ...]` for every installed distribution.
pub const INSTALLED_SCRIPT: &str = r#"
import json
try:
    from importlib import metadata as md
except ImportError:
    import importlib_metadata as md

rows = []
for dist in md.distributions():
    try:
        name = dist.metadata["Name"]
        version = dist.version
    except Exception:
        continue
    if not (name and version):
        continue
    url = None
    try:
        raw = dist.read_text("direct_url.json")
        if raw:
            url = json.loads(raw).get("url")
    except Exception:
        url = None
    rows.append([name, version, url])
print(json.dumps(rows))
"#;

/// What is installed in an environment.
#[derive(Debug, Default, Clone)]
pub struct Installed {
    /// Normalised distribution name -> version.
    pub versions: HashMap<String, String>,
    /// Normalised source URL (for VCS/URL installs, from `direct_url.json`) -> version.
    pub urls: HashMap<String, String>,
}

/// A usable Python interpreter together with its marker environment.
#[derive(Debug, Clone)]
pub struct Interpreter {
    /// Path (or command name) of the interpreter.
    pub path: PathBuf,
    /// PEP 508 marker environment of this interpreter.
    pub markers: HashMap<String, String>,
}

/// Runs `python -c script args...` and returns its stdout.
pub fn run_python(python: &Path, script: &str, args: &[&str]) -> Result<String> {
    log::debug!("running {} -c <script> {:?}", python.display(), args);
    let output = Command::new(python)
        .arg("-c")
        .arg(script)
        .args(args)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to run {}", python.display()))?;
    if !output.status.success() {
        bail!(
            "{} exited with {}: {}",
            python.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("the interpreter produced non-UTF-8 output")
}

fn candidates(explicit: Option<&Path>) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return vec![path.to_path_buf()];
    }
    let mut list = Vec::new();
    for var in ["VIRTUAL_ENV", "CONDA_PREFIX"] {
        if let Some(prefix) = std::env::var_os(var) {
            let prefix = PathBuf::from(prefix);
            if cfg!(windows) {
                list.push(prefix.join("Scripts").join("python.exe"));
                list.push(prefix.join("python.exe"));
            } else {
                list.push(prefix.join("bin").join("python"));
            }
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["python", "python3"]
    } else {
        &["python3", "python"]
    };
    list.extend(names.iter().map(|name| PathBuf::from(*name)));
    list
}

impl Interpreter {
    /// Finds an interpreter: the explicit path if given, otherwise an active
    /// virtualenv/conda env, otherwise `python3`/`python` from `PATH`.
    pub fn discover(explicit: Option<&Path>) -> Result<Self> {
        let mut failures = Vec::new();
        for path in candidates(explicit) {
            match run_python(&path, MARKER_SCRIPT, &[]) {
                Ok(json) => {
                    let markers: HashMap<String, String> = serde_json::from_str(&json)
                        .with_context(|| {
                            format!("unexpected marker output from {}", path.display())
                        })?;
                    log::debug!("using interpreter {}", path.display());
                    return Ok(Interpreter { path, markers });
                }
                Err(e) => {
                    if explicit.is_some() {
                        return Err(e);
                    }
                    log::debug!("candidate {} rejected: {e:#}", path.display());
                    failures.push(format!("{}: {e}", path.display()));
                }
            }
        }
        bail!(
            "no working Python interpreter found (tried: {}). \
             Activate a virtualenv or pass one with --python.",
            failures.join("; ")
        )
    }

    /// The interpreter's full version string, e.g. `3.12.3`.
    pub fn version(&self) -> &str {
        self.markers
            .get("python_full_version")
            .map(String::as_str)
            .unwrap_or("unknown")
    }

    /// Lists the distributions installed in this interpreter's environment.
    pub fn installed_packages(&self) -> Result<Installed> {
        let json = run_python(&self.path, INSTALLED_SCRIPT, &[])?;
        let rows: Vec<(String, String, Option<String>)> = serde_json::from_str(&json)
            .context("unexpected output while listing installed packages")?;
        let mut installed = Installed::default();
        for (name, version, url) in rows {
            if let Some(url) = url {
                installed
                    .urls
                    .entry(normalize_url(&url))
                    .or_insert_with(|| version.clone());
            }
            // The first hit wins, matching Python's import precedence.
            installed
                .versions
                .entry(normalize_name(&name))
                .or_insert(version);
        }
        Ok(installed)
    }
}
