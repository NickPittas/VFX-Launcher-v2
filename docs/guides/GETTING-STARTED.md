# Getting started

[All guides](../README.md) · [Downloads](../../README.md#downloads)

Install the package using the [Linux](../LINUX-APPIMAGE.md) or [Windows](../WINDOWS.md) guide. The guides describe the current 0.3.4 source/Linux interface; Windows 0.2.0 has the earlier desktop and may lack later controls and fixes.

Use the top tabs to switch between Projects, Shots & Sequences, Organize Incoming, Copy by File Type and Back Up Projects. Settings is at the right. Drag panel dividers and column boundaries where available; right-click rows for context actions. Logs and file-operation history are under View.

1. Open Settings, configure your application paths, and select the user used for attribution. Save and wait for confirmation.
2. Add an existing project folder in Projects, scan it, and select a project to browse files.
3. For a new folder structure, use New project… in Projects; add later shots through Shots & Sequences.
4. Before a transfer, check both roots, selected files, proposed destinations and the existing-file policy. Validate the current selection, then execute explicitly.
5. Read the final per-file results, including skips, errors and recovery paths. A completed background job does not mean every file succeeded.

Changing a selection or transfer setting requires fresh validation. For first use, choose disposable copies and a local destination. Keep recoverable originals when adopting a new workflow.

The application has its own local data area. Existing Python catalogs are not adopted automatically. Use Settings → Import previous Launcher data to review an import. `--data-dir /absolute/path` selects an isolated data area; see platform instructions for invocation.
