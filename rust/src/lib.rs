//! VFX Launcher desktop application (Rust/egui migration).
//!
//! `app` owns UI coordination and the approved desktop composition. `jobs`
//! owns worker identity, cooperative control and event delivery. `paths`
//! owns native path identity, containment validation and read-only root
//! inspection; `discovery` owns read-only filesystem scanning with explicit
//! coverage. Feature packets attach to these seams (`storage`, `transfers`,
//! `backup`) instead of adding a second coordination layer.

pub mod app;
pub mod discovery;
pub mod jobs;
pub mod paths;

pub use app::{VfxLauncherApp, run_gui};
