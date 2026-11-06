# VFX Launcher 0.2.0 — Windows

[Download portable ZIP](https://github.com/NickPittas/VFX-Launcher-v2/releases/download/v0.2.0/VFX-Launcher-0.2.0-Windows-x86_64.zip) · [SHA-256 checksum](https://github.com/NickPittas/VFX-Launcher-v2/releases/download/v0.2.0/VFX-Launcher-0.2.0-Windows-x86_64.zip.sha256) · [User guides](README.md)

This is the published Windows prerelease. Windows 0.3.4 is not published; the later Linux Incoming/media and validation fixes are not included here. See [Windows verification evidence](migration/NATIVE-WINDOWS-EVIDENCE.md).

Verify the archive before extracting:

```powershell
Get-FileHash .\VFX-Launcher-0.2.0-Windows-x86_64.zip -Algorithm SHA256
Get-Content .\VFX-Launcher-0.2.0-Windows-x86_64.zip.sha256
```

Compare the hash values; they must match.

Extract `VFX-Launcher-0.2.0-Windows-x86_64.zip` into a local folder, then
double-click `vfx-launcher.exe`. No Python, Rust or installer is needed to run it.
Windows 10/11 x64 with working desktop OpenGL drivers is required.

The Windows package includes the native desktop, project catalog/scanning,
version selection, settings, template/project/shot creation, Incoming copy/move,
Copy by File Type and backup execution. Configured Nuke/After Effects and the
optional Pi harness must already be installed.

Transfers use the same reviewed, journaled executor as Linux, with native Windows
handle-relative opens, exclusive staging and no-clobber renames. Reparse points
and cross-volume descendants below a selected root are refused. File contents
are flushed before publication. Under the approved Windows completion policy,
Move/Replace completes after the native operation succeeds; sudden power-loss
durability of directory entries remains unconfirmed and is recorded in review,
journal and results. Permissions/ACLs and alternate data streams are not copied.
Read-only or locked files can refuse replacement/removal and retain recovery
paths. Network shares and power-loss recovery have not been runtime verified.


Settings and the catalog use the local Windows application-data directory.
Existing Python databases are not adopted automatically. For isolated use:

```powershell
Start-Process .\vfx-launcher.exe -ArgumentList '--data-dir C:\VfxLauncherData' -Wait
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
portable zip and SHA-256 checksum in `artifacts/releases/`. Dependencies,
temporary files and logs stay inside the checkout. The build has a 20-minute
limit. No test suites are run.
