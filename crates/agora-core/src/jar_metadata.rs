use crate::dependency_ops::{IncompatibilityDecl, IncompatibilitySource, JarDeps, ProvidedMod};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Seek};
use std::path::Path;

/// Loader/framework mod IDs that are part of the ecosystem but never present as
/// installable mod JARs. Declaring any of these as a dependency would produce a
/// false `MissingRequiredDependency` blocker, so they are filtered out.
const DEPENDENCY_IGNORE_LIST: &[&str] = &[
    "minecraft",
    "fabricloader",
    "quilt_loader",
    "java",
    "forge",
    "neoforge",
];

/// Maximum nesting depth for explicitly declared nested JARs.
const MAX_NESTING_DEPTH: u32 = 4;

/// Maximum total number of nested JARs across all nesting levels.
const MAX_TOTAL_NESTED_JARS: u32 = 128;

/// Maximum decompressed bytes for a single nested JAR entry.
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

/// Maximum total decompressed bytes across all nested JARs.
const MAX_TOTAL_NESTED_BYTES: u64 = 128 * 1024 * 1024;

/// Maximum bytes read from a single metadata text entry.
const MAX_METADATA_TEXT_BYTES: u64 = 1024 * 1024;

/// Shared budget tracker for nested JAR parsing limits.
#[derive(Default)]
struct NestedJarBudget {
    total_count: u32,
    total_bytes: u64,
}

/// Parse a `.jar` file to extract Java packages, mod ID, mod version, and
/// declared dependencies from `fabric.mod.json`, `quilt.mod.json`,
/// `META-INF/mods.toml` / `META-INF/neoforge.mods.toml`, and
/// `META-INF/MANIFEST.MF` (for `Implementation-Version`).
///
/// Also parses Fabric/Quilt `provides` aliases and explicitly declared nested
/// JAR entries (via `jars`), respecting depth/count/size bounds.
///
/// Returns [`JarDeps::default()`] on any error — never panics.
pub fn parse_jar_metadata(jar_path: &Path) -> JarDeps {
    let file = match std::fs::File::open(jar_path) {
        Ok(f) => f,
        Err(_) => return JarDeps::default(),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(_) => return JarDeps::default(),
    };
    let mut budget = NestedJarBudget::default();
    parse_from_archive(&mut archive, 0, &mut budget)
}

/// Generic recursive JAR metadata parser.
///
/// Works over any `R: Read + Seek` so it can be called with
/// `ZipArchive<File>` for the top-level JAR and `ZipArchive<Cursor<Vec<u8>>>`
/// for nested JARs extracted from parent entries.
fn parse_from_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    depth: u32,
    budget: &mut NestedJarBudget,
) -> JarDeps {
    if depth > MAX_NESTING_DEPTH {
        return JarDeps::default();
    }

    let total_len = archive.len();
    let mut packages: BTreeSet<String> = BTreeSet::new();
    let mut mod_jar_id: Option<String> = None;
    let mut mod_version: Option<String> = None;
    let mut depends_on: BTreeSet<String> = BTreeSet::new();
    let mut optional_deps: BTreeSet<String> = BTreeSet::new();
    let mut incompatible_ids: BTreeSet<String> = BTreeSet::new();
    let mut incompatibility_decls: Vec<IncompatibilityDecl> = Vec::new();
    let mut forge_mod_id: Option<String> = None;
    let mut forge_version: Option<String> = None;
    let mut manifest_impl_version: Option<String> = None;
    let mut saw_neoforge_toml = false;

    // Fabric/Quilt specific accumulators.
    let mut fabric_version: Option<String> = None;
    let mut fabric_provides_strs: Vec<String> = Vec::new();
    let mut fabric_jars_strs: Vec<String> = Vec::new();
    let mut quilt_version: Option<String> = None;
    let mut quilt_provides: Vec<(String, Option<String>)> = Vec::new();
    let mut quilt_jars_strs: Vec<String> = Vec::new();

    // ---- First pass: collect metadata and record declared nested paths ----
    for i in 0..total_len {
        let name = match archive.by_index(i) {
            Ok(e) => e.name().to_string(),
            Err(_) => continue,
        };
        if name.ends_with(".class") {
            let stem = match name.strip_suffix(".class") {
                Some(s) => s,
                None => continue,
            };
            let replaced = stem.replace('\\', "/");
            let segments: Vec<&str> = replaced.split('/').collect();
            if segments.len() < 3 {
                continue;
            }
            let dir_segments: Vec<&str> = segments[..segments.len() - 1].to_vec();
            packages.insert(dir_segments.join("."));
            continue;
        }
        if name == "fabric.mod.json" {
            if let Some(content_str) = read_entry_utf8_bounded(archive, i, MAX_METADATA_TEXT_BYTES)
            {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content_str) {
                    if let Some(id_str) = value.get("id").and_then(|v| v.as_str()) {
                        if !id_str.is_empty() {
                            mod_jar_id = Some(id_str.to_string());
                        }
                    }
                    if let Some(ver) = value.get("version").and_then(|v| v.as_str()) {
                        if !ver.is_empty() {
                            mod_version = Some(ver.to_string());
                            fabric_version = Some(ver.to_string());
                        }
                    }
                    // Required deps.
                    if let Some(val) = value.get("depends") {
                        extract_fabric_deps(val, &mut depends_on, None);
                    }
                    // Optional deps (recommends + suggests both soft).
                    for key in ["recommends", "suggests"] {
                        if let Some(val) = value.get(key) {
                            extract_fabric_deps(val, &mut optional_deps, None);
                        }
                    }
                    // breaks -> hard incompat; conflicts -> soft incompat.
                    if let Some(val) = value.get("breaks") {
                        extract_fabric_deps(
                            val,
                            &mut incompatible_ids,
                            Some((
                                IncompatibilitySource::FabricBreaks,
                                &mut incompatibility_decls,
                            )),
                        );
                    }
                    if let Some(val) = value.get("conflicts") {
                        extract_fabric_deps(
                            val,
                            &mut incompatible_ids,
                            Some((
                                IncompatibilitySource::FabricConflicts,
                                &mut incompatibility_decls,
                            )),
                        );
                    }
                    // Fabric `provides` array of strings.
                    if let Some(provides) = value.get("provides").and_then(|v| v.as_array()) {
                        for elem in provides {
                            if let Some(s) = elem.as_str() {
                                if !s.is_empty() {
                                    fabric_provides_strs.push(s.to_string());
                                }
                            }
                        }
                    }
                    // Fabric `jars` array of objects with `file` string.
                    if let Some(jars) = value.get("jars").and_then(|v| v.as_array()) {
                        for elem in jars {
                            if let Some(file) = elem.get("file").and_then(|v| v.as_str()) {
                                if !file.is_empty() {
                                    fabric_jars_strs.push(file.to_string());
                                }
                            }
                        }
                    }
                }
            }
            continue;
        }
        if name == "quilt.mod.json" {
            if let Some(content_str) = read_entry_utf8_bounded(archive, i, MAX_METADATA_TEXT_BYTES)
            {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content_str) {
                    if let Some(ql) = value.get("quilt_loader").or(value.get("quiltLoader")) {
                        if let Some(id_str) = ql.get("id").and_then(|v| v.as_str()) {
                            if !id_str.is_empty() && mod_jar_id.is_none() {
                                mod_jar_id = Some(id_str.to_string());
                            }
                        }
                        if let Some(ver) = ql.get("version").and_then(|v| v.as_str()) {
                            if !ver.is_empty() {
                                mod_version = Some(ver.to_string());
                                quilt_version = Some(ver.to_string());
                            }
                        }
                        // Quilt deps/breaks/conflicts use the same grammar as Fabric.
                        if let Some(val) = ql.get("depends") {
                            extract_fabric_deps(val, &mut depends_on, None);
                        }
                        if let Some(val) = ql.get("breaks") {
                            extract_fabric_deps(
                                val,
                                &mut incompatible_ids,
                                Some((
                                    IncompatibilitySource::QuiltBreaks,
                                    &mut incompatibility_decls,
                                )),
                            );
                        }
                        if let Some(val) = ql.get("conflicts") {
                            extract_fabric_deps(
                                val,
                                &mut incompatible_ids,
                                Some((
                                    IncompatibilitySource::QuiltConflicts,
                                    &mut incompatibility_decls,
                                )),
                            );
                        }
                        // Quilt `quilt_loader.provides`: array of strings or objects.
                        if let Some(provides) = ql.get("provides").and_then(|v| v.as_array()) {
                            for elem in provides {
                                if let Some(s) = elem.as_str() {
                                    if !s.is_empty() {
                                        quilt_provides.push((s.to_string(), None));
                                    }
                                } else if let Some(obj) = elem.as_object() {
                                    let pid = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                    if !pid.is_empty() {
                                        let pver = obj
                                            .get("version")
                                            .and_then(|v| v.as_str())
                                            .filter(|s| !s.is_empty())
                                            .map(|s| s.to_string());
                                        quilt_provides.push((pid.to_string(), pver));
                                    }
                                }
                            }
                        }
                        // Quilt `quilt_loader.jars`: array of string paths.
                        if let Some(jars) = ql.get("jars").and_then(|v| v.as_array()) {
                            for elem in jars {
                                if let Some(s) = elem.as_str() {
                                    if !s.is_empty() {
                                        quilt_jars_strs.push(s.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            continue;
        }
        // NeoForge ships neoforge.mods.toml; Forge ships mods.toml. Same TOML
        // schema, so both go through the same parser. Prefer neoforge when both
        // are present (a NeoForge mod's mods.toml, if also shipped, is usually
        // a stub).
        if name == "META-INF/neoforge.mods.toml" {
            saw_neoforge_toml = true;
            if let Some(content) = read_entry_utf8_bounded(archive, i, MAX_METADATA_TEXT_BYTES) {
                extract_forge_deps(
                    &content,
                    &mut depends_on,
                    &mut optional_deps,
                    &mut incompatible_ids,
                    &mut incompatibility_decls,
                    &mut forge_mod_id,
                    &mut forge_version,
                );
            }
            continue;
        }
        if name == "META-INF/mods.toml" && !saw_neoforge_toml {
            if let Some(content) = read_entry_utf8_bounded(archive, i, MAX_METADATA_TEXT_BYTES) {
                extract_forge_deps(
                    &content,
                    &mut depends_on,
                    &mut optional_deps,
                    &mut incompatible_ids,
                    &mut incompatibility_decls,
                    &mut forge_mod_id,
                    &mut forge_version,
                );
            }
            continue;
        }
        if name == "META-INF/MANIFEST.MF" {
            if let Some(content) = read_entry_utf8_bounded(archive, i, MAX_METADATA_TEXT_BYTES) {
                manifest_impl_version = parse_manifest_version(&content);
            }
            continue;
        }
    }

    // Resolve mod_jar_id and mod_version (same logic as original).
    if mod_jar_id.is_none() {
        mod_jar_id = forge_mod_id;
    }
    if mod_version.is_none() {
        mod_version = forge_version.take();
    }
    if mod_version.as_deref() == Some("${file.jarVersion}") {
        mod_version = manifest_impl_version.take();
    }
    if mod_version.is_none() {
        mod_version = manifest_impl_version;
    }

    // ---- Build initial provided_mods from Fabric/Quilt provides ----
    let mut provided_mods_map: BTreeMap<String, Option<String>> = BTreeMap::new();

    for alias in fabric_provides_strs {
        let ver = fabric_version.as_ref().cloned();
        provided_mods_map
            .entry(alias)
            .and_modify(|e| {
                if e.is_none() && ver.is_some() {
                    *e = ver.clone();
                }
            })
            .or_insert(ver);
    }

    for (pid, explicit_ver) in quilt_provides {
        let ver = explicit_ver.or_else(|| quilt_version.clone());
        provided_mods_map
            .entry(pid)
            .and_modify(|e| {
                if e.is_none() && ver.is_some() {
                    *e = ver.clone();
                }
            })
            .or_insert(ver);
    }

    // ---- Validate and dedupe declared nested JAR paths ----
    let nested_paths: BTreeSet<String> = fabric_jars_strs
        .into_iter()
        .chain(quilt_jars_strs.into_iter())
        .filter(|p| is_safe_nested_path(p))
        .map(|p| p.replace('\\', "/"))
        .collect();

    // ---- Second pass: read and parse declared nested JARs ----
    let mut processed_nested_paths: BTreeSet<String> = BTreeSet::new();
    for i in 0..total_len {
        let entry_name = match archive.by_index(i) {
            Ok(e) => e.name().to_string(),
            Err(_) => continue,
        };
        let normalized = entry_name.replace('\\', "/");
        if !nested_paths.contains(&normalized) || !processed_nested_paths.insert(normalized.clone())
        {
            continue;
        }

        if budget.total_count >= MAX_TOTAL_NESTED_JARS {
            break;
        }

        let bytes = read_entry_bytes_bounded(archive, i, MAX_ENTRY_BYTES);
        let bytes = match bytes {
            Some(b) if !b.is_empty() => b,
            _ => continue,
        };

        let entry_size = bytes.len() as u64;
        if budget.total_bytes + entry_size > MAX_TOTAL_NESTED_BYTES {
            continue;
        }
        budget.total_bytes += entry_size;
        budget.total_count += 1;

        let cursor = Cursor::new(bytes);
        let mut nested_archive = match zip::ZipArchive::new(cursor) {
            Ok(a) => a,
            Err(_) => continue,
        };
        let nested = parse_from_archive(&mut nested_archive, depth + 1, budget);

        // Add nested primary ID as ProvidedMod.
        if let Some(ref nested_id) = nested.mod_jar_id {
            let nv = nested.mod_version.clone();
            provided_mods_map
                .entry(nested_id.clone())
                .and_modify(|e| {
                    if e.is_none() && nv.is_some() {
                        *e = nv.clone();
                    }
                })
                .or_insert(nv);
        }

        // Aggregate nested provided_mods.
        for pm in &nested.provided_mods {
            let pv = pm.version.clone();
            provided_mods_map
                .entry(pm.mod_id.clone())
                .and_modify(|e| {
                    if e.is_none() && pv.is_some() {
                        *e = pv.clone();
                    }
                })
                .or_insert(pv);
        }

        // Aggregate dependencies (except intra-JAR deps filtered later).
        depends_on.extend(nested.depends_on);
        optional_deps.extend(nested.optional_deps);
        incompatible_ids.extend(nested.incompatible_deps);
        incompatibility_decls.extend(nested.incompatibility_decls);
        packages.extend(nested.java_packages);
    }

    // ---- After all nesting, filter intra-JAR dependencies ----
    let supplied_ids: BTreeSet<&str> = {
        let mut ids = BTreeSet::new();
        if let Some(ref id) = mod_jar_id {
            ids.insert(id.as_str());
        }
        for (id, _) in &provided_mods_map {
            ids.insert(id.as_str());
        }
        ids
    };

    depends_on.retain(|dep| !supplied_ids.contains(dep.as_str()));
    optional_deps.retain(|dep| !supplied_ids.contains(dep.as_str()));
    // Do NOT filter incompatible_deps or incompatibility_decls — an internally
    // bundled incompatibility is a real loader failure.

    // Apply DEPENDENCY_IGNORE_LIST.
    depends_on.retain(|dep| !DEPENDENCY_IGNORE_LIST.contains(&dep.as_str()));
    optional_deps.retain(|dep| !DEPENDENCY_IGNORE_LIST.contains(&dep.as_str()));
    incompatible_ids.retain(|dep| !DEPENDENCY_IGNORE_LIST.contains(&dep.as_str()));
    incompatibility_decls.retain(|d| !DEPENDENCY_IGNORE_LIST.contains(&d.mod_id.as_str()));

    // Build final ProvidedMod vec from map (already sorted by BTreeMap).
    let provided_mods: Vec<ProvidedMod> = provided_mods_map
        .into_iter()
        .map(|(mod_id, version)| ProvidedMod { mod_id, version })
        .collect();

    JarDeps {
        java_packages: packages.into_iter().collect(),
        mod_jar_id,
        mod_version,
        depends_on: depends_on.into_iter().collect(),
        optional_deps: optional_deps.into_iter().collect(),
        incompatible_deps: incompatible_ids.into_iter().collect(),
        incompatibility_decls,
        provided_mods,
    }
}

/// Validate that a declared nested JAR path is safe to read.
///
/// Rejects absolute paths, drive-prefixed paths, backslash-traversal patterns,
/// and any `..` path segment. Backslashes are normalized to forward slashes
/// for validation.
fn is_safe_nested_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");

    if normalized.is_empty() {
        return false;
    }

    // Reject absolute paths (start with /).
    if normalized.starts_with('/') {
        return false;
    }

    // Reject Windows drive-prefixed paths (e.g., "C:" or "c:").
    if normalized.len() >= 2 {
        let bytes = normalized.as_bytes();
        if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return false;
        }
    }

    // Reject any path segment that is ".."
    for segment in normalized.split('/') {
        if segment == ".." {
            return false;
        }
    }

    true
}

/// Read a text entry from the archive with a byte-size bound.
/// Returns `None` if the entry exceeds `max_bytes` or is not valid UTF-8.
fn read_entry_utf8_bounded<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    max_bytes: u64,
) -> Option<String> {
    let entry = archive.by_index(index).ok()?;
    let mut buf = Vec::new();
    entry.take(max_bytes + 1).read_to_end(&mut buf).ok()?;
    if buf.len() > max_bytes as usize {
        return None;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Read a binary entry from the archive with a byte-size bound.
/// Returns `None` if the entry exceeds `max_bytes`.
fn read_entry_bytes_bounded<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    max_bytes: u64,
) -> Option<Vec<u8>> {
    let entry = archive.by_index(index).ok()?;
    let mut buf = Vec::new();
    entry.take(max_bytes + 1).read_to_end(&mut buf).ok()?;
    if buf.len() > max_bytes as usize {
        return None;
    }
    Some(buf)
}

/// Extract Fabric dependency ids (and, for `breaks`/`conflicts`, structured
/// `IncompatibilityDecl`s carrying version-range predicates + severity).
///
/// - `out` receives the dep id strings (shared across depends/optional/incompat
///   flat lists depending on the caller).
/// - `incompat`: when `Some((severity, decls))`, also emit structured decls
///   capturing each version predicate. `None` for depends/recommends/suggests.
///
/// Fabric semantics:
/// - Object form `{"modid": "<2.0"}` → single AND predicate string.
/// - Object form `{"modid": ["<2.0", ">=3.0"]}` → OR array of predicate strings.
/// - Object form `{"modid": "*"}` → unconditional (any version).
/// - Array form `[{"id":..,"version":..}, {"identifier":..,"version":..}]` →
///   each object's `version` (may be absent) becomes a single-element range.
fn extract_fabric_deps(
    depends: &serde_json::Value,
    out: &mut BTreeSet<String>,
    mut incompat: Option<(IncompatibilitySource, &mut Vec<IncompatibilityDecl>)>,
) {
    match depends {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                let ranges = fabric_version_ranges(val);
                out.insert(key.clone());
                if let Some((sev, decls)) = incompat.as_mut() {
                    decls.push(IncompatibilityDecl {
                        mod_id: key.clone(),
                        version_ranges: ranges,
                        source: *sev,
                    });
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for elem in arr {
                let id = elem
                    .get("id")
                    .and_then(|v| v.as_str())
                    .or_else(|| elem.get("identifier").and_then(|v| v.as_str()));
                if let Some(id) = id {
                    out.insert(id.to_string());
                    if let Some((sev, decls)) = incompat.as_mut() {
                        let ranges = match elem.get("version") {
                            Some(v) => fabric_version_ranges(v),
                            None => Vec::new(),
                        };
                        decls.push(IncompatibilityDecl {
                            mod_id: id.to_string(),
                            version_ranges: ranges,
                            source: *sev,
                        });
                    }
                }
            }
        }
        _ => {}
    }
}

/// Normalize a Fabric version value into a list of OR-joined predicate strings.
/// - String → single predicate (may contain space-separated AND predicates).
/// - Array of strings → OR list.
/// - Anything else → empty (unconditional).
fn fabric_version_ranges(val: &serde_json::Value) -> Vec<String> {
    match val {
        serde_json::Value::String(s) => {
            let t = s.trim();
            if t == "*" || t.is_empty() {
                Vec::new()
            } else {
                vec![s.clone()]
            }
        }
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|e| match e {
                serde_json::Value::String(s) => {
                    let t = s.trim();
                    if t == "*" || t.is_empty() {
                        None
                    } else {
                        Some(s.clone())
                    }
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// In-flight Forge dependency block state.
#[derive(Default)]
struct PendingForgeDep {
    /// The TARGET mod id (the dependency), read from the inner `modId` line.
    mod_id: Option<String>,
    /// NeoForge `type`.
    dep_type: Option<String>,
    /// Traditional Forge `mandatory` (true=required, false=optional).
    mandatory: Option<bool>,
    /// `versionRange` (Maven range; empty string = any version).
    version_range: Option<String>,
}

/// Parse a Forge/NeoForge `mods.toml`/`neoforge.mods.toml` manifest.
///
/// Key fix: the section header `[[dependencies.<owner>]]` names the OWNER mod,
/// NOT the dependency. The dependency id is the `modId` line INSIDE the block.
/// Previously the parser stored the owner id as the dependency, which caused a
/// mod to appear to depend on / conflict with itself.
fn extract_forge_deps(
    content: &str,
    required_out: &mut BTreeSet<String>,
    optional_out: &mut BTreeSet<String>,
    incompatible_ids_out: &mut BTreeSet<String>,
    incompatibility_decls_out: &mut Vec<IncompatibilityDecl>,
    mod_id_out: &mut Option<String>,
    mod_version_out: &mut Option<String>,
) {
    let mut pending = PendingForgeDep::default();
    let mut in_dep_block = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("[[dependencies.") {
            // Flush previous block, then open a new one.
            flush_forge_dep(
                &pending,
                required_out,
                optional_out,
                incompatible_ids_out,
                incompatibility_decls_out,
            );
            pending = PendingForgeDep::default();
            in_dep_block = false;
            if let Some(end) = rest.find(']') {
                let block_key = &rest[..end];
                if !block_key.is_empty() {
                    in_dep_block = true;
                    // block_key is the OWNER; intentionally NOT stored as the
                    // dependency id. The dependency id comes from the inner
                    // `modId` line.
                }
            }
            continue;
        }
        if trimmed.starts_with("[[") {
            flush_forge_dep(
                &pending,
                required_out,
                optional_out,
                incompatible_ids_out,
                incompatibility_decls_out,
            );
            pending = PendingForgeDep::default();
            in_dep_block = false;
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // `key = value` pairs. Capture inside dep blocks; also capture the
        // top-level (file-level) `modId` for the jar's own mod id.
        if let Some((key, value)) = parse_toml_kv(trimmed) {
            match key.as_str() {
                "modid" => {
                    if in_dep_block {
                        pending.mod_id = Some(value);
                    } else if mod_id_out.is_none() {
                        *mod_id_out = Some(value);
                    }
                }
                "version" => {
                    if !in_dep_block && mod_version_out.is_none() {
                        *mod_version_out = Some(value);
                    }
                }
                "type" => {
                    if in_dep_block {
                        pending.dep_type = Some(value);
                    }
                }
                "mandatory" => {
                    if in_dep_block {
                        pending.mandatory = parse_toml_bool(&value);
                    }
                }
                "versionrange" => {
                    if in_dep_block {
                        pending.version_range = Some(value);
                    }
                }
                _ => {}
            }
        }
    }
    flush_forge_dep(
        &pending,
        required_out,
        optional_out,
        incompatible_ids_out,
        incompatibility_decls_out,
    );
}

/// Finalize a pending Forge dependency block, routing it to the right buckets.
fn flush_forge_dep(
    pending: &PendingForgeDep,
    required_out: &mut BTreeSet<String>,
    optional_out: &mut BTreeSet<String>,
    incompatible_ids_out: &mut BTreeSet<String>,
    incompatibility_decls_out: &mut Vec<IncompatibilityDecl>,
) {
    let dep_id = match &pending.mod_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return,
    };

    let ranges = match &pending.version_range {
        Some(r) => {
            let t = r.trim();
            if t.is_empty() || t == "*" {
                Vec::new()
            } else {
                vec![r.clone()]
            }
        }
        None => Vec::new(),
    };

    let effective = pending.dep_type.as_deref();
    match effective {
        Some("incompatible") => {
            incompatible_ids_out.insert(dep_id.clone());
            incompatibility_decls_out.push(IncompatibilityDecl {
                mod_id: dep_id,
                version_ranges: ranges,
                source: IncompatibilitySource::ForgeIncompatible,
            });
        }
        Some("discouraged") => {
            incompatible_ids_out.insert(dep_id.clone());
            incompatibility_decls_out.push(IncompatibilityDecl {
                mod_id: dep_id,
                version_ranges: ranges,
                source: IncompatibilitySource::ForgeDiscouraged,
            });
        }
        Some("optional") => {
            optional_out.insert(dep_id);
        }
        Some("required") | Some(_) => {
            required_out.insert(dep_id);
        }
        None => match pending.mandatory {
            Some(false) => {
                optional_out.insert(dep_id);
            }
            _ => {
                required_out.insert(dep_id);
            }
        },
    }
}

/// Parse a `key = "value"` / `key = value` / `key = true` line into (key, value).
fn parse_toml_kv(trimmed: &str) -> Option<(String, String)> {
    let eq = trimmed.find('=')?;
    let key = trimmed[..eq].trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    let raw = trimmed[eq + 1..].trim();
    let value = if let Some(rest) = raw.strip_prefix(['"', '\'']) {
        match rest.split(['"', '\'']).next() {
            Some(v) => v.to_string(),
            None => rest.to_string(),
        }
    } else {
        let v = raw.split_whitespace().next().unwrap_or("");
        v.to_string()
    };
    Some((key, value))
}

/// Parse a TOML boolean value string.
fn parse_toml_bool(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Parse a `META-INF/MANIFEST.MF` file and extract the
/// `Implementation-Version` attribute value (if present).
fn parse_manifest_version(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let colon = line.find(':')?;
        let key = line[..colon].trim();
        if key.eq_ignore_ascii_case("Implementation-Version") {
            let val = line[colon + 1..].trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an in-memory `.jar` (zip) with the given `(entry_name, content)`
    /// pairs and write it to a unique temp file, returning the path.
    fn build_test_jar(entries: &[(&str, &str)]) -> std::path::PathBuf {
        use std::io::{Seek, Write};
        let mut file = tempfile::NamedTempFile::new().expect("create temp file");
        {
            let mut zip = zip::ZipWriter::new(&file);
            let opts = zip::write::FileOptions::default();
            for (name, content) in entries {
                zip.start_file(*name, opts).expect("start_file");
                zip.write_all(content.as_bytes()).expect("write_all");
            }
            zip.finish().expect("finish zip");
        }
        file.seek(std::io::SeekFrom::Start(0)).expect("rewind");
        let (_file, path) = file.keep().expect("keep temp file");
        path
    }

    /// Build an in-memory `.jar` (zip) with binary entries and write to a
    /// unique temp file, returning the path.
    fn build_test_jar_binary(entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        use std::io::{Seek, Write};
        let mut file = tempfile::NamedTempFile::new().expect("create temp file");
        {
            let mut zip = zip::ZipWriter::new(&file);
            let opts = zip::write::FileOptions::default();
            for (name, content) in entries {
                zip.start_file(*name, opts).expect("start_file");
                zip.write_all(content).expect("write_all");
            }
            zip.finish().expect("finish zip");
        }
        file.seek(std::io::SeekFrom::Start(0)).expect("rewind");
        let (_file, path) = file.keep().expect("keep temp file");
        path
    }

    /// Build a JAR (zip) entirely in memory and return its raw bytes.
    fn build_jar_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::FileOptions::default();
            for (name, content) in entries {
                zip.start_file(*name, opts).expect("start_file");
                zip.write_all(content).expect("write_all");
            }
            zip.finish().expect("finish zip");
        }
        buf.into_inner()
    }

    #[test]
    fn parse_jar_metadata_missing_file_returns_default() {
        let meta = parse_jar_metadata(std::path::Path::new("/nonexistent/jar.jar"));
        assert!(meta.java_packages.is_empty());
        assert!(meta.mod_jar_id.is_none());
        assert!(meta.depends_on.is_empty());
        assert!(meta.incompatibility_decls.is_empty());
        assert!(meta.provided_mods.is_empty());
    }

    // -------------------------------------------------------------------
    // Fabric provides alias tests
    // -------------------------------------------------------------------

    #[test]
    fn fabric_provides_string_aliases_captured_with_outer_version() {
        let jar = build_test_jar(&[(
            "fabric.mod.json",
            r#"{"id":"a","version":"1.0","provides":["b","c"]}"#,
        )]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        assert_eq!(meta.mod_jar_id.as_deref(), Some("a"));
        assert_eq!(meta.mod_version.as_deref(), Some("1.0"));
        // Both aliases with the outer version.
        let b = meta
            .provided_mods
            .iter()
            .find(|p| p.mod_id == "b")
            .expect("b should be provided");
        assert_eq!(b.version.as_deref(), Some("1.0"));
        let c = meta
            .provided_mods
            .iter()
            .find(|p| p.mod_id == "c")
            .expect("c should be provided");
        assert_eq!(c.version.as_deref(), Some("1.0"));
    }

    // -------------------------------------------------------------------
    // Quilt provides tests
    // -------------------------------------------------------------------

    #[test]
    fn quilt_provides_string_and_object_forms() {
        let jar = build_test_jar(&[(
            "quilt.mod.json",
            r#"{"quilt_loader":{"id":"q","version":"2.0","provides":["a",{"id":"b","version":"3.0"},{"id":"c"}]}}"#,
        )]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        assert_eq!(meta.mod_jar_id.as_deref(), Some("q"));
        // 'a' is a plain string → defaults to outer version.
        let a = meta
            .provided_mods
            .iter()
            .find(|p| p.mod_id == "a")
            .expect("a should be provided");
        assert_eq!(a.version.as_deref(), Some("2.0"));
        // 'b' has explicit version.
        let b = meta
            .provided_mods
            .iter()
            .find(|p| p.mod_id == "b")
            .expect("b should be provided");
        assert_eq!(b.version.as_deref(), Some("3.0"));
        // 'c' has no explicit version → defaults to outer version.
        let c = meta
            .provided_mods
            .iter()
            .find(|p| p.mod_id == "c")
            .expect("c should be provided");
        assert_eq!(c.version.as_deref(), Some("2.0"));
    }

    // -------------------------------------------------------------------
    // Nested JAR tests
    // -------------------------------------------------------------------

    #[test]
    fn fabric_nested_jar_primary_id_and_deps_aggregate() {
        // Build inner JAR bytes.
        let inner_bytes = build_jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"inner","version":"2.0","depends":{"req":"1.0"}}"# as &[u8],
        )]);

        let jar = build_test_jar_binary(&[
            (
                "fabric.mod.json",
                br#"{"id":"outer","version":"1.0","jars":[{"file":"nested.jar"}]}"# as &[u8],
            ),
            ("nested.jar", &inner_bytes),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        assert_eq!(meta.mod_jar_id.as_deref(), Some("outer"));
        // Nested primary ID appears in provided_mods.
        let inner = meta
            .provided_mods
            .iter()
            .find(|p| p.mod_id == "inner")
            .expect("inner should be provided");
        assert_eq!(inner.version.as_deref(), Some("2.0"));
        // Nested dep "req" should be in depends_on.
        assert!(meta.depends_on.contains(&"req".to_string()));
    }

    #[test]
    fn fabric_nested_jar_intra_jar_dep_removed() {
        // Outer depends on "inner" but inner supplies that ID.
        let inner_bytes = build_jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"inner","version":"1.0"}"# as &[u8],
        )]);

        let jar = build_test_jar_binary(&[
            (
                "fabric.mod.json",
                br#"{"id":"outer","version":"1.0","depends":{"inner":"1.0"},"jars":[{"file":"nested.jar"}]}"#
                    as &[u8],
            ),
            ("nested.jar", &inner_bytes),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        // "inner" is supplied by the same physical JAR (via the nested JAR),
        // so it should NOT appear in depends_on.
        assert!(
            !meta.depends_on.contains(&"inner".to_string()),
            "intra-JAR dep must be filtered out"
        );
    }

    #[test]
    fn nested_jar_provided_mods_propagate() {
        // Inner JAR has its own provides aliases.
        let inner_bytes = build_jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"core","version":"2.0","provides":["core_api","core_utils"]}"# as &[u8],
        )]);

        let jar = build_test_jar_binary(&[
            (
                "fabric.mod.json",
                br#"{"id":"wrapper","version":"1.0","jars":[{"file":"inner.jar"}]}"# as &[u8],
            ),
            ("inner.jar", &inner_bytes),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        assert_eq!(meta.mod_jar_id.as_deref(), Some("wrapper"));
        // Inner's primary ID propagates.
        assert!(meta.provided_mods.iter().any(|p| p.mod_id == "core"));
        // Inner's provides aliases propagate.
        assert!(meta.provided_mods.iter().any(|p| p.mod_id == "core_api"));
        assert!(meta.provided_mods.iter().any(|p| p.mod_id == "core_utils"));
    }

    #[test]
    fn quilt_string_jars_path_works() {
        let inner_bytes = build_jar_bytes(&[(
            "quilt.mod.json",
            br#"{"quilt_loader":{"id":"qchild","version":"1.5"}}"# as &[u8],
        )]);

        let jar = build_test_jar_binary(&[
            (
                "quilt.mod.json",
                br#"{"quilt_loader":{"id":"qparent","version":"1.0","jars":["child.jar"]}}"#
                    as &[u8],
            ),
            ("child.jar", &inner_bytes),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        assert_eq!(meta.mod_jar_id.as_deref(), Some("qparent"));
        assert!(meta.provided_mods.iter().any(|p| p.mod_id == "qchild"));
        let qc = meta
            .provided_mods
            .iter()
            .find(|p| p.mod_id == "qchild")
            .expect("qchild should be provided");
        assert_eq!(qc.version.as_deref(), Some("1.5"));
    }

    // -------------------------------------------------------------------
    // Path security tests
    // -------------------------------------------------------------------

    #[test]
    fn undeclared_embedded_jar_ignored() {
        // A .jar entry exists but is NOT declared in `jars` → must be ignored.
        let undeclared_bytes = build_jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"sneaky","version":"9.9"}"# as &[u8],
        )]);

        let jar = build_test_jar_binary(&[
            (
                "fabric.mod.json",
                br#"{"id":"main","version":"1.0"}"# as &[u8],
            ),
            ("hidden.jar", &undeclared_bytes),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        assert_eq!(meta.mod_jar_id.as_deref(), Some("main"));
        assert!(
            !meta.provided_mods.iter().any(|p| p.mod_id == "sneaky"),
            "undeclared nested JAR must not be parsed"
        );
    }

    #[test]
    fn unsafe_dotdot_nested_path_ignored() {
        let resolved_bytes = build_jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"resolved","version":"1.0"}"# as &[u8],
        )]);

        let jar = build_test_jar_binary(&[
            (
                "fabric.mod.json",
                br#"{"id":"main","version":"1.0","jars":[{"file":"../nested.jar"}]}"# as &[u8],
            ),
            // The entry exists at the unsafe path — it should still be rejected.
            ("../nested.jar", &resolved_bytes),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        assert_eq!(meta.mod_jar_id.as_deref(), Some("main"));
        assert!(
            !meta.provided_mods.iter().any(|p| p.mod_id == "resolved"),
            "path with '..' segment must be rejected"
        );
    }

    // -------------------------------------------------------------------
    // Depth guard test
    // -------------------------------------------------------------------

    #[test]
    fn nested_depth_greater_than_4_terminates() {
        // Build a chain: level0.jar -> level1.jar -> level2.jar -> level3.jar
        // -> level4.jar -> level5.jar (exceeds max depth 4).
        // Only levels 0-4 should be exposed.
        let level5_bytes = build_jar_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"level5","version":"5.0"}"# as &[u8],
        )]);
        let level4_bytes = build_jar_bytes(&[
            (
                "fabric.mod.json",
                br#"{"id":"level4","version":"4.0","jars":[{"file":"dirty.jar"}]}"# as &[u8],
            ),
            ("dirty.jar", &level5_bytes),
        ]);
        let level3_bytes = build_jar_bytes(&[
            (
                "fabric.mod.json",
                br#"{"id":"level3","version":"3.0","jars":[{"file":"l4.jar"}]}"# as &[u8],
            ),
            ("l4.jar", &level4_bytes),
        ]);
        let level2_bytes = build_jar_bytes(&[
            (
                "fabric.mod.json",
                br#"{"id":"level2","version":"2.0","jars":[{"file":"l3.jar"}]}"# as &[u8],
            ),
            ("l3.jar", &level3_bytes),
        ]);
        let level1_bytes = build_jar_bytes(&[
            (
                "fabric.mod.json",
                br#"{"id":"level1","version":"1.0","jars":[{"file":"l2.jar"}]}"# as &[u8],
            ),
            ("l2.jar", &level2_bytes),
        ]);

        let jar = build_test_jar_binary(&[
            (
                "fabric.mod.json",
                br#"{"id":"level0","version":"0.0","jars":[{"file":"l1.jar"}]}"# as &[u8],
            ),
            ("l1.jar", &level1_bytes),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);

        assert_eq!(meta.mod_jar_id.as_deref(), Some("level0"));
        // Levels 1-4 should be visible.
        assert!(meta.provided_mods.iter().any(|p| p.mod_id == "level1"));
        assert!(meta.provided_mods.iter().any(|p| p.mod_id == "level2"));
        assert!(meta.provided_mods.iter().any(|p| p.mod_id == "level3"));
        assert!(meta.provided_mods.iter().any(|p| p.mod_id == "level4"));
        // Level 5 exceeds max depth and must NOT appear.
        assert!(
            !meta.provided_mods.iter().any(|p| p.mod_id == "level5"),
            "depth >4 must not expose deepest ID"
        );
    }

    // -------------------------------------------------------------------
    // JarDeps::default serde test for provided_mods
    // -------------------------------------------------------------------

    #[test]
    fn jar_deps_default_serde_roundtrip() {
        // Default should serialize and deserialize correctly with provided_mods.
        let default = JarDeps::default();
        let json = serde_json::to_string(&default).expect("serialize");
        let restored: JarDeps = serde_json::from_str(&json).expect("deserialize");
        assert!(restored.provided_mods.is_empty());
        assert!(restored.mod_jar_id.is_none());
        assert!(restored.depends_on.is_empty());
    }

    #[test]
    fn jar_deps_deserialize_missing_provided_mods_defaults_empty() {
        // JSON without provided_mods field should work (serde default).
        let json = r#"{"java_packages":[],"mod_jar_id":null,"depends_on":[],"optional_deps":[],"incompatible_deps":[],"incompatibility_decls":[],"mod_version":null}"#;
        let restored: JarDeps = serde_json::from_str(json).expect("deserialize");
        assert!(restored.provided_mods.is_empty());
    }

    // -------------------------------------------------------------------
    // Existing tests below — kept unchanged
    // -------------------------------------------------------------------

    #[test]
    fn extract_fabric_deps_object_form() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"fabric-api": ">=0.40.0", "minecraft": ">=1.20"}"#).unwrap();
        let mut out = BTreeSet::new();
        extract_fabric_deps(&v, &mut out, None);
        assert!(out.contains("fabric-api"));
        assert!(out.contains("minecraft"));
    }

    #[test]
    fn extract_fabric_deps_array_form() {
        let v: serde_json::Value =
            serde_json::from_str(r#"[{"id": "sodium"}, {"identifier": "lithium"}]"#).unwrap();
        let mut out = BTreeSet::new();
        extract_fabric_deps(&v, &mut out, None);
        assert!(out.contains("sodium"));
        assert!(out.contains("lithium"));
    }

    #[test]
    fn fabric_breaks_captured_as_hard_with_predicate() {
        let v: serde_json::Value = serde_json::from_str(r#"{"optifine": "<2.0"}"#).unwrap();
        let mut ids = BTreeSet::new();
        let mut decls = Vec::new();
        extract_fabric_deps(
            &v,
            &mut ids,
            Some((IncompatibilitySource::FabricBreaks, &mut decls)),
        );
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].mod_id, "optifine");
        assert_eq!(decls[0].version_ranges, vec!["<2.0".to_string()]);
        assert_eq!(decls[0].source, IncompatibilitySource::FabricBreaks);
        assert!(decls[0].source.is_hard());
    }

    #[test]
    fn fabric_conflicts_is_soft() {
        let v: serde_json::Value = serde_json::from_str(r#"{"foo": "*"}"#).unwrap();
        let mut decls = Vec::new();
        let mut ids = BTreeSet::new();
        extract_fabric_deps(
            &v,
            &mut ids,
            Some((IncompatibilitySource::FabricConflicts, &mut decls)),
        );
        assert_eq!(decls.len(), 1);
        assert!(!decls[0].source.is_hard());
        assert!(decls[0].version_ranges.is_empty());
    }

    #[test]
    fn fabric_array_predicates_captured_as_or() {
        let v: serde_json::Value = serde_json::from_str(r#"{"foo": ["<2.0", ">=3.0"]}"#).unwrap();
        let mut decls = Vec::new();
        let mut ids = BTreeSet::new();
        extract_fabric_deps(
            &v,
            &mut ids,
            Some((IncompatibilitySource::FabricBreaks, &mut decls)),
        );
        assert_eq!(decls[0].version_ranges, vec!["<2.0", ">=3.0"]);
    }

    #[test]
    fn fabric_array_of_objects_form() {
        let v: serde_json::Value =
            serde_json::from_str(r#"[{"id":"foo","version":"<2.0"},{"id":"bar"}]"#).unwrap();
        let mut decls = Vec::new();
        let mut ids = BTreeSet::new();
        extract_fabric_deps(
            &v,
            &mut ids,
            Some((IncompatibilitySource::FabricBreaks, &mut decls)),
        );
        let foo = decls.iter().find(|d| d.mod_id == "foo").unwrap();
        assert_eq!(foo.version_ranges, vec!["<2.0"]);
        let bar = decls.iter().find(|d| d.mod_id == "bar").unwrap();
        assert!(bar.version_ranges.is_empty());
    }

    // -------------------------------------------------------------------
    // Forge/NeoForge parsing
    // -------------------------------------------------------------------

    #[test]
    fn forge_dep_block_reads_inner_modid_not_owner() {
        let toml = r#"modId="mymod"
version="1.0"

[[dependencies.mymod]]
    modId="fabric-api"
    type="required"

[[dependencies.mymod]]
    modId="sodium"
    type="optional"
"#;
        let mut required = BTreeSet::new();
        let mut optional = BTreeSet::new();
        let mut incompat_ids = BTreeSet::new();
        let mut decls = Vec::new();
        let mut mod_id = None;
        let mut mod_version = None;
        extract_forge_deps(
            &toml,
            &mut required,
            &mut optional,
            &mut incompat_ids,
            &mut decls,
            &mut mod_id,
            &mut mod_version,
        );
        assert_eq!(mod_id, Some("mymod".to_string()));
        assert!(required.contains("fabric-api"));
        assert!(!required.contains("mymod"), "owner must NOT be its own dep");
        assert!(optional.contains("sodium"));
    }

    #[test]
    fn forge_mandatory_false_is_optional() {
        let toml = r#"[[dependencies.foo]]
    modId="bar"
    mandatory=false
"#;
        let mut required = BTreeSet::new();
        let mut optional = BTreeSet::new();
        let mut incompat_ids = BTreeSet::new();
        let mut decls = Vec::new();
        let mut mod_id = None;
        let mut mod_version = None;
        extract_forge_deps(
            &toml,
            &mut required,
            &mut optional,
            &mut incompat_ids,
            &mut decls,
            &mut mod_id,
            &mut mod_version,
        );
        assert!(optional.contains("bar"));
        assert!(!required.contains("bar"));
    }

    #[test]
    fn forge_mandatory_true_is_required() {
        let toml = r#"[[dependencies.foo]]
    modId="bar"
    mandatory=true
"#;
        let mut required = BTreeSet::new();
        let mut optional = BTreeSet::new();
        let mut incompat_ids = BTreeSet::new();
        let mut decls = Vec::new();
        let mut mod_id = None;
        let mut mod_version = None;
        extract_forge_deps(
            &toml,
            &mut required,
            &mut optional,
            &mut incompat_ids,
            &mut decls,
            &mut mod_id,
            &mut mod_version,
        );
        assert!(required.contains("bar"));
    }

    #[test]
    fn forge_incompatible_uses_target_id_and_captures_version_range() {
        let toml = r#"[[dependencies.mymod]]
    modId="optifine"
    type="incompatible"
    versionRange="[1.0,2.0)"
"#;
        let mut required = BTreeSet::new();
        let mut optional = BTreeSet::new();
        let mut incompat_ids = BTreeSet::new();
        let mut decls = Vec::new();
        let mut mod_id = None;
        let mut mod_version = None;
        extract_forge_deps(
            &toml,
            &mut required,
            &mut optional,
            &mut incompat_ids,
            &mut decls,
            &mut mod_id,
            &mut mod_version,
        );
        assert!(incompat_ids.contains("optifine"));
        assert!(
            !incompat_ids.contains("mymod"),
            "owner must not self-conflict"
        );
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].mod_id, "optifine");
        assert_eq!(decls[0].version_ranges, vec!["[1.0,2.0)".to_string()]);
        assert_eq!(decls[0].source, IncompatibilitySource::ForgeIncompatible);
    }

    #[test]
    fn forge_discouraged_is_soft() {
        let toml = r#"[[dependencies.mymod]]
    modId="bar"
    type="discouraged"
"#;
        let mut required = BTreeSet::new();
        let mut optional = BTreeSet::new();
        let mut incompat_ids = BTreeSet::new();
        let mut decls = Vec::new();
        let mut mod_id = None;
        let mut mod_version = None;
        extract_forge_deps(
            &toml,
            &mut required,
            &mut optional,
            &mut incompat_ids,
            &mut decls,
            &mut mod_id,
            &mut mod_version,
        );
        assert_eq!(decls.len(), 1);
        assert!(!decls[0].source.is_hard());
    }

    #[test]
    fn forge_empty_version_range_is_unconditional() {
        let toml = r#"[[dependencies.mymod]]
    modId="bar"
    type="incompatible"
    versionRange=""
"#;
        let mut required = BTreeSet::new();
        let mut optional = BTreeSet::new();
        let mut incompat_ids = BTreeSet::new();
        let mut decls = Vec::new();
        let mut mod_id = None;
        let mut mod_version = None;
        extract_forge_deps(
            &toml,
            &mut required,
            &mut optional,
            &mut incompat_ids,
            &mut decls,
            &mut mod_id,
            &mut mod_version,
        );
        assert!(decls[0].version_ranges.is_empty());
    }

    #[test]
    fn forge_dep_block_without_modid_is_skipped() {
        let toml = r#"[[dependencies.someowner]]
    type="required"
"#;
        let mut required = BTreeSet::new();
        let mut optional = BTreeSet::new();
        let mut incompat_ids = BTreeSet::new();
        let mut decls = Vec::new();
        let mut mod_id = None;
        let mut mod_version = None;
        extract_forge_deps(
            &toml,
            &mut required,
            &mut optional,
            &mut incompat_ids,
            &mut decls,
            &mut mod_id,
            &mut mod_version,
        );
        assert!(required.is_empty());
        assert!(incompat_ids.is_empty());
        assert!(decls.is_empty());
    }

    // -------------------------------------------------------------------
    // Full JAR parse (zip fixture) end-to-end
    // -------------------------------------------------------------------

    #[test]
    fn parse_jar_fabric_breaks_and_conflicts() {
        let jar = build_test_jar(&[(
            "fabric.mod.json",
            r#"{"id":"mod_a","breaks":{"bad":"<2.0"},"conflicts":{"iffy":"*"}, "depends":{"fabric-api":">=0.40"}}"#,
        )]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert_eq!(meta.mod_jar_id.as_deref(), Some("mod_a"));
        assert!(meta.depends_on.contains(&"fabric-api".to_string()));
        assert!(meta.incompatible_deps.contains(&"bad".to_string()));
        assert!(meta.incompatible_deps.contains(&"iffy".to_string()));
        let bad = meta
            .incompatibility_decls
            .iter()
            .find(|d| d.mod_id == "bad")
            .unwrap();
        assert_eq!(bad.source, IncompatibilitySource::FabricBreaks);
        assert_eq!(bad.version_ranges, vec!["<2.0".to_string()]);
        let iffy = meta
            .incompatibility_decls
            .iter()
            .find(|d| d.mod_id == "iffy")
            .unwrap();
        assert_eq!(iffy.source, IncompatibilitySource::FabricConflicts);
        assert!(iffy.version_ranges.is_empty());
    }

    #[test]
    fn parse_jar_neoforge_mods_toml_parsed_and_inner_modid_read() {
        let jar = build_test_jar(&[
            (
                "META-INF/neoforge.mods.toml",
                "modId=\"neomod\"\n\n[[dependencies.neomod]]\n    modId=\"optifine\"\n    type=\"incompatible\"\n",
            ),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert_eq!(meta.mod_jar_id.as_deref(), Some("neomod"));
        assert!(meta.incompatible_deps.contains(&"optifine".to_string()));
        assert!(
            !meta.incompatible_deps.contains(&"neomod".to_string()),
            "owner must not self-conflict"
        );
        let optifine = meta
            .incompatibility_decls
            .iter()
            .find(|d| d.mod_id == "optifine")
            .expect("optifine decl");
        assert_eq!(optifine.source, IncompatibilitySource::ForgeIncompatible);
    }

    #[test]
    fn parse_jar_forge_self_conflict_bug_fixed() {
        let jar = build_test_jar(&[
            (
                "META-INF/mods.toml",
                "modId=\"examplemod\"\n[[dependencies.examplemod]]\n    modId=\"othermod\"\n    type=\"incompatible\"\n",
            ),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert!(meta.incompatible_deps.contains(&"othermod".to_string()));
        assert!(
            !meta.incompatible_deps.contains(&"examplemod".to_string()),
            "examplemod must not appear incompatible with itself"
        );
    }

    #[test]
    fn parse_jar_forge_neoforge_loader_deps_ignored() {
        let jar = build_test_jar(&[
            (
                "META-INF/neoforge.mods.toml",
                "modId=\"m\"\n[[dependencies.m]]\n    modId=\"neoforge\"\n    type=\"required\"\n[[dependencies.m]]\n    modId=\"realdep\"\n    type=\"required\"\n",
            ),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert!(!meta.depends_on.contains(&"neoforge".to_string()));
        assert!(meta.depends_on.contains(&"realdep".to_string()));
    }

    #[test]
    fn parse_fabric_version_is_extracted() {
        let jar = build_test_jar(&[("fabric.mod.json", r#"{"id":"a","version":"1.2.3-build.4"}"#)]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert_eq!(meta.mod_version.as_deref(), Some("1.2.3-build.4"));
    }

    #[test]
    fn parse_forge_version_placeholder_resolves_to_manifest() {
        let jar = build_test_jar(&[
            (
                "META-INF/mods.toml",
                "modId=\"m\"\nversion=\"${file.jarVersion}\"\n",
            ),
            (
                "META-INF/MANIFEST.MF",
                "Manifest-Version: 1.0\nImplementation-Version: 3.7.2\n",
            ),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert_eq!(meta.mod_version.as_deref(), Some("3.7.2"));
    }

    #[test]
    fn parse_forge_explicit_version_used_directly() {
        let jar = build_test_jar(&[
            ("META-INF/mods.toml", "modId=\"m\"\nversion=\"2.1.0\"\n"),
            (
                "META-INF/MANIFEST.MF",
                "Manifest-Version: 1.0\nImplementation-Version: 9.9.9\n",
            ),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert_eq!(meta.mod_version.as_deref(), Some("2.1.0"));
    }

    #[test]
    fn parse_forge_no_version_falls_back_to_manifest_only() {
        let jar = build_test_jar(&[
            ("META-INF/mods.toml", "modId=\"m\"\n"),
            (
                "META-INF/MANIFEST.MF",
                "Manifest-Version: 1.0\nImplementation-Version: 0.4.1\n",
            ),
        ]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert_eq!(meta.mod_version.as_deref(), Some("0.4.1"));
    }

    #[test]
    fn parse_quilt_mod_id_and_version_extracted() {
        let jar = build_test_jar(&[(
            "quilt.mod.json",
            r#"{"quilt_loader":{"id":"qmod","version":"5.6.7"}}"#,
        )]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert_eq!(meta.mod_jar_id.as_deref(), Some("qmod"));
        assert_eq!(meta.mod_version.as_deref(), Some("5.6.7"));
    }

    #[test]
    fn parse_quilt_breaks_and_conflicts_captured() {
        let jar = build_test_jar(&[(
            "quilt.mod.json",
            r#"{"quilt_loader":{"id":"qmod","version":"1.0","breaks":{"target_a":"<2.0"},"conflicts":{"target_b":"*"}}}"#,
        )]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert!(meta.incompatible_deps.contains(&"target_a".to_string()));
        assert!(meta.incompatible_deps.contains(&"target_b".to_string()));
        let a_decl = meta
            .incompatibility_decls
            .iter()
            .find(|d| d.mod_id == "target_a")
            .expect("expected target_a decl");
        assert_eq!(a_decl.source, IncompatibilitySource::QuiltBreaks);
        let b_decl = meta
            .incompatibility_decls
            .iter()
            .find(|d| d.mod_id == "target_b")
            .expect("expected target_b decl");
        assert_eq!(b_decl.source, IncompatibilitySource::QuiltConflicts);
        assert!(a_decl.source.is_hard());
        assert!(!b_decl.source.is_hard());
        assert!(a_decl.source.is_fabric_grammar());
        assert!(b_decl.source.is_fabric_grammar());
    }

    #[test]
    fn parse_quilt_depends_collected_as_required() {
        let jar = build_test_jar(&[(
            "quilt.mod.json",
            r#"{"quilt_loader":{"id":"qmod","version":"1.0","depends":{"needed_dep":">=1.0"}}}"#,
        )]);
        let meta = parse_jar_metadata(&jar);
        let _ = std::fs::remove_file(&jar);
        assert!(meta.depends_on.contains(&"needed_dep".to_string()));
    }
}
