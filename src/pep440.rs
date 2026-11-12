//! A small, dependency-free implementation of the parts of [PEP 440] that a
//! requirements checker needs: version parsing/ordering and version specifiers.
//!
//! [PEP 440]: https://peps.python.org/pep-0440/

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// Error returned when a string is not a valid PEP 440 version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid version: {0:?}")]
pub struct VersionParseError(pub String);

/// Error returned when a string is not a valid PEP 440 version specifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid version specifier: {0:?}")]
pub struct SpecifierParseError(pub String);

fn bad_version(input: &str) -> VersionParseError {
    VersionParseError(input.to_string())
}

/// Kind of pre-release segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PreKind {
    /// `a` / `alpha`
    Alpha,
    /// `b` / `beta`
    Beta,
    /// `rc` / `c` / `pre` / `preview`
    Rc,
}

/// One dot-separated segment of a local version label.
///
/// Variant order matters: alphanumeric segments sort *before* numeric ones.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LocalSegment {
    /// Alphanumeric segment.
    Str(String),
    /// Purely numeric segment.
    Num(u64),
}

/// A parsed PEP 440 version.
#[derive(Debug, Clone)]
pub struct Version {
    /// Epoch (`N!` prefix), `0` when absent.
    pub epoch: u64,
    /// Release segments, e.g. `[1, 4, 2]` for `1.4.2`.
    pub release: Vec<u64>,
    /// Pre-release, if any.
    pub pre: Option<(PreKind, u64)>,
    /// Post-release, if any.
    pub post: Option<u64>,
    /// Development release, if any.
    pub dev: Option<u64>,
    /// Local version label segments (after `+`).
    pub local: Vec<LocalSegment>,
}

impl Version {
    /// `true` for pre-releases and development releases.
    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some() || self.dev.is_some()
    }

    /// `true` for post-releases.
    pub fn is_postrelease(&self) -> bool {
        self.post.is_some()
    }

    /// Epoch + release only (drops pre/post/dev/local).
    fn base(&self) -> Version {
        Version {
            epoch: self.epoch,
            release: self.release.clone(),
            pre: None,
            post: None,
            dev: None,
            local: Vec::new(),
        }
    }

    /// The version without its local label.
    fn public(&self) -> Version {
        let mut v = self.clone();
        v.local.clear();
        v
    }

    fn zero() -> Version {
        Version {
            epoch: 0,
            release: vec![0],
            pre: None,
            post: None,
            dev: None,
            local: Vec::new(),
        }
    }

    fn pre_key(&self) -> (i8, u8, u64) {
        match (&self.pre, &self.post, &self.dev) {
            // A bare `X.devN` sorts before any pre-release of X.
            (None, None, Some(_)) => (-1, 0, 0),
            (None, _, _) => (1, 0, 0),
            (Some((kind, n)), _, _) => (0, *kind as u8, *n),
        }
    }

    fn post_key(&self) -> (i8, u64) {
        match self.post {
            None => (-1, 0),
            Some(n) => (0, n),
        }
    }

    fn dev_key(&self) -> (i8, u64) {
        match self.dev {
            None => (1, 0),
            Some(n) => (0, n),
        }
    }
}

fn cmp_release(a: &[u64], b: &[u64]) -> Ordering {
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.epoch
            .cmp(&other.epoch)
            .then_with(|| cmp_release(&self.release, &other.release))
            .then_with(|| self.pre_key().cmp(&other.pre_key()))
            .then_with(|| self.post_key().cmp(&other.post_key()))
            .then_with(|| self.dev_key().cmp(&other.dev_key()))
            .then_with(|| self.local.cmp(&other.local))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.epoch != 0 {
            write!(f, "{}!", self.epoch)?;
        }
        let release: Vec<String> = self.release.iter().map(|n| n.to_string()).collect();
        f.write_str(&release.join("."))?;
        if let Some((kind, n)) = &self.pre {
            let tag = match kind {
                PreKind::Alpha => "a",
                PreKind::Beta => "b",
                PreKind::Rc => "rc",
            };
            write!(f, "{tag}{n}")?;
        }
        if let Some(n) = self.post {
            write!(f, ".post{n}")?;
        }
        if let Some(n) = self.dev {
            write!(f, ".dev{n}")?;
        }
        if !self.local.is_empty() {
            let parts: Vec<String> = self
                .local
                .iter()
                .map(|seg| match seg {
                    LocalSegment::Str(s) => s.clone(),
                    LocalSegment::Num(n) => n.to_string(),
                })
                .collect();
            write!(f, "+{}", parts.join("."))?;
        }
        Ok(())
    }
}

fn take_number(s: &str) -> Option<(u64, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let n = s[..end].parse::<u64>().ok()?;
    Some((n, &s[end..]))
}

fn skip_sep(s: &str) -> &str {
    s.strip_prefix(['.', '-', '_']).unwrap_or(s)
}

fn parse_pre(s: &str) -> Option<(PreKind, u64, &str)> {
    let r = skip_sep(s);
    // Longer spellings must be tried before their prefixes.
    const NAMES: [(&str, PreKind); 8] = [
        ("alpha", PreKind::Alpha),
        ("beta", PreKind::Beta),
        ("preview", PreKind::Rc),
        ("pre", PreKind::Rc),
        ("rc", PreKind::Rc),
        ("a", PreKind::Alpha),
        ("b", PreKind::Beta),
        ("c", PreKind::Rc),
    ];
    for (name, kind) in NAMES {
        if let Some(after) = r.strip_prefix(name) {
            let numbered = skip_sep(after);
            return Some(match take_number(numbered) {
                Some((n, rest)) => (kind, n, rest),
                None => (kind, 0, after),
            });
        }
    }
    None
}

fn parse_post(s: &str) -> Option<(u64, &str)> {
    // Implicit post-release: `1.0-1`.
    if let Some(after) = s.strip_prefix('-') {
        if let Some(found) = take_number(after) {
            return Some(found);
        }
    }
    let r = skip_sep(s);
    for name in ["post", "rev", "r"] {
        if let Some(after) = r.strip_prefix(name) {
            let numbered = skip_sep(after);
            return Some(match take_number(numbered) {
                Some((n, rest)) => (n, rest),
                None => (0, after),
            });
        }
    }
    None
}

fn parse_dev(s: &str) -> Option<(u64, &str)> {
    let r = skip_sep(s).strip_prefix("dev")?;
    let numbered = skip_sep(r);
    Some(match take_number(numbered) {
        Some((n, rest)) => (n, rest),
        None => (0, r),
    })
}

impl FromStr for Version {
    type Err = VersionParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let lowered = input.trim().to_ascii_lowercase();
        let body = lowered.strip_prefix('v').unwrap_or(&lowered);
        if body.is_empty() {
            return Err(bad_version(input));
        }

        let (public, local_str) = match body.split_once('+') {
            Some((p, l)) => (p, Some(l)),
            None => (body, None),
        };

        let (epoch, rest) = match public.split_once('!') {
            Some((e, r)) => (e.parse::<u64>().map_err(|_| bad_version(input))?, r),
            None => (0, public),
        };

        let (first, mut rest) = take_number(rest).ok_or_else(|| bad_version(input))?;
        let mut release = vec![first];
        while let Some(tail) = rest.strip_prefix('.') {
            match take_number(tail) {
                Some((n, r)) => {
                    release.push(n);
                    rest = r;
                }
                None => break,
            }
        }

        let mut pre = None;
        let mut post = None;
        let mut dev = None;

        if let Some((kind, n, r)) = parse_pre(rest) {
            pre = Some((kind, n));
            rest = r;
        }
        if let Some((n, r)) = parse_post(rest) {
            post = Some(n);
            rest = r;
        }
        if let Some((n, r)) = parse_dev(rest) {
            dev = Some(n);
            rest = r;
        }
        if !rest.is_empty() {
            return Err(bad_version(input));
        }

        let mut local = Vec::new();
        if let Some(label) = local_str {
            if label.is_empty() {
                return Err(bad_version(input));
            }
            for seg in label.split(['.', '-', '_']) {
                if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(bad_version(input));
                }
                let parsed = if seg.chars().all(|c| c.is_ascii_digit()) {
                    match seg.parse::<u64>() {
                        Ok(n) => LocalSegment::Num(n),
                        Err(_) => LocalSegment::Str(seg.to_string()),
                    }
                } else {
                    LocalSegment::Str(seg.to_string())
                };
                local.push(parsed);
            }
        }

        Ok(Version {
            epoch,
            release,
            pre,
            post,
            dev,
            local,
        })
    }
}

/// A version comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `<=`
    LessEq,
    /// `>=`
    GreaterEq,
    /// `<`
    Less,
    /// `>`
    Greater,
    /// `~=`
    Compatible,
    /// `===`
    Arbitrary,
}

impl Operator {
    /// The operator as written in a requirement.
    pub fn as_str(self) -> &'static str {
        match self {
            Operator::Equal => "==",
            Operator::NotEqual => "!=",
            Operator::LessEq => "<=",
            Operator::GreaterEq => ">=",
            Operator::Less => "<",
            Operator::Greater => ">",
            Operator::Compatible => "~=",
            Operator::Arbitrary => "===",
        }
    }
}

/// A single version clause such as `>=1.2` or `==2.*`.
#[derive(Debug, Clone)]
pub struct Specifier {
    op: Operator,
    version: Version,
    wildcard: bool,
    raw: String,
}

impl Specifier {
    /// Builds a specifier from an operator and the version text that follows it.
    pub fn new(op: Operator, version: &str) -> Result<Self, SpecifierParseError> {
        let raw = version.trim();
        let bad = || SpecifierParseError(format!("{}{}", op.as_str(), raw));

        if op == Operator::Arbitrary {
            if raw.is_empty() {
                return Err(bad());
            }
            return Ok(Specifier {
                op,
                version: Version::from_str(raw).unwrap_or_else(|_| Version::zero()),
                wildcard: false,
                raw: raw.to_string(),
            });
        }

        let (core, wildcard) = match raw.strip_suffix(".*") {
            Some(core) => (core, true),
            None => (raw, false),
        };
        if wildcard && !matches!(op, Operator::Equal | Operator::NotEqual) {
            return Err(bad());
        }
        let parsed = Version::from_str(core).map_err(|_| bad())?;
        if op == Operator::Compatible && parsed.release.len() < 2 {
            return Err(bad());
        }
        Ok(Specifier {
            op,
            version: parsed,
            wildcard,
            raw: raw.to_string(),
        })
    }

    /// The operator of this clause.
    pub fn operator(&self) -> Operator {
        self.op
    }

    fn prefix_match(&self, candidate: &Version, prefix: &[u64]) -> bool {
        candidate.epoch == self.version.epoch
            && prefix
                .iter()
                .enumerate()
                .all(|(i, p)| candidate.release.get(i).copied().unwrap_or(0) == *p)
    }

    fn equal_match(&self, candidate: &Version) -> bool {
        if self.wildcard {
            self.prefix_match(candidate, &self.version.release)
        } else if self.version.local.is_empty() {
            candidate.public() == self.version
        } else {
            *candidate == self.version
        }
    }

    /// Returns `true` when `candidate` satisfies this clause.
    ///
    /// Pre-releases are accepted: the checker asks whether an *installed*
    /// version fits, mirroring `pip check`.
    pub fn contains(&self, candidate: &Version) -> bool {
        match self.op {
            Operator::Arbitrary => candidate.to_string().eq_ignore_ascii_case(&self.raw),
            Operator::Equal => self.equal_match(candidate),
            Operator::NotEqual => !self.equal_match(candidate),
            Operator::LessEq => candidate.public() <= self.version,
            Operator::GreaterEq => candidate.public() >= self.version,
            Operator::Less => {
                let c = candidate.public();
                if c >= self.version {
                    return false;
                }
                // `<V` must not match pre-releases of V itself (PEP 440).
                let excluded_pre = !self.version.is_prerelease()
                    && c.is_prerelease()
                    && c.base() == self.version.base();
                !excluded_pre
            }
            Operator::Greater => {
                let c = candidate.public();
                if c <= self.version {
                    return false;
                }
                // `>V` must not match post-releases of V itself (PEP 440).
                let excluded_post = !self.version.is_postrelease()
                    && c.is_postrelease()
                    && c.base() == self.version.base();
                !excluded_post
            }
            Operator::Compatible => {
                let keep = self.version.release.len() - 1;
                candidate.public() >= self.version
                    && self.prefix_match(candidate, &self.version.release[..keep])
            }
        }
    }
}

impl fmt::Display for Specifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.op.as_str(), self.raw)
    }
}

impl FromStr for Specifier {
    type Err = SpecifierParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let s = input.trim();
        // `===` must be tried before `==`, and two-character operators before `<`/`>`.
        const OPS: [(&str, Operator); 8] = [
            ("===", Operator::Arbitrary),
            ("==", Operator::Equal),
            ("!=", Operator::NotEqual),
            ("<=", Operator::LessEq),
            (">=", Operator::GreaterEq),
            ("~=", Operator::Compatible),
            ("<", Operator::Less),
            (">", Operator::Greater),
        ];
        for (token, op) in OPS {
            if let Some(rest) = s.strip_prefix(token) {
                return Specifier::new(op, rest);
            }
        }
        Err(SpecifierParseError(s.to_string()))
    }
}

/// A comma-separated set of [`Specifier`]s. The empty set matches everything.
#[derive(Debug, Clone, Default)]
pub struct SpecifierSet {
    specs: Vec<Specifier>,
}

impl SpecifierSet {
    /// `true` when there are no clauses (any version is acceptable).
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Returns `true` when `candidate` satisfies every clause.
    pub fn contains(&self, candidate: &Version) -> bool {
        self.specs.iter().all(|s| s.contains(candidate))
    }
}

impl fmt::Display for SpecifierSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.specs.iter().map(|s| s.to_string()).collect();
        f.write_str(&parts.join(","))
    }
}

impl FromStr for SpecifierSet {
    type Err = SpecifierParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut s = input.trim();
        if s.starts_with('(') && s.ends_with(')') && s.len() >= 2 {
            s = s[1..s.len() - 1].trim();
        }
        if s.is_empty() {
            return Ok(SpecifierSet::default());
        }
        let mut specs = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(SpecifierParseError(input.trim().to_string()));
            }
            specs.push(Specifier::from_str(part)?);
        }
        Ok(SpecifierSet { specs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::from_str(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn set(s: &str) -> SpecifierSet {
        SpecifierSet::from_str(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn holds(spec: &str, version: &str) -> bool {
        set(spec).contains(&v(version))
    }

    #[test]
    fn parses_and_normalises() {
        assert_eq!(v("1.0RC1").to_string(), "1.0rc1");
        assert_eq!(v("v2.0-1").to_string(), "2.0.post1");
        assert_eq!(v("1!2.0").to_string(), "1!2.0");
        assert_eq!(v("1.0.dev3").to_string(), "1.0.dev3");
        assert_eq!(v("1.0-alpha.2").to_string(), "1.0a2");
        assert_eq!(v("1.0+Local.7").to_string(), "1.0+local.7");
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "abc", "1..0", "1.0+", "1.0.foo", "linux", "x86_64"] {
            assert!(Version::from_str(bad).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn orders_like_pep440() {
        let order = [
            "1.0.dev1", "1.0a1", "1.0b2", "1.0rc1", "1.0", "1.0.post1", "1.1",
        ];
        for pair in order.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("1.0"), v("1.0.0"));
    }

    #[test]
    fn range_specifiers() {
        assert!(holds(">=1.2,<2", "1.5"));
        assert!(!holds(">=1.2,<2", "2.0"));
        assert!(!holds("<2", "2.0rc1"));
        assert!(holds(">=2.25.0", "2.28.1"));
        assert!(!holds("==1.3.0", "1.2.0"));
        assert!(!holds("<=1.0", "1.0.post1"));
        assert!(!holds(">=1.0", "1.0rc1"));
    }

    #[test]
    fn compatible_release() {
        assert!(holds("~=1.4.5", "1.4.9"));
        assert!(!holds("~=1.4.5", "1.5.0"));
        assert!(!holds("~=1.4.5", "1.4.4"));
        assert!(holds("~=2.2", "2.9"));
        assert!(!holds("~=2.2", "3.0"));
        assert!(SpecifierSet::from_str("~=2").is_err());
    }

    #[test]
    fn wildcard_and_equality() {
        assert!(holds("==1.4.*", "1.4.7"));
        assert!(!holds("==1.4.*", "1.5.0"));
        assert!(holds("==1.4.*", "1.4"));
        assert!(!holds("!=1.0", "1.0.0"));
        assert!(!holds("!=1.5.*", "1.5.2"));
        assert!(holds("==1.0", "1.0+local"));
        assert!(holds("===1.0", "1.0"));
    }

    #[test]
    fn exclusive_comparisons_skip_related_releases() {
        assert!(!holds(">1.0", "1.0.post1"));
        assert!(holds(">1.0.post1", "1.0.post2"));
    }

    #[test]
    fn set_parsing() {
        assert!(set("").is_empty());
        assert!(set("(>=1.0)").contains(&v("1.1")));
        assert_eq!(set(">= 1.0 , < 2").to_string(), ">=1.0,<2");
        assert!(SpecifierSet::from_str(">=1.0,").is_err());
        assert!(SpecifierSet::from_str("1.0").is_err());
        assert!(SpecifierSet::from_str("==").is_err());
    }
}
