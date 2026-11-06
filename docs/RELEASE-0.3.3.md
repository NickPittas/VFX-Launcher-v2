# VFX Launcher 0.3.3

- Reveal the selected Projects file using the Linux desktop file-manager
  selection interface, preserving spaces, Unicode and special characters.
  Report selection-request failures instead of treating process creation as
  proof that the file manager opened. Requires host gdbus and a file manager
  implementing org.freedesktop.FileManager1.
- Retain validation errors in Copy by File Type and Backup so failed validation
  shows the actual refusal beside execution controls. Show current blocked-item
  details above historical results and label previous-run advisories explicitly.
- Failed revalidation clears any older approval. Per-file validation and
  transfer safety checks remain enforced.

The Nuke launch command is unchanged. Compile and headless argument/URI checks
passed; native file-manager selection and validation UI remain unverified.
The transfer and runtime limitations documented in 0.3.1 continue to apply.
