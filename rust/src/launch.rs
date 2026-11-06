//! P2: opening a project file in its DCC application, and revealing a file.
//!
//! Every child process is started with an explicit argv — never a shell — so a
//! file name containing spaces or quotes stays exactly one argument. The legacy
//! Linux Nuke command is tokenized with [`shlex::split`] (a POSIX shell lexer,
//! the same word splitting the legacy `shlex.split` performed) instead of an
//! ad-hoc quote parser.
//!
//! Legacy references (`gitclones`-independent, in-repo):
//!
//! - `ui_slick/main_window.py::_launch_file` — argv shapes:
//!   `.nk`/`.nknc` on Linux: `shlex.split(nuke_launch_cmd_linux) + [file]`;
//!   `.nk`/`.nknc` elsewhere: `[nuke_path, "--nukex", file]`;
//!   `.aep`/`.aet`: `[after_effects_path, file]`.
//! - `core/utils.py::get_external_process_env` — environment sanitation.
//! - `ui_slick/file_context_menu.py::open_in_file_manager` — reveal.
//!
//! Deliberate differences from the legacy code, both stated where they happen:
//! the process is started only after the configured program and the chosen file
//! were re-checked in the worker (the legacy path spawned straight from the UI
//! callback), and file access is logged by the caller only after a successful
//! spawn. A successful spawn is not evidence that the application finished
//! starting, so nothing here reports "started" as if it were verified.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Applications;

/// Application a project file is opened in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Nuke,
    AfterEffects,
}

impl Tool {
    /// Maps a catalog `files.filetype` value to the tool that opens it.
    pub fn from_filetype(filetype: &str) -> Option<Self> {
        match filetype {
            "nk" => Some(Self::Nuke),
            "aep" => Some(Self::AfterEffects),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Nuke => "Nuke",
            Self::AfterEffects => "After Effects",
        }
    }
}

/// A launch failure with a readable cause. The message is product copy; the
/// technical detail belongs in the caller's collapsible details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchError {
    message: String,
}

impl LaunchError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// One fully resolved child process: program, argv and the tool it belongs to.
/// Arguments are native `OsString`s, so nothing is re-split or re-quoted.
#[derive(Clone, Debug)]
pub struct PlannedLaunch {
    pub tool: Tool,
    /// The file being opened, kept separately from `arguments` so the pre-spawn
    /// check never has to guess which argument it is.
    pub file: PathBuf,
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    /// True when the program is a bare name resolved through `PATH` rather than a
    /// configured path, so no executable check is possible before spawning.
    pub path_search: bool,
}

/// Builds the argv for one file without touching the filesystem.
pub fn plan(
    tool: Tool,
    file: &Path,
    applications: &Applications,
) -> Result<PlannedLaunch, LaunchError> {
    match tool {
        Tool::Nuke if !cfg!(target_os = "linux") => {
            let program = configured(applications.nuke_path.as_ref(), "Nuke")?;
            Ok(planned(
                tool,
                file,
                program,
                [OsString::from("--nukex"), file.as_os_str().to_owned()],
            ))
        }
        Tool::Nuke => {
            // Legacy Linux path: the whole configured command line is the launcher
            // (a wrapper script, an env prefix, a terminal, …) and the file is
            // appended as one argument.
            let command = applications
                .nuke_launch_cmd_linux
                .as_deref()
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .ok_or_else(|| {
                    LaunchError::new(
                        "The Linux Nuke launch command is not configured. Set it in Settings.",
                    )
                })?;
            let words = shlex::split(command).ok_or_else(|| {
                LaunchError::new("The Linux Nuke launch command has unbalanced quotes.")
            })?;
            let mut words = words.into_iter();
            let program = words
                .next()
                .filter(|word| !word.is_empty())
                .ok_or_else(|| LaunchError::new("The Linux Nuke launch command is empty."))?;
            let mut arguments: Vec<OsString> = words.map(OsString::from).collect();
            arguments.push(file.as_os_str().to_owned());
            Ok(planned(tool, file, PathBuf::from(program), arguments))
        }
        Tool::AfterEffects => {
            let program = configured(applications.after_effects_path.as_ref(), "After Effects")?;
            Ok(planned(tool, file, program, [file.as_os_str().to_owned()]))
        }
    }
}

fn configured(path: Option<&crate::config::JsonPath>, tool: &str) -> Result<PathBuf, LaunchError> {
    path.map(|path| path.as_path().to_path_buf())
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            LaunchError::new(format!(
                "The {tool} application path is not configured. Set it in Settings."
            ))
        })
}

fn planned(
    tool: Tool,
    file: &Path,
    program: PathBuf,
    arguments: impl IntoIterator<Item = OsString>,
) -> PlannedLaunch {
    let path_search = !program.is_absolute() && program.components().count() == 1;
    PlannedLaunch {
        tool,
        file: file.to_path_buf(),
        program,
        arguments: arguments.into_iter().collect(),
        path_search,
    }
}

/// Re-validates the plan against the live filesystem and starts the process.
///
/// Called on a worker thread: the metadata checks and the spawn both stay off
/// the UI thread. `Ok(())` means the OS accepted the spawn, nothing more.
pub fn spawn(launch: &PlannedLaunch) -> Result<(), LaunchError> {
    if !launch.path_search {
        let metadata = std::fs::metadata(&launch.program).map_err(|error| {
            LaunchError::new(format!(
                "The configured {} application could not be found: {} ({error})",
                launch.tool.label(),
                launch.program.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(LaunchError::new(format!(
                "The configured {} application is not a file: {}",
                launch.tool.label(),
                launch.program.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(LaunchError::new(format!(
                    "The configured {} application is not executable: {}",
                    launch.tool.label(),
                    launch.program.display()
                )));
            }
        }
    }

    match std::fs::metadata(&launch.file) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(LaunchError::new(format!(
                "The selected file is no longer a file: {}",
                launch.file.display()
            )));
        }
        Err(error) => {
            return Err(LaunchError::new(format!(
                "The selected file is no longer available: {} ({error})",
                launch.file.display()
            )));
        }
    }

    let mut command = Command::new(&launch.program);
    command.args(&launch.arguments);
    sanitize_environment(&mut command);
    let child = command.spawn().map_err(|error| {
        LaunchError::new(format!(
            "The {} launch command could not be started: {error}",
            launch.tool.label()
        ))
    })?;
    reap_in_background(child);
    Ok(())
}

/// Plans and starts one launch in a single call, for the worker thread.
pub fn start(tool: Tool, file: &Path, applications: &Applications) -> Result<(), LaunchError> {
    let launch = plan(tool, file, applications)?;
    spawn(&launch)
}

/// Reveals and selects a file in the platform file manager.
pub fn reveal(file: &Path) -> Result<(), LaunchError> {
    #[cfg(windows)]
    {
        // Explorer's `/select,` switch takes the switch and the quoted path as one
        // token; `"` cannot occur in a Windows file name, so quoting is exact.
        let text = file.to_str().ok_or_else(|| {
            LaunchError::new("The file path cannot be represented for Explorer's selection.")
        })?;
        let mut command = Command::new("explorer.exe");
        command.arg(OsString::from(format!("/select,\"{text}\"")));
        return spawn_and_reap(command, "the file manager");
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg("-R").arg(file);
        return spawn_and_reap(command, "the file manager");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::os::unix::ffi::OsStrExt;
        let file = std::path::absolute(file)
            .map_err(|error| LaunchError::new(format!("The file path is unavailable: {error}")))?;
        std::fs::metadata(&file).map_err(|error| {
            LaunchError::new(format!(
                "The selected file is unavailable: {} ({error})",
                file.display()
            ))
        })?;
        // Encode native filename bytes, including spaces, #, quotes and Unicode.
        let mut uri = String::from("file://");
        for byte in file.as_os_str().as_bytes() {
            if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
                uri.push(*byte as char);
            } else {
                use std::fmt::Write;
                write!(uri, "%{byte:02X}").expect("writing to a String");
            }
        }
        let mut command = Command::new("gdbus");
        command
            .args([
                "call",
                "--session",
                "--timeout",
                "5",
                "--dest",
                "org.freedesktop.FileManager1",
                "--object-path",
                "/org/freedesktop/FileManager1",
                "--method",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("['{uri}']"))
            .arg("");
        sanitize_environment(&mut command);
        let output = command.output().map_err(|error| {
            LaunchError::new(format!(
                "The file manager selection request could not start: {error}"
            ))
        })?;
        if !output.status.success() {
            return Err(LaunchError::new(format!(
                "The file manager could not reveal {}: {}",
                file.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        return Ok(());
    }
    #[allow(unreachable_code)]
    Err(LaunchError::new(
        "Revealing a file is not supported on this platform.",
    ))
}

#[cfg(any(windows, target_os = "macos"))]
fn spawn_and_reap(mut command: Command, what: &str) -> Result<(), LaunchError> {
    sanitize_environment(&mut command);
    let child = command
        .spawn()
        .map_err(|error| LaunchError::new(format!("{what} could not be started: {error}")))?;
    reap_in_background(child);
    Ok(())
}

/// The launched application is not ours to wait for, but an unreaped child would
/// stay a zombie for the life of this process. One detached thread per launch
/// reaps it without blocking the UI or delaying exit (`on_exit` never joins it).
fn reap_in_background(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Mirrors `core/utils.py::get_external_process_env`: the parent environment is
/// inherited unchanged — licence variables such as `FOUNDRY_LICENSE` or
/// `LM_LICENSE_FILE` must reach the DCC — and only values that a bundled parent
/// injects are corrected.
///
/// On Linux the legacy rule is `LD_LIBRARY_PATH_ORIG` → `LD_LIBRARY_PATH`, else
/// `LD_LIBRARY_PATH` is dropped when the process runs from a bundle. A Rust build
/// is never a PyInstaller bundle itself, so the detectable equivalent is an
/// inherited marker: `_MEIPASS`, `LD_LIBRARY_PATH_ORIG`, or an inherited
/// `LD_LIBRARY_PATH` / `PYTHONHOME` / `PYTHONPATH` / `QT_PLUGIN_PATH` /
/// `QT_QPA_PLATFORM_PLUGIN_PATH` entry inside an extracted `_MEI*` bundle
/// directory. Nothing else is touched, and no variable is dropped wholesale.
pub(crate) fn sanitize_environment(command: &mut Command) {
    if cfg!(windows) {
        return;
    }

    // The AppImage's GTK libraries and schemas must not leak into host tools.
    if std::env::var_os("VFX_APPIMAGE").as_deref() == Some(OsStr::new("1")) {
        for name in [
            "LD_LIBRARY_PATH",
            "XDG_DATA_DIRS",
            "XLOCALEDIR",
            "GSETTINGS_SCHEMA_DIR",
            "GTK_PATH",
            "GTK_EXE_PREFIX",
            "GTK_DATA_PREFIX",
            "GTK_IM_MODULE_FILE",
            "GDK_PIXBUF_MODULE_FILE",
            "GDK_PIXBUF_MODULEDIR",
            "GIO_MODULE_DIR",
            "GTK_MODULES",
            "GTK3_MODULES",
        ] {
            let saved = format!("VFX_ORIGINAL_{name}");
            match std::env::var_os(&saved) {
                Some(value) => {
                    command.env(name, value);
                }
                None => {
                    command.env_remove(name);
                }
            }
            command.env_remove(saved);
        }
        command.env_remove("VFX_APPIMAGE");
        return;
    }

    if let Some(original) = std::env::var_os("LD_LIBRARY_PATH_ORIG") {
        command.env("LD_LIBRARY_PATH", original);
        return;
    }

    let bundled = std::env::var_os("_MEIPASS").is_some();
    for name in [
        "LD_LIBRARY_PATH",
        "PYTHONHOME",
        "PYTHONPATH",
        "QT_PLUGIN_PATH",
        "QT_QPA_PLATFORM_PLUGIN_PATH",
    ] {
        let Some(value) = std::env::var_os(name) else {
            continue;
        };
        if bundled || points_into_bundle(&value) {
            command.env_remove(name);
        }
    }
}

/// True when any path entry of a variable points into an extracted PyInstaller
/// bundle (`…/_MEIxxxxx/…`). Detection only; the value is never stored.
fn points_into_bundle(value: &OsStr) -> bool {
    let text = value.to_string_lossy();
    text.split([':', ';']).any(|entry| {
        entry
            .split(['/', '\\'])
            .any(|part| part.len() > 4 && part.starts_with("_MEI"))
    })
}
