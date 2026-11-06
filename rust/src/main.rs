//! GUI/CLI entry point.
//!
//! No command is routed into the UI: with no arguments the native window starts,
//! `--help` prints usage, and anything else is rejected instead of being silently
//! ignored. `--data-dir <ABSOLUTE_PATH>` selects the local data area, which is how
//! a verification run keeps its catalog and settings away from the real one.
//! Arguments are read as [`OsString`], so a path that is not valid UTF-8 survives
//! unchanged.
//!
//! Command packets (`backup --job ...`) attach here when they land.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
VFX Launcher

Usage:
  vfx-launcher                        Start the desktop application
  vfx-launcher --data-dir <PATH>      Start with this absolute data directory
  vfx-launcher backup --job <JSON> [--dry-run] [--data-dir <PATH>]
  vfx-launcher backup --source <PATH> --destination <PATH> --dry-run
  vfx-launcher --help                 Show this message
  vfx-launcher --version              Show the application version

--data-dir must be given once, with an absolute path. The default location is the
platform application data directory. Backup schedules run only in the desktop app.";

enum Command {
    Gui {
        data_dir: Option<PathBuf>,
    },
    Backup {
        data_dir: Option<PathBuf>,
        arguments: Vec<OsString>,
    },
    Help,
    Version,
    Usage(String),
}

fn main() -> ExitCode {
    match parse(std::env::args_os().skip(1)) {
        Command::Backup {
            data_dir,
            arguments,
        } => match vfx_launcher::backup_cli::run(arguments, data_dir) {
            Ok(code) => ExitCode::from(code),
            Err(error) => {
                eprintln!("vfx-launcher backup: {error}");
                ExitCode::from(1)
            }
        },
        Command::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("VFX Launcher {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Usage(message) => {
            eprintln!("vfx-launcher: {message}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Command::Gui { data_dir } => {
            let data_dir = match data_dir {
                Some(path) => path,
                None => match vfx_launcher::config::default_data_dir() {
                    Ok(path) => path,
                    Err(error) => {
                        eprintln!("vfx-launcher: {error}");
                        return ExitCode::FAILURE;
                    }
                },
            };
            match vfx_launcher::run_gui(data_dir) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("vfx-launcher: {error}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn parse(mut args: impl Iterator<Item = OsString>) -> Command {
    let mut data_dir: Option<PathBuf> = None;
    while let Some(argument) = args.next() {
        if argument == "backup" {
            return Command::Backup {
                data_dir,
                arguments: args.collect(),
            };
        }
        if argument == "-h" || argument == "--help" {
            return Command::Help;
        }
        if argument == "-V" || argument == "--version" {
            return Command::Version;
        }
        if argument == "--data-dir" {
            if data_dir.is_some() {
                return Command::Usage("--data-dir was given more than once.".to_string());
            }
            let Some(value) = args.next() else {
                return Command::Usage("--data-dir needs an absolute path.".to_string());
            };
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Command::Usage(format!(
                    "--data-dir must be an absolute path, not '{}'.",
                    path.display()
                ));
            }
            data_dir = Some(path);
            continue;
        }
        return Command::Usage(format!(
            "argument '{}' is not recognised in this build.",
            argument.to_string_lossy()
        ));
    }
    Command::Gui { data_dir }
}
