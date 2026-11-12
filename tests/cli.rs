//! End-to-end tests that exercise the compiled binary.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_reqtidy"))
}

#[test]
fn prints_version() {
    let output = bin().arg("--version").output().expect("run reqtidy");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "{stdout}");
}

#[test]
fn help_lists_the_pipr_compatible_flags() {
    let output = bin().arg("--help").output().expect("run reqtidy");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--force-install",
        "--force-retry",
        "--summary",
        "--check",
        "--pypi",
        "--no-install",
        "--no-show",
        "--debug",
        "--scan",
    ] {
        assert!(stdout.contains(flag), "help is missing {flag}:\n{stdout}");
    }
}

#[test]
fn missing_requirements_file_is_a_usage_error() {
    let output = bin()
        .arg("definitely-not-here.txt")
        .output()
        .expect("run reqtidy");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"), "{stderr}");
}

#[test]
fn empty_requirements_file_succeeds_without_python() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("requirements.txt");
    std::fs::write(&file, "# nothing here\n").expect("write");
    let output = bin().arg(&file).output().expect("run reqtidy");
    assert_eq!(output.status.code(), Some(0));
}
