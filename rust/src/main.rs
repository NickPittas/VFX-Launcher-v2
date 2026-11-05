//! GUI/CLI entry point.
//!
//! No command is routed into the UI: with no arguments the native window starts,
//! `--help` prints usage, and anything else is rejected instead of being silently
//! ignored. Command packets (`backup --job ...`) attach here when they land.

use std::process::ExitCode;

const USAGE: &str = "\
VFX Launcher

Usage:
  vfx-launcher            Start the desktop application
  vfx-launcher --help     Show this message

No other command is implemented in this build.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        None => match vfx_launcher::run_gui() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("vfx-launcher: {error}");
                ExitCode::FAILURE
            }
        },
        Some("-h" | "--help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("vfx-launcher: command '{command}' is not implemented in this build.");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
