//! VFX Launcher desktop application (Rust/egui migration).
//!
//! `app` owns UI coordination and the approved desktop composition.
//! `project_workspace` owns the Projects view's asynchronous catalog state.
//! `jobs` owns worker identity, cooperative control and event delivery. `paths`
//! owns native path identity, containment validation and read-only root
//! inspection; `discovery` owns read-only filesystem scanning with explicit
//! coverage. `storage` owns the single-worker SQLite catalog and `config` owns
//! the versioned JSON settings files. `projects` owns catalog registration, scan
//! reconciliation and the project/version views read through that worker.
//! `launch` owns the native argv for opening a project file and revealing it.
//! Feature packets attach to these seams
//! (`transfers`, `backup`) instead of adding a second coordination layer.

pub mod app;
pub mod config;
pub mod discovery;
pub mod jobs;
pub mod launch;
pub mod paths;
pub mod project_workspace;
pub mod projects;
pub mod storage;
pub mod transfers;

pub use app::{VfxLauncherApp, run_gui};
