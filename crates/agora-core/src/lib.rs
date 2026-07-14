//! Agora launcher core — shared business logic consumed by the Tauri GUI,
//! the standalone `agora` CLI, and the in-process MCP listener.
//!
//! Constraint (plan C2/C3): this crate MUST NOT depend on `tauri`, `clap`,
//! or any MCP-protocol crate. Every operation takes a `&Ctx` (introduced
//! later). For now this crate only hosts the pure data/error modules moved
//! out of the desktop crate in Phase 1A.

pub mod ai_assistant;
pub mod auth;
pub mod browse_cache;
pub mod catalog;
pub mod clone;
pub mod crash_diagnostics;
pub mod ctx;
pub mod db;
pub mod dependency_ops;
pub mod download;
pub mod error;
pub mod gc;
pub mod github_ratelimit;
pub mod governance;
pub mod health;
pub mod import;
pub mod install_pipeline;
pub mod installed_artifact;
pub mod installed_profile;
pub mod jar_metadata;
pub mod java;
/// Metadata types and Forge/NeoForge install-profile helpers.
///
/// # Deprecation
/// This module retains **only** reusable type definitions and Forge helpers.
/// The legacy direct-launch orchestration (`fetch_version_manifest`,
/// `build_launch_command`, `spawn_java`, `prepare_loader`, etc.) has been
/// removed. Use [`crate::launch_planner`] for all production Java launches.
pub mod launch;
pub mod launch_planner;
pub mod launcher_profiles;
pub mod lkg;
pub mod loader_manifests;
pub mod loadout;
pub mod lockfile;
pub mod log_sanitizer;
pub mod mod_cache;
pub mod models;
pub mod modrinth;
pub mod msa;
pub mod network;
pub mod override_sanitizer;
pub mod pack_install;
pub mod paths;
pub mod process_identity;
pub mod registry;
pub mod registry_sync;
pub mod runtime_catalog;
pub mod runtime_manager;
pub mod server_export;
pub mod snapshot;
pub mod state;
pub mod version_match;
