//! Comparing declared requirements against what is installed.

use std::collections::HashMap;
use std::str::FromStr;

use serde::Serialize;

use crate::pep440::Version;
use crate::python::Installed;
use crate::requirement::{normalize_url, Requirement};

/// Outcome of checking one requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Installed and satisfying the constraint.
    Ok,
    /// Not installed.
    Missing,
    /// Installed, but the version does not satisfy the constraint.
    Mismatch,
    /// Not applicable on this platform (environment marker evaluated to false).
    Skipped,
    /// Installed, but its version could not be compared.
    Unverifiable,
}

impl Status {
    /// Emoji used in the table output.
    pub fn emoji(self) -> &'static str {
        match self {
            Status::Ok => "✅",
            Status::Missing => "❌",
            Status::Mismatch => "⚠️",
            Status::Skipped | Status::Unverifiable => "ℹ️",
        }
    }

    /// `true` when the requirement is not satisfied and could be fixed by installing.
    pub fn is_problem(self) -> bool {
        matches!(self, Status::Missing | Status::Mismatch)
    }
}

/// The result for a single requirement.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    /// Package name as written in the requirements file.
    pub name: String,
    /// Installed version, if any.
    pub installed: Option<String>,
    /// Human readable constraint (`>=1.0`, `@ url`, `any`).
    pub required: String,
    /// Outcome.
    pub status: Status,
    /// Human readable explanation.
    pub detail: String,
    /// What to hand to the installer to fix this requirement.
    pub install_spec: String,
}

/// Checks every requirement against the marker environment and installed set.
///
/// `installed` describes the target environment (see [`Installed`]).
pub fn check_all(
    requirements: &[Requirement],
    markers: &HashMap<String, String>,
    installed: &Installed,
) -> Vec<CheckResult> {
    requirements
        .iter()
        .map(|req| check_one(req, markers, installed))
        .collect()
}

fn check_one(
    req: &Requirement,
    markers: &HashMap<String, String>,
    installed: &Installed,
) -> CheckResult {
    // For URL requirements the recorded install source is the most reliable match.
    let by_url = req
        .url
        .as_deref()
        .and_then(|url| installed.urls.get(&normalize_url(url)));
    let by_name = installed.versions.get(&req.normalized_name());
    let installed_version: Option<String> = by_url.or(by_name).cloned();
    let required = match (&req.url, req.specifiers.is_empty()) {
        (Some(url), _) => format!("@ {url}"),
        (None, true) => "any".to_string(),
        (None, false) => req.specifiers.to_string(),
    };
    let result = |status: Status, detail: String| CheckResult {
        name: req.name.clone(),
        installed: installed_version.clone(),
        required: required.clone(),
        status,
        detail,
        install_spec: req.install_spec(),
    };

    if let Some(marker) = &req.marker {
        if !marker.evaluate(markers) {
            return result(
                Status::Skipped,
                format!("Skipped (marker: {})", marker.source()),
            );
        }
    }

    let Some(version_text) = installed_version.as_deref() else {
        return result(Status::Missing, "Not installed".to_string());
    };

    if req.url.is_some() {
        let detail = if by_url.is_some() {
            "Installed from this URL"
        } else {
            "Installed (matched by name, source not verified)"
        };
        return result(Status::Ok, detail.to_string());
    }
    if req.specifiers.is_empty() {
        return result(Status::Ok, "OK (any version)".to_string());
    }

    match Version::from_str(version_text) {
        Ok(version) if req.specifiers.contains(&version) => {
            result(Status::Ok, format!("OK (within {})", req.specifiers))
        }
        Ok(_) => result(
            Status::Mismatch,
            format!("Mismatch (need {})", req.specifiers),
        ),
        Err(_) => result(
            Status::Unverifiable,
            format!("Installed version {version_text:?} is not PEP 440 compliant"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reqs(lines: &[&str]) -> Vec<Requirement> {
        lines
            .iter()
            .map(|l| Requirement::from_str(l).unwrap_or_else(|e| panic!("{e}")))
            .collect()
    }

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn classifies_each_case() {
        let requirements = reqs(&[
            "requests>=2.25.0",
            "numpy>=1.20.0",
            "pandas==1.3.0",
            "pywin32>=227; sys_platform == 'win32'",
            "Flask",
            "weird>=1.0",
        ]);
        let markers = map(&[("sys_platform", "linux")]);
        let installed = Installed {
            versions: map(&[
                ("requests", "2.28.1"),
                ("pandas", "1.2.0"),
                ("flask", "3.0.0"),
                ("weird", "not-a-version"),
            ]),
            urls: HashMap::new(),
        };
        let results = check_all(&requirements, &markers, &installed);
        let statuses: Vec<Status> = results.iter().map(|r| r.status).collect();
        assert_eq!(
            statuses,
            [
                Status::Ok,
                Status::Missing,
                Status::Mismatch,
                Status::Skipped,
                Status::Ok,
                Status::Unverifiable,
            ]
        );
        assert_eq!(results[0].detail, "OK (within >=2.25.0)");
        assert_eq!(results[1].install_spec, "numpy>=1.20.0");
        assert_eq!(results[2].detail, "Mismatch (need ==1.3.0)");
        assert_eq!(results[4].detail, "OK (any version)");
    }

    #[test]
    fn vcs_requirements_match_by_recorded_url() {
        let present = Requirement::from_bare_url("git+https://github.com/cumulus13/mimelist")
            .unwrap_or_else(|e| panic!("{e}"));
        let absent = Requirement::from_bare_url("git+https://github.com/cumulus13/uptobox")
            .unwrap_or_else(|e| panic!("{e}"));
        let installed = Installed {
            versions: map(&[("mimelist", "1.0")]),
            urls: map(&[("https://github.com/cumulus13/mimelist", "1.0")]),
        };
        let results = check_all(&[present, absent], &HashMap::new(), &installed);
        assert_eq!(results[0].status, Status::Ok);
        assert_eq!(results[0].detail, "Installed from this URL");
        assert_eq!(results[0].installed.as_deref(), Some("1.0"));
        assert_eq!(results[1].status, Status::Missing);
        assert_eq!(
            results[1].install_spec,
            "git+https://github.com/cumulus13/uptobox"
        );
    }
}
