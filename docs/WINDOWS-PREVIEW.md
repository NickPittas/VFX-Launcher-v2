# VFX Launcher 0.2.0 — Windows preview

Extract `VFX-Launcher-0.2.0-Windows-x86_64-preview.zip` into a local folder, then
double-click `vfx-launcher.exe`. No Python, Rust or installer is needed to run it.
Windows 10/11 x64 with working desktop OpenGL drivers is required.

This preview provides the native desktop, project catalog/scanning, version
selection, settings, and configured Nuke/After Effects launching. Those external
applications must already be installed. Read-only Incoming/transfer planning is
available, but Windows filesystem execution has not been ported: project/shot
creation, copying/moving and backup execution refuse without modifying files.
The application displays this limitation. Use the Linux release for those file
operations. This is not Windows feature parity or a production sign-off.

Settings and the catalog use the local Windows application-data directory.
Existing Python databases are not adopted automatically. For isolated use:

```powershell
Start-Process .\vfx-launcher.exe -ArgumentList '--data-dir C:\VfxPreviewData' -Wait
```

The executable uses the Windows GUI subsystem, so console output should be
captured when using its CLI from automation. For example:

```powershell
Start-Process .\vfx-launcher.exe -ArgumentList '--version' -Wait `
  -RedirectStandardOutput version.txt
Get-Content version.txt
```

## Build

Install Rust 1.95+ and Visual Studio C++ Build Tools with a Windows SDK. From a
checkout, run `powershell -NoProfile -File .\build_windows.ps1`.
The script builds x64 with the C runtime linked statically, then creates the
portable preview zip and SHA-256 checksum in `artifacts/releases/`. Dependencies,
temporary files and logs stay inside the checkout. The build has a 20-minute
limit. No test suites are run.
