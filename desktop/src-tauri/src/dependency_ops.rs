//! Desktop shim for dependency resolution.
//!
//! Re-exports all types and functions from `agora_core::dependency_ops`. The
//! JAR metadata parser has been deduplicated to `agora_core::jar_metadata`;
//! callers use `agora_core::jar_metadata::parse_jar_metadata` directly.

use agora_core::dependency_ops::JarDeps;

// ---------------------------------------------------------------------------
// 1. Re-export all public types from core
// ---------------------------------------------------------------------------

pub use agora_core::dependency_ops::{
    AliasMap, DepCandidate, DepConflict, DepSource, DependentInfo, DisablePlan,
    IncompatibilityDecl, IncompatibilitySource, InstallPlan, RemovalPlan, Requirement,
    ResolvedInstallDeps,
};

// ---------------------------------------------------------------------------
// 2. Re-export core functions that callers reference
// ---------------------------------------------------------------------------

pub use agora_core::dependency_ops::{
    build_disable_plan_with_aliases, build_install_plan_with_aliases,
    build_removal_plan_with_aliases, detect_source_disagreement, find_dependents,
    find_dependents_with_aliases, resolve_install_deps, resolve_install_deps_with_aliases,
};

// ---------------------------------------------------------------------------
// 3. Desktop-specific wrappers preserving original signatures
// ---------------------------------------------------------------------------

/// Build a disable plan for a target mod.
///
/// Preserves the original signature used by `commands::get_disable_plan`.
pub fn build_disable_plan(
    installed: &[crate::models::InstalledMod],
    target: &crate::models::InstalledMod,
) -> DisablePlan {
    agora_core::dependency_ops::build_disable_plan(installed, target)
}

/// Build a removal plan for a target mod.
///
/// Preserves the original signature used by `commands::get_removal_plan`.
pub fn build_removal_plan(
    installed: &[crate::models::InstalledMod],
    target: &crate::models::InstalledMod,
) -> RemovalPlan {
    agora_core::dependency_ops::build_removal_plan(installed, target)
}

/// Build an install plan for a target mod.
///
/// Delegates directly to `agora_core::dependency_ops::build_install_plan`.
/// Callers pass `agora_core::dependency_ops::JarDeps` (from
/// `agora_core::jar_metadata::parse_jar_metadata`) directly.
pub fn build_install_plan(
    target_manifest_deps: Option<crate::registry::ManifestDeps>,
    target_jar_deps: &JarDeps,
    installed: &[crate::models::InstalledMod],
) -> InstallPlan {
    agora_core::dependency_ops::build_install_plan(target_manifest_deps, target_jar_deps, installed)
}

/// Refresh dependency identity metadata from the physical JARs before a
/// user-initiated install/disable/removal plan.
///
/// `provided_mod_ids` is persisted as a cache for new installs, but manifests
/// created by older Agora versions do not contain it. Correctness therefore
/// comes from re-reading authoritative local JAR metadata at plan time rather
/// than requiring a migration or hardcoded alias table.
pub fn refresh_installed_jar_metadata<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    instance_id: &str,
    installed: &mut [crate::models::InstalledMod],
) -> crate::error::LauncherResult<()> {
    let mods_dir = crate::paths::instance_dir(app, instance_id)
        .map_err(|_| crate::error::LauncherError::InstanceCreateFailed)?
        .join("mods");

    for installed_mod in installed {
        if installed_mod.content_type != "mod" {
            continue;
        }

        let active_path = mods_dir.join(&installed_mod.filename);
        let disabled_path = mods_dir.join(format!("{}.disabled", installed_mod.filename));
        let jar_path = if active_path.is_file() {
            active_path
        } else if disabled_path.is_file() {
            disabled_path
        } else {
            continue;
        };

        let parsed = agora_core::jar_metadata::parse_jar_metadata(&jar_path);
        // A valid mod metadata file has a primary ID. If parsing failed or the
        // file is not a recognized mod JAR, retain the manifest's cached data.
        if parsed.mod_jar_id.is_none() {
            continue;
        }

        installed_mod.java_packages = parsed.java_packages;
        installed_mod.mod_jar_id = parsed.mod_jar_id;
        installed_mod.depends_on = parsed.depends_on;
        installed_mod.optional_deps = parsed.optional_deps;
        installed_mod.incompatible_deps = parsed.incompatible_deps;
        installed_mod.provided_mod_ids = parsed
            .provided_mods
            .into_iter()
            .map(|provided| provided.mod_id)
            .collect();
    }

    Ok(())
}
