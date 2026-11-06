# Frozen native transfer seam

Coordinator owns `native.rs`, Linux adapter and shared executor extraction.
The Windows packet owns only new `native/windows.rs`; read parent definitions
in the main checkout's `rust/src/transfers/native.rs` if its worktree predates
this seam. No tests or mutation probes; coordinator performs native verification.

Exports from each platform:

```text
Directory: owned directory handle (Linux OwnedFd, Windows File)
AsDirectory: borrowed-handle trait (Linux AsFd, Windows AsHandle)
Root: selected root, implements AsDirectory
Staging::into_file(self) -> File
handle_info(&impl AsDirectory) -> io::Result<EntryInfo>
preserve_metadata(target: &File, source: &Metadata) -> Vec<String>
no_clobber_unavailable(&io::Error) -> bool

Root::open(&OsStr) -> io::Result<Root>
Root::try_clone(&self) -> io::Result<Root>
Root::duplicate(&self) -> io::Result<Directory>
Root::open_dir(&self, relative: &OsStr) -> io::Result<Directory>
Root::open_regular(&self, parent: &impl AsDirectory, name: &OsStr, write: bool)
    -> io::Result<(File, EntryIdentity)>
Root::entry_identity(&self, parent: &impl AsDirectory, name: &OsStr)
    -> io::Result<EntryIdentity>
Root::entry_stat(&self, parent: &impl AsDirectory, name: &OsStr)
    -> io::Result<EntryInfo>
Root::create_staging(&self, parent: &impl AsDirectory, name: &OsStr)
    -> io::Result<Staging>
Root::mkdir(&self, parent: &impl AsDirectory, name: &OsStr)
    -> io::Result<Directory>
Root::publish_noreplace(&self, parent: &impl AsDirectory, file: &File,
    stage: &OsStr, dest: &OsStr, expected: EntryIdentity) -> io::Result<()>
Root::rename_between(&self, from_parent: &impl AsDirectory, from_name: &OsStr,
    to_parent: &impl AsDirectory, to_name: &OsStr, expected: EntryIdentity)
    -> io::Result<()>
Root::remove_owned(&self, parent: &impl AsDirectory, name: &OsStr,
    expected: EntryIdentity) -> io::Result<RemovalOutcome>
Root::sync_dir(&self, dir: &impl AsDirectory) -> io::Result<NamespaceDurability>
```

Windows: user-mode NtCreateFile, relative single-component opens, no reparse
traversal, same-volume descent, exclusive creation, held-file no-replace rename
with NtSetInformationFile(FileRenameInformation). The Win32
SetFileInformationByHandle wrapper rejected a non-null RootDirectory with error
87 during native verification; no path-based fallback is used. Source and destination parents can belong to
different selected roots: entry methods validate against the supplied parent,
not always `self`'s destination volume. No path-based move/copy fallback.

The user explicitly chose standard Windows completion semantics after file
flush and native no-clobber rename. Windows namespace durability returns
Unconfirmed, never Confirmed. The coordinator records that limit in validation,
journal warnings and outcomes; Linux still requires successful directory fsync.
Deletion acceptance is not proof that the name is gone. Prefer supported
handle-based POSIX disposition; return Pending if the name's removal remains
unconfirmed. Do not clear user read-only flags or weaken identity checks.

Use `EntryInfo`/`EntryIdentity` defined in the parent module. Metadata copying
returns human-readable omission warnings, preserves mtime and supported Windows
attributes, and makes no ACL-preservation claim. Keep permissions to the minimum
needed and close owned handles through RAII. All mutable paths remain native.
