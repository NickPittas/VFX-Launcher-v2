//! VFX Launcher desktop application (Rust/egui migration).
//!
//! `app` owns UI coordination and the approved desktop composition. `jobs`
//! owns worker identity, cooperative control and event delivery. `paths`
//! owns native path identity, containment validation and read-only root
//! inspection; `discovery` owns read-only filesystem scanning with explicit
//! coverage. `storage` owns the single-worker SQLite catalog and `config` owns
//! the versioned JSON settings files. Feature packets attach to these seams
//! (`transfers`, `backup`) instead of adding a second coordination layer.

pub mod app;
pub mod config;
pub mod discovery;
pub mod jobs;
pub mod paths;
pub mod storage;

pub use app::{VfxLauncherApp, run_gui};
