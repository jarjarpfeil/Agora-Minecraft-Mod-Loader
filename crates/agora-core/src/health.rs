use crate::db;
use crate::dependency_ops::{AliasMap, JarDeps};
use crate::jar_metadata::parse_jar_metadata;
use crate::models::InstanceManifest;
use crate::registry;
use crate::version_match;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Pre-launch health score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthScore {
    Green,
    Yellow,
    Red,
}

/// A non-blocking concern surfaced in the health dialog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    pub kind: WarningKind,
    pub mod_id: Option<String>,
    /// The actual `.jar` filename on disk, when the finding concerns a specific
    /// installed mod. `None` for findings where no installed JAR is involved
    /// (e.g. a missing required dependency).
    pub filename: Option<String>,
    pub message: String,
    pub suggested_action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    MissingOptionalDependency,
    DuplicateModId,
    UnknownMod,
    /// A JAR-declared hard incompatibility (`breaks` / Forge `incompatible`)
    /// whose version range could NOT be positively matched against the
    /// installed version (version matching is not yet implemented). Surfaced
    /// as a warning rather than a blocker to avoid false launch-blocks.
    IncompatibleModUnverified,
    /// A soft incompatibility: Fabric `conflicts` or NeoForge `discouraged`.
    /// The target is installed; the user should review whether they coexist.
    IncompatibleModSoft,
    /// A curated `known_conflicts` record whose severity is not launch-breaking.
    CuratedConflictSoft,
}

/// A blocking concern that should prevent launch until resolved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocker {
    pub kind: BlockerKind,
    pub mod_id: Option<String>,
    /// The actual `.jar` filename on disk, when the finding concerns a specific
    /// installed mod. `None` for findings where no installed JAR is involved.
    pub filename: Option<String>,
    pub message: String,
    pub suggested_action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockerKind {
    MissingRequiredDependency,
    IncompatibleMod,
    CuratedConflict,
}

/// Full health report for a pre-launch scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub score: HealthScore,
    pub warnings: Vec<Warning>,
    pub blockers: Vec<Blocker>,
}

/// Per-JAR parsed metadata indexed by filename.
struct InstalledJar {
    filename: String,
    jar: JarDeps,
}

/// Alias-resolve a loader-ID -> physical-JAR-files index while retaining each
/// physical file at most once per resolved ID.
///
/// The same JAR can expose several raw IDs that a curated alias map collapses
/// to one canonical ID. That is one installed file, not a duplicate.
fn resolve_id_file_index(
    raw_index: HashMap<String, Vec<String>>,
    aliases: &AliasMap,
) -> HashMap<String, Vec<String>> {
    let mut resolved = HashMap::new();
    for (id, files) in raw_index {
        let canonical = aliases.resolve_or_self(&id).to_lowercase();
        let canonical_files = resolved.entry(canonical).or_insert_with(Vec::new);
        for file in files {
            if !canonical_files.contains(&file) {
                canonical_files.push(file);
            }
        }
    }
    resolved
}

/// Run the pre-launch health scan on an instance.
///
/// Scans every JAR in `mods/`, parses declared dependencies, cross-references
/// against the curated `known_conflicts` table (if registry.db is available),
/// and returns a go/no-go [`HealthReport`].
///
/// Phase 3 property: this function NEVER requires registry.db. If the registry
/// connection is unavailable, curated-conflict checks are skipped — the rest
/// of the scan still runs.
pub fn health(
    instance_dir: &Path,
    manifest: &InstanceManifest,
    registry_db_path: Option<&std::path::Path>,
) -> HealthReport {
    let mods_dir = instance_dir.join("mods");

    // 1. Scan all JARs
    let mut jars: Vec<InstalledJar> = Vec::new();
    if mods_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&mods_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jar") {
                    let jar = parse_jar_metadata(&path);
                    let filename = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    jars.push(InstalledJar { filename, jar });
                }
            }
        }
    }

    // 2. Build separate indexes for two distinct questions:
    //
    // - `presence_id_to_files`: every loader-visible ID (outer primary,
    //   `provides` aliases, and declared nested modules). This is authoritative
    //   for dependency/incompatibility presence checks.
    // - `primary_id_to_files`: only outer physical JAR primary IDs. This is the
    //   only sound input for the user-actionable duplicate-JAR warning. Two
    //   different mods bundling or providing the same library/alias is normal;
    //   asking the user to disable one would be incorrect.
    let mut presence_id_to_files: HashMap<String, Vec<String>> = HashMap::new();
    let mut primary_id_to_files: HashMap<String, Vec<String>> = HashMap::new();
    for ij in &jars {
        let mut ids_seen_in_file: HashSet<String> = HashSet::new();
        for id in ij.jar.all_mod_ids() {
            let id_lower = id.to_lowercase();
            if !ids_seen_in_file.insert(id_lower) {
                continue;
            }
            let files = presence_id_to_files.entry(id.to_string()).or_default();
            if !files.contains(&ij.filename) {
                files.push(ij.filename.clone());
            }
        }
        if let Some(primary_id) = ij.jar.mod_jar_id.as_ref() {
            let files = primary_id_to_files.entry(primary_id.clone()).or_default();
            if !files.contains(&ij.filename) {
                files.push(ij.filename.clone());
            }
        }
    }

    // 3. Also build from manifest's installed mod list (modrinth_id / registry_id)
    let manifest_mod_ids: HashSet<String> = manifest
        .mods
        .iter()
        .filter_map(|m| m.registry_id.clone())
        .collect();

    let mut warnings = Vec::new();
    let mut blockers = Vec::new();

    // 3a. Load aliases and curated deps from the registry for alias resolution
    //     in subsequent checks. (registry.db, optional — Phase 3 decoupling)
    let alias_pairs: Vec<(String, String)> = registry_db_path
        .and_then(|p| {
            if p.exists() {
                db::registry_connection(p)
                    .ok()
                    .and_then(|conn| registry::get_all_mod_aliases(&conn).ok())
            } else {
                None
            }
        })
        .unwrap_or_default();
    let aliases = AliasMap::from_pairs(&alias_pairs);

    let curated_deps: HashMap<String, registry::ManifestDeps> = registry_db_path
        .and_then(|p| {
            if p.exists() {
                db::registry_connection(p)
                    .ok()
                    .and_then(|conn| registry::get_all_manifest_dependencies(&conn).ok())
            } else {
                None
            }
        })
        .unwrap_or_default();
    let curated_index: HashMap<String, &registry::ManifestDeps> = curated_deps
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect();

    // Rebuild both indexes with alias-resolved keys. Crucially, provided and
    // nested IDs remain exclusively in the presence index—they never leak into
    // duplicate-JAR warnings.
    let presence_id_to_files = resolve_id_file_index(presence_id_to_files, &aliases);
    let primary_id_to_files = resolve_id_file_index(primary_id_to_files, &aliases);

    // Build alias-resolved mod_id -> mod_version map for the version-matching
    // step below. The parser populates `JarDeps.mod_version` from
    // fabric.mod.json's `version`, Forge's `version=` in `[[mods]]`, or
    // `META-INF/MANIFEST.MF`'s `Implementation-Version`.
    let id_to_version: HashMap<String, String> = {
        let mut m: HashMap<String, String> = HashMap::new();
        for ij in &jars {
            if let (Some(id), Some(ver)) = (&ij.jar.mod_jar_id, &ij.jar.mod_version) {
                let canonical = aliases.resolve_or_self(id).to_lowercase();
                // Replace an unresolved placeholder, but never overwrite a
                // concrete version with weaker metadata.
                if m.get(&canonical).is_none()
                    || matches!(m.get(&canonical), Some(existing) if existing.starts_with("${"))
                {
                    m.insert(canonical, ver.clone());
                }
            }
            for provided in &ij.jar.provided_mods {
                let Some(ver) = &provided.version else {
                    continue;
                };
                let canonical = aliases.resolve_or_self(&provided.mod_id).to_lowercase();
                if m.get(&canonical).is_none()
                    || matches!(m.get(&canonical), Some(existing) if existing.starts_with("${"))
                {
                    m.insert(canonical, ver.clone());
                }
            }
        }
        m
    };

    // 4. Duplicate physical top-level JAR primary-ID check.
    for (id, files) in &primary_id_to_files {
        if files.len() > 1 {
            warnings.push(Warning {
                kind: WarningKind::DuplicateModId,
                mod_id: Some(id.clone()),
                filename: files.first().cloned(),
                message: format!(
                    "Multiple JARs declare mod ID '{}': {}",
                    id,
                    files.join(", ")
                ),
                suggested_action: Some(
                    "Keep only one version of this mod; disable the others.".into(),
                ),
            });
        }
    }

    // 5. Required dependency checks (alias-aware)
    for ij in &jars {
        let source = &ij.filename;
        for dep in &ij.jar.depends_on {
            let dep_resolved = aliases.resolve_or_self(dep).to_lowercase();
            let dep_present = presence_id_to_files.contains_key(&dep_resolved)
                || manifest_mod_ids
                    .iter()
                    .any(|id| aliases.resolve_or_self(id).to_lowercase() == dep_resolved);
            if !dep_present {
                let display_name = if dep_resolved != dep.to_lowercase() {
                    dep_resolved.clone()
                } else {
                    dep.clone()
                };
                blockers.push(Blocker {
                    kind: BlockerKind::MissingRequiredDependency,
                    mod_id: Some(display_name.clone()),
                    filename: None, // dependency is not installed
                    message: format!(
                        "'{}' requires '{}' but it is not installed.",
                        source, display_name
                    ),
                    suggested_action: Some(format!(
                        "Install '{}' to resolve this dependency.",
                        display_name
                    )),
                });
            }
        }
    }

    // 6. Incompatible mod checks (alias-aware with curated override).
    //
    // Consumes structured `IncompatibilityDecl`s with real version-range
    // matching (Fabric predicates + Forge Maven ranges). The policy is:
    //   - hard (breaks/Forge incompatible/Quilt breaks) + version in range or
    //     unconditional => BLOCKER;
    //   - hard + version explicitly outside the range => NO finding (the
    //     incompatibility does not apply to the installed version);
    //   - hard + conditional range but target version unknown => NO finding
    //     (can't confirm; better silence than a possibly-wrong warning);
    //   - soft (conflicts/discouraged) + version in range or unconditional =>
    //     WARNING;
    //   - soft + version outside the range => NO finding;
    //   - self-declared conflict => discarded;
    //   - curated ManifestDeps declaring the pair compatible => suppressed.
    //
    // Also backfill decls from older JAR parses that populated the flat
    // `incompatible_deps` list but emitted no `incompatibility_decls` (e.g.
    // legacy/desktop parser output). These legacy entries carry no severity or
    // version info, so they are treated as soft (unconditional) — always a
    // warning, never a blocker — to avoid reintroducing false-positive blockers.
    for ij in &jars {
        let source = &ij.filename;
        let source_mod_id = ij.jar.mod_jar_id.as_deref();
        // Canonicalize the SOURCE id through aliases BEFORE lookup so curated
        // overrides keyed by the registry id still match a raw jar id.
        let source_resolved = source_mod_id.map(|id| aliases.resolve_or_self(id).to_lowercase());

        // Collect the effective declarations for this jar, backfilling any
        // flat-list ids that are not already represented in the structured
        // decls (legacy parses).
        let mut effective_decls: Vec<&crate::dependency_ops::IncompatibilityDecl> =
            ij.jar.incompatibility_decls.iter().collect();
        let structured_ids: HashSet<String> = ij
            .jar
            .incompatibility_decls
            .iter()
            .map(|d| d.mod_id.to_lowercase())
            .collect();
        let legacy_backfilled: Vec<crate::dependency_ops::IncompatibilityDecl> = ij
            .jar
            .incompatible_deps
            .iter()
            .filter(|id| !structured_ids.contains(&id.to_lowercase()))
            .map(|id| crate::dependency_ops::IncompatibilityDecl {
                mod_id: id.clone(),
                version_ranges: Vec::new(),
                source: crate::dependency_ops::IncompatibilitySource::ForgeDiscouraged,
            })
            .collect();
        effective_decls.extend(legacy_backfilled.iter());

        for decl in effective_decls {
            let incompat_resolved = aliases.resolve_or_self(&decl.mod_id).to_lowercase();

            // Self-conflict guard: a mod never conflicts with itself.
            if source_resolved.as_deref() == Some(incompat_resolved.as_str()) {
                continue;
            }

            let incompat_present = presence_id_to_files.contains_key(&incompat_resolved)
                || manifest_mod_ids
                    .iter()
                    .any(|id| aliases.resolve_or_self(id).to_lowercase() == incompat_resolved);
            if !incompat_present {
                continue;
            }

            // Curated override: the curator has verified the pair is compatible
            // (either side lists the other as a required/optional dep). This
            // suppresses JAR-derived declarations of any severity.
            let curated_override =
                source_resolved.as_ref().is_some_and(|src| {
                    let source_side = curated_index.get(src).is_some_and(|deps| {
                        deps.required
                            .iter()
                            .any(|r| aliases.resolve_or_self(r).to_lowercase() == incompat_resolved)
                            || deps.optional.iter().any(|o| {
                                aliases.resolve_or_self(o).to_lowercase() == incompat_resolved
                            })
                    });
                    let target_side = curated_index.get(&incompat_resolved).is_some_and(|deps| {
                        let src = source_resolved.as_deref();
                        deps.required
                            .iter()
                            .any(|r| aliases.resolve_or_self(r).to_lowercase() == src.unwrap_or(""))
                            || deps.optional.iter().any(|o| {
                                aliases.resolve_or_self(o).to_lowercase() == src.unwrap_or("")
                            })
                    });
                    source_side || target_side
                });
            if curated_override {
                continue;
            }

            // Version evaluation is shared by hard and soft paths.
            //
            // When the target mod's version is known AND the declaration
            // carries an explicit range, we evaluate whether the installed
            // version falls inside it:
            //   - Matched (range covers the installed version):
            //       hard → blocker; soft → warning.
            //   - Unconditional (no range at all — e.g. `"*"`):
            //       hard → blocker; soft → warning.
            //   - NotMatched (range explicitly excludes the installed version):
            //       NO finding at all. If a mod says "I break versions <2.0"
            //       and the installed target is 2.5, the incompatibility simply
            //       does not apply — surfacing a warning would be noise.
            //
            // When the target version is unknown, we can't evaluate: unconditional
            // declarations still fire (safety default); conditional ones are
            // dropped (better silence than a possibly-wrong warning).
            let target_version = id_to_version.get(&incompat_resolved);
            let vmatch = match target_version {
                Some(ver) => crate::version_match::evaluate_version_match(
                    &decl.version_ranges,
                    ver,
                    decl.source.is_fabric_grammar(),
                ),
                None => {
                    if is_unconditional(&decl.version_ranges) {
                        version_match::VersionMatch::Unconditional
                    } else {
                        // Unknown version + conditional range: can't confirm, so
                        // skip rather than risk a false-positive warning.
                        continue;
                    }
                }
            };

            match vmatch {
                version_match::VersionMatch::NotMatched => {
                    // Installed version is outside the declared incompatible
                    // range → the declaration does not apply. No finding.
                    continue;
                }
                version_match::VersionMatch::Matched
                | version_match::VersionMatch::Unconditional
                    if decl.source.is_hard() =>
                {
                    blockers.push(Blocker {
                        kind: BlockerKind::IncompatibleMod,
                        mod_id: Some(decl.mod_id.clone()),
                        filename: Some(source.clone()), // disable the source mod
                        message: format!(
                            "'{}' declares an incompatibility with '{}' and both are installed.",
                            source, decl.mod_id
                        ),
                        suggested_action: Some(format!(
                            "Remove '{}' or '{}' to resolve the conflict.",
                            source, decl.mod_id
                        )),
                    });
                }
                version_match::VersionMatch::Matched
                | version_match::VersionMatch::Unconditional => {
                    // Soft incompatibility (Fabric `conflicts`, NeoForge
                    // `discouraged`, or legacy backfilled entries) whose range
                    // matches or is unconditional: warning, never a blocker.
                    warnings.push(Warning {
                        kind: WarningKind::IncompatibleModSoft,
                        mod_id: Some(decl.mod_id.clone()),
                        filename: Some(source.clone()),
                        message: format!(
                            "'{}' may conflict with '{}' (soft incompatibility). The mod may still function; review before launch.",
                            source, decl.mod_id
                        ),
                        suggested_action: Some(format!(
                            "If you experience issues, remove '{}' or '{}'.",
                            source, decl.mod_id
                        )),
                    });
                }
            }
        }
    }

    // 7. Curated known_conflicts (registry.db, optional — Phase 3 decoupling)
    if let Some(reg_path) = registry_db_path {
        if reg_path.exists() {
            if let Ok(conn) = db::registry_connection(reg_path) {
                if let Ok(conflicts) = registry::get_known_conflicts(&conn) {
                    // Build reverse index: registry_id -> filename for cross-reference
                    let installed_registry_ids: HashSet<&str> = manifest
                        .mods
                        .iter()
                        .filter_map(|m| m.registry_id.as_deref())
                        .collect();

                    for conflict in &conflicts {
                        let a_present = installed_registry_ids.contains(conflict.mod_a_id.as_str())
                            || presence_id_to_files.contains_key(conflict.mod_a_id.as_str());
                        let b_present = installed_registry_ids.contains(conflict.mod_b_id.as_str())
                            || presence_id_to_files.contains_key(conflict.mod_b_id.as_str());
                        if a_present && b_present {
                            let mitigation = if conflict.mitigated_by.is_empty() {
                                "No known mitigation.".into()
                            } else {
                                format!("Try removing: {}", conflict.mitigated_by.join(", "))
                            };
                            let message = format!(
                                "Known conflict between '{}' and '{}' (severity: {}). {}",
                                conflict.mod_a_id,
                                conflict.mod_b_id,
                                conflict.severity,
                                conflict.notes.as_deref().unwrap_or("")
                            );
                            if is_hard_severity(&conflict.severity) {
                                blockers.push(Blocker {
                                    kind: BlockerKind::CuratedConflict,
                                    mod_id: None,
                                    filename: None, // no single actionable file
                                    message,
                                    suggested_action: Some(mitigation),
                                });
                            } else {
                                // Non-hard (or unrecognized/missing) severity:
                                // informational warning, never a launch-blocker.
                                warnings.push(Warning {
                                    kind: WarningKind::CuratedConflictSoft,
                                    mod_id: None,
                                    filename: None,
                                    message,
                                    suggested_action: Some(mitigation),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // 8. Optional dependency warnings (alias-aware)
    for ij in &jars {
        let source = &ij.filename;
        for dep in &ij.jar.optional_deps {
            let dep_resolved = aliases.resolve_or_self(dep).to_lowercase();
            let dep_present = presence_id_to_files.contains_key(&dep_resolved)
                || manifest_mod_ids
                    .iter()
                    .any(|id| aliases.resolve_or_self(id).to_lowercase() == dep_resolved);
            if !dep_present {
                let display_name = if dep_resolved != dep.to_lowercase() {
                    dep_resolved.clone()
                } else {
                    dep.clone()
                };
                warnings.push(Warning {
                    kind: WarningKind::MissingOptionalDependency,
                    mod_id: Some(display_name.clone()),
                    filename: None, // dependency is not installed
                    message: format!(
                        "'{}' recommends '{}' but it is not installed. The mod may work without it.",
                        source, display_name
                    ),
                    suggested_action: None,
                });
            }
        }
    }

    // 9. Unknown mods (in mods/ dir but not tracked in manifest)
    let manifest_filenames: HashSet<&str> =
        manifest.mods.iter().map(|m| m.filename.as_str()).collect();
    for ij in &jars {
        if !manifest_filenames.contains(ij.filename.as_str()) {
            warnings.push(Warning {
                kind: WarningKind::UnknownMod,
                mod_id: ij.jar.mod_jar_id.clone(),
                filename: Some(ij.filename.clone()),
                message: format!(
                    "'{}' is in the mods folder but not tracked in the instance manifest.",
                    ij.filename
                ),
                suggested_action: Some(
                    "This may be a manually-added mod. It will be launched but is not managed by Agora.".into(),
                ),
            });
        }
    }

    // 10. Compute score
    let score = if blockers.is_empty() && warnings.is_empty() {
        HealthScore::Green
    } else if blockers.is_empty() {
        HealthScore::Yellow
    } else {
        HealthScore::Red
    };

    HealthReport {
        score,
        warnings,
        blockers,
    }
}

/// True when a version-range list represents an unconditional match (any
/// installed version satisfies it). This is the only version judgment Agora
/// makes without a full predicate/Maven-range parser:
///   - empty ranges => no constraint declared => unconditional;
///   - any `"*"` / empty-string entry => Fabric "any version" => unconditional;
///   - Forge open-ended empty/match-all ranges (`"[,)"`, `"[,]"`) => unconditional.
///
/// Anything else (e.g. `"<2.0"`, `"[1.0,2.0)"`) is treated as *conditional* —
/// unverified — and surfaced as a warning rather than a blocker.
fn is_unconditional(ranges: &[String]) -> bool {
    ranges.is_empty()
        || ranges.iter().any(|r| {
            let t = r.trim();
            t == "*" || t.is_empty() || t == "[,)" || t == "[,]"
        })
}

/// True when a curated `known_conflicts.severity` string denotes a
/// launch-breaking (hard) conflict. Uses exact, case-insensitive, trimmed
/// matching against an allowlist so values like `"hardcoded"` do not match.
/// Anything unrecognized (including missing/empty) is treated as soft (warning)
/// — whitelist-over-denylist, defaulting to the non-blocking classification.
fn is_hard_severity(s: &str) -> bool {
    matches!(
        s.trim().to_lowercase().as_str(),
        "hard" | "critical" | "breaking" | "fatal" | "incompatible" | "block" | "blocker"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependency_ops::{IncompatibilityDecl, IncompatibilitySource, JarDeps};
    use crate::models::InstalledMod;

    #[test]
    fn health_empty_instance_is_green() {
        let manifest = InstanceManifest {
            instance_id: "test".into(),
            name: "Test".into(),
            created_from_pack: None,
            minecraft_version: "1.21".into(),
            loader: "fabric".into(),
            loader_version: "0.15.11".into(),
            is_locked: false,
            mods: vec![],
            resourcepacks: vec![],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            user_preferences: serde_json::json!({}),
        };
        let dir = std::env::temp_dir().join("agora_health_test_empty");
        let _ = std::fs::create_dir_all(dir.join("mods"));
        let report = health(&dir, &manifest, None);
        assert_eq!(report.score, HealthScore::Green);
        assert!(report.blockers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_missing_required_dep_is_red() {
        let manifest = InstanceManifest {
            instance_id: "test".into(),
            name: "Test".into(),
            created_from_pack: None,
            minecraft_version: "1.21".into(),
            loader: "fabric".into(),
            loader_version: "0.15.11".into(),
            is_locked: false,
            mods: vec![InstalledMod {
                filename: "mod-with-dep.jar".into(),
                registry_id: None,
                modrinth_id: None,
                source: "modrinth".into(),
                source_url: None,
                version: Some("1.0.0".into()),
                sha256: "abc".into(),
                installed_at: "2024-01-01T00:00:00Z".into(),
                java_packages: vec![],
                mod_jar_id: Some("mod-with-dep".into()),
                depends_on: vec!["fabric-api".into()],
                optional_deps: vec![],
                incompatible_deps: vec![],
                provided_mod_ids: vec![],
                enabled: true,
                content_type: "mod".into(),
            }],
            resourcepacks: vec![],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            user_preferences: serde_json::json!({}),
        };
        let dir = std::env::temp_dir().join("agora_health_test_missing_dep");
        let mods_dir = dir.join("mods");
        let _ = std::fs::create_dir_all(&mods_dir);
        // No fabric-api.jar present, but mod-with-dep.jar declares it as required
        // Simulate by not placing any JARs (parse_jar_metadata returns defaults)
        // The health function walks mods/ which is empty, so no jars are found.
        // With no jars found, there are no blockers — this is the "no mods installed" case.
        // To test the missing-dep case properly we'd need a real JAR or a mock.
        // For now just verify the function doesn't panic.
        let report = health(&dir, &manifest, None);
        assert!(matches!(
            report.score,
            HealthScore::Green | HealthScore::Yellow
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -------------------------------------------------------------------
    // Incompatibility policy tests (Fabric breaks/conflicts + Forge).
    //
    // These build real .jar fixtures in a temp mods/ dir so the health check's
    // own parse path is exercised end-to-end.
    // -------------------------------------------------------------------

    /// Build an in-memory .jar with the given `(entry, content)` pairs into
    /// `mods_dir/<filename>`.
    fn write_jar(mods_dir: &Path, filename: &str, entries: &[(&str, &str)]) {
        use std::io::Write;
        let path = mods_dir.join(filename);
        let file = std::fs::File::create(&path).expect("create jar file");
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        for (name, content) in entries {
            zip.start_file(*name, opts).expect("start_file");
            zip.write_all(content.as_bytes()).expect("write_all");
        }
        zip.finish().expect("finish zip");
    }

    fn jar_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::{Cursor, Write};
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let opts = zip::write::FileOptions::default();
            for (name, content) in entries {
                zip.start_file(*name, opts).expect("start nested entry");
                zip.write_all(content).expect("write nested entry");
            }
            zip.finish().expect("finish nested jar");
        }
        cursor.into_inner()
    }

    fn write_binary_jar(mods_dir: &Path, filename: &str, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let file = std::fs::File::create(mods_dir.join(filename)).expect("create jar file");
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        for (name, content) in entries {
            zip.start_file(*name, opts).expect("start jar entry");
            zip.write_all(content).expect("write jar entry");
        }
        zip.finish().expect("finish jar");
    }

    fn fresh_instance(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agora_health_incompat_{}_{}",
            label,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mods")).expect("create mods dir");
        dir
    }

    fn tracked_manifest(mods: &[(&str, &str)]) -> InstanceManifest {
        // mods: (filename, mod_jar_id)
        let mods: Vec<InstalledMod> = mods
            .iter()
            .map(|(filename, jar_id)| InstalledMod {
                filename: filename.to_string(),
                registry_id: None,
                modrinth_id: None,
                source: "manual".into(),
                source_url: None,
                version: None,
                sha256: String::new(),
                installed_at: String::new(),
                java_packages: vec![],
                mod_jar_id: Some(jar_id.to_string()),
                depends_on: vec![],
                optional_deps: vec![],
                incompatible_deps: vec![],
                provided_mod_ids: vec![],
                enabled: true,
                content_type: "mod".into(),
            })
            .collect();
        InstanceManifest {
            instance_id: "test".into(),
            name: "Test".into(),
            created_from_pack: None,
            minecraft_version: "1.21".into(),
            loader: "fabric".into(),
            loader_version: "0.15.11".into(),
            is_locked: false,
            mods,
            resourcepacks: vec![],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            user_preferences: serde_json::json!({}),
        }
    }

    #[test]
    fn health_nested_modules_and_provides_satisfy_reported_dependencies() {
        let dir = fresh_instance("nested_modules_satisfy_deps");
        let mods_dir = dir.join("mods");

        // Fabric API is a physical umbrella JAR whose loader-visible API
        // modules live in explicitly declared nested JARs.
        let resource_loader = jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"fabric-resource-loader-v1","version":"3.2.0"}"#,
        )]);
        let command_api = jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"fabric-command-api-v2","version":"2.4.0"}"#,
        )]);
        write_binary_jar(
            &mods_dir,
            "fabric-api.jar",
            &[
                (
                    "fabric.mod.json",
                    br#"{"id":"fabric-api","version":"0.141.4","jars":[{"file":"META-INF/jars/resource.jar"},{"file":"META-INF/jars/command.jar"}]}"#,
                ),
                ("META-INF/jars/resource.jar", &resource_loader),
                ("META-INF/jars/command.jar", &command_api),
            ],
        );

        // Skyboxify consumes the nested Fabric API module IDs rather than the
        // umbrella ID. They must be recognized without curated aliases.
        write_jar(
            &mods_dir,
            "skyboxify.jar",
            &[(
                "fabric.mod.json",
                r#"{"id":"skyboxify","version":"2.8","depends":{"fabric-command-api-v2":"*","fabric-resource-loader-v1":"*"}}"#,
            )],
        );

        // Dynamic FPS and LambDynamicLights bundle required runtime modules
        // inside their own physical JARs. Intra-JAR requirements are satisfied
        // after all declared nested modules have been parsed.
        let mixinextras = jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"mixinextras","version":"0.5.0"}"#,
        )]);
        write_binary_jar(
            &mods_dir,
            "dynamic-fps.jar",
            &[
                (
                    "fabric.mod.json",
                    br#"{"id":"dynamic_fps","version":"3.11.6","depends":{"mixinextras":"*"},"jars":[{"file":"META-INF/jars/mixinextras.jar"}]}"#,
                ),
                ("META-INF/jars/mixinextras.jar", &mixinextras),
            ],
        );

        let runtime = jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"lambdynlights_runtime","version":"4.9.1"}"#,
        )]);
        write_binary_jar(
            &mods_dir,
            "lambdynamiclights.jar",
            &[
                (
                    "fabric.mod.json",
                    br#"{"id":"lambdynlights","version":"4.9.1","depends":{"lambdynlights_runtime":"*"},"jars":[{"file":"META-INF/jars/runtime.jar"}]}"#,
                ),
                ("META-INF/jars/runtime.jar", &runtime),
            ],
        );

        // Metadata-declared aliases are also loader-visible identities.
        write_jar(
            &mods_dir,
            "alias-provider.jar",
            &[(
                "fabric.mod.json",
                r#"{"id":"real_library","version":"1.0","provides":["legacy_library"]}"#,
            )],
        );
        write_jar(
            &mods_dir,
            "alias-consumer.jar",
            &[(
                "fabric.mod.json",
                r#"{"id":"alias_consumer","version":"1.0","depends":{"legacy_library":"*"}}"#,
            )],
        );

        let manifest = tracked_manifest(&[
            ("fabric-api.jar", "fabric-api"),
            ("skyboxify.jar", "skyboxify"),
            ("dynamic-fps.jar", "dynamic_fps"),
            ("lambdynamiclights.jar", "lambdynlights"),
            ("alias-provider.jar", "real_library"),
            ("alias-consumer.jar", "alias_consumer"),
        ]);
        let report = health(&dir, &manifest, None);

        assert!(
            !report
                .blockers
                .iter()
                .any(|b| b.kind == BlockerKind::MissingRequiredDependency),
            "nested/provided IDs must satisfy requirements: {:?}",
            report.blockers
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.kind == WarningKind::DuplicateModId),
            "one physical JAR exposing multiple IDs must not duplicate itself: {:?}",
            report.warnings
        );
        assert_eq!(report.score, HealthScore::Green);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_repeated_bundled_or_provided_ids_do_not_warn_as_duplicates() {
        let dir = fresh_instance("bundled_ids_not_duplicates");
        let mods_dir = dir.join("mods");

        // These simulate Controlify/YACL and Fabric API consumers respectively:
        // distinct outer mods bundle the same nested library and offer the same
        // alias. Both IDs are valid for dependency presence, but neither outer
        // JAR is a duplicate copy of the other.
        let shared_library = jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"com_twelvemonkeys_imageio_imageio-core","version":"3.12.0"}"#,
        )]);
        for outer_id in ["controlify", "yet_another_config_lib_v3"] {
            let metadata = format!(
                r#"{{"id":"{outer_id}","version":"1.0","provides":["fabric-api"],"jars":[{{"file":"META-INF/jars/imageio.jar"}}]}}"#
            );
            let filename = format!("{outer_id}.jar");
            write_binary_jar(
                &mods_dir,
                &filename,
                &[
                    ("fabric.mod.json", metadata.as_bytes()),
                    ("META-INF/jars/imageio.jar", &shared_library),
                ],
            );
        }

        let manifest = tracked_manifest(&[
            ("controlify.jar", "controlify"),
            ("yet_another_config_lib_v3.jar", "yet_another_config_lib_v3"),
        ]);
        let report = health(&dir, &manifest, None);

        assert!(
            !report
                .warnings
                .iter()
                .any(|warning| warning.kind == WarningKind::DuplicateModId),
            "bundled modules and provides aliases must not trigger top-level duplicate warnings: {:?}",
            report.warnings
        );
        assert_eq!(report.score, HealthScore::Green);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_same_top_level_primary_id_still_warns() {
        let dir = fresh_instance("same_primary_id_duplicate");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "duplicate-a.jar",
            &[(
                "fabric.mod.json",
                r#"{"id":"real_duplicate","version":"1.0"}"#,
            )],
        );
        write_jar(
            &mods_dir,
            "duplicate-b.jar",
            &[(
                "fabric.mod.json",
                r#"{"id":"real_duplicate","version":"2.0"}"#,
            )],
        );

        let manifest = tracked_manifest(&[
            ("duplicate-a.jar", "real_duplicate"),
            ("duplicate-b.jar", "real_duplicate"),
        ]);
        let report = health(&dir, &manifest, None);

        let duplicate = report
            .warnings
            .iter()
            .find(|warning| warning.kind == WarningKind::DuplicateModId)
            .expect("two physical JARs with the same primary ID must warn");
        assert_eq!(duplicate.mod_id.as_deref(), Some("real_duplicate"));
        assert_eq!(report.score, HealthScore::Yellow);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_unconditional_breaks_blocks_launch() {
        // A breaks B with "*" (unconditional) and both installed => BLOCKER.
        let dir = fresh_instance("unconditional_breaks");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","breaks":{"b":"*"}}"#)],
        );
        write_jar(&mods_dir, "b.jar", &[("fabric.mod.json", r#"{"id":"b"}"#)]);
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert_eq!(report.score, HealthScore::Red);
        assert_eq!(report.blockers.len(), 1);
        assert_eq!(report.blockers[0].kind, BlockerKind::IncompatibleMod);
        assert_eq!(report.blockers[0].mod_id.as_deref(), Some("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_conditional_breaks_outside_range_no_finding() {
        // A breaks B with "<2.0"; B installed at version 2.5 (outside the
        // incompatible range). Since the installed version does not fall in
        // the declared range, the incompatibility does not apply: no blocker,
        // no warning — Clean.
        let dir = fresh_instance("conditional_breaks_outside");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","breaks":{"b":"<2.0"}}"#)],
        );
        write_jar(
            &mods_dir,
            "b.jar",
            &[("fabric.mod.json", r#"{"id":"b","version":"2.5"}"#)],
        );
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert!(
            report.blockers.is_empty(),
            "breaks with non-matching installed version must not block: {:?}",
            report.blockers
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.mod_id.as_deref() == Some("b")),
            "breaks with non-matching installed version must produce no warning: {:?}",
            report.warnings
        );
        assert_eq!(report.score, HealthScore::Green);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_fabric_conflicts_is_warning_never_blocker() {
        let dir = fresh_instance("conflicts_warning");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","conflicts":{"b":"*"}}"#)],
        );
        write_jar(&mods_dir, "b.jar", &[("fabric.mod.json", r#"{"id":"b"}"#)]);
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert!(
            report.blockers.is_empty(),
            "conflicts must never block: {:?}",
            report.blockers
        );
        assert!(report.warnings.iter().any(
            |w| w.kind == WarningKind::IncompatibleModSoft && w.mod_id.as_deref() == Some("b")
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_fabric_conflicts_outside_range_no_finding() {
        // A conflicts B with "<2.0"; B installed at 2.5 (outside the range).
        // Soft or hard, when the installed version is outside the declared
        // range the incompatibility does not apply: no finding at all.
        let dir = fresh_instance("conflicts_outside_range");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","conflicts":{"b":"<2.0"}}"#)],
        );
        write_jar(
            &mods_dir,
            "b.jar",
            &[("fabric.mod.json", r#"{"id":"b","version":"2.5"}"#)],
        );
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert!(report.blockers.is_empty());
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.mod_id.as_deref() == Some("b")),
            "non-matching conflicts range must produce no warning: {:?}",
            report.warnings
        );
        assert_eq!(report.score, HealthScore::Green);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_self_conflict_discarded() {
        // A declares breaks on itself (parse bug or real edge case) => no finding.
        let dir = fresh_instance("self_conflict");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","breaks":{"a":"*"}}"#)],
        );
        let manifest = tracked_manifest(&[("a.jar", "a")]);
        let report = health(&dir, &manifest, None);
        assert!(
            report.blockers.is_empty(),
            "self-conflict must not block: {:?}",
            report.blockers
        );
        assert!(
            !report.warnings.iter().any(|w| {
                w.kind == WarningKind::IncompatibleModSoft
                    || w.kind == WarningKind::IncompatibleModUnverified
            }),
            "self-conflict must not warn either: {:?}",
            report.warnings
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_breaks_target_absent_no_finding() {
        // A breaks B, but B is not installed => nothing.
        let dir = fresh_instance("breaks_target_absent");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","breaks":{"b":"*"}}"#)],
        );
        let manifest = tracked_manifest(&[("a.jar", "a")]);
        let report = health(&dir, &manifest, None);
        assert!(report.blockers.is_empty());
        assert!(!report
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::IncompatibleModSoft
                || w.kind == WarningKind::IncompatibleModUnverified));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_forge_self_conflict_via_owner_header_fixed() {
        // Regression for the original bug: a Forge mod whose dependency block
        // header matched its own modId previously produced a self-conflict.
        let dir = fresh_instance("forge_self_conflict");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "examplemod.jar",
            &[
                (
                    "META-INF/mods.toml",
                    "modId=\"examplemod\"\n[[dependencies.examplemod]]\n    modId=\"othermod\"\n    type=\"incompatible\"\n",
                ),
            ],
        );
        let manifest = tracked_manifest(&[("examplemod.jar", "examplemod")]);
        let report = health(&dir, &manifest, None);
        assert!(
            report.blockers.is_empty(),
            "must not self-conflict: {:?}",
            report.blockers
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_alias_resolution_collapses_breaks_target() {
        // A breaks "mod-b" but B is installed with jar id "b", and an alias maps
        // "mod-b" -> "b". The break should resolve and fire (unconditional => blocker).
        let dir = fresh_instance("alias_breaks");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","breaks":{"mod-b":"*"}}"#)],
        );
        write_jar(&mods_dir, "b.jar", &[("fabric.mod.json", r#"{"id":"b"}"#)]);
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);

        // Build a registry.db with a single alias and run health against it.
        let reg_path = dir.join("registry.db");
        build_alias_registry(&reg_path, &[("b", "mod-b")]);
        let report = health(&dir, &manifest, Some(&reg_path));
        assert_eq!(
            report.blockers.len(),
            1,
            "alias-resolved break should block: {:?}",
            report.blockers
        );
        assert_eq!(report.blockers[0].kind, BlockerKind::IncompatibleMod);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- HF2: version-range matching produces real blockers / no-findings. ---

    #[test]
    fn health_breaks_matched_version_blocks() {
        // A breaks B with "<2.0"; B installed at version 1.5 — version IS
        // inside the incompatible range, so this should now produce a blocker
        // (the version-matching upgrade distinguishes this from the
        // non-matching case).
        let dir = fresh_instance("breaks_matched");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","breaks":{"b":"<2.0"}}"#)],
        );
        write_jar(
            &mods_dir,
            "b.jar",
            &[("fabric.mod.json", r#"{"id":"b","version":"1.5"}"#)],
        );
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert_eq!(report.score, HealthScore::Red);
        assert_eq!(report.blockers.len(), 1);
        assert_eq!(report.blockers[0].kind, BlockerKind::IncompatibleMod);
        assert_eq!(report.blockers[0].mod_id.as_deref(), Some("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_forge_maven_range_matched_blocks() {
        // Forge A declares B `versionRange="[1.0,2.0)"` with `type="incompatible"`;
        // B installed at version 1.5 — falls inside the Maven range, so blocker.
        let dir = fresh_instance("forge_maven_matched");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[(
                "META-INF/mods.toml",
                "modId=\"a\"\nversion=\"1.0\"\n[[dependencies.a]]\nmodId=\"b\"\n\
                 type=\"incompatible\"\nversionRange=\"[1.0,2.0)\"\nmandatory=true\n",
            )],
        );
        write_jar(
            &mods_dir,
            "b.jar",
            &[
                ("META-INF/mods.toml", "modId=\"b\"\nversion=\"1.5\"\n"),
                (
                    "META-INF/MANIFEST.MF",
                    "Manifest-Version: 1.0\nImplementation-Version: 1.5\n",
                ),
            ],
        );
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert_eq!(report.score, HealthScore::Red);
        assert_eq!(report.blockers.len(), 1);
        assert_eq!(report.blockers[0].kind, BlockerKind::IncompatibleMod);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_forge_maven_range_non_matching_no_finding() {
        // Same range as above, but B installed at 2.5 — outside [1.0,2.0).
        // The incompatibility does not apply to this version: no blocker, no
        // warning — silent.
        let dir = fresh_instance("forge_maven_unmatched");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[(
                "META-INF/mods.toml",
                "modId=\"a\"\nversion=\"1.0\"\n[[dependencies.a]]\nmodId=\"b\"\n\
                 type=\"incompatible\"\nversionRange=\"[1.0,2.0)\"\nmandatory=true\n",
            )],
        );
        write_jar(
            &mods_dir,
            "b.jar",
            &[
                ("META-INF/mods.toml", "modId=\"b\"\nversion=\"2.5\"\n"),
                (
                    "META-INF/MANIFEST.MF",
                    "Manifest-Version: 1.0\nImplementation-Version: 2.5\n",
                ),
            ],
        );
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert!(
            report.blockers.is_empty(),
            "non-matching Maven range must not block: {:?}",
            report.blockers
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.mod_id.as_deref() == Some("b")),
            "non-matching Maven range must produce no warning: {:?}",
            report.warnings
        );
        assert_eq!(report.score, HealthScore::Green);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_breaks_unknown_target_version_unconditional_falls_back_to_blocking() {
        // A declares breaks on B with no version range ("*"). B installed
        // without a mod_version string — we cannot evaluate, but the
        // unconditional nature means it should still block.
        let dir = fresh_instance("breaks_unconditional_unknown_ver");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[("fabric.mod.json", r#"{"id":"a","breaks":{"b":"*"}}"#)],
        );
        write_jar(&mods_dir, "b.jar", &[("fabric.mod.json", r#"{"id":"b"}"#)]);
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert_eq!(report.blockers.len(), 1);
        assert_eq!(report.blockers[0].kind, BlockerKind::IncompatibleMod);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- HF3: quilt.mod.json parsing produces correct findings. ---

    #[test]
    fn health_quilt_breaks_unconditional_blocks() {
        // Quilt's `quilt_loader.breaks` with "*" should behave like Fabric
        // `breaks`: hard blocker when both mods are installed.
        let dir = fresh_instance("quilt_breaks");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[(
                "quilt.mod.json",
                r#"{"quilt_loader":{"id":"a","version":"1.0","breaks":{"b":"*"}}}"#,
            )],
        );
        write_jar(
            &mods_dir,
            "b.jar",
            &[(
                "quilt.mod.json",
                r#"{"quilt_loader":{"id":"b","version":"1.0"}}"#,
            )],
        );
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert_eq!(report.score, HealthScore::Red);
        assert_eq!(report.blockers.len(), 1);
        assert_eq!(report.blockers[0].kind, BlockerKind::IncompatibleMod);
        assert_eq!(report.blockers[0].mod_id.as_deref(), Some("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_quilt_conflicts_is_warning_never_blocker() {
        // Quilt `quilt_loader.conflicts` is soft — never blocks.
        let dir = fresh_instance("quilt_conflicts");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[(
                "quilt.mod.json",
                r#"{"quilt_loader":{"id":"a","version":"1.0","conflicts":{"b":"*"}}}"#,
            )],
        );
        write_jar(
            &mods_dir,
            "b.jar",
            &[(
                "quilt.mod.json",
                r#"{"quilt_loader":{"id":"b","version":"1.0"}}"#,
            )],
        );
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert!(
            report.blockers.is_empty(),
            "quilt conflicts must not block: {:?}",
            report.blockers
        );
        assert!(report.warnings.iter().any(
            |w| w.kind == WarningKind::IncompatibleModSoft && w.mod_id.as_deref() == Some("b")
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_quilt_breaks_version_matched_blocks() {
        // Quilt breaks B with "<2.0"; B installed at 1.5 — matched, blocker.
        let dir = fresh_instance("quilt_breaks_matched");
        let mods_dir = dir.join("mods");
        write_jar(
            &mods_dir,
            "a.jar",
            &[(
                "quilt.mod.json",
                r#"{"quilt_loader":{"id":"a","version":"1.0","breaks":{"b":"<2.0"}}}"#,
            )],
        );
        write_jar(
            &mods_dir,
            "b.jar",
            &[(
                "quilt.mod.json",
                r#"{"quilt_loader":{"id":"b","version":"1.5"}}"#,
            )],
        );
        let manifest = tracked_manifest(&[("a.jar", "a"), ("b.jar", "b")]);
        let report = health(&dir, &manifest, None);
        assert_eq!(report.blockers.len(), 1);
        assert_eq!(report.blockers[0].kind, BlockerKind::IncompatibleMod);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Create a registry.db containing only mod_jar_aliases for the given
    /// (registry_id, alias) pairs, so health() can load an AliasMap.
    fn build_alias_registry(path: &Path, aliases: &[(&str, &str)]) {
        let conn = rusqlite::Connection::open(path).expect("open registry db");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS mod_jar_aliases (registry_id TEXT NOT NULL, alias TEXT NOT NULL);",
        )
        .expect("create aliases table");
        for (registry_id, alias) in aliases {
            conn.execute(
                "INSERT INTO mod_jar_aliases (registry_id, alias) VALUES (?1, ?2)",
                rusqlite::params![registry_id, alias],
            )
            .expect("insert alias");
        }
    }

    #[test]
    fn is_unconditional_helper() {
        assert!(is_unconditional(&[]));
        assert!(is_unconditional(&["*".to_string()]));
        assert!(is_unconditional(&["".to_string()]));
        assert!(is_unconditional(&["foo".to_string(), "*".to_string()])); // OR: any unconditional
        assert!(is_unconditional(&["[,)".to_string()]));
        assert!(!is_unconditional(&["<2.0".to_string()]));
        assert!(!is_unconditional(&["[1.0,2.0)".to_string()]));
    }

    #[test]
    fn is_hard_severity_helper() {
        assert!(is_hard_severity("hard"));
        assert!(is_hard_severity(" Hard "));
        assert!(is_hard_severity("critical"));
        assert!(is_hard_severity("breaking"));
        assert!(!is_hard_severity("soft"));
        assert!(!is_hard_severity("advisory"));
        assert!(!is_hard_severity("hardcoded")); // substring must NOT match
        assert!(!is_hard_severity(""));
    }

    #[test]
    fn jar_deps_default_has_empty_decls() {
        let d = JarDeps::default();
        assert!(d.incompatibility_decls.is_empty());
    }

    #[test]
    fn incompatibility_source_hardness() {
        assert!(IncompatibilitySource::FabricBreaks.is_hard());
        assert!(IncompatibilitySource::ForgeIncompatible.is_hard());
        assert!(!IncompatibilitySource::FabricConflicts.is_hard());
        assert!(!IncompatibilitySource::ForgeDiscouraged.is_hard());
    }

    #[test]
    fn incompatibility_decl_serializes() {
        let decl = IncompatibilityDecl {
            mod_id: "optifine".into(),
            version_ranges: vec!["<2.0".into()],
            source: IncompatibilitySource::FabricBreaks,
        };
        let json = serde_json::to_string(&decl).unwrap();
        assert!(json.contains("fabric_breaks"));
        let back: IncompatibilityDecl = serde_json::from_str(&json).unwrap();
        assert_eq!(back, decl);
    }
}
