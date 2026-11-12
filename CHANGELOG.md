# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
this project adheres to [Semantic Versioning](https://semver.org/).

## [0.1.0] - Unreleased

### Added
- Initial Rust rewrite of [`pipr`](https://github.com/cumulus13/pipr).
- `requirements.txt` parsing with PEP 440 specifiers and PEP 508 markers,
  `-r` includes, extras, direct references, UTF-16 support.
- Status table, interactive install flow, batch install with per-package
  fallback and retry, `pip` and `uv` backends.
- `--check` / `--json` for CI, `--pypi` comparison, `--scan` for missing imports.
- Optional desktop notifications (`notify` feature).
