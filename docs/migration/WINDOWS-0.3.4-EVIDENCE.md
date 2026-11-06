# Windows 0.3.4 packaging evidence — 2026-11-06

Built natively on kamino-3 in the existing `C:\Users\Nick\VFX-Launcher-v2-release\0.2.0` release checkout, updated to current main. The directory name is historical; Cargo and the executable report 0.3.4. Used `build_windows.ps1`, x64 MSVC and static CRT. No GitHub-hosted build was used for this package; the attempted Actions run was cancelled and its workflow removed.

The native optimized build succeeded in 40 seconds. The extracted portable executable reported `VFX Launcher 0.3.4`. A disposable backup preview reviewed one file with no destination mutation. Actual backup copied one 40-byte Nuke fixture, with zero failures/skips/cancellations; source and destination SHA-256 hashes matched. No production files or catalog were used and no test suites were run.

Remote evidence is under `artifacts/verification/package-0.3.4-d20e0a5051b44182ac838ddab5a06e04/`; local packaging inputs/results are under `artifacts/verification/windows-0.3.4/`. `BUILD-INFO.txt` in the final archive records its exact clean source commit. Documentation was refreshed before final packaging; the executable source is unchanged from the verified build.

No new GUI, Nuke/player, Pi-provider, network-share, cancellation/load, hostile-writer or power-loss checks were performed. Earlier [Windows 0.2.0 native evidence](NATIVE-WINDOWS-EVIDENCE.md) is historical, not a fresh verification of these workflows. Directory-entry power-loss durability remains unconfirmed; ACLs and alternate data streams are not copied.
