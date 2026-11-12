//! `--scan`: find third-party imports in Python sources that are not covered by
//! the requirements file (the "tidy" half of `go mod tidy`).
//!
//! Imports are detected with a line-based scanner, so occurrences inside
//! strings or docstrings can show up as false positives. Mapping an import
//! name to its distribution (`import yaml` -> `PyYAML`) and recognising the
//! standard library is delegated to the interpreter (Python 3.10+).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

use crate::python::{run_python, Interpreter};
use crate::requirement::{normalize_name, Requirement};

/// Directories that never contain project sources.
const IGNORED_DIRS: [&str; 12] = [
    "__pycache__",
    "node_modules",
    "site-packages",
    "venv",
    "env",
    "build",
    "dist",
    "target",
    "htmlcov",
    ".git",
    ".tox",
    ".eggs",
];

/// Maximum number of example files recorded per import.
const MAX_EXAMPLE_FILES: usize = 3;

/// Maps import names to distributions and recognises the standard library.
const PROBE_SCRIPT: &str = r#"
import json, sys

modules = [m for m in sys.argv[1].split(",") if m]
stdlib = getattr(sys, "stdlib_module_names", None)
if stdlib is None:
    supported = False
    stdlib = set()
else:
    supported = True
    stdlib = set(stdlib) | set(sys.builtin_module_names)

try:
    from importlib import metadata as md
    owners = md.packages_distributions()
except Exception:
    owners = {}

print(json.dumps({
    "stdlib_supported": supported,
    "stdlib": [m for m in modules if m in stdlib],
    "dists": {m: sorted(set(owners[m])) for m in modules if m in owners},
}))
"#;

#[derive(Debug, Deserialize)]
struct Probe {
    #[serde(default)]
    stdlib_supported: bool,
    #[serde(default)]
    stdlib: Vec<String>,
    #[serde(default)]
    dists: HashMap<String, Vec<String>>,
}

/// An import that no requirement covers.
#[derive(Debug, Serialize)]
pub struct MissingImport {
    /// Top-level module name as imported.
    pub module: String,
    /// Installed distributions that provide the module.
    pub packages: Vec<String>,
    /// A few files that import it.
    pub files: Vec<String>,
}

/// An import that could not be attributed to anything.
#[derive(Debug, Serialize)]
pub struct UnresolvedImport {
    /// Top-level module name as imported.
    pub module: String,
    /// A few files that import it.
    pub files: Vec<String>,
}

/// Result of a scan.
#[derive(Debug, Default, Serialize)]
pub struct ScanReport {
    /// Number of `.py` files read.
    pub files_scanned: usize,
    /// Imports already covered by a requirement.
    pub covered: usize,
    /// Imports of the standard library or of local modules.
    pub ignored: usize,
    /// Third-party imports missing from the requirements file.
    pub missing: Vec<MissingImport>,
    /// Imports that are not installed, so no package could be suggested.
    pub unresolved: Vec<UnresolvedImport>,
    /// Set when the interpreter cannot identify standard-library modules.
    pub stdlib_detection_unavailable: bool,
}

#[derive(Debug, Default)]
struct Collected {
    files_scanned: usize,
    /// module -> files importing it
    imports: BTreeMap<String, BTreeSet<String>>,
    /// names of local modules and packages
    local: HashSet<String>,
}

fn is_ignored(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return false;
    }
    match entry.file_name().to_str() {
        Some(name) => name.starts_with('.') || IGNORED_DIRS.contains(&name),
        None => false,
    }
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => chars.all(|c| c.is_alphanumeric() || c == '_'),
        _ => false,
    }
}

fn push_top_level(name: &str, out: &mut Vec<String>) {
    let end = name
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(name.len());
    let top = name[..end].split('.').next().unwrap_or("");
    if is_identifier(top) {
        out.push(top.to_string());
    }
}

/// Extracts top-level imported module names from Python source text.
pub fn imports_in(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        if let Some(rest) = line.strip_prefix("import ") {
            for part in rest.split(',') {
                let name = part.split_whitespace().next().unwrap_or("");
                push_top_level(name, &mut found);
            }
        } else if let Some(rest) = line.strip_prefix("from ") {
            let name = rest.split_whitespace().next().unwrap_or("");
            // Skip relative imports and prose that merely starts with "from".
            if !name.starts_with('.') && rest.contains(" import") {
                push_top_level(name, &mut found);
            }
        }
    }
    found
}

fn collect(root: &Path, recursive: bool) -> Collected {
    let mut collected = Collected::default();
    let max_depth = if recursive { usize::MAX } else { 1 };
    let walker = WalkDir::new(root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_ignored(entry));

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log::debug!("skipping unreadable entry: {e}");
                continue;
            }
        };
        if entry.file_type().is_dir() {
            if entry.depth() > 0 {
                if let Some(name) = entry.file_name().to_str() {
                    collected.local.insert(name.to_string());
                }
            }
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            collected.local.insert(stem.to_string());
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::debug!("cannot read {}: {e}", path.display());
                continue;
            }
        };
        collected.files_scanned += 1;
        let shown = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        for module in imports_in(&String::from_utf8_lossy(&bytes)) {
            collected
                .imports
                .entry(module)
                .or_default()
                .insert(shown.clone());
        }
    }
    collected
}

fn examples(files: &BTreeSet<String>) -> Vec<String> {
    files.iter().take(MAX_EXAMPLE_FILES).cloned().collect()
}

/// Scans `root` and compares the imports with `requirements`.
pub fn run(
    interpreter: &Interpreter,
    root: &Path,
    recursive: bool,
    requirements: &[Requirement],
) -> Result<ScanReport> {
    let collected = collect(root, recursive);
    let mut report = ScanReport {
        files_scanned: collected.files_scanned,
        ..ScanReport::default()
    };

    let candidates: Vec<&str> = collected
        .imports
        .keys()
        .filter(|module| !collected.local.contains(*module))
        .map(String::as_str)
        .collect();
    report.ignored = collected.imports.len() - candidates.len();
    if candidates.is_empty() {
        return Ok(report);
    }

    let joined = candidates.join(",");
    let output = run_python(&interpreter.path, PROBE_SCRIPT, &[joined.as_str()])?;
    let probe: Probe =
        serde_json::from_str(&output).context("unexpected output from the import probe")?;
    report.stdlib_detection_unavailable = !probe.stdlib_supported;

    let stdlib: HashSet<&str> = probe.stdlib.iter().map(String::as_str).collect();
    let required: HashSet<String> = requirements
        .iter()
        .map(Requirement::normalized_name)
        .collect();

    for module in candidates {
        let files = &collected.imports[module];
        if stdlib.contains(module) {
            report.ignored += 1;
            continue;
        }
        match probe.dists.get(module) {
            Some(distributions) => {
                let is_covered = distributions
                    .iter()
                    .any(|d| required.contains(&normalize_name(d)));
                if is_covered {
                    report.covered += 1;
                } else {
                    report.missing.push(MissingImport {
                        module: module.to_string(),
                        packages: distributions.clone(),
                        files: examples(files),
                    });
                }
            }
            None => report.unresolved.push(UnresolvedImport {
                module: module.to_string(),
                files: examples(files),
            }),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_top_level_modules() {
        let source = "\
import os, sys
import numpy as np, requests.adapters
from yaml import safe_load
from . import sibling
from .pkg import thing
from collections.abc import Mapping
import a; import b
    import indented
# import commented_out
from the beginning of time
x = 1
";
        let mut found = imports_in(source);
        found.sort();
        assert_eq!(
            found,
            [
                "a",
                "collections",
                "indented",
                "numpy",
                "os",
                "requests",
                "sys",
                "yaml"
            ]
        );
    }

    #[test]
    fn walks_directories_and_marks_local_modules() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let pkg = dir.path().join("mypkg");
        fs::create_dir_all(&pkg).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            dir.path().join("main.py"),
            "import mypkg\nimport requests\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        fs::write(pkg.join("util.py"), "import rich\nfrom . import x\n")
            .unwrap_or_else(|e| panic!("{e}"));
        let ignored = dir.path().join("venv");
        fs::create_dir_all(&ignored).unwrap_or_else(|e| panic!("{e}"));
        fs::write(ignored.join("skip.py"), "import should_not_appear\n")
            .unwrap_or_else(|e| panic!("{e}"));

        let shallow = collect(dir.path(), false);
        assert_eq!(shallow.files_scanned, 1);
        assert!(shallow.imports.contains_key("requests"));
        assert!(!shallow.imports.contains_key("rich"));

        let deep = collect(dir.path(), true);
        assert_eq!(deep.files_scanned, 2);
        assert!(deep.imports.contains_key("rich"));
        assert!(!deep.imports.contains_key("should_not_appear"));
        assert!(deep.local.contains("mypkg"));
        assert!(deep.local.contains("util"));
    }
}
