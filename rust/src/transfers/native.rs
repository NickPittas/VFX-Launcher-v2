//! Native transfer filesystem operations beneath held directory handles.

use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntryKind {
    Regular,
    Directory,
    Other,
}

/// Unix device/inode, or Windows volume serial/file index; kind is not permissions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntryIdentity {
    pub device: u64,
    pub inode: u64,
    pub kind: EntryKind,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct EntryInfo {
    pub identity: EntryIdentity,
    pub size: u64,
    pub modified_ns: Option<i64>,
}

impl From<EntryInfo> for EntryIdentity {
    fn from(info: EntryInfo) -> Self {
        info.identity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NamespaceDurability {
    #[cfg_attr(windows, allow(dead_code))]
    Confirmed,
    Unconfirmed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RemovalOutcome {
    Removed,
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Pending,
}

pub(crate) fn require_same_entry(left: EntryIdentity, right: EntryIdentity) -> io::Result<()> {
    if left == right {
        Ok(())
    } else {
        Err(io::Error::other("entry identity changed"))
    }
}

#[cfg(target_os = "linux")]
#[path = "native/linux.rs"]
mod platform;

#[cfg(windows)]
#[path = "native/windows.rs"]
mod platform;

#[cfg(any(target_os = "linux", windows))]
pub(crate) use platform::{
    AsDirectory, Directory, Root, handle_info, no_clobber_unavailable, preserve_metadata,
};
