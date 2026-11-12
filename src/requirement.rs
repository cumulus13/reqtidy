//! Parsing of individual requirements ([PEP 508]) and `requirements.txt` files.
//!
//! [PEP 508]: https://peps.python.org/pep-0508/

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{bail, Context, Result};

use crate::marker::Marker;
use crate::pep440::SpecifierSet;

/// Maximum nesting of `-r other.txt` includes.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Error returned for a malformed requirement line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RequirementParseError(pub String);

/// Where a requirement was declared.
#[derive(Debug, Clone, Default)]
pub struct Origin {
    /// File the line came from.
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
}

/// A single parsed requirement.
#[derive(Debug, Clone)]
pub struct Requirement {
    /// Package name as written.
    pub name: String,
    /// Requested extras (lower-cased).
    pub extras: Vec<String>,
    /// Version constraints.
    pub specifiers: SpecifierSet,
    /// Direct reference (`name @ url`), if any.
    pub url: Option<String>,
    /// Environment marker, if any.
    pub marker: Option<Marker>,
    /// Source location.
    pub origin: Origin,
    /// `true` for bare VCS/URL lines (`git+https://...`) whose package name
    /// was derived from the URL rather than written by the user.
    pub name_guessed: bool,
}

/// Normalises a distribution name as described in PEP 503
/// (case-folded, runs of `-`, `_`, `.` collapsed into one `-`).
pub fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut previous_was_separator = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !previous_was_separator {
                out.push('-');
            }
            previous_was_separator = true;
        } else {
            out.extend(c.to_lowercase());
            previous_was_separator = false;
        }
    }
    out
}

impl Requirement {
    /// The normalised package name.
    pub fn normalized_name(&self) -> String {
        normalize_name(&self.name)
    }

    /// The string handed to the installer for this requirement
    /// (name, extras and version constraints, without the marker).
    pub fn install_spec(&self) -> String {
        let extras = if self.extras.is_empty() {
            String::new()
        } else {
            format!("[{}]", self.extras.join(","))
        };
        match &self.url {
            Some(url) if self.name_guessed => url.clone(),
            Some(url) => format!("{}{} @ {}", self.name, extras, url),
            None => format!("{}{}{}", self.name, extras, self.specifiers),
        }
    }

    /// Parses a bare VCS/URL requirement line such as
    /// `git+https://github.com/user/repo` or `https://host/pkg-1.0.whl`.
    ///
    /// The package name comes from `#egg=NAME` if present, otherwise from the
    /// last path segment of the URL.
    pub fn from_bare_url(line: &str) -> Result<Self, RequirementParseError> {
        let url = line.trim();
        let name = guess_name_from_url(url).ok_or_else(|| {
            RequirementParseError(format!(
                "cannot determine a package name from the URL: {url:?} (add #egg=NAME)"
            ))
        })?;
        Ok(Requirement {
            name,
            extras: Vec::new(),
            specifiers: SpecifierSet::default(),
            url: Some(url.to_string()),
            marker: None,
            origin: Origin::default(),
            name_guessed: true,
        })
    }
}

fn is_valid_name(name: &str) -> bool {
    let first = name.chars().next();
    let last = name.chars().next_back();
    first.is_some_and(|c| c.is_ascii_alphanumeric())
        && last.is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn strip_archive_extension(file: &str) -> Option<&str> {
    let lower = file.to_ascii_lowercase();
    [".tar.gz", ".tar.bz2", ".tar.xz", ".tgz", ".zip"]
        .iter()
        .find(|ext| lower.ends_with(**ext))
        .map(|ext| &file[..file.len() - ext.len()])
}

/// Derives a package name from a VCS or archive URL.
fn guess_name_from_url(url: &str) -> Option<String> {
    let (without_fragment, fragment) = match url.split_once('#') {
        Some((head, tail)) => (head, Some(tail)),
        None => (url, None),
    };
    if let Some(fragment) = fragment {
        for part in fragment.split('&') {
            if let Some(egg) = part.strip_prefix("egg=") {
                let name = egg.split('[').next().unwrap_or("").trim();
                if is_valid_name(name) {
                    return Some(name.to_string());
                }
            }
        }
    }
    let path = without_fragment
        .split('?')
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    // A URL without a path below the host has no package name in it.
    let after_scheme = path.split_once("://").map_or(path, |(_, rest)| rest);
    if !after_scheme.contains('/') {
        return None;
    }
    let last = path.rsplit('/').next().unwrap_or("");
    // Drop a trailing `@ref` (branch, tag or commit).
    let last = last.split('@').next().unwrap_or("");

    let candidate = if last.to_ascii_lowercase().ends_with(".whl") {
        last.split('-').next().unwrap_or("")
    } else if let Some(stem) = strip_archive_extension(last) {
        stem.rsplit_once('-').map_or(stem, |(name, _version)| name)
    } else {
        last.strip_suffix(".git").unwrap_or(last)
    };
    if is_valid_name(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// Normalises a requirement or `direct_url.json` URL so both can be compared:
/// no `git+` prefix, fragment, query, `@ref` or `.git` suffix; lower-cased.
pub fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_ascii_lowercase();
    };
    let scheme = scheme.rsplit('+').next().unwrap_or(scheme);
    let rest = rest.split(['#', '?']).next().unwrap_or(rest);
    let rest = rest.trim_end_matches('/');
    let rest = match (rest.rfind('@'), rest.rfind('/')) {
        (Some(at), Some(slash)) if at > slash => &rest[..at],
        _ => rest,
    };
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    format!("{scheme}://{rest}").to_ascii_lowercase()
}

/// `true` for lines that are a bare URL (`git+https://...`) rather than
/// `name @ url`.
fn is_bare_url(text: &str) -> bool {
    match text.split_whitespace().next() {
        Some(first) => match first.find("://") {
            Some(index) => !first[..index].contains('@'),
            None => false,
        },
        None => false,
    }
}

/// Finds a `;` that starts a marker after a URL (it must follow whitespace).
fn find_marker_separator(s: &str) -> Option<usize> {
    s.char_indices()
        .find(|(i, c)| *c == ';' && s[..*i].ends_with(char::is_whitespace))
        .map(|(i, _)| i)
}

impl FromStr for Requirement {
    type Err = RequirementParseError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let s = line.trim();
        let err = |msg: &str| RequirementParseError(format!("{msg}: {s:?}"));

        let name_end = s
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
            .unwrap_or(s.len());
        let name = &s[..name_end];
        let starts_ok = name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
        let ends_ok = name
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric());
        if !starts_ok || !ends_ok {
            return Err(err("invalid package name"));
        }

        let mut rest = s[name_end..].trim_start();

        let mut extras = Vec::new();
        if let Some(after) = rest.strip_prefix('[') {
            let end = after
                .find(']')
                .ok_or_else(|| err("unclosed extras bracket"))?;
            extras = after[..end]
                .split(',')
                .map(|e| e.trim().to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect();
            rest = after[end + 1..].trim_start();
        }

        let mut url = None;
        let (spec_src, marker_src) = if let Some(after) = rest.strip_prefix('@') {
            let after = after.trim_start();
            let (target, marker) = match find_marker_separator(after) {
                Some(i) => (after[..i].trim(), Some(after[i + 1..].trim())),
                None => (after.trim(), None),
            };
            if target.is_empty() {
                return Err(err("missing URL after '@'"));
            }
            url = Some(target.to_string());
            ("", marker)
        } else {
            match rest.split_once(';') {
                Some((spec, marker)) => (spec.trim(), Some(marker.trim())),
                None => (rest.trim(), None),
            }
        };

        let specifiers =
            SpecifierSet::from_str(spec_src).map_err(|e| err(&e.to_string()))?;
        let marker = match marker_src {
            Some(m) if !m.is_empty() => {
                Some(Marker::from_str(m).map_err(|e| err(&e.to_string()))?)
            }
            _ => None,
        };

        Ok(Requirement {
            name: name.to_string(),
            extras,
            specifiers,
            url,
            marker,
            origin: Origin::default(),
            name_guessed: false,
        })
    }
}

/// The result of parsing a requirements file (and the files it includes).
#[derive(Debug, Default)]
pub struct ParsedRequirements {
    /// Every requirement found, in file order.
    pub requirements: Vec<Requirement>,
    /// Non-fatal problems (unsupported lines, missing includes, ...).
    pub warnings: Vec<String>,
}

/// Parses a requirements file, following `-r other.txt` includes.
pub fn parse_file(path: &Path) -> Result<ParsedRequirements> {
    let mut parsed = ParsedRequirements::default();
    let mut seen = HashSet::new();
    parse_into(path, &mut parsed, &mut seen, 0)?;
    Ok(parsed)
}

fn parse_into(
    path: &Path,
    parsed: &mut ParsedRequirements,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_INCLUDE_DEPTH {
        bail!(
            "requirements includes are nested more than {MAX_INCLUDE_DEPTH} levels deep (at {})",
            path.display()
        );
    }
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(canonical) {
        parsed
            .warnings
            .push(format!("{}: already included, skipping", path.display()));
        return Ok(());
    }

    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let text =
        decode_text(&bytes).with_context(|| format!("{} is not valid text", path.display()))?;
    let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();

    for (line_no, logical) in logical_lines(&text) {
        let line = strip_comment(&logical).trim();
        if line.is_empty() {
            continue;
        }
        let here = format!("{}:{}", path.display(), line_no);

        if let Some(target) = include_target(line) {
            let included = base_dir.join(target);
            if included.is_file() {
                parse_into(&included, parsed, seen, depth + 1)?;
            } else {
                parsed
                    .warnings
                    .push(format!("{here}: included file {target:?} not found"));
            }
            continue;
        }

        if line.starts_with('-') {
            if line.starts_with("-e") || line.starts_with("--editable") {
                parsed
                    .warnings
                    .push(format!("{here}: editable installs are not checked: {line}"));
            } else {
                log::debug!("{here}: ignoring option line {line:?}");
            }
            continue;
        }

        // Drop per-requirement options such as `--hash=sha256:...`.
        let requirement_text = match line.find(" --") {
            Some(i) => line[..i].trim_end(),
            None => line,
        };

        let attempt = if is_bare_url(requirement_text) {
            Requirement::from_bare_url(requirement_text)
        } else {
            Requirement::from_str(requirement_text)
        };
        match attempt {
            Ok(mut req) => {
                req.origin = Origin {
                    file: path.to_path_buf(),
                    line: line_no,
                };
                parsed.requirements.push(req);
            }
            Err(e) => {
                let hint = if requirement_text.contains("://") {
                    " (VCS/URL requirements must use the form `name @ url`)"
                } else {
                    ""
                };
                parsed
                    .warnings
                    .push(format!("{here}: unsupported requirement, skipped: {e}{hint}"));
            }
        }
    }
    Ok(())
}

/// Decodes file bytes as UTF-8 or UTF-16 (PowerShell's `>` writes UTF-16 LE).
fn decode_text(bytes: &[u8]) -> Result<String> {
    if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (pairs, _trailing) = body.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        return String::from_utf16(&units).context("invalid UTF-16LE data");
    }
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (pairs, _trailing) = body.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
        return String::from_utf16(&units).context("invalid UTF-16BE data");
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(body.to_vec()).context("invalid UTF-8 data")
}

/// Joins lines that end in a backslash. Returns `(first line number, text)`.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut buffer = String::new();
    let mut start = 0;
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        if buffer.is_empty() {
            start = number;
        }
        match raw.strip_suffix('\\') {
            Some(stripped) => buffer.push_str(stripped),
            None => {
                buffer.push_str(raw);
                out.push((start, std::mem::take(&mut buffer)));
            }
        }
    }
    if !buffer.is_empty() {
        out.push((start, buffer));
    }
    out
}

/// Removes a trailing `# comment` (the `#` must start the line or follow whitespace).
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

/// Returns the path of a `-r`/`--requirement` include line.
fn include_target(line: &str) -> Option<&str> {
    for prefix in ["--requirement", "-r"] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let target = rest.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
            if !target.is_empty() {
                return Some(target.trim());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(s: &str) -> Requirement {
        Requirement::from_str(s).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn normalises_names() {
        assert_eq!(normalize_name("Foo_Bar.baz"), "foo-bar-baz");
        assert_eq!(normalize_name("a--b"), "a-b");
        assert_eq!(normalize_name("Requests"), "requests");
    }

    #[test]
    fn parses_plain_and_versioned() {
        let r = req("requests");
        assert_eq!(r.name, "requests");
        assert!(r.specifiers.is_empty());

        let r = req("numpy>=1.20.0,<2");
        assert_eq!(r.install_spec(), "numpy>=1.20.0,<2");
    }

    #[test]
    fn parses_extras_marker_and_parentheses() {
        let r = req("Flask[async, dotenv]==2.0.*");
        assert_eq!(r.extras, ["async", "dotenv"]);
        assert_eq!(r.install_spec(), "Flask[async,dotenv]==2.0.*");

        let r = req("pywin32>=227; sys_platform == \"win32\"");
        assert_eq!(r.name, "pywin32");
        assert!(r.marker.is_some());

        let r = req("name (>=1.0)");
        assert_eq!(r.install_spec(), "name>=1.0");
    }

    #[test]
    fn parses_direct_references() {
        let r = req("pkg @ https://example.com/p.zip ; sys_platform == 'win32'");
        assert_eq!(r.url.as_deref(), Some("https://example.com/p.zip"));
        assert!(r.marker.is_some());
        assert_eq!(r.install_spec(), "pkg @ https://example.com/p.zip");
    }

    #[test]
    fn rejects_bad_lines() {
        for bad in ["", "=1.0", "foo==", "foo bar", "./local/path", "foo[bar", "pkg @"] {
            assert!(Requirement::from_str(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn parses_files_with_includes_comments_and_options() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let main = dir.path().join("requirements.txt");
        let extra = dir.path().join("extra.txt");
        fs::write(&extra, "rich\n").unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            &main,
            "# comment\n\
             requests>=2.0  # inline comment\n\
             -r extra.txt\n\
             --index-url https://pypi.org/simple\n\
             numpy==1.26.0 \\\n    --hash=sha256:abc\n\
             -e ./local\n\
             git+https://github.com/a/b.git#egg=b\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let parsed = parse_file(&main).unwrap_or_else(|e| panic!("{e}"));
        let names: Vec<&str> = parsed
            .requirements
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, ["requests", "rich", "numpy", "b"]);
        // Only the editable install is unsupported; the git+ URL is understood.
        assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
        assert_eq!(parsed.requirements[2].install_spec(), "numpy==1.26.0");
    }

    #[test]
    fn understands_bare_vcs_and_archive_urls() {
        let name = |u: &str| Requirement::from_bare_url(u).map(|r| r.name);
        assert_eq!(name("git+https://github.com/cumulus13/files.im").as_deref(), Ok("files.im"));
        assert_eq!(name("git+https://github.com/cumulus13/mimelist.git").as_deref(), Ok("mimelist"));
        assert_eq!(name("git+https://github.com/a/b.git@v1.0").as_deref(), Ok("b"));
        assert_eq!(name("git+https://x.com/a/b.git#egg=Real_Name").as_deref(), Ok("Real_Name"));
        assert_eq!(name("https://example.com/dl/pkg-1.0-py3-none-any.whl").as_deref(), Ok("pkg"));
        assert_eq!(name("https://example.com/dl/my_pkg-1.2.3.tar.gz").as_deref(), Ok("my_pkg"));
        assert!(Requirement::from_bare_url("https://example.com/").is_err());

        let r = Requirement::from_bare_url("git+https://github.com/a/b").unwrap_or_else(|e| panic!("{e}"));
        assert!(r.name_guessed);
        assert_eq!(r.install_spec(), "git+https://github.com/a/b");
    }

    #[test]
    fn bare_url_detection_ignores_name_at_url_form() {
        assert!(is_bare_url("git+ssh://git@github.com/a/b.git"));
        assert!(!is_bare_url("pkg @ https://example.com/p.zip"));
        assert!(!is_bare_url("pkg@https://example.com/p.zip"));
        assert!(!is_bare_url("requests>=2"));
    }

    #[test]
    fn normalises_urls_for_comparison() {
        assert_eq!(
            normalize_url("git+https://github.com/Cumulus13/mimelist.git@main#egg=x"),
            normalize_url("https://github.com/cumulus13/mimelist"),
        );
        assert_eq!(normalize_url("https://h.com/a/b/"), "https://h.com/a/b");
    }

    #[test]
    fn handles_utf16_and_bom() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("req.txt");
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "six==1.16.0\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap_or_else(|e| panic!("{e}"));
        let parsed = parse_file(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed.requirements.len(), 1);
        assert_eq!(parsed.requirements[0].name, "six");

        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"attrs\n");
        fs::write(&path, bytes).unwrap_or_else(|e| panic!("{e}"));
        let parsed = parse_file(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed.requirements[0].name, "attrs");
    }

    #[test]
    fn detects_include_cycles() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "one\n-r b.txt\n").unwrap_or_else(|e| panic!("{e}"));
        fs::write(&b, "two\n-r a.txt\n").unwrap_or_else(|e| panic!("{e}"));
        let parsed = parse_file(&a).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed.requirements.len(), 2);
        assert_eq!(parsed.warnings.len(), 1);
    }
}
