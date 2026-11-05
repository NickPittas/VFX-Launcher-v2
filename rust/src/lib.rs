//! VFX Launcher desktop application (Rust/egui migration).
//!
//! `app` owns UI coordination and the approved desktop composition. `jobs`
//! owns worker identity, cooperative control and event delivery. Feature
//! packets attach to these seams (`paths`, `discovery`, `storage`, `transfers`,
//! `backup`) instead of adding a second coordination layer.

pub mod app;
pub mod jobs;

pub use app::{VfxLauncherApp, run_gui};
