//! Integration tests for the `agora` CLI binary.
//!
//! These tests execute the compiled `agora` binary against a temporary
//! `--data-dir` to verify paths, settings, instance list/show, JSON output
//! purity, error envelopes, exit codes, and import registration.  Tests
//! avoid network access and never touch the user's real data root.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Path to the compiled `agora` binary, set by Cargo for integration tests.
const AGORA_BIN: &str = env!("CARGO_BIN_EXE_agora");

/// Helper: run `agora --data-dir <dir>` with the given args.
fn run_agora(data_dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(AGORA_BIN);
    cmd.arg("--data-dir").arg(data_dir);
    cmd.args(args);
    cmd.output().expect("failed to execute agora binary")
}

/// Helper: run with `--json` flag prepended.
fn run_agora_json(data_dir: &Path, args: &[&str]) -> std::process::Output {
    let mut full_args: Vec<&str> = vec!["--json"];
    full_args.extend_from_slice(args);
    run_agora(data_dir, &full_args)
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir() -> TempDir {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "agora-cli-integration-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("failed to create temp dir");
    TempDir { path }
}

/// Create a temp dir for use as `--data-dir`.
fn temp_data_dir() -> (TempDir, PathBuf) {
    let tmp = tempdir();
    let path = tmp.path().to_path_buf();
    (tmp, path)
}

/// Assert stdout is valid JSON and return the parsed value.
fn assert_json_stdout(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be valid JSON: {e}\nstdout:\n{stdout}"))
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

#[test]
fn paths_human_shows_all_expected_keys() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["paths"]);
    assert!(output.status.success(), "agora paths should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("root: {}", data_dir.display())),
        "stdout should contain root: {}\n{}",
        data_dir.display(),
        stdout,
    );
    assert!(stdout.contains("local_state_db:"));
    assert!(stdout.contains("instances:"));
    assert!(stdout.contains("minecraft_runtime:"));
    assert!(stdout.contains("snapshots:"));
    assert!(stdout.contains("locks:"));
    assert!(stdout.contains("loader_cache:"));
    assert!(stdout.contains("java_runtimes:"));
    assert!(stdout.contains("staging:"));

    // Human mode must NOT emit JSON objects to stdout
    assert!(
        !stdout.contains(r#""root""#),
        "human mode should not emit JSON:\n{stdout}"
    );
}

#[test]
fn paths_json_has_expected_keys() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora_json(&data_dir, &["paths"]);
    assert!(output.status.success(), "agora paths --json should succeed");

    let parsed = assert_json_stdout(&output);
    let obj = parsed
        .as_object()
        .expect("paths JSON output should be an object");

    assert!(obj.contains_key("root"), "missing 'root' key");
    assert!(
        obj.contains_key("local_state_db"),
        "missing 'local_state_db'"
    );
    assert!(obj.contains_key("instances"), "missing 'instances'");
    assert!(
        obj.contains_key("minecraft_runtime"),
        "missing 'minecraft_runtime'"
    );
    assert!(obj.contains_key("loader_cache"), "missing 'loader_cache'");
    assert!(obj.contains_key("java_runtimes"), "missing 'java_runtimes'");
    assert!(obj.contains_key("snapshots"), "missing 'snapshots'");
    assert!(obj.contains_key("staging"), "missing 'staging'");
    assert!(obj.contains_key("locks"), "missing 'locks'");

    assert_eq!(
        obj["root"].as_str().unwrap(),
        data_dir.to_string_lossy(),
        "root path should match data dir",
    );
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[test]
fn settings_set_and_get_string() {
    let (_tmp, data_dir) = temp_data_dir();

    // Set — pass a JSON string literal so it is stored as a JSON string
    let set = run_agora(&data_dir, &["settings", "set", "greeting", r#""hello""#]);
    assert!(
        set.status.success(),
        "settings set failed:\n{}",
        String::from_utf8_lossy(&set.stderr)
    );
    let set_stdout = String::from_utf8_lossy(&set.stdout);
    assert!(
        set_stdout.contains("hello"),
        "stdout should contain value:\n{set_stdout}"
    );

    // Get
    let get = run_agora(&data_dir, &["settings", "get", "greeting"]);
    assert!(
        get.status.success(),
        "settings get failed:\n{}",
        String::from_utf8_lossy(&get.stderr)
    );
    let get_stdout = String::from_utf8_lossy(&get.stdout);
    assert!(
        get_stdout.contains("hello"),
        "get stdout should contain value:\n{get_stdout}"
    );
}

#[test]
fn settings_set_and_get_json() {
    let (_tmp, data_dir) = temp_data_dir();

    // Set in JSON mode
    let set = run_agora_json(&data_dir, &["settings", "set", "theme", r#""dark""#]);
    assert!(set.status.success(), "settings set --json failed");

    let set_parsed = assert_json_stdout(&set);
    assert_eq!(set_parsed["status"], "set");
    assert_eq!(set_parsed["key"], "theme");
    assert_eq!(set_parsed["value"], "dark");

    // Get in JSON mode
    let get = run_agora_json(&data_dir, &["settings", "get", "theme"]);
    let get_parsed = assert_json_stdout(&get);
    assert_eq!(get_parsed, "dark");
}

#[test]
fn settings_set_boolean() {
    let (_tmp, data_dir) = temp_data_dir();

    let set = run_agora(&data_dir, &["settings", "set", "flag", "true"]);
    assert!(set.status.success(), "settings set boolean failed");

    let get = run_agora_json(&data_dir, &["settings", "get", "flag"]);
    let get_parsed = assert_json_stdout(&get);
    assert_eq!(get_parsed, serde_json::Value::Bool(true));
}

#[test]
fn settings_set_json_object_and_get() {
    let (_tmp, data_dir) = temp_data_dir();

    let set = run_agora_json(&data_dir, &["settings", "set", "obj", r#"{"nested":42}"#]);
    assert!(set.status.success(), "settings set object failed");

    let set_parsed = assert_json_stdout(&set);
    assert_eq!(set_parsed["value"]["nested"], 42);

    let get = run_agora_json(&data_dir, &["settings", "get", "obj"]);
    let get_parsed = assert_json_stdout(&get);
    assert_eq!(get_parsed["nested"], 42);
}

#[test]
fn settings_get_unknown_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["settings", "get", "nonexistent"]);
    assert!(
        !output.status.success(),
        "expected non-zero exit for unknown key"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention 'not found':\n{stderr}"
    );
}

#[test]
fn settings_list_persists_value() {
    let (_tmp, data_dir) = temp_data_dir();

    // Set a value, then list
    run_agora(&data_dir, &["settings", "set", "my_key", r#""my_val""#]);
    let list = run_agora_json(&data_dir, &["settings", "list"]);
    let parsed = assert_json_stdout(&list);
    let obj = parsed
        .as_object()
        .expect("settings list JSON should be an object");
    assert_eq!(obj.get("my_key").and_then(|v| v.as_str()), Some("my_val"));
}

// ---------------------------------------------------------------------------
// Instance list and show
// ---------------------------------------------------------------------------

#[test]
fn instance_list_human_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["list-instances"]);
    assert!(output.status.success(), "list-instances should succeed");
    // Context initialization may emit warnings to stderr; that is normal.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("No local state database"),
        "should not report missing DB (context initializes it)",
    );
}

#[test]
fn instance_list_json_empty_array() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora_json(&data_dir, &["list-instances"]);
    assert!(
        output.status.success(),
        "list-instances --json should succeed"
    );

    let parsed = assert_json_stdout(&output);
    assert_eq!(
        parsed,
        serde_json::json!([]),
        "JSON output should be an empty array"
    );
}

#[test]
fn instance_get_unknown_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["get-instance", "nonexistent"]);
    assert!(
        !output.status.success(),
        "get-instance should fail for unknown ID"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("database"),
        "stderr should mention 'not found':\n{stderr}",
    );
}

// ---------------------------------------------------------------------------
// JSON output purity and error envelopes
// ---------------------------------------------------------------------------

#[test]
fn json_stdout_is_parseable() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora_json(&data_dir, &["paths"]);
    assert!(output.status.success());
    assert_json_stdout(&output); // panics if not valid JSON
}

#[test]
fn json_error_envelope_on_failure() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora_json(&data_dir, &["get-instance", "missing-instance"]);
    assert!(!output.status.success(), "expected non-zero exit");

    // The error envelope is emitted to stderr, but stderr may also contain
    // context-warning lines before the JSON payload.  Find the last JSON
    // object by extracting from the first '{' to the matching '}'.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_start = stderr.rfind('{').unwrap_or_else(|| {
        panic!("stderr must contain a JSON envelope:\n{stderr}");
    });
    let json_end = stderr[json_start..]
        .rfind('}')
        .map(|i| json_start + i + 1)
        .unwrap_or_else(|| {
            panic!("stderr JSON envelope must have a closing '}}':\n{stderr}");
        });
    let json_text = &stderr[json_start..json_end];

    let parsed: serde_json::Value = serde_json::from_str(json_text)
        .unwrap_or_else(|e| panic!("failed to parse JSON envelope: {e}\ntext:\n{json_text}"));

    assert!(
        parsed.get("error").is_some(),
        "error envelope must contain 'error' field",
    );
    assert!(
        parsed.get("exitCode").is_some(),
        "error envelope must contain 'exitCode' field",
    );
    let code = parsed["exitCode"].as_i64().unwrap();
    assert!(code > 0, "exitCode must be > 0 on error, got {code}");
}

#[test]
fn json_stdout_empty_stderr_on_success() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora_json(&data_dir, &["paths"]);
    assert!(output.status.success());
    // Warnings from CoreContext::initialize go to stderr, but that is normal.
    // The important thing is stdout is valid JSON.
    assert_json_stdout(&output);
}

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

#[test]
fn exit_code_for_nonexistent_import_path() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["import", "Z:\\does-not-exist\\instance"]);
    assert!(
        !output.status.success(),
        "import of nonexistent path should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "stderr should mention 'does not exist':\n{stderr}"
    );
}

#[test]
fn exit_code_for_unsupported_file_extension() {
    let (_tmp, data_dir) = temp_data_dir();

    // Create a dummy file with unsupported extension
    let src = tempdir();
    let bad_file = src.path().join("instance.xyz");
    std::fs::write(&bad_file, b"garbage").unwrap();

    let output = run_agora(&data_dir, &["import", &bad_file.to_string_lossy()]);
    assert!(
        !output.status.success(),
        "import of unsupported file should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unsupported"),
        "stderr should mention 'Unsupported':\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Import registration (local directory, no network)
// ---------------------------------------------------------------------------

#[test]
fn import_directory_creates_instance() {
    let (_tmp, data_dir) = temp_data_dir();

    // Create a source directory that looks like a simple Minecraft instance
    let src = tempdir();
    let instance_src = src.path().join("my-test-instance");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(
        instance_src.join("mods").join("useful-mod.jar"),
        b"fake jar",
    )
    .unwrap();

    let output = run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);
    assert!(
        output.status.success(),
        "import directory should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Imported"),
        "stdout should say 'Imported':\n{stdout}"
    );

    // List instances — should show the imported instance
    let list = run_agora_json(&data_dir, &["list-instances"]);
    let list_parsed = assert_json_stdout(&list);
    let instances: Vec<serde_json::Value> = serde_json::from_value(list_parsed).unwrap();
    assert_eq!(instances.len(), 1, "should have 1 instance");
    assert_eq!(instances[0]["name"], "my-test-instance");
    assert_eq!(instances[0]["instance_id"], "my-test-instance");

    // Get the instance detail
    let show = run_agora_json(&data_dir, &["get-instance", "my-test-instance"]);
    let show_parsed = assert_json_stdout(&show);
    assert_eq!(show_parsed["name"], "my-test-instance");
    assert_eq!(show_parsed["instance_id"], "my-test-instance");
    assert_eq!(show_parsed["is_modpack"], serde_json::Value::Bool(false));
    assert_eq!(show_parsed["is_locked"], serde_json::Value::Bool(false));
}

#[test]
fn import_directory_json_output() {
    let (_tmp, data_dir) = temp_data_dir();

    let src = tempdir();
    let instance_src = src.path().join("json-import");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("a.jar"), b"mod content").unwrap();

    let output = run_agora_json(&data_dir, &["import", &instance_src.to_string_lossy()]);
    assert!(output.status.success(), "import --json should succeed");

    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["name"], "json-import");
    assert_eq!(parsed["instance_id"], "json-import");
    assert!(
        parsed.get("imported_mods").is_some(),
        "JSON output should include imported_mods"
    );
}

#[test]
fn import_twice_reuses_name_with_suffix() {
    let (_tmp, data_dir) = temp_data_dir();

    let src1 = tempdir();
    let inst1 = src1.path().join("duplicate-name");
    std::fs::create_dir_all(inst1.join("mods")).unwrap();
    std::fs::write(inst1.join("mods").join("a.jar"), b"mod").unwrap();
    let out1 = run_agora(&data_dir, &["import", &inst1.to_string_lossy()]);
    assert!(out1.status.success(), "first import should succeed");

    let src2 = tempdir();
    let inst2 = src2.path().join("duplicate-name");
    std::fs::create_dir_all(inst2.join("mods")).unwrap();
    std::fs::write(inst2.join("mods").join("b.jar"), b"other").unwrap();
    let out2 = run_agora(&data_dir, &["import", &inst2.to_string_lossy()]);
    assert!(
        !out2.status.success(),
        "second import with duplicate name should fail"
    );
    let stderr = String::from_utf8_lossy(&out2.stderr);
    assert!(
        stderr.contains("already exists"),
        "should mention 'already exists':\n{stderr}",
    );
}

// ---------------------------------------------------------------------------
// Instance delete
// ---------------------------------------------------------------------------

#[test]
fn instance_delete_nonexistent_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["instance", "delete", "no-such-instance"]);
    assert!(
        !output.status.success(),
        "instance delete on nonexistent instance should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention 'not found':\n{stderr}"
    );
}

#[test]
fn instance_delete_after_import_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("to-delete");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("a.jar"), b"mod").unwrap();

    let import = run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);
    assert!(import.status.success(), "import should succeed");

    let delete = run_agora(&data_dir, &["instance", "delete", "to-delete"]);
    assert!(delete.status.success(), "delete should succeed");
    let stdout = String::from_utf8_lossy(&delete.stdout);
    assert!(
        stdout.contains("Deleted"),
        "human output should contain 'Deleted':\n{stdout}"
    );

    // Verify it's gone
    let get = run_agora(&data_dir, &["get-instance", "to-delete"]);
    assert!(!get.status.success(), "instance should no longer exist");
}

#[test]
fn instance_delete_json_output() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("json-delete");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("a.jar"), b"mod").unwrap();

    run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);

    let output = run_agora_json(&data_dir, &["instance", "delete", "json-delete"]);
    assert!(output.status.success(), "delete --json should succeed");
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["status"], "deleted");
    assert_eq!(parsed["instanceId"], "json-delete");
}

// ---------------------------------------------------------------------------
// Instance rename
// ---------------------------------------------------------------------------

#[test]
fn instance_rename_after_import_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("old-name");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();

    run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);

    let rename = run_agora(&data_dir, &["instance", "rename", "old-name", "New Name"]);
    assert!(rename.status.success(), "rename should succeed");
    let stdout = String::from_utf8_lossy(&rename.stdout);
    assert!(
        stdout.contains("New Name") && stdout.contains("old-name"),
        "stdout should mention both old and new name:\n{stdout}"
    );

    // Verify under new name
    let get = run_agora_json(&data_dir, &["get-instance", "old-name"]);
    let parsed = assert_json_stdout(&get);
    assert_eq!(parsed["name"], "New Name");
}

#[test]
fn instance_rename_json_output() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("json-rename");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();

    run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);

    let output = run_agora_json(
        &data_dir,
        &["instance", "rename", "json-rename", "Json Renamed"],
    );
    assert!(output.status.success(), "rename --json should succeed");
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["status"], "renamed");
    assert_eq!(parsed["instanceId"], "json-rename");
    assert_eq!(parsed["name"], "Json Renamed");
}

// ---------------------------------------------------------------------------
// Snapshot (no instance — should fail gracefully)
// ---------------------------------------------------------------------------

#[test]
fn snapshot_list_nonexistent_instance_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["snapshots", "list", "no-such-instance"]);
    assert!(
        !output.status.success(),
        "snapshot list on nonexistent instance should fail"
    );
}

#[test]
fn snapshot_delete_nonexistent_instance_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(
        &data_dir,
        &["snapshots", "delete", "no-such-instance", "snap-1"],
    );
    assert!(
        !output.status.success(),
        "snapshot delete on nonexistent instance should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention 'not found':\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Health on nonexistent instance
// ---------------------------------------------------------------------------

#[test]
fn health_nonexistent_instance_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["health", "no-such-instance"]);
    assert!(
        !output.status.success(),
        "health on nonexistent instance should fail"
    );
}

// ---------------------------------------------------------------------------
// Registry status (no registry.db — should not panic)
// ---------------------------------------------------------------------------

#[test]
fn registry_status_does_not_panic() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["registry", "status"]);
    // This should either succeed or fail gracefully — but never panic
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panic"),
        "registry status should not panic:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Launch helpers
// ---------------------------------------------------------------------------

use agora_core::download::sha1_hex;
use agora_core::msa::{clear_credentials, store_credentials, MsaCredentials};

/// Platform key used in natives directory name.
fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// Pre-populate the Minecraft runtime cache so `materialize()` runs without
/// network.  All `sha1`/`size` fields in the synthetic version JSON match
/// the dummy files we create below, producing cache hits in every
/// `download_sha1_atomic` call.
///
/// `java_major` must match the system Java version that will be discovered.
fn populate_runtime_cache(minecraft_runtime: &Path, version: &str, java_major: i64) {
    let versions_dir = minecraft_runtime.join("versions").join(version);
    let libraries_dir = minecraft_runtime.join("libraries");
    let assets_dir = minecraft_runtime.join("assets");
    let natives_dir = minecraft_runtime
        .join("natives")
        .join(version)
        .join(platform());

    std::fs::create_dir_all(&versions_dir).unwrap();
    std::fs::create_dir_all(&libraries_dir).unwrap();
    std::fs::create_dir_all(assets_dir.join("indexes")).unwrap();
    std::fs::create_dir_all(&natives_dir).unwrap();

    // --- Client JAR ---------------------------------------------------------
    let client_bytes = b"agora-test-client-jar";
    std::fs::write(versions_dir.join(format!("{version}.jar")), client_bytes).unwrap();

    // --- Library (fake Mojang-hosted artifact) ------------------------------
    let lib_rel = "net/minecraft/minecraft/1.21/minecraft-1.21.jar";
    let lib_path = libraries_dir.join(lib_rel);
    std::fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
    let lib_bytes = b"agora-test-library";
    std::fs::write(&lib_path, lib_bytes).unwrap();

    // --- Asset index (empty objects map) ------------------------------------
    let asset_json = br#"{"objects":{}}"#;
    std::fs::write(
        assets_dir.join("indexes").join(format!("{version}.json")),
        asset_json,
    )
    .unwrap();

    // --- Version JSON with matching sha1/size -------------------------------
    let version_json = serde_json::json!({
        "id": version,
        "mainClass": "net.minecraft.client.main.Main",
        "type": "release",
        "libraries": [{
            "name": "net.minecraft:minecraft:1.21",
            "downloads": {
                "artifact": {
                    "path": lib_rel,
                    "url": "https://piston-data.mojang.com/v1/objects/0000000000000000000000000000000000000000/fake.jar",
                    "sha1": sha1_hex(lib_bytes),
                    "size": lib_bytes.len()
                }
            }
        }],
        "assetIndex": {
            "id": version,
            "url": "https://piston-meta.mojang.com/v1/packages/0000000000000000000000000000000000000000/fake.json",
            "sha1": sha1_hex(asset_json),
            "size": asset_json.len() as i64
        },
        "downloads": {
            "client": {
                "url": "https://piston-data.mojang.com/v1/objects/0000000000000000000000000000000000000000/client.jar",
                "sha1": sha1_hex(client_bytes),
                "size": client_bytes.len() as i64
            }
        },
        "javaVersion": {
            "component": "java-runtime-gamma",
            "majorVersion": java_major
        }
    });

    std::fs::write(
        versions_dir.join(format!("{version}.json")),
        serde_json::to_vec(&version_json).unwrap(),
    )
    .unwrap();
}

/// Write a minimal `InstanceManifest` and upsert an `InstanceRow` into the
/// local state DB so the CLI launch command finds a valid instance.
fn create_vanilla_instance(data_dir: &Path, instance_id: &str, version: &str) {
    let instance_dir = data_dir.join("instances").join(instance_id);
    std::fs::create_dir_all(instance_dir.join("mods")).unwrap();

    let manifest = serde_json::json!({
        "instance_id": instance_id,
        "name": instance_id,
        "minecraft_version": version,
        "loader": "vanilla",
        "loader_version": "",
        "is_locked": false,
        "mods": [],
        "resourcepacks": [],
        "shaders": [],
        "datapacks": [],
        "worlds": [],
        "user_preferences": {}
    });
    std::fs::write(
        instance_dir.join("instance_manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();

    let db_path = data_dir.join("local_state.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    agora_core::db::upsert_instance(
        &conn,
        &agora_core::models::InstanceRow {
            instance_id: instance_id.to_string(),
            name: instance_id.to_string(),
            minecraft_version: version.to_string(),
            loader: "vanilla".into(),
            loader_version: String::new(),
            is_modpack: false,
            is_locked: false,
            last_launched_at: None,
            jvm_memory_mb: 4096,
            jvm_gc: "g1gc".into(),
            jvm_custom_args: String::new(),
            jvm_always_pre_touch: true,
            created_at: chrono::Utc::now().to_rfc3339(),
            java_path: None,
            java_incompatible_override: false,
        },
    )
    .unwrap();
}

/// The MSA credential fixture uses one OS-wide keyring entry. Keep the
/// credential-backed launch tests isolated even when Cargo runs tests in
/// parallel; macOS keychain operations otherwise race with each other's
/// setup and cleanup.
static CREDENTIAL_FIXTURE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// RAII guard that sets up fake MSA credentials and clears them on drop.
struct CredentialGuard {
    _lock: MutexGuard<'static, ()>,
}

impl CredentialGuard {
    fn setup() -> Self {
        let lock = CREDENTIAL_FIXTURE_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("credential fixture lock poisoned");
        let creds = MsaCredentials {
            username: "test".into(),
            uuid: "00000000-0000-0000-0000-000000000000".into(),
            access_token: "test_access_token".into(),
            refresh_token: "test_refresh_token".into(),
            expires: chrono::Utc::now() + chrono::Duration::hours(1),
        };
        store_credentials(&creds).expect("store fake MSA credentials");
        CredentialGuard { _lock: lock }
    }
}

impl Drop for CredentialGuard {
    fn drop(&mut self) {
        let _ = clear_credentials();
    }
}

// ---------------------------------------------------------------------------
// Launch helpers
// ---------------------------------------------------------------------------

/// Write a fake Java script file that:
///   - Responds to `-XshowSettings:properties -version` (the `inspect_java`
///     probe) by printing a valid-looking Java version line and exiting 0.
///   - For any other invocation (the actual Minecraft launch command), exits
///     with the given `exit_code`.
///
/// `java_major` is the version number reported by the probe (e.g. 8, 21).
fn write_fake_java(dir: &Path, exit_code: i32, java_major: i64) -> PathBuf {
    if cfg!(target_os = "windows") {
        let path = dir.join("fake_java.bat");
        let script = format!(
            "@echo off\r\n\
             if \"%1\"==\"-XshowSettings:properties\" (\r\n\
               if \"%2\"==\"-version\" (\r\n\
                 echo java.specification.version = {java_major}\r\n\
                 echo java.version = {java_major}.0.1\r\n\
                 echo os.arch = amd64\r\n\
                 exit /b 0\r\n\
               )\r\n\
             )\r\n\
             exit /b {exit_code}\r\n"
        );
        std::fs::write(&path, script).unwrap();
        path
    } else {
        let path = dir.join("fake_java.sh");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"-XshowSettings:properties\" ] && [ \"$2\" = \"-version\" ]; then\n\
               echo \"java.specification.version = {java_major}\"\n\
               echo \"java.version = {java_major}.0.1\"\n\
               echo \"os.arch = amd64\"\n\
               exit 0\n\
             fi\n\
             exit {exit_code}\n"
        );
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
}

/// Set up a full launch environment with fake Java, fake credentials, a
/// pre-populated runtime cache, and a vanilla instance ready for `agora launch`.
fn prepare_launch_state(
    data_dir: &Path,
    exit_code: i32,
    java_major: i64,
) -> (CredentialGuard, String) {
    let creds = CredentialGuard::setup();
    let version = "1.21";
    let instance_id = "launch-test";
    let minecraft_runtime = data_dir.join("minecraft-runtime");

    populate_runtime_cache(&minecraft_runtime, version, java_major);
    create_vanilla_instance(data_dir, instance_id, version);

    // Pre-create a snapshot and write LKG state so the child process's
    // `create_or_reuse_snapshot` finds a matching index and skips the call.
    let instance_dir = data_dir.join("instances").join(instance_id);
    let snapshot = agora_core::snapshot::create_snapshot(&instance_dir, Some("Initial"))
        .expect("create initial snapshot");
    let lkg = serde_json::json!({
        "currentLkgSnapshotId": snapshot.id,
        "lastPromotedAt": null,
        "lastLaunchSessionId": null,
        "lastLaunchOutcome": null,
        "promotedSnapshotIds": [],
        "schemaVersion": 1
    });
    std::fs::write(
        instance_dir.join("lkg.json"),
        serde_json::to_vec(&lkg).unwrap(),
    )
    .unwrap();

    let fake_path = write_fake_java(data_dir, exit_code, java_major);
    let java_str = fake_path.to_string_lossy();
    let java_val = serde_json::json!(java_str.as_ref()).to_string();
    let set = run_agora_json(data_dir, &["settings", "set", "java_path", &java_val]);
    if !set.status.success() {
        panic!(
            "set java_path failed:\n{}",
            String::from_utf8_lossy(&set.stderr)
        );
    }

    (creds, instance_id.to_owned())
}

// ---------------------------------------------------------------------------
// Launch tests
// ---------------------------------------------------------------------------

#[test]
fn launch_invalid_instance_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    // Prime the context so the local state DB is initialised.
    run_agora(&data_dir, &["paths"]);

    let output = run_agora(&data_dir, &["launch", "nonexistent"]);
    assert!(
        !output.status.success(),
        "launch must fail for unknown instance"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr must mention 'not found':\n{stderr}"
    );
}

#[test]
fn launch_fake_java_success() {
    let (_tmp, data_dir) = temp_data_dir();
    run_agora(&data_dir, &["paths"]);

    let (_creds, instance_id) = prepare_launch_state(&data_dir, 0, 21);
    let output = run_agora(&data_dir, &["launch", &instance_id, "--yes"]);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!(
            "launch with fake Java (exit 0) failed.\nstdout:\n{stdout}\nstderr:\n{stderr}\nexit code: {:?}",
            output.status.code()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Abandoned") || stdout.contains("abandoned"),
        "stdout should mention Abandoned (exit 0, short runtime):\n{stdout}"
    );
}

#[test]
fn launch_fake_java_crash() {
    let (_tmp, data_dir) = temp_data_dir();
    run_agora(&data_dir, &["paths"]);

    let (_creds, instance_id) = prepare_launch_state(&data_dir, 1, 21);
    let output = run_agora(&data_dir, &["launch", &instance_id, "--yes"]);
    assert!(
        !output.status.success(),
        "launch must exit with non-zero code for crash outcome"
    );
    assert_eq!(output.status.code(), Some(7), "crash must exit code 7");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Crash") || stdout.contains("crash"),
        "stdout should mention Crash (non-zero exit):\n{stdout}"
    );
}

#[test]
fn launch_json_stdout() {
    let (_tmp, data_dir) = temp_data_dir();
    run_agora(&data_dir, &["paths"]);

    let (_creds, instance_id) = prepare_launch_state(&data_dir, 0, 21);
    let output = run_agora_json(&data_dir, &["launch", &instance_id, "--yes"]);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "launch --json failed:\nstderr:\n{stderr}\nexit code: {:?}",
            output.status.code()
        );
    }
    let parsed = assert_json_stdout(&output);
    assert!(parsed.get("pid").is_some(), "JSON must contain 'pid'");
    assert!(
        parsed.get("session_id").is_some(),
        "JSON must contain 'session_id'"
    );
    assert!(
        parsed.get("outcome").is_some(),
        "JSON must contain 'outcome'"
    );
}

// ---------------------------------------------------------------------------
// MCP serve --stdio (JSON-RPC 2.0 over stdio)
// ---------------------------------------------------------------------------

#[test]
fn mcp_serve_stdio_jsonrpc_requests() {
    let (_tmp, data_dir) = temp_data_dir();

    let mut cmd = Command::new(AGORA_BIN);
    cmd.arg("--data-dir").arg(&data_dir);
    cmd.arg("mcp").arg("serve").arg("--stdio");
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .expect("failed to spawn agora mcp serve --stdio");

    let mut child_stdin = child.stdin.take().expect("missing stdin");
    let mut child_stdout = child.stdout.take().expect("missing stdout");
    let mut child_stderr = child.stderr.take().expect("missing stderr");

    // Newline-delimited JSON-RPC 2.0 requests: two with id, one notification (no id)
    let input = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","method":"notifications/initialized"}
"#;

    child_stdin
        .write_all(input.as_bytes())
        .expect("write to child stdin");
    drop(child_stdin); // close stdin → signals EOF to the child

    // Read all output (responses are small; pipe buffer is large enough)
    let mut stdout_buf = String::new();
    child_stdout
        .read_to_string(&mut stdout_buf)
        .expect("read child stdout");

    let mut stderr_buf = String::new();
    child_stderr
        .read_to_string(&mut stderr_buf)
        .expect("read child stderr");

    let status = child.wait().expect("wait for child");
    assert!(
        status.success(),
        "mcp serve must exit cleanly on EOF, stderr:\n{stderr_buf}"
    );

    // Exactly two response lines (notification produces no response)
    let lines: Vec<&str> = stdout_buf.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "expected 2 JSON-RPC response lines, got {}:\n{:?}",
        lines.len(),
        lines,
    );

    // Line 1 — initialize
    let r1: serde_json::Value = serde_json::from_str(lines[0])
        .unwrap_or_else(|e| panic!("invalid JSON on response 1: {e}\n{}", lines[0]));
    assert_eq!(r1["jsonrpc"], "2.0");
    assert_eq!(r1["id"], 1);
    assert!(r1.get("result").is_some(), "initialize must have 'result'");
    assert!(
        r1.get("error").is_none(),
        "initialize must not have 'error'"
    );
    assert_eq!(r1["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(r1["result"]["serverInfo"]["name"], "agora");

    // Line 2 — tools/list
    let r2: serde_json::Value = serde_json::from_str(lines[1])
        .unwrap_or_else(|e| panic!("invalid JSON on response 2: {e}\n{}", lines[1]));
    assert_eq!(r2["jsonrpc"], "2.0");
    assert_eq!(r2["id"], 2);
    assert!(r2.get("result").is_some(), "tools/list must have 'result'");
    assert!(
        r2.get("error").is_none(),
        "tools/list must not have 'error'"
    );
    let tools = r2["result"]["tools"]
        .as_array()
        .expect("tools/list result.tools must be an array");
    assert!(
        !tools.is_empty(),
        "tools/list should return at least one tool"
    );

    // Stderr must NOT carry JSON-RPC response framing (responses are stdout-only)
    assert!(
        !stderr_buf.contains(r#""jsonrpc":"#),
        "stderr must not contain JSON-RPC responses:\n{stderr_buf}",
    );
}

// ---------------------------------------------------------------------------
// Mod install dry-run — optional/conflict policy, no filesystem mutation
// ---------------------------------------------------------------------------

#[test]
fn mod_install_dry_run_nonexistent_instance_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(
        &data_dir,
        &["mod", "install", "sodium", "no-such-inst", "--dry-run"],
    );
    assert!(
        !output.status.success(),
        "mod install dry-run on nonexistent instance should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention 'not found':\n{stderr}"
    );
}

#[test]
fn mod_install_dry_run_json_envelope() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora_json(
        &data_dir,
        &["mod", "install", "sodium", "no-such-inst", "--dry-run"],
    );
    assert!(
        !output.status.success(),
        "mod install dry-run should fail in JSON mode"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_start = stderr.rfind('{').unwrap_or_else(|| {
        panic!("stderr must contain JSON envelope:\n{stderr}");
    });
    let json_end = stderr[json_start..]
        .rfind('}')
        .map(|i| json_start + i + 1)
        .unwrap_or_else(|| {
            panic!("JSON envelope must have closing '}}':\n{stderr}");
        });
    let json_text = &stderr[json_start..json_end];
    let parsed: serde_json::Value = serde_json::from_str(json_text)
        .unwrap_or_else(|e| panic!("failed to parse JSON envelope: {e}\ntext:\n{json_text}"));
    assert!(
        parsed.get("error").is_some(),
        "error envelope must contain 'error'"
    );
    assert!(
        parsed.get("exitCode").is_some(),
        "error envelope must contain 'exitCode'"
    );
}

#[test]
fn mod_install_dry_run_optional_flags_parsed() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(
        &data_dir,
        &[
            "mod",
            "install",
            "sodium",
            "no-such-inst",
            "--dry-run",
            "--exclude-optional",
        ],
    );
    assert!(
        !output.status.success(),
        "mod install with --exclude-optional should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention 'not found':\n{stderr}"
    );
}

#[test]
fn mod_install_dry_run_no_filesystem_mutation() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("dry-run-inst");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("a.jar"), b"mod").unwrap();
    let import = run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);
    assert!(import.status.success(), "import should succeed");

    let pre_files: Vec<_> = std::fs::read_dir(data_dir.join("instances/dry-run-inst/mods"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .collect();

    let output = run_agora(
        &data_dir,
        &[
            "mod",
            "install",
            "nonexistent-mod",
            "dry-run-inst",
            "--dry-run",
        ],
    );
    assert!(
        !output.status.success(),
        "dry-run on unresolved mod should fail"
    );

    let post_files: Vec<_> = std::fs::read_dir(data_dir.join("instances/dry-run-inst/mods"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .collect();

    assert_eq!(
        pre_files.len(),
        post_files.len(),
        "dry-run must not modify filesystem",
    );
}

// ---------------------------------------------------------------------------
// Pack install smoke (local manifest, no network)
// ---------------------------------------------------------------------------

#[test]
fn pack_install_nonexistent_manifest_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(
        &data_dir,
        &["pack", "install", "Z:\\no-such-pack.json", "my-inst"],
    );
    assert!(
        !output.status.success(),
        "pack install with nonexistent manifest should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cannot read"),
        "stderr should mention 'Cannot read':\n{stderr}"
    );
}

#[test]
fn pack_install_nonexistent_instance_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let manifest_path = src.path().join("pack.json");
    std::fs::write(
        &manifest_path,
        r#"{"name":"TestPack","minecraft_version":"1.21","loader":"fabric","loader_version":"0.16.0","mods":[{"id":"sodium","source":"modrinth","status":"required"}]}"#,
    )
    .unwrap();
    let output = run_agora(
        &data_dir,
        &[
            "pack",
            "install",
            &manifest_path.to_string_lossy(),
            "no-such-instance",
        ],
    );
    assert!(
        !output.status.success(),
        "pack install into nonexistent instance should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention 'not found':\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Export — creates expected output
// ---------------------------------------------------------------------------

#[test]
fn export_nonexistent_instance_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let dest = tempdir();
    let output = run_agora(
        &data_dir,
        &[
            "export",
            "no-such-instance",
            &dest.path().join("out").to_string_lossy(),
        ],
    );
    assert!(
        !output.status.success(),
        "export from nonexistent instance should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention 'not found':\n{stderr}"
    );
}

#[test]
fn export_creates_server_mods_directory() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("export-test");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("server-mod.jar"), b"server").unwrap();
    std::fs::write(instance_src.join("mods").join("client-mod.jar"), b"client").unwrap();

    let import = run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);
    assert!(import.status.success(), "import should succeed");

    let dest = tempdir();
    let export_dest = dest.path().join("server");
    let output = run_agora(
        &data_dir,
        &["export", "export-test", &export_dest.to_string_lossy()],
    );
    assert!(
        output.status.success(),
        "export should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Exported"),
        "stdout should contain 'Exported':\n{stdout}"
    );

    assert!(
        export_dest.join("mods").is_dir(),
        "export destination should have mods/ directory"
    );

    let entries: Vec<_> = std::fs::read_dir(export_dest.join("mods"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .collect();
    assert!(
        entries
            .iter()
            .any(|n| n.to_string_lossy().contains("server")),
        "server-mod should be exported"
    );
}

#[test]
fn export_json_output_valid() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("json-export");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("mod.jar"), b"mod").unwrap();

    run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);

    let dest = tempdir();
    let export_dest = dest.path().join("server-json");
    let output = run_agora_json(
        &data_dir,
        &["export", "json-export", &export_dest.to_string_lossy()],
    );
    assert!(output.status.success(), "export --json should succeed");

    let parsed = assert_json_stdout(&output);
    assert!(
        parsed.get("total_mods").is_some(),
        "JSON must contain 'total_mods'"
    );
    assert!(
        parsed.get("server_mods").is_some(),
        "JSON must contain 'server_mods'"
    );
    assert!(
        parsed.get("removed_client_only").is_some(),
        "JSON must contain 'removed_client_only'"
    );
}

// ---------------------------------------------------------------------------
// Loadout — create / list / apply / delete
// ---------------------------------------------------------------------------

fn create_minimal_instance(data_dir: &std::path::Path, id: &str) {
    let src = tempdir();
    let instance_src = src.path().join(id);
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("sodium.jar"), b"sodium").unwrap();
    std::fs::write(instance_src.join("mods").join("lithium.jar"), b"lithium").unwrap();
    std::fs::write(instance_src.join("mods").join("phosphor.jar"), b"phosphor").unwrap();
    let import = run_agora(data_dir, &["import", &instance_src.to_string_lossy()]);
    assert!(import.status.success(), "import {id} should succeed");
}

#[test]
fn loadout_create_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    create_minimal_instance(&data_dir, "loadout-test");

    let output = run_agora(
        &data_dir,
        &["loadout", "create", "loadout-test", "my-profile"],
    );
    assert!(
        output.status.success(),
        "loadout create should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Created loadout"),
        "stdout should contain 'Created loadout':\n{stdout}"
    );
}

#[test]
fn loadout_create_json_output() {
    let (_tmp, data_dir) = temp_data_dir();
    create_minimal_instance(&data_dir, "loadout-json");

    let output = run_agora_json(
        &data_dir,
        &["loadout", "create", "loadout-json", "json-prof"],
    );
    assert!(
        output.status.success(),
        "loadout create --json should succeed"
    );
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["name"], "json-prof");
    assert!(parsed.get("enabled_mods").is_some());
    assert!(parsed.get("created_at").is_some());
}

#[test]
fn loadout_list_after_create() {
    let (_tmp, data_dir) = temp_data_dir();
    create_minimal_instance(&data_dir, "loadout-list");

    run_agora(&data_dir, &["loadout", "create", "loadout-list", "alpha"]);
    run_agora(&data_dir, &["loadout", "create", "loadout-list", "beta"]);

    let output = run_agora_json(&data_dir, &["loadout", "list", "loadout-list"]);
    assert!(output.status.success(), "loadout list should succeed");
    let parsed = assert_json_stdout(&output);
    let profiles: Vec<serde_json::Value> = serde_json::from_value(parsed).unwrap();
    assert_eq!(profiles.len(), 2, "should have 2 profiles");
    assert_eq!(profiles[0]["name"], "alpha");
    assert_eq!(profiles[1]["name"], "beta");
}

#[test]
fn loadout_apply_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    create_minimal_instance(&data_dir, "loadout-apply");

    run_agora(&data_dir, &["loadout", "create", "loadout-apply", "full"]);
    let output = run_agora(&data_dir, &["loadout", "apply", "loadout-apply", "full"]);
    assert!(
        output.status.success(),
        "loadout apply should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Applied"),
        "stdout should contain 'Applied':\n{stdout}"
    );
}

#[test]
fn loadout_apply_json_output() {
    let (_tmp, data_dir) = temp_data_dir();
    create_minimal_instance(&data_dir, "loadout-apply-json");

    run_agora(
        &data_dir,
        &["loadout", "create", "loadout-apply-json", "prof"],
    );
    let output = run_agora_json(
        &data_dir,
        &["loadout", "apply", "loadout-apply-json", "prof"],
    );
    assert!(
        output.status.success(),
        "loadout apply --json should succeed"
    );
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["status"], "applied");
    assert_eq!(parsed["profile"], "prof");
}

#[test]
fn loadout_delete_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    create_minimal_instance(&data_dir, "loadout-del");

    run_agora(&data_dir, &["loadout", "create", "loadout-del", "todelete"]);
    let output = run_agora(&data_dir, &["loadout", "delete", "loadout-del", "todelete"]);
    assert!(
        output.status.success(),
        "loadout delete should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Deleted"),
        "stdout should contain 'Deleted':\n{stdout}"
    );

    let list = run_agora_json(&data_dir, &["loadout", "list", "loadout-del"]);
    let parsed = assert_json_stdout(&list);
    let profiles: Vec<serde_json::Value> = serde_json::from_value(parsed).unwrap();
    assert!(profiles.is_empty(), "no profiles after delete");
}

#[test]
fn loadout_delete_json_output() {
    let (_tmp, data_dir) = temp_data_dir();
    create_minimal_instance(&data_dir, "loadout-del-json");

    run_agora(
        &data_dir,
        &["loadout", "create", "loadout-del-json", "prof"],
    );
    let output = run_agora_json(
        &data_dir,
        &["loadout", "delete", "loadout-del-json", "prof"],
    );
    assert!(
        output.status.success(),
        "loadout delete --json should succeed"
    );
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["status"], "deleted");
}

// ---------------------------------------------------------------------------
// Lockfile — export / verify / import / repair
// ---------------------------------------------------------------------------

/// Create an instance via DB upsert + manifest for testing lockfile/export commands
/// that require a non-empty loader_version.
fn create_loader_instance(data_dir: &std::path::Path, id: &str, mod_names: &[&str]) {
    // Ensure the local state DB exists and has the schema.
    let db_path = data_dir.join("local_state.db");
    if !db_path.exists() {
        agora_core::db::init_local_state_db(&db_path).unwrap();
    }
    let instance_dir = data_dir.join("instances").join(id);
    std::fs::create_dir_all(instance_dir.join("mods")).unwrap();
    for m in mod_names {
        std::fs::write(instance_dir.join("mods").join(m), m.as_bytes()).unwrap();
    }
    let manifest = serde_json::json!({
        "instance_id": id,
        "name": id,
        "minecraft_version": "1.21",
        "loader": "fabric",
        "loader_version": "0.16.0",
        "is_locked": false,
        "mods": mod_names.iter().map(|m| {
            serde_json::json!({
                "filename": m,
                "source": "manual",
                "version": null,
                "sha256": "0".repeat(64),
                "installed_at": "2024-01-01T00:00:00Z",
                "java_packages": [],
                "modrinth_id": null,
                "registry_id": null,
                "mod_jar_id": null,
                "provided_mod_ids": [],
                "enabled": true,
                "content_type": "mod",
                "depends_on": [],
                "optional_deps": [],
                "incompatible_deps": [],
                "source_url": null,
            })
        }).collect::<Vec<_>>(),
        "resourcepacks": [],
        "shaders": [],
        "datapacks": [],
        "worlds": [],
        "user_preferences": {}
    });
    std::fs::write(
        instance_dir.join("instance_manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    agora_core::db::upsert_instance(
        &conn,
        &agora_core::models::InstanceRow {
            instance_id: id.to_string(),
            name: id.to_string(),
            minecraft_version: "1.21".to_string(),
            loader: "fabric".into(),
            loader_version: "0.16.0".into(),
            is_modpack: false,
            is_locked: false,
            last_launched_at: None,
            jvm_memory_mb: 4096,
            jvm_gc: "g1gc".into(),
            jvm_custom_args: String::new(),
            jvm_always_pre_touch: true,
            created_at: chrono::Utc::now().to_rfc3339(),
            java_path: None,
            java_incompatible_override: false,
        },
    )
    .unwrap();
}

#[test]
fn lockfile_export_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-export", &["sodium.jar", "lithium.jar"]);

    let output = run_agora(&data_dir, &["lockfile", "export", "lf-export"]);
    assert!(
        output.status.success(),
        "lockfile export should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // With no --out flag, lockfile content is printed to stdout
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout should be valid lockfile JSON: {e}\n{stdout}"));
    assert_eq!(parsed["schemaVersion"], 1);
    assert_eq!(parsed["instance"]["name"], "lf-export");
    assert!(
        parsed.get("artifacts").and_then(|a| a.as_array()).is_some(),
        "lockfile should have artifacts array"
    );
}

#[test]
fn lockfile_export_to_file() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-file", &["sodium.jar"]);

    let out_path = data_dir.join("lockfile.json");
    let output = run_agora(
        &data_dir,
        &[
            "lockfile",
            "export",
            "lf-file",
            "--out",
            &out_path.to_string_lossy(),
        ],
    );
    assert!(
        output.status.success(),
        "lockfile export --out should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Exported lockfile"),
        "stdout should mention 'Exported lockfile':\n{stdout}"
    );

    assert!(out_path.exists(), "lockfile file should exist");
    let text = std::fs::read_to_string(&out_path).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("lockfile file should be valid JSON");
    assert_eq!(parsed["schemaVersion"], 1);
}

#[test]
fn lockfile_export_json_output() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-json", &["sodium.jar"]);

    let out_path = data_dir.join("lockfile-out.json");
    let output = run_agora_json(
        &data_dir,
        &[
            "lockfile",
            "export",
            "lf-json",
            "--out",
            &out_path.to_string_lossy(),
        ],
    );
    assert!(
        output.status.success(),
        "lockfile export --json should succeed"
    );
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["status"], "exported");
    assert!(
        parsed["path"]
            .as_str()
            .unwrap_or("")
            .contains("lockfile-out.json"),
        "exported path should be mentioned in JSON output"
    );
}

#[test]
fn lockfile_verify_valid() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-verify", &["sodium.jar"]);

    let lf_path = data_dir.join("lf-verify.json");
    run_agora(
        &data_dir,
        &[
            "lockfile",
            "export",
            "lf-verify",
            "--out",
            &lf_path.to_string_lossy(),
        ],
    );

    let output = run_agora(
        &data_dir,
        &["lockfile", "verify", &lf_path.to_string_lossy()],
    );
    assert!(
        output.status.success(),
        "lockfile verify should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Lockfile is valid") | stdout.contains("valid"),
        "stdout should confirm validity:\n{stdout}"
    );
}

#[test]
fn lockfile_verify_invalid_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let bad_path = data_dir.join("bad-lockfile.json");
    std::fs::write(&bad_path, r#"{"schemaVersion":999,"instance":{"name":"","minecraftVersion":"","loader":"","loaderVersion":""},"artifacts":[],"loader":{},"manifestSha256":"","configPolicy":{},"contentHash":""}"#)
        .unwrap();

    let output = run_agora(
        &data_dir,
        &["lockfile", "verify", &bad_path.to_string_lossy()],
    );
    assert!(
        !output.status.success(),
        "lockfile verify should fail for invalid lockfile"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid") || stderr.contains("Unsupported"),
        "stderr should mention 'invalid' or 'Unsupported':\n{stderr}"
    );
}

#[test]
fn lockfile_verify_json_output() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-verify-json", &["sodium.jar"]);

    let lf_path = data_dir.join("lf-verify-json.json");
    run_agora(
        &data_dir,
        &[
            "lockfile",
            "export",
            "lf-verify-json",
            "--out",
            &lf_path.to_string_lossy(),
        ],
    );

    let output = run_agora_json(
        &data_dir,
        &["lockfile", "verify", &lf_path.to_string_lossy()],
    );
    assert!(
        output.status.success(),
        "lockfile verify --json should succeed"
    );
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["status"], "valid");
    assert!(parsed.get("schemaVersion").is_some());
}

#[test]
fn lockfile_repair_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-repair", &["sodium.jar"]);

    let out_path = data_dir.join("repaired.json");
    let output = run_agora(
        &data_dir,
        &[
            "lockfile",
            "repair",
            "lf-repair",
            "--out",
            &out_path.to_string_lossy(),
        ],
    );
    assert!(
        output.status.success(),
        "lockfile repair should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Repaired"),
        "stdout should contain 'Repaired':\n{stdout}"
    );
    assert!(out_path.exists(), "repaired lockfile should exist");
    let text = std::fs::read_to_string(&out_path).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("repaired lockfile should be valid JSON");
    assert_eq!(parsed["schemaVersion"], 1);
}

#[test]
fn lockfile_repair_json_output() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-repair-json", &["sodium.jar"]);

    let out_path = data_dir.join("repaired.json");
    let output = run_agora_json(
        &data_dir,
        &[
            "lockfile",
            "repair",
            "lf-repair-json",
            "--out",
            &out_path.to_string_lossy(),
        ],
    );
    assert!(
        output.status.success(),
        "lockfile repair --json should succeed"
    );
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["status"], "repaired");
}

#[test]
fn lockfile_import_detects_drift() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-import", &["sodium.jar"]);

    // Export the lockfile
    let lf_path = data_dir.join("drift-lockfile.json");
    run_agora(
        &data_dir,
        &[
            "lockfile",
            "export",
            "lf-import",
            "--out",
            &lf_path.to_string_lossy(),
        ],
    );

    // Add a new mod after export (creates drift)
    std::fs::write(
        data_dir.join("instances/lf-import/mods/extra.jar"),
        b"extra",
    )
    .unwrap();

    let output = run_agora(
        &data_dir,
        &[
            "lockfile",
            "import",
            &lf_path.to_string_lossy(),
            "lf-import",
        ],
    );
    assert!(
        output.status.success(),
        "lockfile import should succeed (drift is informational):\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Drift") || stdout.contains("differences"),
        "stdout should mention drift:\n{stdout}"
    );
}

#[test]
fn lockfile_import_without_drift_reports_in_sync() {
    let (_tmp, data_dir) = temp_data_dir();
    create_loader_instance(&data_dir, "lf-sync", &["sodium.jar"]);

    let lf_path = data_dir.join("sync-lockfile.json");
    run_agora(
        &data_dir,
        &[
            "lockfile",
            "export",
            "lf-sync",
            "--out",
            &lf_path.to_string_lossy(),
        ],
    );

    let output = run_agora(
        &data_dir,
        &["lockfile", "import", &lf_path.to_string_lossy(), "lf-sync"],
    );
    assert!(
        output.status.success(),
        "lockfile import on in-sync instance should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("already in sync"),
        "stdout should mention 'already in sync':\n{stdout}"
    );
}

#[test]
fn lockfile_nonexistent_instance_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let output = run_agora(&data_dir, &["lockfile", "export", "no-such-instance"]);
    assert!(
        !output.status.success(),
        "lockfile export on nonexistent instance should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention 'not found':\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// User-decision-required — stable exit code 71 and JSON envelope
// ---------------------------------------------------------------------------

#[test]
fn user_decision_required_json_envelope_has_exit_code() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("udr-test");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("a.jar"), b"mod").unwrap();
    run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);

    // The mod remove pipeline on a nonexistent mod produces blocking errors
    // (not UserDecisionRequired directly), but the JSON envelope should still
    // contain an exitCode > 0 field.
    let output = run_agora_json(&data_dir, &["mod", "remove", "nonexistent-mod", "udr-test"]);
    assert!(!output.status.success(), "mod remove should fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_start = stderr.rfind('{').unwrap_or_else(|| {
        panic!("stderr must contain JSON envelope:\n{stderr}");
    });
    let json_end = stderr[json_start..]
        .rfind('}')
        .map(|i| json_start + i + 1)
        .unwrap_or_else(|| {
            panic!("JSON envelope must have closing '}}':\n{stderr}");
        });
    let json_text = &stderr[json_start..json_end];
    let parsed: serde_json::Value = serde_json::from_str(json_text)
        .unwrap_or_else(|e| panic!("failed to parse JSON envelope: {e}\ntext:\n{json_text}"));

    let exit_code = parsed["exitCode"]
        .as_i64()
        .expect("JSON envelope must contain 'exitCode'");
    assert!(
        exit_code > 0,
        "exitCode must be > 0 on error, got {exit_code}"
    );
    assert!(
        parsed.get("error").is_some(),
        "JSON envelope must contain 'error' field"
    );
}

#[test]
fn user_decision_required_human_output_mentions_blocked() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let instance_src = src.path().join("udr-human");
    std::fs::create_dir_all(instance_src.join("mods")).unwrap();
    std::fs::write(instance_src.join("mods").join("a.jar"), b"mod").unwrap();
    run_agora(&data_dir, &["import", &instance_src.to_string_lossy()]);

    let output = run_agora(
        &data_dir,
        &["mod", "remove", "nonexistent-mod", "udr-human"],
    );
    assert!(!output.status.success(), "mod remove must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not installed")
            || stderr.contains("blocked")
            || stderr.contains("unresolved"),
        "stderr should mention 'not installed' or 'blocked':\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// migrate-data command
// ---------------------------------------------------------------------------

#[test]
fn migrate_data_from_nonexistent_path_fails() {
    let (_tmp, data_dir) = temp_data_dir();
    let bad_path = std::path::Path::new("Z:\\no-such-path\\agora-data");
    let output = run_agora(
        &data_dir,
        &["migrate-data", "--from", &bad_path.to_string_lossy()],
    );
    assert!(
        !output.status.success(),
        "migrate-data from nonexistent path should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "stderr should mention 'does not exist':\n{stderr}"
    );
}

#[test]
fn migrate_data_dry_run_shows_inventory() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let src_path = src.path().join("old-data");
    std::fs::create_dir_all(src_path.join("instances/my-inst/mods")).unwrap();
    std::fs::write(src_path.join("local_state.db"), b"old state").unwrap();
    std::fs::write(
        src_path.join("instances/my-inst/instance_manifest.json"),
        br#"{"instance_id":"my-inst"}"#,
    )
    .unwrap();

    let output = run_agora(
        &data_dir,
        &["migrate-data", "--from", &src_path.to_string_lossy()],
    );
    assert!(
        output.status.success(),
        "dry-run should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Dry-run"),
        "stdout should say 'Dry-run':\n{stdout}"
    );
    assert!(
        stdout.contains("my-inst"),
        "stdout should list instances:\n{stdout}"
    );
    assert!(
        stdout.contains("--yes"),
        "stdout should mention --yes flag:\n{stdout}"
    );
}

#[test]
fn migrate_data_json_dry_run() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let src_path = src.path().join("old-data");
    std::fs::create_dir_all(&src_path).unwrap();
    std::fs::write(src_path.join("test.txt"), b"hello").unwrap();

    let output = run_agora_json(
        &data_dir,
        &["migrate-data", "--from", &src_path.to_string_lossy()],
    );
    assert!(
        output.status.success(),
        "JSON dry-run should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["status"], "dry-run");
    assert!(parsed.get("sourceInventory").is_some());
    assert!(parsed.get("conflicts").is_some());
}

#[test]
fn migrate_data_execute_moves_files() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let src_path = src.path().join("old-data");
    std::fs::create_dir_all(src_path.join("instances/my-inst/mods")).unwrap();
    std::fs::write(src_path.join("local_state.db"), b"old state").unwrap();
    std::fs::write(src_path.join("registry.db"), b"old registry").unwrap();
    std::fs::write(
        src_path.join("instances/my-inst/instance_manifest.json"),
        br#"{"name":"my-inst"}"#,
    )
    .unwrap();
    std::fs::write(
        src_path.join("instances/my-inst/mods/sodium.jar"),
        b"sodium",
    )
    .unwrap();

    let output = run_agora(
        &data_dir,
        &[
            "migrate-data",
            "--from",
            &src_path.to_string_lossy(),
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "migrate-data --yes should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Migration complete"),
        "stdout should contain 'Migration complete':\n{stdout}"
    );
    assert!(
        stdout.contains("my-inst"),
        "stdout should mention migrated instances:\n{stdout}"
    );

    // Verify files are at the destination
    assert!(
        data_dir.join("local_state.db").exists(),
        "local_state.db should be migrated"
    );
    assert!(
        data_dir
            .join("instances/my-inst/instance_manifest.json")
            .exists(),
        "instance manifest should be migrated"
    );
    assert!(
        data_dir.join("instances/my-inst/mods/sodium.jar").exists(),
        "mod file should be migrated"
    );

    // Verify backup was created
    let stdout_str = stdout.to_string();
    assert!(
        stdout_str.contains("Backup:"),
        "stdout should mention backup:\n{stdout}"
    );

    // Verify content integrity
    assert_eq!(
        std::fs::read(data_dir.join("local_state.db")).unwrap(),
        b"old state"
    );
    assert_eq!(
        std::fs::read(data_dir.join("instances/my-inst/mods/sodium.jar")).unwrap(),
        b"sodium"
    );
}

#[test]
fn migrate_data_json_execute() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let src_path = src.path().join("old-data");
    std::fs::create_dir_all(&src_path).unwrap();
    std::fs::write(src_path.join("test.txt"), b"data").unwrap();

    let output = run_agora_json(
        &data_dir,
        &[
            "migrate-data",
            "--from",
            &src_path.to_string_lossy(),
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "JSON migrate-data --yes should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let parsed = assert_json_stdout(&output);
    assert_eq!(parsed["files_migrated"], 1);
    assert!(parsed.get("backup_path").is_some());
    assert!(!parsed["backup_path"].as_str().unwrap_or("").is_empty());
}

#[test]
fn migrate_data_conflict_refused() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let src_path = src.path().join("old-data");
    std::fs::create_dir_all(&src_path).unwrap();
    std::fs::write(src_path.join("local_state.db"), b"source db").unwrap();

    // Pre-create a local_state.db at the destination
    std::fs::write(data_dir.join("local_state.db"), b"dest db").unwrap();

    let output = run_agora(
        &data_dir,
        &[
            "migrate-data",
            "--from",
            &src_path.to_string_lossy(),
            "--yes",
        ],
    );
    assert!(
        !output.status.success(),
        "migrate-data with conflicting db should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("blocked") || stderr.contains("conflict") || stderr.contains("conflict"),
        "stderr should mention conflict:\n{stderr}"
    );
}

#[test]
fn migrate_data_empty_source_succeeds_dry_run() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let src_path = src.path().join("empty-data");
    std::fs::create_dir_all(&src_path).unwrap();

    let output = run_agora(
        &data_dir,
        &["migrate-data", "--from", &src_path.to_string_lossy()],
    );
    assert!(
        output.status.success(),
        "dry-run on empty source should succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Dry-run"),
        "stdout should say Dry-run:\n{stdout}"
    );
}

#[test]
fn migrate_data_empty_source_execute_succeeds() {
    let (_tmp, data_dir) = temp_data_dir();
    let src = tempdir();
    let src_path = src.path().join("empty-data");
    std::fs::create_dir_all(&src_path).unwrap();

    let output = run_agora(
        &data_dir,
        &[
            "migrate-data",
            "--from",
            &src_path.to_string_lossy(),
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "execute on empty source should succeed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}
