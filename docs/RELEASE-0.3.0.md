# VFX Launcher 0.3.0

Linux x86_64 AppImage of the merged Rust/egui application.

- Incoming image/sequence preview with exact-frame navigation, zoom and pan.
- DJV, MRV2 and Nuke Player context actions and executable settings.
- Send to Nuke through a running localhost NukeServerSocket session on port 49512.
- Bounded Python-style regex matching, including lookarounds and backreferences,
  with explicit unsupported-pattern and execution-limit diagnostics.
- Settings acknowledgement, user attribution/deletion, import adoption and
  history corrections; current-match validation and project-dialog layout fixes.

The package includes Projects, Shots & Sequences, Incoming, Copy by File Type
and Backup. This release does not claim complete runtime parity. The latest
media/player/Nuke socket and lifecycle changes compile, but live verification
is pending while the user works on the build machine. No GUI was launched for
this packaging run and no test suites were run.

GTK3 and supporting libraries are bundled. GPU drivers, Nuke, external players,
FFmpeg/ffprobe for additional preview formats, and the optional Pi harness are
host dependencies. Nuke must have NukeServerSocket enabled to accept sends.
Schedules run while the application is open. Production-data, network-storage,
hostile namespace-race and power-loss behavior are not established by this build.

No Windows 0.3.0 package or Flatpak is included in this Linux packaging request.
