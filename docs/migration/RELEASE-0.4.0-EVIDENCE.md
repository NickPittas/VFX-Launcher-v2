# Version 0.4.0 packaging — 2026-11-09

User-approved UI branch `ui/keyloom-visual-style` was committed and merged
into main. Both packages use clean source revision
`d7be84da1d13d8c21c101a892ec3b6f2de4a0261` (version 0.4.0).

| Package | SHA-256 |
| --- | --- |
| Linux x86_64 AppImage | `59b7a019ea1afdd75820b214f593cbab270a8283c7d1a778b2080f3ace06fec2` |
| Windows x64 portable ZIP | `a7ee7dfc9add013abda8edd9b4e6e3e6dae97c8af4d02298b29a358fbf5a6518` |

Linux optimized compilation passed in the existing Ubuntu 22.04 user-namespace
build environment. The AppImage returned version 0.4.0 normally and through
extraction inside Ubuntu 22.04; extraction-mode help returned successfully.
The checksum sidecar verified. The staged font notices matched their sources.
Logs: `artifacts/packaging/release-0.4.0-build.log` and
`artifacts/packaging/appimage-0.4.0-build.log`.

Windows was built natively on kamino-3 using the established checkout and
`build_windows.ps1`, x64 MSVC and static CRT. Its existing single-branch fetch
was fast-forwarded using FETCH_HEAD; no local changes were discarded. The
optimized build passed. The extracted ZIP's executable returned 0.4.0 and
help successfully on Windows. BUILD-INFO confirmed the revision above and a
clean source tree. Release notes and both font license notices matched their
sources after normalizing Windows line endings. The downloaded ZIP checksum
matched the remote archive and sidecar. Local logs and packaging checks are in
`artifacts/verification/windows-0.4.0/`; remote extracted-package evidence is
under the release checkout's `artifacts/verification/package-0.4.0/`.

The approved Linux preview is recorded in KEYLOOM-STYLE.md and
`artifacts/screenshots/keyloom-ui-first-native.png`. That approval is not a
fresh full-workflow, every-page, DPI or Windows appearance pass. No GUI was
launched during release packaging, no production data was used, and no test
suites or global tests were run. Transfer/media/provider/network/power-loss
behavior was not rechecked for this styling release. The user's open Linux
preview was left running.
