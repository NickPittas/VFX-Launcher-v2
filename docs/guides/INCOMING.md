# Organize Incoming

[All guides](../README.md) · [Downloads](../../README.md#downloads)

1. Choose Source and Destination with Browse…. Select a Folder layout and inspect Rules… before scanning unfamiliar material.
2. Click Scan folders. Review grouped sequences, frame counts, missing frames, tags and each proposed destination. Use the source-folder filter and selection checkboxes to limit the batch.
3. Select a row to edit its tags or destination. Use Refresh preview after changing rules or layout. Resolve ambiguous matches rather than accepting an unintended folder.
4. Inspect the destination tree and Existing files policy. Choose Copy or Move deliberately; Move can remove source files after successful publication.
5. Click Validate selection. Correct the reported blocked items and validate again after changes.
6. Click Copy selected or Move selected. Follow progress and read the per-file report, including recovery paths.

In 0.3.4, newer ARRI clip names use the configured ARRI rule. Dot-prefixed keywords such as `.cube` match extensions case-insensitively; competing destinations remain ambiguous. Resolution reads use native headers where supported and host ffprobe for other supported formats. One representative frame is read per sequence. Explicit tag edits take precedence; metadata read failures are visible.

The media preview supports image/sequence inspection and frame navigation, zoom and pan. Available Open in context actions use the configured DJV, MRV2 or Nuke Player executable. Send to Nuke requires a running localhost NukeServerSocket session on port 49512. External formats may require FFmpeg/ffprobe. These integrations retain the runtime limits recorded in the release notes.

Suggest folders… prepares local context and discovers models through the configured Pi harness. Refresh the model list if necessary and inspect errors. Review the context before Send to Pi: only that explicit action submits it to the selected provider. Applying suggestions changes the draft; inspect destinations and validate again before copying. An unavailable explicit model does not silently fall back.
