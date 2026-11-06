//! Native transfer filesystem operations. Linux is currently the only supported backend.

#[cfg(target_os = "linux")]
#[path = "native/linux.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
pub(crate) use platform::{EntryIdentity, Root, Staging};
