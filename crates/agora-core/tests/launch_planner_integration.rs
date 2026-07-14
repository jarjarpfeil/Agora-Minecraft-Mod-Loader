//! Integration tests for the shared direct-launch planner pipeline.
//!
//! These tests construct fully materialized synthetic plans on disk using
//! temporary directories and validate every pure-function stage (`validate`,
//! `build_command`, and deserialization) without contacting remote servers.
//! Spawn and exit-classification are also exercised with a deterministic
//! platform-appropriate fake executable (`cmd.exe` on Windows, `/bin/sh`
//! elsewhere).

use std::collections::BTreeMap;
use std::path::PathBuf;

use agora_core::launch;
use agora_core::launch_planner;
use agora_core::lkg;
use agora_core::network::{NetworkCategory, NetworkPolicy};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Platform-dependent natives subdirectory name matching `platform_key()` in
/// `launch_planner.rs`.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    }
}

/// Build a fully materialized **vanilla** `MaterializedLaunchPlan` with real
/// temporary files for the client JAR, two libraries, an asset index, and a
/// natives directory.
///
/// Returns the plan, a representative `LaunchIdentity`, and default
/// `LaunchFeatures`.
fn build_vanilla_plan(
    tmp: &tempfile::TempDir,
) -> (
    launch_planner::MaterializedLaunchPlan,
    launch_planner::LaunchIdentity,
    launch_planner::LaunchFeatures,
) {
    build_plan_in_dirs(tmp, "game", "assets", "cache")
}

/// Internal helper: like `build_vanilla_plan` but accepts custom top-level
/// directory names so callers can exercise paths containing spaces, Unicode,
/// or other special characters without duplicating the full setup.
fn build_plan_in_dirs(
    tmp: &tempfile::TempDir,
    game_dir_name: &str,
    assets_dir_name: &str,
    cache_dir_name: &str,
) -> (
    launch_planner::MaterializedLaunchPlan,
    launch_planner::LaunchIdentity,
    launch_planner::LaunchFeatures,
) {
    let game_dir = tmp.path().join(game_dir_name);
    let assets_dir = tmp.path().join(assets_dir_name);
    let cache_dir = tmp.path().join(cache_dir_name);

    for d in [&game_dir, &assets_dir, &cache_dir] {
        std::fs::create_dir_all(d).unwrap();
    }

    // -- Client JAR ----------------------------------------------------------
    let client_dir = cache_dir.join("versions").join("1.21");
    std::fs::create_dir_all(&client_dir).unwrap();
    let client_jar_path = client_dir.join("1.21.jar");
    std::fs::write(&client_jar_path, b"fake client jar content").unwrap();

    // -- Libraries -----------------------------------------------------------
    let libs_dir = cache_dir.join("libraries");
    let lib_entries = [
        "net/minecraft/minecraft/1.21/minecraft-1.21.jar",
        "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar",
    ];
    let mut classpath: Vec<launch_planner::VerifiedArtifact> = Vec::new();
    for entry in &lib_entries {
        let path = libs_dir.join(entry);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"fake library content").unwrap();
        classpath.push(launch_planner::VerifiedArtifact {
            path,
            sha1: None,
            size: None,
        });
    }
    // materialize() pushes client.jar after libraries
    classpath.push(launch_planner::VerifiedArtifact {
        path: client_jar_path.clone(),
        sha1: None,
        size: None,
    });

    // -- Natives directory ---------------------------------------------------
    let natives_dir = cache_dir.join("natives").join("1.21").join(platform());
    std::fs::create_dir_all(&natives_dir).unwrap();

    // -- Asset index ---------------------------------------------------------
    let indexes_dir = assets_dir.join("indexes");
    std::fs::create_dir_all(&indexes_dir).unwrap();
    let asset_index_path = indexes_dir.join("1.21.json");
    std::fs::write(&asset_index_path, br#"{"objects":{}}"#).unwrap();

    // -- Metadata version info -----------------------------------------------
    let version = launch::VersionInfo {
        id: "1.21".into(),
        main_class: "net.minecraft.client.main.Main".into(),
        // Modern argument structure with pre-populated -cp and -Djava.library.path
        arguments: Some(launch::VersionArguments {
            jvm: vec![
                serde_json::json!("-Xmx2G"),
                serde_json::json!("-Djava.library.path=${natives_directory}"),
                serde_json::json!("-cp"),
                serde_json::json!("${classpath}"),
            ],
            game: vec![
                serde_json::json!("--username"),
                serde_json::json!("${auth_player_name}"),
                serde_json::json!("--version"),
                serde_json::json!("${version_name}"),
                serde_json::json!("--gameDir"),
                serde_json::json!("${game_directory}"),
                serde_json::json!("--assetsDir"),
                serde_json::json!("${assets_root}"),
                serde_json::json!("--assetIndex"),
                serde_json::json!("${assets_index_name}"),
                serde_json::json!("--uuid"),
                serde_json::json!("${auth_uuid}"),
                serde_json::json!("--accessToken"),
                serde_json::json!("${auth_access_token}"),
                serde_json::json!("--userType"),
                serde_json::json!("${user_type}"),
                serde_json::json!("--versionType"),
                serde_json::json!("${version_type}"),
            ],
        }),
        libraries: vec![
            launch::Library {
                name: "net.minecraft:minecraft:1.21".into(),
                ..Default::default()
            },
            launch::Library {
                name: "org.lwjgl:lwjgl:3.3.1".into(),
                ..Default::default()
            },
        ],
        asset_index: Some(launch::AssetIndex {
            id: "1.21".into(),
            url: "https://example.com/1.21.json".into(),
            ..Default::default()
        }),
        type_: "release".into(),
        java_version: Some(launch::JavaVersion {
            component: "java-runtime-gamma".into(),
            major_version: 21,
        }),
        downloads: Some(launch::VersionDownloads {
            client: Some(launch::DownloadArtifact {
                url: "https://piston-data.mojang.com/v1/objects/client.jar".into(),
                sha1: None,
                size: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let resolved = launch_planner::ResolvedLaunchPlan {
        instance_id: "test-instance".into(),
        version_id: "1.21".into(),
        base_version_id: "1.21".into(),
        loader: None,
        java: launch_planner::ResolvedJava {
            path: std::path::PathBuf::from("java"),
            major_version: 21,
            required_major_version: 21,
            incompatible_override: false,
        },
        version,
        game_dir,
        assets_dir,
        cache_dir: cache_dir.clone(),
        network_policy: NetworkPolicy::all_enabled(),
        adopted_profile: None,
    };

    let client_jar = launch_planner::VerifiedArtifact {
        path: client_jar_path,
        sha1: None,
        size: None,
    };

    let plan = launch_planner::MaterializedLaunchPlan {
        resolved,
        classpath,
        client_jar,
        natives_dir,
        asset_index_path,
        logging_config_path: None,
    };

    let identity = launch_planner::LaunchIdentity {
        username: "Player123".into(),
        access_token: "test-access-token".into(),
        uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
        user_type: "msa".into(),
        client_id: "agora-client".into(),
        xuid: "xuid98765".into(),
        user_properties: r#"{}"#.into(),
    };

    let features = launch_planner::LaunchFeatures::default();

    (plan, identity, features)
}

/// Variant of the vanilla plan whose `arguments.jvm` intentionally lacks
/// `-Djava.library.path=` and `-cp`, forcing `build_command` to inject them.
fn build_plan_without_cp_and_native(
    tmp: &tempfile::TempDir,
) -> (
    launch_planner::MaterializedLaunchPlan,
    launch_planner::LaunchIdentity,
    launch_planner::LaunchFeatures,
) {
    let (mut plan, identity, features) = build_vanilla_plan(tmp);
    if let Some(args) = &mut plan.resolved.version.arguments {
        args.jvm = vec![serde_json::json!("-Xmx2G")];
    }
    (plan, identity, features)
}

/// Variant of the vanilla plan with legacy `minecraftArguments` string instead
/// of the structured `arguments` field (pre-1.13 style).
fn build_legacy_plan(
    tmp: &tempfile::TempDir,
) -> (
    launch_planner::MaterializedLaunchPlan,
    launch_planner::LaunchIdentity,
    launch_planner::LaunchFeatures,
) {
    let (mut plan, identity, features) = build_vanilla_plan(tmp);
    plan.resolved.version.arguments = None;
    plan.resolved.version.minecraft_arguments = Some(
        "--username ${auth_player_name} --accessToken ${auth_access_token} \
         --uuid ${auth_uuid} --version ${version_name} \
         --gameDir ${game_directory} --assetsDir ${assets_root} \
         --assetIndex ${assets_index_name} --userType ${user_type} \
         --versionType ${version_type}"
            .into(),
    );
    (plan, identity, features)
}

/// A cross-platform fake-Java command that exits immediately with a given code.
fn fake_java(exit_code: i32) -> (PathBuf, Vec<String>) {
    if cfg!(target_os = "windows") {
        (
            PathBuf::from("cmd.exe"),
            vec!["/c".into(), format!("exit {}", exit_code)],
        )
    } else {
        (
            PathBuf::from("sh"),
            vec!["-c".into(), format!("exit {}", exit_code)],
        )
    }
}

// ---------------------------------------------------------------------------
// Goal 1: validate – fully materialized synthetic plan
// ---------------------------------------------------------------------------

#[test]
fn validate_succeeds_for_materialized_vanilla_plan() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, _, _) = build_vanilla_plan(&tmp);
    launch_planner::validate(&plan).unwrap();
}

#[test]
fn validate_fails_when_client_jar_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, _, _) = build_vanilla_plan(&tmp);
    std::fs::remove_file(&plan.client_jar.path).unwrap();
    let err = launch_planner::validate(&plan).unwrap_err();
    assert!(
        err.to_string().contains("client JAR"),
        "Expected error mentioning 'client JAR', got: {err}"
    );
}

#[test]
fn validate_fails_when_asset_index_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, _, _) = build_vanilla_plan(&tmp);
    std::fs::remove_file(&plan.asset_index_path).unwrap();
    let err = launch_planner::validate(&plan).unwrap_err();
    assert!(
        err.to_string().contains("asset index"),
        "Expected error mentioning 'asset index', got: {err}"
    );
}

#[test]
fn validate_fails_when_natives_dir_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, _, _) = build_vanilla_plan(&tmp);
    std::fs::remove_dir_all(&plan.natives_dir).unwrap();
    let err = launch_planner::validate(&plan).unwrap_err();
    assert!(
        err.to_string().contains("native"),
        "Expected error mentioning 'native', got: {err}"
    );
}

#[test]
fn validate_fails_when_classpath_entry_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, _, _) = build_vanilla_plan(&tmp);
    // Remove the first library JAR (not the client jar)
    std::fs::remove_file(&plan.classpath[0].path).unwrap();
    let err = launch_planner::validate(&plan).unwrap_err();
    assert!(
        err.to_string().contains("classpath"),
        "Expected error mentioning 'classpath', got: {err}"
    );
}

#[test]
fn validate_fails_when_main_class_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (mut plan, _, _) = build_vanilla_plan(&tmp);
    plan.resolved.version.main_class.clear();
    let err = launch_planner::validate(&plan).unwrap_err();
    assert!(
        err.to_string().contains("main class"),
        "Expected error mentioning 'main class', got: {err}"
    );
}

#[test]
fn validate_fails_when_java_too_old() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (mut plan, _, _) = build_vanilla_plan(&tmp);
    plan.resolved.java.major_version = 8; // required is 21
    let err = launch_planner::validate(&plan).unwrap_err();
    assert!(matches!(
        err,
        agora_core::error::LauncherError::JavaIncompatible
    ));
}

#[test]
fn validate_fails_when_java_major_too_high_without_override() {
    // Default policy: exact major match required.
    // Java 21 selected for a Java 17 instance must be rejected even though
    // 21 >= 17, because modded Minecraft depends on exact-major behaviour.
    let tmp = tempfile::TempDir::new().unwrap();
    let (mut plan, _, _) = build_vanilla_plan(&tmp);
    plan.resolved.java.required_major_version = 17;
    plan.resolved.java.major_version = 21;
    plan.resolved.java.incompatible_override = false;
    let err = launch_planner::validate(&plan).unwrap_err();
    assert!(matches!(
        err,
        agora_core::error::LauncherError::JavaIncompatible
    ));
}

#[test]
fn validate_allows_higher_java_with_explicit_override() {
    // Override policy: user explicitly accepted a higher Java.
    // Java 21 for a Java 17 instance should pass when override is set.
    let tmp = tempfile::TempDir::new().unwrap();
    let (mut plan, _, _) = build_vanilla_plan(&tmp);
    plan.resolved.java.required_major_version = 17;
    plan.resolved.java.major_version = 21;
    plan.resolved.java.incompatible_override = true;
    launch_planner::validate(&plan).unwrap();
}

#[test]
fn validate_rejects_lower_java_even_with_override() {
    // Override is for using a *newer* Java, not an insufficiently old one.
    // Java 8 for a Java 17 instance must still be rejected with override.
    let tmp = tempfile::TempDir::new().unwrap();
    let (mut plan, _, _) = build_vanilla_plan(&tmp);
    plan.resolved.java.required_major_version = 17;
    plan.resolved.java.major_version = 8;
    plan.resolved.java.incompatible_override = true;
    let err = launch_planner::validate(&plan).unwrap_err();
    assert!(matches!(
        err,
        agora_core::error::LauncherError::JavaIncompatible
    ));
}

// ---------------------------------------------------------------------------
// Goal 2: client JAR appears in final classpath
// ---------------------------------------------------------------------------

#[test]
fn build_command_includes_client_jar_in_classpath() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();

    let args_concat: String = cmd.args.join(" ");
    let client_path_str = plan.client_jar.path.to_string_lossy();
    assert!(
        args_concat.contains(&*client_path_str),
        "Client JAR path '{client_path_str}' not found in arguments:\n{}",
        args_concat
    );
}

// ---------------------------------------------------------------------------
// Goal 3: no unresolved ${...} tokens after expansion
// ---------------------------------------------------------------------------

#[test]
fn build_command_no_unresolved_placeholders_modern_arguments() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();
    for arg in &cmd.args {
        assert!(
            !arg.contains("${"),
            "Argument contains unresolved placeholder: {arg}"
        );
    }
}

#[test]
fn build_command_no_unresolved_placeholders_legacy_arguments() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_legacy_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();
    for arg in &cmd.args {
        assert!(
            !arg.contains("${"),
            "Legacy argument contains unresolved placeholder: {arg}"
        );
    }
}

#[test]
fn build_command_detects_unresolved_placeholder() {
    // An identity field referenced in the template but set empty should still
    // produce the empty string (not a literal ${…}). The only way to see a
    // literal placeholder in practice is if a new token is added to metadata
    // without a corresponding entry in `substitute`. We simulate that by
    // injecting a raw token into the user JVM args.
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let err = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[
            "--unknownToken".to_string(),
            "${unknown_placeholder}".to_string(),
        ],
    })
    .unwrap_err();
    assert!(
        matches!(err, agora_core::error::LauncherError::UnresolvedPlaceholder),
        "Expected UnresolvedPlaceholder, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Goal 4a: duplicate -Djava.library.path is NOT emitted when metadata has it
// ---------------------------------------------------------------------------

#[test]
fn build_command_does_not_duplicate_native_path_when_in_metadata() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();

    let native_count = cmd
        .args
        .iter()
        .filter(|a| a.starts_with("-Djava.library.path="))
        .count();
    assert_eq!(
        native_count, 1,
        "Expected exactly one -Djava.library.path=, got {native_count}"
    );
}

// ---------------------------------------------------------------------------
// Goal 4b: -Djava.library.path IS added when metadata lacks it
// ---------------------------------------------------------------------------

#[test]
fn build_command_adds_native_path_when_metadata_lacks_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_plan_without_cp_and_native(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();

    let native_count = cmd
        .args
        .iter()
        .filter(|a| a.starts_with("-Djava.library.path="))
        .count();
    assert_eq!(
        native_count, 1,
        "Expected exactly one -Djava.library.path=, got {native_count}"
    );
    // Verify it points at the correct directory
    let native_arg = cmd
        .args
        .iter()
        .find(|a| a.starts_with("-Djava.library.path="))
        .unwrap();
    assert!(
        native_arg.contains(&plan.natives_dir.to_string_lossy().to_string()),
        "Native path argument '{native_arg}' does not contain '{}'",
        plan.natives_dir.display()
    );
}

// ---------------------------------------------------------------------------
// Goal 4c: duplicate -cp flag is NOT emitted when metadata has it
// ---------------------------------------------------------------------------

#[test]
fn build_command_does_not_duplicate_classpath_flag_when_in_metadata() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();

    let cp_count = cmd.args.iter().filter(|a| *a == "-cp").count();
    assert_eq!(cp_count, 1, "Expected exactly one -cp flag, got {cp_count}");
}

// ---------------------------------------------------------------------------
// Goal 4d: -cp IS added when metadata lacks it
// ---------------------------------------------------------------------------

#[test]
fn build_command_adds_classpath_flag_when_metadata_lacks_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_plan_without_cp_and_native(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();

    let cp_count = cmd.args.iter().filter(|a| *a == "-cp").count();
    assert_eq!(cp_count, 1, "Expected exactly one -cp flag, got {cp_count}");
}

// ---------------------------------------------------------------------------
// Additional build_command structural tests
// ---------------------------------------------------------------------------

#[test]
fn build_command_main_class_present() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();
    assert!(
        cmd.args
            .contains(&"net.minecraft.client.main.Main".to_string()),
        "Main class not found in args: {:?}",
        cmd.args
    );
}

#[test]
fn build_command_extra_game_args_appended() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &["--quickPlaySingleplayer".into(), "MyWorld".into()],
    })
    .unwrap();
    assert!(cmd.args.contains(&"--quickPlaySingleplayer".into()));
    assert!(cmd.args.contains(&"MyWorld".into()));
}

#[test]
fn build_command_user_jvm_args_appended() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[
            "-XX:+UseG1GC".into(),
            "-Dsun.rmi.dgc.server.gcInterval=1".into(),
        ],
        extra_game_args: &[],
    })
    .unwrap();
    assert!(cmd.args.contains(&"-XX:+UseG1GC".into()));
    assert!(cmd
        .args
        .contains(&"-Dsun.rmi.dgc.server.gcInterval=1".into()));
}

#[test]
fn build_command_rejects_reserved_user_jvm_args_cp() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let err = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &["-cp".into()],
        extra_game_args: &[],
    })
    .unwrap_err();
    assert!(
        err.to_string().contains("Classpath"),
        "Expected 'Classpath' in error, got: {err}"
    );
}

#[test]
fn build_command_rejects_reserved_user_jvm_args_classpath() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let err = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &["-classpath".into()],
        extra_game_args: &[],
    })
    .unwrap_err();
    assert!(
        err.to_string().contains("Classpath"),
        "Expected 'Classpath' in error, got: {err}"
    );
}

#[test]
fn build_command_rejects_reserved_user_jvm_args_native_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let err = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &["-Djava.library.path=/malicious".into()],
        extra_game_args: &[],
    })
    .unwrap_err();
    assert!(
        err.to_string().contains("Classpath and native-path"),
        "Expected 'Classpath and native-path' in error, got: {err}"
    );
}

#[test]
fn build_command_prepared_command_fields_are_populated() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) = build_vanilla_plan(&tmp);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();
    assert_eq!(cmd.program, plan.resolved.java.path);
    assert_eq!(cmd.cwd, plan.resolved.game_dir);
    assert!(
        !cmd.args.is_empty(),
        "PreparedCommand args should not be empty"
    );
}

// ---------------------------------------------------------------------------
// Goal 5: spawn and exit classification with deterministic fake Java
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_and_wait_exit_zero_classifies_abandoned() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (program, args) = fake_java(0);
    let prepared = launch_planner::PreparedCommand {
        program,
        args,
        cwd: tmp.path().to_path_buf(),
        env: BTreeMap::new(),
    };
    let child = launch_planner::spawn(&prepared).unwrap();
    let outcome = launch_planner::wait_and_classify(child, tmp.path(), &[])
        .await
        .unwrap();
    // Exit code 0 with runtime < 60 s → Abandoned (not enough uptime for Success)
    assert_eq!(outcome, lkg::LaunchOutcome::Abandoned);
}

#[tokio::test]
async fn spawn_and_wait_exit_nonzero_classifies_crash() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (program, args) = fake_java(1);
    let prepared = launch_planner::PreparedCommand {
        program,
        args,
        cwd: tmp.path().to_path_buf(),
        env: BTreeMap::new(),
    };
    let child = launch_planner::spawn(&prepared).unwrap();
    let outcome = launch_planner::wait_and_classify(child, tmp.path(), &[])
        .await
        .unwrap();
    assert_eq!(outcome, lkg::LaunchOutcome::Crash);
}

#[tokio::test]
async fn spawn_and_wait_exit_signal_classifies_crash() {
    let tmp = tempfile::TempDir::new().unwrap();
    // On Windows cmd.exe /c "exit -1" produces exit code 255 (non-zero = crash).
    // On Unix `sh -c "kill -9 $$"` or simply exit with a negative code.
    let (program, args) = if cfg!(target_os = "windows") {
        fake_java(3) // arbitrary non-zero
    } else {
        (PathBuf::from("sh"), vec!["-c".into(), "kill -9 $$".into()])
    };
    let prepared = launch_planner::PreparedCommand {
        program,
        args,
        cwd: tmp.path().to_path_buf(),
        env: BTreeMap::new(),
    };
    let child = launch_planner::spawn(&prepared).unwrap();
    let outcome = launch_planner::wait_and_classify(child, tmp.path(), &[])
        .await
        .unwrap();
    assert_eq!(outcome, lkg::LaunchOutcome::Crash);
}

// ---------------------------------------------------------------------------
// Goal 5b: validate that spawn returns a valid PID
// ---------------------------------------------------------------------------

#[test]
fn spawn_returns_ok_with_pid() {
    // Synchronous spawn via tokio runtime
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (program, args) = fake_java(0);
        let prepared = launch_planner::PreparedCommand {
            program,
            args,
            cwd: tmp.path().to_path_buf(),
            env: BTreeMap::new(),
        };
        let child = launch_planner::spawn(&prepared).unwrap();
        // The child has a PID, which is checked inside spawn
        drop(child); // we're not waiting, just checking PID was obtained
    });
}

// ---------------------------------------------------------------------------
// Goal 6: fixture deserialization — Mojang version JSON and Fabric profile
// ---------------------------------------------------------------------------

#[test]
fn fixture_vanilla_version_info_deserializes() {
    let json = r#"{
        "id": "1.21",
        "mainClass": "net.minecraft.client.main.Main",
        "type": "release",
        "libraries": [
            {
                "name": "net.minecraft:minecraft:1.21",
                "downloads": {
                    "artifact": {
                        "path": "net/minecraft/minecraft/1.21/minecraft-1.21.jar",
                        "url": "https://piston-data.mojang.com/v1/objects/client.jar",
                        "sha1": "0123456789abcdef0123456789abcdef01234567",
                        "size": 12345
                    }
                }
            }
        ],
        "arguments": {
            "jvm": ["-Xmx2G", "-Djava.library.path=${natives_directory}", "-cp", "${classpath}"],
            "game": ["--username", "${auth_player_name}"]
        },
        "assetIndex": {
            "id": "1.21",
            "url": "https://piston-meta.mojang.com/v1/packages/1.21.json"
        },
        "javaVersion": {
            "component": "java-runtime-gamma",
            "majorVersion": 21
        },
        "downloads": {
            "client": {
                "url": "https://piston-data.mojang.com/v1/objects/client.jar",
                "sha1": "0123456789abcdef0123456789abcdef01234567",
                "size": 5000
            }
        }
    }"#;

    let version: launch::VersionInfo = serde_json::from_str(json).unwrap();
    assert_eq!(version.id, "1.21");
    assert_eq!(version.main_class, "net.minecraft.client.main.Main");
    assert_eq!(version.type_, "release");
    assert_eq!(version.libraries.len(), 1);
    assert_eq!(version.libraries[0].name, "net.minecraft:minecraft:1.21");

    let args = version
        .arguments
        .expect("arguments field should be present");
    assert_eq!(args.jvm.len(), 4);
    assert_eq!(args.game.len(), 2);

    let ai = version.asset_index.expect("assetIndex should be present");
    assert_eq!(ai.id, "1.21");

    let jv = version.java_version.expect("javaVersion should be present");
    assert_eq!(jv.major_version, 21);

    let dl = version.downloads.expect("downloads should be present");
    let client = dl.client.expect("client download should be present");
    assert_eq!(
        client.url,
        "https://piston-data.mojang.com/v1/objects/client.jar"
    );
}

#[test]
fn fixture_fabric_profile_deserializes() {
    let json = r#"{
        "id": "fabric-loader-0.16.0-1.21",
        "inheritsFrom": "1.21",
        "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
        "type": "release",
        "libraries": [
            {
                "name": "net.fabricmc:fabric-loader:0.16.0",
                "downloads": {
                    "artifact": {
                        "path": "net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar",
                        "url": "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar",
                        "sha1": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "size": 1048576
                    }
                }
            }
        ],
        "arguments": {
            "jvm": ["-Dfabric.remapClasspath=true"],
            "game": []
        }
    }"#;

    let profile: launch::VersionInfo = serde_json::from_str(json).unwrap();
    assert_eq!(profile.id, "fabric-loader-0.16.0-1.21");
    assert_eq!(
        profile.inherits_from.as_deref(),
        Some("1.21"),
        "Fabric profile should declare inheritsFrom"
    );
    assert_eq!(
        profile.main_class,
        "net.fabricmc.loader.impl.launch.knot.KnotClient"
    );
    assert_eq!(profile.libraries.len(), 1);
    assert_eq!(
        profile.libraries[0].name,
        "net.fabricmc:fabric-loader:0.16.0"
    );

    let args = profile.arguments.expect("arguments should be present");
    assert_eq!(args.jvm.len(), 1);
    assert!(args.jvm[0].as_str().unwrap().contains("fabric"));
    assert!(args.game.is_empty());
}

// ---------------------------------------------------------------------------
// Additional: parse_argument_string integration sanity
// ---------------------------------------------------------------------------

#[test]
fn parse_argument_string_quoted_windows_path() {
    let parsed = launch_planner::parse_argument_string(
        r#"-Dsome.path="C:\Program Files\Something" -javaagent:"C:\Tools\agent.jar""#,
    )
    .unwrap();
    assert_eq!(
        parsed,
        vec![
            r#"-Dsome.path=C:\Program Files\Something"#,
            r#"-javaagent:C:\Tools\agent.jar"#,
        ]
    );
}

// ---------------------------------------------------------------------------
// Canary: token redaction in Debug output
// ---------------------------------------------------------------------------

#[test]
fn launch_identity_debug_omits_access_token() {
    let identity = launch_planner::LaunchIdentity {
        username: "TestPlayer".into(),
        access_token: "eyJhbGciOiJIUzI1NiJ9.eyJ4dWlkIjoiMjUzNTQzMjM0NTY3ODkwMSJ9.abcd1234+5678/90"
            .into(),
        uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
        user_type: "msa".into(),
        client_id: String::new(),
        xuid: String::new(),
        user_properties: "{}".into(),
    };
    let debug_str = format!("{identity:?}");
    assert!(
        !debug_str.contains("eyJhbGciOiJIUzI1NiJ9"),
        "LaunchIdentity Debug leaked JWT-like access_token"
    );
    assert!(
        debug_str.contains("[REDACTED]"),
        "LaunchIdentity Debug should contain [REDACTED] for access_token"
    );
}

#[test]
fn prepared_command_debug_never_shows_args() {
    let cmd = launch_planner::PreparedCommand {
        program: PathBuf::from("java"),
        args: vec!["--accessToken".into(), "super-secret-token".into()],
        cwd: PathBuf::from("/tmp"),
        env: BTreeMap::new(),
    };
    let debug_str = format!("{cmd:?}");
    assert!(
        !debug_str.contains("super-secret-token"),
        "PreparedCommand Debug leaked token via args"
    );
    assert!(
        !debug_str.contains("--accessToken"),
        "PreparedCommand Debug should not expose --accessToken flag"
    );
    assert!(
        debug_str.contains("arg_count: 2"),
        "PreparedCommand Debug should report arg_count"
    );
}

#[test]
fn parse_argument_string_unmatched_quote_error() {
    let err = launch_planner::parse_argument_string(r#"-Dfoo="unclosed"#).unwrap_err();
    assert!(
        err.to_string().contains("unmatched"),
        "Expected mismatch error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Additional: feature-gated argument filtering (has_custom_resolution)
// ---------------------------------------------------------------------------

#[test]
fn build_command_resolution_features_expand() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (mut plan, identity, mut features) = build_vanilla_plan(&tmp);

    // Add a rule-gated jvm argument that only fires when has_custom_resolution is true
    if let Some(args) = &mut plan.resolved.version.arguments {
        args.jvm.push(serde_json::json!({
            "rules": [{"action": "allow", "features": {"has_custom_resolution": true}}],
            "value": ["--width", "${resolution_width}", "--height", "${resolution_height}"]
        }));
    }

    // Without the feature enabled, the resolution args should NOT appear
    features
        .values
        .insert("has_custom_resolution".into(), false);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();
    assert!(!cmd.args.contains(&"--width".into()));
    assert!(!cmd.args.contains(&"--height".into()));

    // With the feature enabled AND resolution set, they SHOULD appear
    features.values.insert("has_custom_resolution".into(), true);
    features.resolution_width = Some(1280);
    features.resolution_height = Some(720);
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();
    assert!(
        cmd.args.contains(&"--width".into()),
        "--width should appear when has_custom_resolution is true"
    );
    assert!(
        cmd.args.contains(&"1280".into()),
        "1280 should appear as resolution width"
    );
    assert!(
        cmd.args.contains(&"--height".into()),
        "--height should appear when has_custom_resolution is true"
    );
    assert!(
        cmd.args.contains(&"720".into()),
        "720 should appear as resolution height"
    );
}

#[test]
fn build_command_logging_config_expands() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (mut plan, identity, features) = build_vanilla_plan(&tmp);

    // Inject a logging config so the ${path} expansion is exercised
    let logging_dir = plan.resolved.cache_dir.join("logging");
    std::fs::create_dir_all(&logging_dir).unwrap();
    let log_config_path = logging_dir.join("log4j2.xml");
    std::fs::write(&log_config_path, b"<Configuration/>").unwrap();
    plan.logging_config_path = Some(log_config_path.clone());

    // Add logging client metadata
    plan.resolved.version.logging = Some(launch::LoggingConfig {
        client: Some(launch::LoggingClient {
            argument: "-Dlog4j.configurationFile=${path}".into(),
            file: Some(launch::LoggingFile {
                id: "log4j2.xml".into(),
                sha1: None,
                size: None,
                url: "https://example.com/log4j2.xml".into(),
            }),
            type_: "log4j2-xml".into(),
        }),
    });

    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();
    // The logging argument should have the path substituted
    let log_arg = cmd
        .args
        .iter()
        .find(|a| a.starts_with("-Dlog4j.configurationFile="));
    assert!(
        log_arg.is_some(),
        "Expected -Dlog4j.configurationFile= in args, got none"
    );
    let log_arg = log_arg.unwrap();
    assert!(
        log_arg.contains(&log_config_path.to_string_lossy().to_string()),
        "Logging argument '{log_arg}' should contain expanded path '{}'",
        log_config_path.display()
    );
    assert!(
        !log_arg.contains("${"),
        "Logging arg should have no placeholder"
    );
}

// ---------------------------------------------------------------------------
// Edge: user_properties is a JSON string (may be empty object or real JSON)
// ---------------------------------------------------------------------------

#[test]
fn build_command_user_properties_roundtrip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, mut identity, features) = build_vanilla_plan(&tmp);
    identity.user_properties = r#"{"preferredLanguage":"en"}"#.into();

    // We need to trigger a ${user_properties} expansion somewhere. Vanilla
    // modern metadata does not normally include it, so we temporarily inject it
    // into game args.
    let mut plan = plan;
    if let Some(args) = &mut plan.resolved.version.arguments {
        args.game.push(serde_json::json!("--userProperties"));
        args.game.push(serde_json::json!("${user_properties}"));
    }

    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();
    let idx = cmd.args.iter().position(|a| a == "--userProperties");
    assert!(idx.is_some(), "--userProperties flag should be present");
    let idx = idx.unwrap();
    // The next argument should be the expanded user_properties value
    assert_eq!(
        cmd.args.get(idx + 1),
        Some(&r#"{"preferredLanguage":"en"}"#.into()),
        "user_properties should be expanded"
    );
}

// ---------------------------------------------------------------------------
// Goal 7: paths with spaces and Unicode
// ---------------------------------------------------------------------------

#[test]
fn validate_succeeds_with_spaces_in_directory_names() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, _, _) = build_plan_in_dirs(&tmp, "game dir with spaces", "asset root", "my cache");
    launch_planner::validate(&plan).unwrap();
}

#[test]
fn build_command_expands_paths_containing_spaces() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, identity, features) =
        build_plan_in_dirs(&tmp, "game dir with spaces", "asset root", "my cache");
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();

    let args_concat: String = cmd.args.join(" ");
    assert!(
        args_concat.contains("game dir with spaces"),
        "game_dir with spaces not found in expanded args:\n{args_concat}"
    );
    assert!(
        args_concat.contains("asset root"),
        "assets_dir with spaces not found in expanded args:\n{args_concat}"
    );
    // Verify no unresolved placeholders remain
    for arg in &cmd.args {
        assert!(
            !arg.contains("${"),
            "Argument contains unresolved placeholder: {arg}"
        );
    }
}

#[test]
fn validate_succeeds_with_unicode_in_directory_names() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, _, _) = build_plan_in_dirs(&tmp, "ゲーム", "アセット", "キャッシュ");
    launch_planner::validate(&plan).unwrap();
}

#[test]
fn build_command_expands_unicode_in_paths_and_identity() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (plan, mut identity, features) =
        build_plan_in_dirs(&tmp, "Minecraft_ゲーム", "assets", "cache");

    // Use a Unicode username that exercises the template substitution path
    identity.username = "Πlayer_游戏".into();

    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &plan,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .unwrap();

    let args_concat: String = cmd.args.join(" ");
    assert!(
        args_concat.contains("Minecraft_ゲーム"),
        "Unicode game_dir not found in expanded args:\n{args_concat}"
    );
    assert!(
        args_concat.contains("Πlayer_游戏"),
        "Unicode username not found in expanded args:\n{args_concat}"
    );
    // Verify no unresolved placeholders remain
    for arg in &cmd.args {
        assert!(
            !arg.contains("${"),
            "Argument contains unresolved placeholder: {arg}"
        );
    }
}

// ---------------------------------------------------------------------------
// New: Cache-first / offline lifecycle tests
// ---------------------------------------------------------------------------
//
// Requirement coverage:
//   - Freshness policy on version_manifest_v2.json (24h TTL)
//   - Offline fallback from stale manifest with all_disabled()
//   - Full pipeline (resolve->materialize->validate->build_command) offline
//   - Denied cache-miss -> dedicated policy errors
//   - Stale manifest fallback via mtime manipulation
//   - Cache tampering -> wrong hash rejected offline
// ---------------------------------------------------------------------------

use agora_core::error::LauncherError;
use sha1::{Digest as Sha1Digest, Sha1};

fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Minimal version JSON that satisfies resolve + materialize with no
/// libraries, no logging, and an empty asset index.
fn make_version_json(
    client_jar_sha1: &str,
    client_jar_size: i64,
    asset_index_sha1: &str,
    asset_index_size: i64,
) -> serde_json::Value {
    serde_json::json!({
        "id": "1.21",
        "mainClass": "net.minecraft.client.main.Main",
        "type": "release",
        "libraries": [],
        "arguments": {
            "jvm": [
                "-Xmx2G",
                "-Djava.library.path=${natives_directory}",
                "-cp",
                "${classpath}"
            ],
            "game": [
                "--username", "${auth_player_name}",
                "--version", "${version_name}",
                "--gameDir", "${game_directory}",
                "--assetsDir", "${assets_root}",
                "--assetIndex", "${assets_index_name}",
                "--uuid", "${auth_uuid}",
                "--accessToken", "${auth_access_token}",
                "--userType", "${user_type}",
                "--versionType", "${version_type}"
            ]
        },
        "assetIndex": {
            "id": "1.21",
            "url": "https://piston-meta.mojang.com/v1/packages/index.json",
            "sha1": asset_index_sha1,
            "size": asset_index_size
        },
        "javaVersion": {
            "component": "java-runtime-gamma",
            "majorVersion": 21
        },
        "downloads": {
            "client": {
                "url": "https://piston-data.mojang.com/v1/objects/client.jar",
                "sha1": client_jar_sha1,
                "size": client_jar_size
            }
        }
    })
}

/// Populate all cache/metadata files for a fully offline 1.21 vanilla launch.
/// The version manifest is written with a *fresh* mtime.
fn prepare_offline_fixtures(
    tmp: &tempfile::TempDir,
    client_jar_content: &[u8],
) -> launch_planner::ResolveRequest {
    let game_dir = tmp.path().join("game");
    let assets_dir = tmp.path().join("assets");
    let cache_dir = tmp.path().join("cache");

    for d in [&game_dir, &assets_dir, &cache_dir] {
        std::fs::create_dir_all(d).unwrap();
    }

    // -- Client JAR ----------------------------------------------------------
    let client_sha1 = sha1_hex(client_jar_content);
    let client_dir = cache_dir.join("versions").join("1.21");
    std::fs::create_dir_all(&client_dir).unwrap();
    let client_jar_path = client_dir.join("1.21.jar");
    std::fs::write(&client_jar_path, client_jar_content).unwrap();

    // -- Asset index (compute sha1 before version JSON references it) --------
    let indexes_dir = assets_dir.join("indexes");
    std::fs::create_dir_all(&indexes_dir).unwrap();
    let index_path = indexes_dir.join("1.21.json");
    let index_content = br#"{"objects":{}}"#;
    std::fs::write(&index_path, index_content).unwrap();
    let asset_index_sha1 = sha1_hex(index_content);

    // -- Version JSON --------------------------------------------------------
    let client_jar_size = client_jar_content.len() as i64;
    let asset_index_size = index_content.len() as i64;
    let version_json_value = make_version_json(
        &client_sha1,
        client_jar_size,
        &asset_index_sha1,
        asset_index_size,
    );
    let version_json_bytes = serde_json::to_vec_pretty(&version_json_value).unwrap();
    let version_sha1 = sha1_hex(&version_json_bytes);

    let meta_dir = cache_dir.join("metadata");
    let versions_dir = meta_dir.join("versions");
    std::fs::create_dir_all(&versions_dir).unwrap();
    std::fs::write(versions_dir.join("1.21.json"), &version_json_bytes).unwrap();

    // -- Version manifest (fresh) --------------------------------------------
    let manifest = launch::MojangVersionManifest {
        latest: launch::MojangLatest {
            release: "1.21".into(),
            snapshot: "1.21".into(),
        },
        versions: vec![launch::MojangVersionRef {
            id: "1.21".into(),
            url: "https://piston-meta.mojang.com/v1/packages/abc123/1.21.json".into(),
            sha1: Some(version_sha1),
            type_: "release".into(),
        }],
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let manifest_path = meta_dir.join("version_manifest_v2.json");
    std::fs::write(&manifest_path, &manifest_bytes).unwrap();
    // Ensure fresh mtime (re-open with write access to set on Windows).
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&manifest_path)
        .unwrap();
    f.set_modified(std::time::SystemTime::now()).unwrap();
    drop(f);

    // -- Natives dir (pre-create so materialize doesn't need to extract) -----
    let platform_key = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "osx",
        _ => "linux",
    };
    let natives_dir = cache_dir.join("natives").join("1.21").join(platform_key);
    std::fs::create_dir_all(&natives_dir).unwrap();

    let request = launch_planner::ResolveRequest {
        instance_id: "offline-test".into(),
        base_version_id: "1.21".into(),
        loader: None,
        game_dir,
        assets_dir,
        cache_dir,
        java_override: None,
        java_candidates: vec![agora_core::java::JavaInstallation {
            path: std::path::PathBuf::from("java"),
            version: 21,
            version_string: "21".into(),
            source: agora_core::java::JavaSource::System,
            arch: None,
        }],
        network_policy: NetworkPolicy::all_disabled(),
        allow_incompatible_java_override: false,
        minecraft_dir: None,
        receipts_root: None,
    };

    request
}

/// Populate only the version manifest cache (stale or fresh) for resolve tests.
fn write_manifest_cache(
    cache_dir: &std::path::Path,
    mtime: Option<std::time::SystemTime>,
) -> String {
    let version_json = br#"{
        "id": "1.21",
        "mainClass": "net.minecraft.client.main.Main",
        "type": "release",
        "libraries": [],
        "arguments": {
            "jvm": ["-Xmx2G"],
            "game": ["--username", "${auth_player_name}"]
        },
        "assetIndex": {
            "id": "1.21",
            "url": "https://example.com/1.21.json"
        },
        "javaVersion": {
            "component": "java-runtime-gamma",
            "majorVersion": 21
        },
        "downloads": {
            "client": {
                "url": "https://piston-data.mojang.com/v1/objects/client.jar",
                "sha1": null,
                "size": null
            }
        }
    }"#;
    let v_sha1 = sha1_hex(version_json);

    let meta_dir = cache_dir.join("metadata");
    let versions_dir = meta_dir.join("versions");
    std::fs::create_dir_all(&versions_dir).unwrap();
    std::fs::write(versions_dir.join("1.21.json"), version_json).unwrap();

    let manifest = launch::MojangVersionManifest {
        latest: launch::MojangLatest {
            release: "1.21".into(),
            snapshot: "1.21".into(),
        },
        versions: vec![launch::MojangVersionRef {
            id: "1.21".into(),
            url: "https://piston-meta.mojang.com/v1/packages/1.21.json".into(),
            sha1: Some(v_sha1.clone()),
            type_: "release".into(),
        }],
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let manifest_path = meta_dir.join("version_manifest_v2.json");
    std::fs::write(&manifest_path, &manifest_bytes).unwrap();

    if let Some(t) = mtime {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&manifest_path)
            .unwrap();
        f.set_modified(t).unwrap();
        drop(f);
    }

    v_sha1
}

fn disabled_resolve_request(tmp: &tempfile::TempDir) -> launch_planner::ResolveRequest {
    launch_planner::ResolveRequest {
        instance_id: "test".into(),
        base_version_id: "1.21".into(),
        loader: None,
        game_dir: tmp.path().join("game"),
        assets_dir: tmp.path().join("assets"),
        cache_dir: tmp.path().join("cache"),
        java_override: None,
        java_candidates: vec![agora_core::java::JavaInstallation {
            path: std::path::PathBuf::from("java"),
            version: 21,
            version_string: "21".into(),
            source: agora_core::java::JavaSource::System,
            arch: None,
        }],
        network_policy: NetworkPolicy::all_disabled(),
        allow_incompatible_java_override: false,
        minecraft_dir: None,
        receipts_root: None,
    }
}

// ---------------------------------------------------------------------------
// Goal: Full offline pipeline (resolve -> materialize -> validate -> build_command)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_offline_pipeline_with_all_disabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let client_jar_content = b"fake client jar for offline pipeline test";
    let request = prepare_offline_fixtures(&tmp, client_jar_content);

    // resolve
    let resolved = launch_planner::resolve(request)
        .await
        .expect("resolve should succeed offline with fresh/full cache");
    assert_eq!(resolved.base_version_id, "1.21");
    assert!(resolved.loader.is_none());

    // materialize
    let materialized = launch_planner::materialize(resolved)
        .await
        .expect("materialize should succeed offline with all artifacts cached");
    assert!(materialized.client_jar.path.is_file());
    assert!(materialized.asset_index_path.is_file());
    assert!(materialized.natives_dir.is_dir());

    // validate
    launch_planner::validate(&materialized).expect("validate should succeed on materialized plan");

    // build_command
    let identity = launch_planner::LaunchIdentity {
        username: "OfflinePlayer".into(),
        access_token: "tok".into(),
        uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
        user_type: "msa".into(),
        client_id: "agora-client".into(),
        xuid: "xuid123".into(),
        user_properties: "{}".into(),
    };
    let features = launch_planner::LaunchFeatures::default();
    let cmd = launch_planner::build_command(launch_planner::BuildCommandRequest {
        plan: &materialized,
        identity: &identity,
        features: &features,
        user_jvm_args: &[],
        extra_game_args: &[],
    })
    .expect("build_command should succeed offline");
    assert!(!cmd.args.is_empty());
    // No unresolved placeholders
    for arg in &cmd.args {
        assert!(!arg.contains("${"), "unresolved placeholder: {arg}");
    }
}

// ---------------------------------------------------------------------------
// Goal: Denied cache-miss - dedicated errors before transport
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_denied_when_manifest_cache_miss_and_metadata_disabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let request = disabled_resolve_request(&tmp);
    let err = launch_planner::resolve(request).await.unwrap_err();
    assert!(
        matches!(err, LauncherError::NetworkMojangMetadataDisabled),
        "expected NetworkMojangMetadataDisabled, got {err:?}"
    );
}

#[tokio::test]
async fn materialize_denied_when_client_jar_cache_miss_and_content_disabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Populate manifest + version JSON so resolve succeeds, but leave
    // client.jar missing.
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    write_manifest_cache(&cache_dir, Some(std::time::SystemTime::now()));

    // Ensure the version JSON is correctly placed but NO client jar exists.
    // The version JSON declares a client download; materialize will see
    // a cache miss and then be blocked by the content-disabled policy.
    let resolved = launch_planner::ResolvedLaunchPlan {
        instance_id: "test".into(),
        version_id: "1.21".into(),
        base_version_id: "1.21".into(),
        loader: None,
        java: launch_planner::ResolvedJava {
            path: std::path::PathBuf::from("java"),
            major_version: 21,
            required_major_version: 21,
            incompatible_override: false,
        },
        version: launch::VersionInfo {
            id: "1.21".into(),
            main_class: "net.minecraft.client.main.Main".into(),
            libraries: vec![],
            asset_index: Some(launch::AssetIndex {
                id: "1.21".into(),
                url: "https://piston-meta.mojang.com/v1/packages/index.json".into(),
                ..Default::default()
            }),
            type_: "release".into(),
            java_version: Some(launch::JavaVersion {
                component: "java-runtime-gamma".into(),
                major_version: 21,
            }),
            downloads: Some(launch::VersionDownloads {
                client: Some(launch::DownloadArtifact {
                    url: "https://piston-data.mojang.com/v1/objects/client.jar".into(),
                    sha1: Some("0000000000000000000000000000000000000000".into()),
                    size: Some(100),
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        game_dir: tmp.path().join("game"),
        assets_dir: tmp.path().join("assets"),
        cache_dir,
        network_policy: {
            let mut p = NetworkPolicy::all_disabled();
            // Enable metadata so resolve doesn't fail, but keep content disabled.
            p.set_category(NetworkCategory::MojangMetadata, true);
            p
        },

        adopted_profile: None,
    };

    let err = launch_planner::materialize(resolved).await.unwrap_err();
    assert!(
        matches!(err, LauncherError::NetworkMojangContentDisabled),
        "expected NetworkMojangContentDisabled, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Goal: Stale manifest - use valid stale cache when online metadata disabled
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stale_manifest_succeeds_offline_when_metadata_disabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 60 * 60);
    write_manifest_cache(&cache_dir, Some(past));

    let request = disabled_resolve_request(&tmp);
    // write_manifest_cache writes to the same cache_dir, so the manifest
    // and version JSON are present but the manifest is stale.
    let resolved = launch_planner::resolve(request)
        .await
        .expect("resolve should fall back to stale manifest when metadata is disabled");
    assert_eq!(resolved.base_version_id, "1.21");
}

// ---------------------------------------------------------------------------
// Goal: Stale manifest - use valid stale cache when refresh fails with
//       transport error (unreachable client)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stale_manifest_falls_back_on_transport_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 60 * 60);
    write_manifest_cache(&cache_dir, Some(past));

    let request = launch_planner::ResolveRequest {
        instance_id: "stale-test".into(),
        base_version_id: "1.21".into(),
        loader: None,
        game_dir: tmp.path().join("game"),
        assets_dir: tmp.path().join("assets"),
        cache_dir,
        java_override: None,
        java_candidates: vec![agora_core::java::JavaInstallation {
            path: std::path::PathBuf::from("java"),
            version: 21,
            version_string: "21".into(),
            source: agora_core::java::JavaSource::System,
            arch: None,
        }],
        // Metadata ENABLED so it attempts refresh, but the client has no
        // route to the manifest URL -> transport error -> fall back.
        network_policy: NetworkPolicy::all_enabled(),
        allow_incompatible_java_override: false,
        minecraft_dir: None,
        receipts_root: None,
    };

    let resolved = launch_planner::resolve(request)
        .await
        .expect("resolve should fall back to stale manifest on transport error");
    assert_eq!(resolved.base_version_id, "1.21");
}

// ---------------------------------------------------------------------------
// Goal: Cache tampering - wrong client/library hash rejected offline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn materialize_rejects_tampered_client_jar_offline() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Write a bogus client.jar at the expected path with wrong SHA-1.
    let client_dir = cache_dir.join("versions").join("1.21");
    std::fs::create_dir_all(&client_dir).unwrap();
    let _client_path = client_dir.join("1.21.jar");
    std::fs::write(&_client_path, b"tampered content, wrong hash").unwrap();

    // The version JSON declares sha1="abc123...deadbeef" for the client
    // download, which does NOT match the file above.

    // Stitch a resolved plan that points at the tampered jar path.
    let plan = launch_planner::ResolvedLaunchPlan {
        instance_id: "tamper-test".into(),
        version_id: "1.21".into(),
        base_version_id: "1.21".into(),
        loader: None,
        java: launch_planner::ResolvedJava {
            path: std::path::PathBuf::from("java"),
            major_version: 21,
            required_major_version: 21,
            incompatible_override: false,
        },
        version: launch::VersionInfo {
            id: "1.21".into(),
            main_class: "net.minecraft.client.main.Main".into(),
            libraries: vec![],
            asset_index: Some(launch::AssetIndex {
                id: "1.21".into(),
                url: "https://piston-meta.mojang.com/v1/packages/index.json".into(),
                ..Default::default()
            }),
            type_: "release".into(),
            java_version: Some(launch::JavaVersion {
                component: "java-runtime-gamma".into(),
                major_version: 21,
            }),
            downloads: Some(launch::VersionDownloads {
                client: Some(launch::DownloadArtifact {
                    url: "https://piston-data.mojang.com/v1/objects/client.jar".into(),
                    // SHA-1 does NOT match "tampered content, wrong hash"
                    sha1: Some("abcdef1234567890abcdef1234567890abcdef12".into()),
                    size: Some(100),
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        game_dir: tmp.path().join("game"),
        assets_dir: tmp.path().join("assets"),
        cache_dir,
        network_policy: NetworkPolicy::all_disabled(),

        adopted_profile: None,
    };

    let err = launch_planner::materialize(plan).await.unwrap_err();
    // With all_disabled() and a wrong-hash cache file, the code detects a
    // cache miss (hash mismatch), then hits the disabled-content policy.
    assert!(
        matches!(err, LauncherError::NetworkMojangContentDisabled),
        "expected NetworkMojangContentDisabled for tampered jar with \
         all_disabled(), got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Goal: Ensure current 100% loader pin enforcement tests remain passing
// ---------------------------------------------------------------------------

#[test]
fn loader_pin_enforcement_still_active() {
    assert!(
        agora_core::loader_manifests::LIBRARY_PIN_ENFORCEMENT_ENABLED,
        "LIBRARY_PIN_ENFORCEMENT_ENABLED must remain true"
    );
}
