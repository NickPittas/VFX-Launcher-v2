//! X1b: the transfer executor.
//!
//! Linux is currently the only supported transfer backend; the other-platform
//! module keeps the public API and refuses execution.

#[cfg(target_os = "linux")]
#[path = "executor/linux.rs"]
mod platform;

#[cfg(not(target_os = "linux"))]
#[path = "executor/unsupported.rs"]
mod platform;

pub use platform::{ItemOutcome, RunReport, execute};
