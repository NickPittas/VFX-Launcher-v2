//! Native transfer filesystem operations. Linux is currently the only supported backend.

#[cfg(target_os = "linux")]
#[path = "native/linux.rs"]
mod platform;

#[cfg(not(target_os = "linux"))]
mod platform {
    use std::io;
    pub(super) fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "native transfer backend is not implemented on this platform",
        )
    }
}

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
pub(crate) use platform::{EntryIdentity, Root, Staging};
