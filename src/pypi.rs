//! Looking up package metadata on pypi.org.

use std::time::Duration;

use anyhow::{bail, Result};
use serde::Deserialize;

use crate::requirement::normalize_name;

/// The subset of PyPI's JSON API that is used.
#[derive(Debug, Deserialize)]
pub struct PypiInfo {
    /// Latest (non-yanked, non-pre-release unless that is all there is) version.
    pub version: String,
    /// One-line summary, if provided.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PypiResponse {
    info: PypiInfo,
}

/// Fetches the latest release info for `name`. Returns `Ok(None)` for unknown packages.
pub fn fetch_latest(name: &str) -> Result<Option<PypiInfo>> {
    let normalized = normalize_name(name);
    if normalized.is_empty()
        || !normalized
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        bail!("{name:?} is not a valid package name");
    }
    let url = format!("https://pypi.org/pypi/{normalized}/json");
    log::debug!("GET {url}");

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("reqtidy/", env!("CARGO_PKG_VERSION")))
        .build();

    match agent.get(&url).call() {
        Ok(response) => {
            let body: PypiResponse = response.into_json()?;
            Ok(Some(body.info))
        }
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context(format!("request to {url} failed"))),
    }
}
