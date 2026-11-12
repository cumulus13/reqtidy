# reqtidy

[![CI](https://github.com/cumulus13/reqtidy/actions/workflows/ci.yml/badge.svg)](https://github.com/cumulus13/reqtidy/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/reqtidy.svg)](https://crates.io/crates/reqtidy)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

**A fast Python requirements checker.** It validates your `requirements.txt`
against what is installed, shows a clear status table, installs what is missing
and tells you which imports your code uses but your requirements forget —
like `go mod tidy`, for Python.

`reqtidy` is a single static binary written in Rust. It is a rewrite of
[`cumulus13/pipr`](https://github.com/cumulus13/pipr) by Hadi Cahyadi: same idea
and the same command-line flags, but with no Python dependencies of its own
(no `rich`, `packaging`, `gntp`, `licface`) and instant start-up.

## Install

```sh
cargo install reqtidy            # from crates.io
cargo binstall reqtidy           # prebuilt binary, if you use cargo-binstall
```

Prebuilt archives for Linux (x86_64/aarch64, glibc and musl), macOS (Intel and
Apple Silicon) and Windows are attached to every
[GitHub Release](https://github.com/cumulus13/reqtidy/releases), together with
SHA-256 checksums.

Optional desktop notifications: `cargo install reqtidy --features notify`.

## Usage

```sh
reqtidy                      # check ./requirements.txt, offer to install what is missing
reqtidy path/to/reqs.txt     # check another file
reqtidy --check              # read-only, exit code 1 if anything is unsatisfied (CI)
reqtidy -F -f                # install without asking, retry failures automatically
reqtidy --json               # machine-readable report
reqtidy -i rich -i requests  # compare installed packages with the latest on PyPI
reqtidy --scan=src -r        # find imports in ./src that requirements.txt does not cover
```

```text
                Package Version Checker
╭──────────┬───────────┬──────────┬────┬──────────────────────╮
│ Package  │ Installed │ Required │    │ Status               │
├──────────┼───────────┼──────────┼────┼──────────────────────┤
│ requests │ 2.28.1    │ >=2.25.0 │ ✅ │ OK (within >=2.25.0) │
│ numpy    │ none      │ >=1.20.0 │ ❌ │ Not installed        │
│ pandas   │ 1.2.0     │ ==1.3.0  │ ⚠️ │ Mismatch (need ==1.3.0) │
╰──────────┴───────────┴──────────┴────┴──────────────────────╯
```

### Options

| Option | Description |
| --- | --- |
| `FILE` | Requirements file (default `requirements.txt`) |
| `-r, --recursive` | Scan subdirectories (with `--scan`) |
| `-f, --force-retry` | Retry failed installations automatically (3 attempts) |
| `-F, --force-install` | Install without asking for confirmation |
| `-s, --summary` / `-c, --check` | Table only; nothing is installed |
| `-i, --pypi PACKAGE` | Compare with the latest release on pypi.org (repeatable) |
| `-n, --no-install` | Never install |
| `-z, --no-show` | Do not print the table |
| `-d, --debug` | Debug logging |
| `--python PATH` | Interpreter to inspect/install into (default: active venv, then `python3`/`python`) |
| `--installer auto\|pip\|uv` | Installation backend |
| `--scan[=DIR]` | Report imports missing from the requirements file |
| `--json` | JSON output, never installs |
| `--notify` | Desktop notification (needs the `notify` feature) |

### Exit codes

`0` everything satisfied · `1` unsatisfied requirements remain (or a PyPI lookup
failed) · `2` usage or runtime error.

### Interactive prompts

For each missing or mismatched package you are asked `[y/N/a/q]`
(yes / no / all / quit). Without a terminal nothing is installed unless you pass
`-F`. If a batch install fails, packages are retried one by one so a single bad
package does not block the rest.

## What is supported

* PEP 440 versions and specifiers (`==`, `!=`, `<`, `<=`, `>`, `>=`, `~=`, `===`, `==1.*`),
  implemented in-crate and following PEP 440's ordering rules.
* PEP 508 environment markers (`sys_platform`, `python_version`, `and`/`or`,
  `in`/`not in`, parentheses). Markers are evaluated against the *target*
  interpreter, so a Windows-only dependency is skipped on Linux.
* Extras, `name @ url` direct references, comments, line continuations, `-r`
  includes (cycle-safe), inline `--hash`, UTF-8 and UTF-16 files (what PowerShell
  produces for `pip freeze > requirements.txt`).
* Installed packages are read through `importlib.metadata`, so `pip` is not even
  required to *check*; `uv` works as an installer.

### Known limitations

* Editable installs (`-e`) and VCS/URL lines without `name @` are reported as
  warnings and skipped.
* Extras of an already-installed package are not verified.
* `--scan` uses a line-based import detector (imports inside docstrings can be
  false positives) and needs Python 3.10+ to recognise the standard library.
* Growl notifications from the Python original are replaced by native desktop
  notifications behind the `notify` feature.

## Development

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Minimum supported Rust version: **1.85**.

## Releasing

1. Bump `version` in `Cargo.toml` and add a section to `CHANGELOG.md`.
2. Commit, then `git tag vX.Y.Z && git push --tags`.
3. The `Release` workflow builds all binaries, creates the GitHub Release and
   runs `cargo publish` (requires the `CARGO_REGISTRY_TOKEN` repository secret).

## License

MIT. Original `pipr` © Hadi Cahyadi.


## 👤 Author
        
[Hadi Cahyadi](mailto:cumulus13@gmail.com)
    

[![Buy Me a Coffee](https://www.buymeacoffee.com/assets/img/custom_images/orange_img.png)](https://www.buymeacoffee.com/cumulus13)

[![Donate via Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/cumulus13)
 
[Support me on Patreon](https://www.patreon.com/cumulus13)