//! The Incoming context-menu players `Open in DJV`, `Open in MRV2` and
//! `Open in Nuke Player`.
//!
//! Port of the three active launch paths of
//! `CleanIncomings/python/utils/media_player.py` (`launch_djv`,
//! `launch_mrv2`, `launch_nuke_player`) as triggered by the preview-tree
//! context menu in `app_gui_pyqt5.py`. The media argument is always the
//! item's actual source path — never a destination or a display label — and an
//! image sequence becomes the pattern those players understand: `@` runs for
//! DJV, printf `%0Nd` for MRV2, `#` runs for Nuke. The generic gallery/audio
//! engine of the legacy module (ffplay, VLC, mpv, socket sends) is not ported
//! here.
//!
//! Nothing is detected on the system: the executable is the path configured in
//! Settings and passed in by the caller.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::incoming::IncomingItem;

/// The external players offered for an Incoming item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaPlayer {
    Djv,
    Mrv2,
    Nuke,
}

impl MediaPlayer {
    /// User-facing name, matching the context-menu action labels.
    pub fn label(self) -> &'static str {
        match self {
            Self::Djv => "DJV",
            Self::Mrv2 => "MRV2",
            Self::Nuke => "Nuke Player",
        }
    }

    /// The frame token the legacy launchers write for the sequence digits:
    /// DJV `@@@@`, MRV2 `%04d`, Nuke `####`. The digit count is the sequence's
    /// actual frame padding, so no minimum is imposed.
    fn frame_token(self, padding: usize) -> String {
        match self {
            Self::Djv => "@".repeat(padding),
            Self::Mrv2 => format!("%0{padding}d"),
            Self::Nuke => "#".repeat(padding),
        }
    }

    /// Arguments that precede the media path. Only Nuke Player takes options
    /// (`--safe --workspace Flipbook --player`, exactly as the legacy builder
    /// added them); DJV and MRV2 receive the path alone.
    fn leading_arguments(self) -> &'static [&'static str] {
        match self {
            Self::Djv | Self::Mrv2 => &[],
            Self::Nuke => &["--safe", "--workspace", "Flipbook", "--player"],
        }
    }
}

/// Plans and starts one player launch, for the worker thread.
///
/// `Ok(())` means the OS accepted the spawn; it says nothing about the player
/// having finished starting. The only filesystem-visible effect is the
/// process itself.
pub fn launch(item: &IncomingItem, player: MediaPlayer, executable: &Path) -> Result<(), String> {
    let media = media_argument(item, player)?;
    let mut command = Command::new(executable);
    command.args(player.leading_arguments());
    command.arg(&media);
    // Same environment rule as every other external launch: licence variables
    // reach the application, bundle-local library paths do not.
    crate::launch::sanitize_environment(&mut command);
    // The player outlives this process and nothing is read back from it, so no
    // pipe may be left open for it to block on.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    no_console_window(&mut command);
    let child = command.spawn().map_err(|error| {
        format!(
            "{} could not be started from {} ({error}).",
            player.label(),
            executable.display()
        )
    })?;
    reap_in_background(child);
    Ok(())
}

/// The native argument for one item: its own path for a single file, a
/// player-specific pattern path for a sequence.
fn media_argument(item: &IncomingItem, player: MediaPlayer) -> Result<OsString, String> {
    match item {
        IncomingItem::Single(single) => Ok(single.file.path.as_os_str().to_os_string()),
        IncomingItem::Sequence(sequence) => {
            crate::incoming::sequence_pattern_path(sequence, &player.frame_token(sequence.padding))
                .map(|path| path.as_os_str().to_os_string())
        }
    }
}

/// A GUI player must not flash a console window when it is started from the
/// Windows build.
fn no_console_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// The launched player is not ours to wait for, but an unreaped child would
/// stay a zombie for the life of this process. One detached thread per launch
/// reaps it without blocking or delaying exit, exactly as
/// `launch::spawn` does for the configured applications.
fn reap_in_background(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}
