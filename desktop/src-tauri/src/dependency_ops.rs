use crate::crash_investigator::JarMetadata;
use crate::models::InstalledMod;
use crate::registry::ManifestDeps;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// 1. Types
// ---------------------------------------------------------------------------

/// Whether a dependency is required or optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Requirement {
    Required,
    Optional,
}

/// Where a dependency declaration came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DepSource {
    Jar,
    Manifest,
}

/// A mod that depends on a target mod.
#[derive(Debug, Clone, Serialize)]
pub struct DependentInfo {
    /// The mod_id of the dependent (registry_id or filename).
    pub mod_id: String,
    pub filename: String,
    pub requirement: Requirement,
    pub source: DepSource,
}

/// A candidate dependency for installation.
#[derive(Debug, Clone, Serialize)]
pub struct DepCandidate {
    /// The dep's jar id (what the parent declared it depends on).
    pub mod_jar_id: String,
    pub requirement: Requirement,
    pub source: DepSource,
}

/// A disagreement between a jar's declared deps and a manifest's declared deps.
///
/// Incompatible entries are intentionally excluded from this comparison —
/// they do not participate in the install/remove flow for v1.
#[derive(Debug, Clone, Serialize)]
pub struct DepConflict {
    pub mod_jar_id: String,
    pub jar_requirement: Option<Requirement>,
    pub manifest_requirement: Option<Requirement>,
}

/// The resolved dependency state for installing a target mod.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedInstallDeps {
    pub missing_required: Vec<DepCandidate>,
    pub missing_optional: Vec<DepCandidate>,
    pub conflicts: Vec<DepConflict>,
}

/// A plan for removing a mod (which other mods would break).
#[derive(Debug, Clone, Serialize)]
pub struct RemovalPlan {
    pub dependents: Vec<DependentInfo>,
}

/// A plan for disabling a mod (which other mods would be affected).
#[derive(Debug, Clone, Serialize)]
pub struct DisablePlan {
    pub dependents: Vec<DependentInfo>,
}

/// A plan for installing a mod's missing dependencies.
///
/// Type alias for `ResolvedInstallDeps` — same fields, different name for
/// the command-layer API.
pub type InstallPlan = ResolvedInstallDeps;

// ---------------------------------------------------------------------------
// 2. Alias normalization
// ---------------------------------------------------------------------------

/// In-memory alias map: lowercase alias → canonical registry_id.
///
/// Built from `(registry_id, alias)` tuples returned by
/// `registry::get_all_mod_aliases()`. Each alias is lowercased and mapped
/// to its canonical registry_id. The canonical id itself (lowercased) is
/// also inserted so the canonical id is always resolvable.
pub struct AliasMap {
    map: std::collections::HashMap<String, String>,
}

impl AliasMap {
    /// Build an alias map from `(registry_id, alias)` tuples.
    pub fn from_pairs(pairs: &[(String, String)]) -> Self {
        let mut map = std::collections::HashMap::new();
        for (registry_id, alias) in pairs {
            let canonical = registry_id.to_lowercase();
            map.insert(alias.to_lowercase(), canonical.clone());
            // Also map the canonical id itself so it is always resolvable.
            map.insert(canonical.clone(), canonical);
        }
        Self { map }
    }

    /// Look up `id` in the alias map. Returns the canonical registry_id
    /// if found, `None` otherwise.
    pub fn resolve(&self, id: &str) -> Option<String> {
        self.map.get(&id.to_lowercase()).cloned()
    }

    /// Resolve `id` to its canonical form, or return the raw id unchanged
    /// if no alias mapping exists.
    pub fn resolve_or_self(&self, id: &str) -> String {
        self.resolve(id).unwrap_or_else(|| id.to_string())
    }

    /// True when the map has no entries (no aliases curated).
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 3. Helpers
// ---------------------------------------------------------------------------

/// Collect every `mod_jar_id` whose value is `Some` into a `HashSet`.
///
/// All IDs are lowercased for case-insensitive matching. Mod IDs are
/// conventionally lowercase, but this normalises defensively against
/// mixed-case entries from different sources (jar metadata vs manifest).
fn installed_jar_ids(installed: &[InstalledMod]) -> HashSet<String> {
    installed
        .iter()
        .filter_map(|m| m.mod_jar_id.as_ref().map(|id| id.to_lowercase()))
        .collect()
}

/// Build a jar-dep classification map: mod_jar_id → Requirement.
///
/// Only includes `depends_on` (Required) and `optional_deps` (Optional).
/// Incompatible deps are skipped per the v1 design decision — they are
/// curatorial notes, not install-time constraints.
fn jar_dep_map(jar: &JarMetadata) -> HashMap<String, Requirement> {
    let mut map = HashMap::new();
    for id in &jar.depends_on {
        map.insert(id.to_lowercase(), Requirement::Required);
    }
    for id in &jar.optional_deps {
        map.entry(id.to_lowercase())
            .or_insert(Requirement::Optional);
    }
    map
}

/// Build a manifest-dep classification map: mod_jar_id → Requirement.
fn manifest_dep_map(manifest: &ManifestDeps) -> HashMap<String, Requirement> {
    let mut map = HashMap::new();
    for id in &manifest.required {
        map.insert(id.to_lowercase(), Requirement::Required);
    }
    for id in &manifest.optional {
        map.entry(id.to_lowercase())
            .or_insert(Requirement::Optional);
    }
    map
}

// ---------------------------------------------------------------------------
// 4. Alias-aware public functions
// ---------------------------------------------------------------------------

/// Alias-aware `find_dependents`: normalize both the target and each
/// installed mod's declared deps via `aliases` before matching.
pub fn find_dependents_with_aliases(
    installed: &[InstalledMod],
    target_mod_jar_id: &str,
    aliases: &AliasMap,
) -> Vec<DependentInfo> {
    let target_resolved = aliases.resolve_or_self(target_mod_jar_id);
    let target_lower = target_resolved.to_lowercase();
    let mut seen: HashMap<String, DependentInfo> = HashMap::new();

    for m in installed {
        let mod_id = m
            .registry_id
            .clone()
            .unwrap_or_else(|| m.filename.clone());

        // Check required deps
        if m.depends_on.iter().any(|d| aliases.resolve_or_self(d).to_lowercase() == target_lower) {
            let entry = DependentInfo {
                mod_id: mod_id.clone(),
                filename: m.filename.clone(),
                requirement: Requirement::Required,
                source: DepSource::Jar,
            };
            seen.insert(mod_id.clone(), entry);
            continue;
        }

        // Check optional deps
        if m.optional_deps.iter().any(|d| aliases.resolve_or_self(d).to_lowercase() == target_lower) {
            seen.entry(mod_id.clone()).or_insert(DependentInfo {
                mod_id,
                filename: m.filename.clone(),
                requirement: Requirement::Optional,
                source: DepSource::Jar,
            });
        }
    }

    let mut result: Vec<DependentInfo> = seen.into_values().collect();
    result.sort_by(|a, b| a.mod_id.cmp(&b.mod_id));
    result
}

/// Alias-aware `resolve_install_deps`: normalize all mod_jar_ids on both
/// sides (candidate deps and installed ids) via `aliases` before comparing.
pub fn resolve_install_deps_with_aliases(
    target_manifest_deps: Option<ManifestDeps>,
    target_jar_deps: &JarMetadata,
    installed: &[InstalledMod],
    aliases: &AliasMap,
) -> ResolvedInstallDeps {
    // Build installed set with alias resolution.
    let installed_ids: HashSet<String> = installed
        .iter()
        .filter_map(|m| m.mod_jar_id.as_ref().map(|id| aliases.resolve_or_self(id).to_lowercase()))
        .collect();

    // Build jar-dep candidates with alias-resolved ids.
    let mut jar_required: Vec<DepCandidate> = Vec::new();
    for id in &target_jar_deps.depends_on {
        let resolved = aliases.resolve_or_self(id);
        jar_required.push(DepCandidate {
            mod_jar_id: resolved,
            requirement: Requirement::Required,
            source: DepSource::Jar,
        });
    }
    let mut jar_optional: Vec<DepCandidate> = Vec::new();
    for id in &target_jar_deps.optional_deps {
        let resolved = aliases.resolve_or_self(id);
        jar_optional.push(DepCandidate {
            mod_jar_id: resolved,
            requirement: Requirement::Optional,
            source: DepSource::Jar,
        });
    }

    // Build manifest-dep candidates with alias-resolved ids.
    let mut manifest_required: Vec<DepCandidate> = Vec::new();
    let mut manifest_optional: Vec<DepCandidate> = Vec::new();
    if let Some(ref m) = target_manifest_deps {
        for id in &m.required {
            let resolved = aliases.resolve_or_self(id);
            manifest_required.push(DepCandidate {
                mod_jar_id: resolved,
                requirement: Requirement::Required,
                source: DepSource::Manifest,
            });
        }
        for id in &m.optional {
            let resolved = aliases.resolve_or_self(id);
            manifest_optional.push(DepCandidate {
                mod_jar_id: resolved,
                requirement: Requirement::Optional,
                source: DepSource::Manifest,
            });
        }
    }

    // Helper: filter out already-installed candidates.
    let filter_installed = |cands: Vec<DepCandidate>| -> Vec<DepCandidate> {
        cands
            .into_iter()
            .filter(|c| !installed_ids.contains(&c.mod_jar_id.to_lowercase()))
            .collect()
    };

    let jar_required = filter_installed(jar_required);
    let jar_optional = filter_installed(jar_optional);
    let manifest_required = filter_installed(manifest_required);
    let manifest_optional = filter_installed(manifest_optional);

    // Build lookup maps for deduplication: mod_jar_id (lowercased) → candidate.
    let jar_required_map: HashMap<String, DepCandidate> = jar_required
        .iter()
        .map(|c| (c.mod_jar_id.to_lowercase(), c.clone()))
        .collect();
    let manifest_required_map: HashMap<String, DepCandidate> = manifest_required
        .iter()
        .map(|c| (c.mod_jar_id.to_lowercase(), c.clone()))
        .collect();
    let jar_optional_map: HashMap<String, DepCandidate> = jar_optional
        .iter()
        .map(|c| (c.mod_jar_id.to_lowercase(), c.clone()))
        .collect();
    let manifest_optional_map: HashMap<String, DepCandidate> = manifest_optional
        .iter()
        .map(|c| (c.mod_jar_id.to_lowercase(), c.clone()))
        .collect();

    // Collect all unique mod_jar_ids from all candidate lists.
    let mut all_ids: HashSet<String> = HashSet::new();
    for k in jar_required_map.keys() {
        all_ids.insert(k.clone());
    }
    for k in manifest_required_map.keys() {
        all_ids.insert(k.clone());
    }
    for k in jar_optional_map.keys() {
        all_ids.insert(k.clone());
    }
    for k in manifest_optional_map.keys() {
        all_ids.insert(k.clone());
    }

    let mut missing_required: Vec<DepCandidate> = Vec::new();
    let mut missing_optional: Vec<DepCandidate> = Vec::new();
    let mut conflict_ids: HashSet<String> = HashSet::new();

    for id in &all_ids {
        let jar_r = jar_required_map.get(id);
        let jar_o = jar_optional_map.get(id);
        let man_r = manifest_required_map.get(id);
        let man_o = manifest_optional_map.get(id);

        let jar_effective = jar_r.or(jar_o);
        let man_effective = man_r.or(man_o);

        match (jar_effective, man_effective) {
            (Some(jr), Some(mr)) if jr.requirement == Requirement::Required
                && mr.requirement == Requirement::Required =>
            {
                missing_required.push(jr.clone());
            }
            (Some(jo), Some(mo)) if jo.requirement == Requirement::Optional
                && mo.requirement == Requirement::Optional =>
            {
                missing_optional.push(jo.clone());
            }
            (Some(j), Some(m)) if j.requirement == m.requirement => {
                if j.requirement == Requirement::Required {
                    missing_required.push(j.clone());
                } else {
                    missing_optional.push(j.clone());
                }
            }
            (Some(_), Some(_)) => {
                conflict_ids.insert(id.clone());
            }
            (Some(j), None) => {
                if j.requirement == Requirement::Required {
                    missing_required.push(j.clone());
                } else {
                    missing_optional.push(j.clone());
                }
            }
            (None, Some(m)) => {
                if m.requirement == Requirement::Required {
                    missing_required.push(DepCandidate {
                        mod_jar_id: m.mod_jar_id.clone(),
                        requirement: Requirement::Required,
                        source: DepSource::Manifest,
                    });
                } else {
                    missing_optional.push(DepCandidate {
                        mod_jar_id: m.mod_jar_id.clone(),
                        requirement: Requirement::Optional,
                        source: DepSource::Manifest,
                    });
                }
            }
            (None, None) => {}
        }
    }

    let mut conflicts: Vec<DepConflict> = Vec::new();
    if !conflict_ids.is_empty() {
        let all_conflicts = detect_source_disagreement(
            target_jar_deps,
            target_manifest_deps.as_ref(),
        );
        for c in all_conflicts {
            if conflict_ids.contains(&c.mod_jar_id) {
                conflicts.push(c);
            }
        }
    }

    conflicts.sort_by(|a, b| a.mod_jar_id.cmp(&b.mod_jar_id));
    missing_required.sort_by(|a, b| a.mod_jar_id.cmp(&b.mod_jar_id));
    missing_optional.sort_by(|a, b| a.mod_jar_id.cmp(&b.mod_jar_id));

    ResolvedInstallDeps {
        missing_required,
        missing_optional,
        conflicts,
    }
}

/// Alias-aware `build_removal_plan`.
pub fn build_removal_plan_with_aliases(
    installed: &[InstalledMod],
    target: &InstalledMod,
    aliases: &AliasMap,
) -> RemovalPlan {
    let dependents = match &target.mod_jar_id {
        Some(jar_id) => find_dependents_with_aliases(installed, jar_id, aliases),
        None => Vec::new(),
    };
    RemovalPlan { dependents }
}

/// Alias-aware `build_disable_plan`.
pub fn build_disable_plan_with_aliases(
    installed: &[InstalledMod],
    target: &InstalledMod,
    aliases: &AliasMap,
) -> DisablePlan {
    let dependents = match &target.mod_jar_id {
        Some(jar_id) => find_dependents_with_aliases(installed, jar_id, aliases),
        None => Vec::new(),
    };
    DisablePlan { dependents }
}

/// Alias-aware `build_install_plan`.
pub fn build_install_plan_with_aliases(
    target_manifest_deps: Option<ManifestDeps>,
    target_jar_deps: &JarMetadata,
    installed: &[InstalledMod],
    aliases: &AliasMap,
) -> InstallPlan {
    resolve_install_deps_with_aliases(target_manifest_deps, target_jar_deps, installed, aliases)
}

// ---------------------------------------------------------------------------
// 5. Public functions (delegating to alias-aware versions with empty map)
// ---------------------------------------------------------------------------

/// Find all installed mods that depend on `target_mod_jar_id`.
///
/// Delegates to `find_dependents_with_aliases` with an empty `AliasMap`,
/// preserving exact existing behavior (no alias resolution → identity).
pub fn find_dependents(
    installed: &[InstalledMod],
    target_mod_jar_id: &str,
) -> Vec<DependentInfo> {
    find_dependents_with_aliases(installed, target_mod_jar_id, &AliasMap::from_pairs(&[]))
}

/// Detect disagreements between a jar's declared deps and a manifest's deps.
///
/// For each dep mod_id present in either the jar or manifest (across required
/// and optional buckets only — incompatibles are excluded), compares the
/// classification. If jar says Required and manifest says Optional (or absent),
/// that's a conflict.
///
/// Incompatible deps are intentionally skipped — they are curatorial notes
/// that don't participate in the install/remove flow for v1.
pub fn detect_source_disagreement(
    jar: &JarMetadata,
    manifest: Option<&ManifestDeps>,
) -> Vec<DepConflict> {
    let jar_map = jar_dep_map(jar);
    let manifest_map = match manifest {
        Some(m) => manifest_dep_map(m),
        None => HashMap::new(),
    };

    // Collect all unique dep ids from both maps.
    let mut all_ids: HashSet<String> = HashSet::new();
    for k in jar_map.keys() {
        all_ids.insert(k.clone());
    }
    for k in manifest_map.keys() {
        all_ids.insert(k.clone());
    }

    let mut conflicts: Vec<DepConflict> = Vec::new();

    for id in &all_ids {
        let jar_req = jar_map.get(id);
        let manifest_req = manifest_map.get(id);

        match (jar_req, manifest_req) {
            // Both present but different classifications.
            (Some(jr), Some(mr)) if jr != mr => {
                conflicts.push(DepConflict {
                    mod_jar_id: id.clone(),
                    jar_requirement: Some(*jr),
                    manifest_requirement: Some(*mr),
                });
            }
            // Present in jar but absent from manifest.
            (Some(_), None) => {
                conflicts.push(DepConflict {
                    mod_jar_id: id.clone(),
                    jar_requirement: jar_req.copied(),
                    manifest_requirement: None,
                });
            }
            // Present in manifest but absent from jar.
            (None, Some(_)) => {
                conflicts.push(DepConflict {
                    mod_jar_id: id.clone(),
                    jar_requirement: None,
                    manifest_requirement: manifest_req.copied(),
                });
            }
            // Both absent — shouldn't happen since we iterate all_ids.
            (None, None) => {}
            // Both present and same classification — already handled above by the guard.
            (Some(_), Some(_)) => {}
        }
    }

    conflicts.sort_by(|a, b| a.mod_jar_id.cmp(&b.mod_jar_id));
    conflicts
}

/// Resolve what dependencies a target mod needs that aren't already installed.
///
/// Delegates to `resolve_install_deps_with_aliases` with an empty `AliasMap`,
/// preserving exact existing behavior (no alias resolution → identity).
pub fn resolve_install_deps(
    target_manifest_deps: Option<ManifestDeps>,
    target_jar_deps: &JarMetadata,
    installed: &[InstalledMod],
) -> ResolvedInstallDeps {
    resolve_install_deps_with_aliases(target_manifest_deps, target_jar_deps, installed, &AliasMap::from_pairs(&[]))
}

/// Build a removal plan for a target mod.
///
/// Delegates to `build_removal_plan_with_aliases` with an empty `AliasMap`.
pub fn build_removal_plan(installed: &[InstalledMod], target: &InstalledMod) -> RemovalPlan {
    build_removal_plan_with_aliases(installed, target, &AliasMap::from_pairs(&[]))
}

/// Build a disable plan for a target mod.
///
/// Delegates to `build_disable_plan_with_aliases` with an empty `AliasMap`.
pub fn build_disable_plan(installed: &[InstalledMod], target: &InstalledMod) -> DisablePlan {
    build_disable_plan_with_aliases(installed, target, &AliasMap::from_pairs(&[]))
}

/// Build an install plan for a target mod.
///
/// Delegates to `build_install_plan_with_aliases` with an empty `AliasMap`.
pub fn build_install_plan(
    target_manifest_deps: Option<ManifestDeps>,
    target_jar_deps: &JarMetadata,
    installed: &[InstalledMod],
) -> InstallPlan {
    build_install_plan_with_aliases(target_manifest_deps, target_jar_deps, installed, &AliasMap::from_pairs(&[]))
}

// ---------------------------------------------------------------------------
// 4. Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InstalledMod;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Construct a minimal `InstalledMod` for tests.
    ///
    /// * `filename` — the jar filename.
    /// * `jar_id` — Some("id") or None for `mod_jar_id`.
    /// * `deps` — required deps (`depends_on`), passed as jar IDs.
    /// * `opt_deps` — optional deps (`optional_deps`), passed as jar IDs.
    fn installed(
        filename: &str,
        jar_id: Option<&str>,
        deps: &[&str],
        opt_deps: &[&str],
    ) -> InstalledMod {
        InstalledMod {
            filename: filename.to_string(),
            registry_id: Some(filename.to_string()),
            modrinth_id: None,
            source: String::new(),
            version: None,
            sha256: String::new(),
            installed_at: String::new(),
            java_packages: Vec::new(),
            mod_jar_id: jar_id.map(String::from),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            optional_deps: opt_deps.iter().map(|s| s.to_string()).collect(),
            incompatible_deps: Vec::new(),
        }
    }

    /// Construct a minimal `JarMetadata` for tests.
    fn jar(
        mod_jar_id: Option<&str>,
        deps: &[&str],
        opt_deps: &[&str],
        incompatible: &[&str],
    ) -> JarMetadata {
        JarMetadata {
            java_packages: Vec::new(),
            mod_jar_id: mod_jar_id.map(String::from),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            optional_deps: opt_deps.iter().map(|s| s.to_string()).collect(),
            incompatible_deps: incompatible.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Construct a minimal `ManifestDeps` for tests.
    fn manifest_deps(required: &[&str], optional: &[&str], incompatible: &[&str]) -> ManifestDeps {
        ManifestDeps {
            required: required.iter().map(|s| s.to_string()).collect(),
            optional: optional.iter().map(|s| s.to_string()).collect(),
            incompatible: incompatible.iter().map(|s| s.to_string()).collect(),
        }
    }

    // -----------------------------------------------------------------------
    // A. find_dependents
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_dependents_required() {
        let a = installed("a.jar", Some("a"), &[], &[]);
        let b = installed("b.jar", Some("b"), &["a"], &[]);
        let installed = vec![a, b];

        let result = find_dependents(&installed, "a");

        assert_eq!(result.len(), 1);
        let dep = &result[0];
        assert_eq!(dep.mod_id, "b.jar");
        assert_eq!(dep.requirement, Requirement::Required);
        assert_eq!(dep.source, DepSource::Jar);
    }

    #[test]
    fn test_find_dependents_optional() {
        let a = installed("a.jar", Some("a"), &[], &[]);
        let b = installed("b.jar", Some("b"), &[], &["a"]);
        let installed = vec![a, b];

        let result = find_dependents(&installed, "a");

        assert_eq!(result.len(), 1);
        let dep = &result[0];
        assert_eq!(dep.mod_id, "b.jar");
        assert_eq!(dep.requirement, Requirement::Optional);
        assert_eq!(dep.source, DepSource::Jar);
    }

    #[test]
    fn test_find_dependents_mixed_required_and_optional() {
        let a = installed("a.jar", Some("a"), &[], &[]);
        let b = installed("b.jar", Some("b"), &["a"], &[]);
        let c = installed("c.jar", Some("c"), &[], &["a"]);
        let installed = vec![a, b, c];

        let result = find_dependents(&installed, "a");

        assert_eq!(result.len(), 2);
        // Sorted by mod_id: "b.jar" < "c.jar"
        assert_eq!(result[0].mod_id, "b.jar");
        assert_eq!(result[0].requirement, Requirement::Required);
        assert_eq!(result[1].mod_id, "c.jar");
        assert_eq!(result[1].requirement, Requirement::Optional);
    }

    #[test]
    fn test_find_dependents_dedup_required_wins() {
        let a = installed("a.jar", Some("a"), &[], &[]);
        let b = installed("b.jar", Some("b"), &["a"], &["a"]);
        let installed = vec![a, b];

        let result = find_dependents(&installed, "a");

        assert_eq!(result.len(), 1);
        let dep = &result[0];
        assert_eq!(dep.mod_id, "b.jar");
        assert_eq!(dep.requirement, Requirement::Required);
        assert_eq!(dep.source, DepSource::Jar);
    }

    #[test]
    fn test_find_dependents_no_match() {
        let a = installed("a.jar", Some("a"), &[], &[]);
        let b = installed("b.jar", Some("b"), &["c"], &[]);
        let installed = vec![a, b];

        let result = find_dependents(&installed, "a");

        assert!(result.is_empty());
    }

    #[test]
    fn test_find_dependents_case_insensitive() {
        let a = installed("Sodium.jar", Some("Sodium"), &[], &[]);
        let b = installed("WTHIT.jar", Some("WTHIT"), &["sodium"], &[]);
        let installed = vec![a, b];

        let result = find_dependents(&installed, "sodium");

        assert_eq!(result.len(), 1);
        let dep = &result[0];
        assert_eq!(dep.mod_id, "WTHIT.jar");
        assert_eq!(dep.requirement, Requirement::Required);
        assert_eq!(dep.source, DepSource::Jar);
    }

    // -----------------------------------------------------------------------
    // B. detect_source_disagreement
    // -----------------------------------------------------------------------

    #[test]
    fn test_disagreement_jar_required_manifest_optional() {
        let jar_meta = jar(Some("target"), &["x"], &[], &[]);
        let manifest = manifest_deps(&["y"], &["x"], &[]);

        let conflicts = detect_source_disagreement(&jar_meta, Some(&manifest));

        // Two conflicts: "x" (jar=Required vs manifest=Optional) and
        // "y" (manifest=Required vs jar=absent). Both are disagreements.
        assert_eq!(conflicts.len(), 2);
        let x_conflict = conflicts.iter().find(|c| c.mod_jar_id == "x").expect("missing conflict for x");
        let y_conflict = conflicts.iter().find(|c| c.mod_jar_id == "y").expect("missing conflict for y");
        assert_eq!(x_conflict.jar_requirement, Some(Requirement::Required));
        assert_eq!(x_conflict.manifest_requirement, Some(Requirement::Optional));
        assert_eq!(y_conflict.jar_requirement, None);
        assert_eq!(y_conflict.manifest_requirement, Some(Requirement::Required));
    }

    #[test]
    fn test_disagreement_jar_only() {
        let jar_meta = jar(Some("target"), &["x"], &[], &[]);

        // When manifest is absent entirely, jar deps surface as
        // "unconfirmed-by-manifest" conflicts.
        let conflicts = detect_source_disagreement(&jar_meta, None);

        assert_eq!(conflicts.len(), 1);
        let c = &conflicts[0];
        assert_eq!(c.mod_jar_id, "x");
        assert_eq!(c.jar_requirement, Some(Requirement::Required));
        assert_eq!(c.manifest_requirement, None);
    }

    #[test]
    fn test_disagreement_no_conflict_same_classification() {
        let jar_meta = jar(Some("target"), &["x"], &[], &[]);
        let manifest = manifest_deps(&["x"], &[], &[]);

        let conflicts = detect_source_disagreement(&jar_meta, Some(&manifest));

        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_disagreement_present_in_jar_absent_in_manifest() {
        let jar_meta = jar(Some("target"), &["x"], &[], &[]);
        let manifest = manifest_deps(&[], &[], &[]);

        let conflicts = detect_source_disagreement(&jar_meta, Some(&manifest));

        assert_eq!(conflicts.len(), 1);
        let c = &conflicts[0];
        assert_eq!(c.mod_jar_id, "x");
        assert_eq!(c.jar_requirement, Some(Requirement::Required));
        assert_eq!(c.manifest_requirement, None);
    }

    // -----------------------------------------------------------------------
    // C. resolve_install_deps / build_install_plan
    // -----------------------------------------------------------------------

    #[test]
    fn test_install_plan_missing_required() {
        let jar_meta = jar(Some("target"), &["missing"], &[], &[]);
        let installed: Vec<InstalledMod> = vec![];

        let plan = resolve_install_deps(None, &jar_meta, &installed);

        assert_eq!(plan.missing_required.len(), 1);
        assert_eq!(plan.missing_required[0].mod_jar_id, "missing");
        assert_eq!(plan.missing_required[0].requirement, Requirement::Required);
        assert_eq!(plan.missing_required[0].source, DepSource::Jar);
        assert!(plan.missing_optional.is_empty());
    }

    #[test]
    fn test_install_plan_already_installed_skipped() {
        let m = installed("m.jar", Some("m"), &[], &[]);
        let installed = vec![m];
        let jar_meta = jar(Some("target"), &["m"], &[], &[]);

        let plan = resolve_install_deps(None, &jar_meta, &installed);

        assert!(plan.missing_required.is_empty());
    }

    #[test]
    fn test_install_plan_mixed_missing_required_optional() {
        let jar_meta = jar(Some("target"), &["req"], &["opt"], &[]);
        let installed: Vec<InstalledMod> = vec![];

        let plan = resolve_install_deps(None, &jar_meta, &installed);

        assert_eq!(plan.missing_required.len(), 1);
        assert_eq!(plan.missing_required[0].mod_jar_id, "req");
        assert_eq!(plan.missing_required[0].source, DepSource::Jar);

        assert_eq!(plan.missing_optional.len(), 1);
        assert_eq!(plan.missing_optional[0].mod_jar_id, "opt");
        assert_eq!(plan.missing_optional[0].source, DepSource::Jar);
    }

    #[test]
    fn test_install_plan_manifest_and_jar_dedup_jar_wins() {
        let jar_meta = jar(Some("target"), &["x"], &[], &[]);
        let manifest = manifest_deps(&["x"], &[], &[]);
        let installed: Vec<InstalledMod> = vec![];

        let plan = resolve_install_deps(Some(manifest), &jar_meta, &installed);

        assert_eq!(plan.missing_required.len(), 1);
        assert_eq!(plan.missing_required[0].mod_jar_id, "x");
        assert_eq!(plan.missing_required[0].source, DepSource::Jar);
    }

    #[test]
    fn test_install_plan_conflict_in_conflicts_not_missing() {
        let jar_meta = jar(Some("target"), &["x"], &[], &[]);
        let manifest = manifest_deps(&[], &["x"], &[]);
        let installed: Vec<InstalledMod> = vec![];

        let plan = resolve_install_deps(Some(manifest), &jar_meta, &installed);

        // "x" should NOT appear in missing_required or missing_optional
        let in_missing_req = plan
            .missing_required
            .iter()
            .any(|c| c.mod_jar_id == "x");
        let in_missing_opt = plan
            .missing_optional
            .iter()
            .any(|c| c.mod_jar_id == "x");
        assert!(!in_missing_req, "x should not be in missing_required");
        assert!(!in_missing_opt, "x should not be in missing_optional");

        // It should appear in conflicts
        let in_conflicts = plan
            .conflicts
            .iter()
            .any(|c| c.mod_jar_id == "x");
        assert!(in_conflicts, "x should be in conflicts");
    }

    // -----------------------------------------------------------------------
    // D. build_removal_plan / build_disable_plan
    // -----------------------------------------------------------------------

    #[test]
    fn test_removal_plan_with_dependents() {
        let a = installed("a.jar", Some("a"), &[], &[]);
        let b = installed("b.jar", Some("b"), &["a"], &[]);
        let installed = vec![a, b];

        let _target = &installed[1]; // b depends on a, but we remove a (index 0)
        let plan = build_removal_plan(&installed, &installed[0]); // remove a

        assert_eq!(plan.dependents.len(), 1);
        let dep = &plan.dependents[0];
        assert_eq!(dep.mod_id, "b.jar");
        assert_eq!(dep.requirement, Requirement::Required);
    }

    #[test]
    fn test_removal_plan_target_no_jar_id_returns_empty() {
        let a = installed("a.jar", None, &[], &[]);
        let installed = vec![a];

        let plan = build_removal_plan(&installed, &installed[0]);

        assert!(plan.dependents.is_empty());
    }

    #[test]
    fn test_disable_plan_same_as_removal_shape() {
        let a = installed("a.jar", Some("a"), &[], &[]);
        let b = installed("b.jar", Some("b"), &["a"], &[]);
        let installed = vec![a, b];

        let plan = build_disable_plan(&installed, &installed[0]); // disable a

        assert_eq!(plan.dependents.len(), 1);
        let dep = &plan.dependents[0];
        assert_eq!(dep.mod_id, "b.jar");
        assert_eq!(dep.requirement, Requirement::Required);
    }

    // -----------------------------------------------------------------------
    // E. AliasMap unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_alias_map_resolve_known() {
        let aliases = AliasMap::from_pairs(&[
            ("fabric-api".to_string(), "fabric".to_string()),
            ("fabric-api".to_string(), "fabric_api".to_string()),
        ]);

        assert_eq!(aliases.resolve("fabric"), Some("fabric-api".to_string()));
        assert_eq!(aliases.resolve("FABRIC_API"), Some("fabric-api".to_string()));
        assert_eq!(aliases.resolve("fabric-api"), Some("fabric-api".to_string()));
    }

    #[test]
    fn test_alias_map_resolve_unknown_returns_none() {
        let aliases = AliasMap::from_pairs(&[
            ("fabric-api".to_string(), "fabric".to_string()),
            ("fabric-api".to_string(), "fabric_api".to_string()),
        ]);

        assert_eq!(aliases.resolve("unknown"), None);
    }

    #[test]
    fn test_alias_map_resolve_or_self() {
        let aliases = AliasMap::from_pairs(&[
            ("fabric-api".to_string(), "fabric".to_string()),
            ("fabric-api".to_string(), "fabric_api".to_string()),
        ]);

        assert_eq!(aliases.resolve_or_self("fabric"), "fabric-api");
        assert_eq!(aliases.resolve_or_self("unknown"), "unknown");
    }

    #[test]
    fn test_alias_map_empty() {
        let aliases = AliasMap::from_pairs(&[]);

        assert!(aliases.is_empty());
        assert_eq!(aliases.resolve("anything"), None);
        assert_eq!(aliases.resolve_or_self("anything"), "anything");
    }

    // -----------------------------------------------------------------------
    // F. Alias-aware dependency functions
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_dependents_with_aliases_cross_source() {
        // Catalog Fabric API with mod_jar_id="fabric-api"
        let catalog_api = installed(
            "Fabric API.jar",
            Some("fabric-api"),
            &[],
            &[],
        );
        // Modrinth-raw SomeMod with mod_jar_id="some_mod", depends_on=["fabric_api"]
        let modrinth_mod = installed(
            "SomeMod.jar",
            Some("some_mod"),
            &["fabric_api"],
            &[],
        );
        let installed = vec![catalog_api, modrinth_mod];

        // Alias map: "fabric" → "fabric-api", "fabric_api" → "fabric-api"
        let aliases = AliasMap::from_pairs(&[
            ("fabric-api".to_string(), "fabric".to_string()),
            ("fabric-api".to_string(), "fabric_api".to_string()),
        ]);

        // With aliases: target "fabric" resolves to "fabric-api", and
        // modrinth_mod's dep "fabric_api" also resolves to "fabric-api" → match!
        let result = find_dependents_with_aliases(&installed, "fabric", &aliases);
        assert_eq!(result.len(), 1);
        let dep = &result[0];
        assert_eq!(dep.mod_id, "SomeMod.jar");
        assert_eq!(dep.requirement, Requirement::Required);
        assert_eq!(dep.source, DepSource::Jar);
    }

    #[test]
    fn test_find_dependents_without_aliases_preserves_behavior() {
        // Same setup but call the no-alias version: no match because
        // "fabric_api" != "fabric" without alias resolution.
        let catalog_api = installed(
            "Fabric API.jar",
            Some("fabric-api"),
            &[],
            &[],
        );
        let modrinth_mod = installed(
            "SomeMod.jar",
            Some("some_mod"),
            &["fabric_api"],
            &[],
        );
        let installed = vec![catalog_api, modrinth_mod];

        let result = find_dependents(&installed, "fabric");
        assert!(result.is_empty());
    }
}
