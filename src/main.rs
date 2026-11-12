use std::process::ExitCode;

use clap::Parser;
use clap_version_flag::colorful_version;
use std::ffi::OsString;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.len() == 2 && (args[1] == "-V" || args[1] == "--version") {
        let version = colorful_version!();
        version.print_and_exit();
    }
    let cli = reqtidy::cli::Cli::parse();
    match reqtidy::run(&cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}
