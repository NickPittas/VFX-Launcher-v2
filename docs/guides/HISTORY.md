# History and troubleshooting

[All guides](../README.md) · [Downloads](../../README.md#downloads)

Open View → Logs and file-operation history… or use the corresponding button in Settings → Logs. Use it alongside the active page report to inspect background jobs, diagnostics and transfer outcomes.

| Symptom | What to check |
| --- | --- |
| Empty Projects list | Register a folder, scan, select a project and clear filters. |
| Launch fails | Confirm installed application paths in Settings; read the reported error and the external application window. |
| Copy/Move/Backup is disabled | Read the reason beside the bottom controls, inspect current blocked items, then validate the current selection. |
| Metadata/preview unavailable | Check the file format, host ffprobe/FFmpeg and the reported read error. |
| Pi model discovery fails | Confirm the external harness is configured/authenticated; refresh and read discovery errors. |
| Linux reveal fails | Check host gdbus and a FileManager1-compatible file manager. |
| Schedule does not run | Keep the app open and inspect the schedule description and errors. |

After a failed or interrupted transfer, record the journal/run identifier, per-file errors and all recovery paths. Preserve retained files until you understand the outcome. Do not delete staging or displaced backups merely to clear an error. Re-scan and review current filesystem state before starting another operation.

When reporting a problem, include the platform, package version, affected page, exact steps and error, and whether storage is local or networked. Use redacted screenshots and disposable reproduction data rather than sharing private catalogs or media. Published checks do not guarantee every NAS or power-loss recovery scenario.
