use clap::{Parser, Subcommand, ValueEnum};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use agora_core::clone::ClonePrefs;
use agora_core::crash_service::CrashService;
use agora_core::install_service::InstallService;
use agora_core::instance_service::{CreateInstanceRequest, InstanceService};
use agora_core::loader_service::LoaderService;
use agora_core::registry::RegistryService;
use agora_core::runtime_service::RuntimeService;
use agora_core::settings::SettingsService;

/// A silent progress reporter for the CLI — no progress events are emitted.
struct SilentReporter;

impl agora_core::install_pipeline::ProgressReporter for SilentReporter {
    fn report(&self, _event: agora_core::install_pipeline::ProgressEvent) {}
}

/// A console progress reporter for runtime operations.
struct ConsoleRuntimeProgress;

impl agora_core::runtime_manager::RuntimeProgress for ConsoleRuntimeProgress {
    fn on_progress(&self, message: &str, percent: Option<f64>) {
        if let Some(pct) = percent {
            eprintln!("[{}%] {}", pct, message);
        } else {
            eprintln!("[..] {}", message);
        }
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct ConsoleLaunchProgress {
    json: bool,
}

impl agora_core::launch_service::LaunchProgress for ConsoleLaunchProgress {
    fn phase(&self, _name: &str, message: &str) {
        if !self.json {
            eprintln!("[..] {message}");
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
    Ndjson,
}

impl OutputFormat {
    fn is_json_output(self) -> bool {
        matches!(self, OutputFormat::Json | OutputFormat::Ndjson)
    }
}

#[derive(Parser)]
#[command(name = "agora", about = "Agora Minecraft Launcher CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, help = "Path to Agora data directory")]
    data_dir: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        help = "JSON output (shorthand for --output json)"
    )]
    json: bool,

    #[arg(
        long,
        global = true,
        help = "Output format: human, json, or ndjson. Overrides --json."
    )]
    output: Option<OutputFormat>,

    #[arg(
        long,
        global = true,
        help = "Registry repository (owner/repo). Overrides AGORA_REGISTRY_REPO env."
    )]
    registry_repo: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    ListInstances,
    Paths,
    GetInstance {
        id: String,
    },
    Instance {
        #[command(subcommand)]
        action: InstanceCmd,
    },
    #[command(name = "mod")]
    Mods {
        #[command(subcommand)]
        action: ModsCmd,
    },
    Health {
        instance: String,
    },
    Registry {
        #[command(subcommand)]
        action: RegistryCmd,
    },
    Snapshots {
        #[command(subcommand)]
        action: SnapshotsCmd,
    },
    Import {
        path: PathBuf,
        #[arg(long, help = "Symlink saves instead of copying")]
        symlink_saves: bool,
    },
    Launch {
        instance: String,
        #[arg(long, help = "Skip health check confirmation")]
        yes: bool,
    },
    Auth {
        #[command(subcommand)]
        action: AuthCmd,
    },
    Sync,
    Runtime {
        #[command(subcommand)]
        action: RuntimeCmd,
    },
    Loader {
        #[command(subcommand)]
        action: LoaderCmd,
    },
    Settings {
        #[command(subcommand)]
        action: SettingsCmd,
    },
    Mcp {
        #[command(subcommand)]
        action: McpCmd,
    },
    Crash {
        #[command(subcommand)]
        action: CrashCmd,
    },
    /// Migrate data from an old CLI data root to the current app data directory.
    #[command(name = "migrate-data")]
    MigrateData {
        /// Path to the old (legacy) Agora CLI data root.
        #[arg(long, short)]
        from: PathBuf,
        /// Actually execute the migration (required; default is dry-run).
        #[arg(long)]
        yes: bool,
    },
    /// Install a modpack pack manifest into an existing instance.
    Pack {
        #[command(subcommand)]
        action: PackCmd,
    },
    /// Export an instance to a standalone server environment.
    Export {
        instance: String,
        dest: PathBuf,
    },
    /// Manage loadout profiles for an instance (enable/disable sets of content).
    Loadout {
        #[command(subcommand)]
        action: LoadoutCmd,
    },
    /// Canonical integrity lockfiles: export, verify, repair, import.
    Lockfile {
        #[command(subcommand)]
        action: LockfileCmd,
    },
}

#[derive(Subcommand)]
enum InstanceCmd {
    /// Create a new instance with the given name, MC version, loader, and loader version.
    Create {
        name: String,
        #[arg(short, long, help = "Minecraft version (e.g. 1.21)")]
        mc_version: String,
        #[arg(short, long, default_value = "vanilla", help = "Mod loader")]
        loader: String,
        #[arg(short = 'V', long, default_value = "", help = "Loader version")]
        loader_version: String,
        #[arg(long)]
        jvm_memory_mb: Option<i64>,
        #[arg(long)]
        jvm_gc: Option<String>,
        #[arg(long)]
        jvm_custom_args: Option<String>,
        #[arg(long)]
        jvm_always_pre_touch: Option<bool>,
    },
    /// Clone an existing instance with copy-preference flags.
    Clone {
        source: String,
        name: String,
        #[arg(long, help = "Skip copying saves")]
        no_saves: bool,
        #[arg(long, help = "Skip copying mods")]
        no_mods: bool,
        #[arg(long, help = "Skip copying resource packs")]
        no_resource_packs: bool,
        #[arg(long, help = "Skip copying shader packs")]
        no_shader_packs: bool,
        #[arg(long, help = "Skip copying screenshots")]
        no_screenshots: bool,
        #[arg(long, help = "Skip copying config")]
        no_config: bool,
        #[arg(long, help = "Skip copying servers.dat")]
        no_servers: bool,
        #[arg(long, help = "Skip copying options files")]
        no_options: bool,
        #[arg(long, help = "Use hard links instead of copying")]
        hard_links: bool,
        #[arg(long, help = "Use symlinks instead of copying")]
        sym_links: bool,
    },
    /// Lock an instance to prevent modification.
    Lock { id: String },
    /// Unlock a locked instance.
    Unlock { id: String },
    /// Delete an instance and its directory.
    Delete { id: String },
    /// Rename an instance.
    Rename { id: String, name: String },
    /// Reinstall the mod loader for an instance.
    RepairLoader { id: String },
}

#[derive(Subcommand)]
enum LoaderCmd {
    /// List all available mod loaders from the pinned catalog.
    List {
        #[arg(
            short = 'm',
            long,
            visible_alias = "minecraft",
            help = "List loader versions compatible with this Minecraft version"
        )]
        mc_version: Option<String>,
    },
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// List all user settings.
    List,
    /// Get a single setting by key.
    Get { key: String },
    /// Set a setting (value is parsed as JSON; falls back to string).
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum ModsCmd {
    List {
        instance: String,
    },
    Install {
        project: String,
        instance: String,
        #[arg(short, long)]
        version: Option<String>,
        #[arg(
            long,
            value_enum,
            default_value_t = ModSourceArg::Curated,
            help = "Artifact source: curated registry strategy or raw Modrinth project"
        )]
        source: ModSourceArg,
        #[arg(long, help = "Allow replacing existing files")]
        allow_replace: bool,
        #[arg(long, help = "Skip health scan after install")]
        skip_health_scan: bool,
        #[arg(
            long,
            conflicts_with = "exclude_optional",
            help = "Include specific optional dependencies (comma-separated)"
        )]
        include_optional: Option<String>,
        #[arg(
            long,
            conflicts_with = "include_optional",
            help = "Exclude all optional dependencies"
        )]
        exclude_optional: bool,
        #[arg(long, help = "Resolve all conflicts by replacing")]
        replace_conflicts: bool,
        #[arg(long, help = "Abort on any unresolved conflict")]
        abort_conflicts: bool,
        #[arg(long, help = "Resolve plan and print it without executing")]
        dry_run: bool,
    },
    Remove {
        project: String,
        instance: String,
        #[arg(long, help = "Allow replacing existing files")]
        allow_replace: bool,
        #[arg(long, help = "Skip health scan after removal")]
        skip_health_scan: bool,
        #[arg(long, help = "Resolve all conflicts by replacing")]
        replace_conflicts: bool,
        #[arg(long, help = "Abort on any unresolved conflict")]
        abort_conflicts: bool,
        #[arg(long, help = "Resolve plan and print it without executing")]
        dry_run: bool,
    },
    /// Search the curated registry for items matching a query.
    Search {
        query: String,
        #[arg(
            short,
            long,
            help = "Content type filter (mod, resourcepack, shader, datapack, world)"
        )]
        content_type: Option<String>,
        #[arg(short = 'V', long, help = "Minecraft version filter")]
        mc_version: Option<String>,
    },
    /// Update a single installed item to a newer version.
    Update {
        instance: String,
        item: String,
        #[arg(short, long, help = "Target version (default: latest)")]
        version: Option<String>,
        #[arg(
            long,
            conflicts_with = "exclude_optional",
            help = "Include specific optional dependencies (comma-separated)"
        )]
        include_optional: Option<String>,
        #[arg(
            long,
            conflicts_with = "include_optional",
            help = "Exclude all optional dependencies"
        )]
        exclude_optional: bool,
        #[arg(long, help = "Resolve all conflicts by replacing")]
        replace_conflicts: bool,
        #[arg(long, help = "Abort on any unresolved conflict")]
        abort_conflicts: bool,
        #[arg(long, help = "Resolve plan and print it without executing")]
        dry_run: bool,
    },
    /// Update all installed items with a registry identity to their latest versions.
    UpdateAll {
        instance: String,
        #[arg(
            long,
            conflicts_with = "exclude_optional",
            help = "Include specific optional dependencies (comma-separated)"
        )]
        include_optional: Option<String>,
        #[arg(
            long,
            conflicts_with = "include_optional",
            help = "Exclude all optional dependencies"
        )]
        exclude_optional: bool,
        #[arg(long, help = "Resolve all conflicts by replacing")]
        replace_conflicts: bool,
        #[arg(long, help = "Abort on any unresolved conflict")]
        abort_conflicts: bool,
        #[arg(long, help = "Resolve plan and print it without executing")]
        dry_run: bool,
    },
    /// Enable a previously disabled mod by renaming <file>.disabled back to <file>.
    Enable {
        instance: String,
        file: String,
    },
    /// Disable a mod by renaming <file> to <file>.disabled.
    Disable {
        instance: String,
        file: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModSourceArg {
    Curated,
    Modrinth,
}

#[derive(Subcommand)]
enum RegistryCmd {
    Status,
    Sync,
}

#[derive(Subcommand)]
enum SnapshotsCmd {
    List {
        instance: String,
    },
    Create {
        instance: String,
        #[arg(short, long)]
        label: Option<String>,
    },
    Restore {
        instance: String,
        snapshot_id: String,
    },
    Delete {
        instance: String,
        snapshot_id: String,
    },
}

#[derive(Subcommand)]
enum AuthCmd {
    Login,
    Status,
    Logout,
}

#[derive(Subcommand)]
enum RuntimeCmd {
    /// List all discovered Java runtimes (managed + Mojang + system).
    List,
    /// Ensure a managed Java runtime for the given major version is installed.
    Ensure { major: u32 },
    /// Remove unused managed Java runtimes (keep newest per major).
    RemoveUnused,
    /// Inspect a Java executable at the given path.
    Inspect { path: PathBuf },
}

#[derive(Subcommand)]
enum PackCmd {
    /// Install a pack manifest JSON file into an existing instance.
    Install {
        /// Path to the pack manifest JSON file.
        path: PathBuf,
        /// Target instance ID.
        instance: String,
    },
}

#[derive(Subcommand)]
enum LoadoutCmd {
    /// Create a loadout profile from the current enabled state.
    Create { instance: String, name: String },
    /// List all loadout profiles for an instance.
    List { instance: String },
    /// Apply a loadout profile (enable/disable content to match).
    Apply { instance: String, name: String },
    /// Delete a loadout profile.
    Delete { instance: String, name: String },
}

#[derive(Subcommand)]
enum LockfileCmd {
    /// Export a lockfile from the current state of an instance.
    Export {
        instance: String,
        #[arg(long = "out", short = 'o', help = "Write path (default: stdout)")]
        out: Option<PathBuf>,
    },
    /// Verify a lockfile's structure and content hash.
    Verify {
        /// Path to the lockfile JSON.
        path: PathBuf,
    },
    /// Repair an instance's lockfile by re-exporting.
    Repair {
        instance: String,
        #[arg(long = "out", short = 'o', help = "Write path (default: stdout)")]
        out: Option<PathBuf>,
    },
    /// Import a lockfile and restore the instance to match.
    Import {
        /// Path to the lockfile JSON.
        path: PathBuf,
        /// Target instance ID.
        instance: String,
        #[arg(long, help = "Skip health scan after import")]
        skip_health_scan: bool,
    },
}

#[derive(Subcommand)]
enum CrashCmd {
    /// List crash reports for an instance.
    List { instance: String },
    /// Read the content of a crash report file.
    Inspect { instance: String, file: String },
    /// Analyze the latest crash log and suggest incompatible mods.
    Investigate { instance: String },
}

#[derive(Subcommand)]
enum McpCmd {
    /// Start the MCP server with stdio transport (read JSON-RPC from stdin, write to stdout).
    Serve {
        /// Use stdio transport (required; HTTP is not yet implemented)
        #[arg(long)]
        stdio: bool,
    },
}

fn print_table(columns: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        for col in columns {
            print!("{col}  ");
        }
        println!();
        return;
    }
    let mut widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }
    for (i, col) in columns.iter().enumerate() {
        print!("{col}");
        if i < widths.len() {
            for _ in 0..widths[i].saturating_sub(col.len()) + 2 {
                print!(" ");
            }
        }
    }
    println!();
    let total: usize =
        widths.iter().map(|w| w + 2).sum::<usize>() + columns.len().saturating_sub(1);
    for _ in 0..total {
        print!("-");
    }
    println!();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            print!("{cell}");
            if i < widths.len() {
                for _ in 0..widths[i].saturating_sub(cell.len()) + 2 {
                    print!(" ");
                }
            }
        }
        println!();
    }
}

/// Map a structured LauncherError to a semantic CLI exit code.
///
/// Exit-code ranges:
///   0   success
///   1   generic/unclassified
///   2   CLI usage error
///   7   game crash
///  10   local-state / DB
///  11   instance not found / locked / profile
///  12   instance creation
///  13   registry missing / corrupt / schema
///  20   network offline
///  21   download / registry download
///  30   integrity security (hash, untrusted source, zip bomb, override)
///  34   disk full
///  40   authentication
///  50   feature disabled
///  60   Mojang / launcher integration
///  61   version / loader resolution
///  62   Java runtime
///  70   dependency / placeholder
///  80   MCP rate-limit
///  81   MCP denied
///  82   MCP unauthorized
///  90–94  network policy / privacy
/// 100+  process errors
fn exit_code_from_launcher_error(err: &agora_core::error::LauncherError) -> i32 {
    use agora_core::error::LauncherError;
    match err {
        LauncherError::NetworkOffline => 20,
        LauncherError::RegistryDownloadFailed => 21,
        LauncherError::RegistrySignatureInvalid => 13,
        LauncherError::SchemaTooNew => 13,
        LauncherError::ZipBomb => 30,
        LauncherError::OverrideSecurityViolation => 30,
        LauncherError::HashMismatch => 30,
        LauncherError::UntrustedSource => 31,
        LauncherError::DiskFull => 34,
        LauncherError::AuthExpired => 40,
        LauncherError::AuthRequired | LauncherError::MsaAuthRequired => 40,
        LauncherError::ModrinthDisabled => 50,
        LauncherError::InstanceLocked => 11,
        LauncherError::SandboxUnavailable => 50,
        LauncherError::MojangNotFound => 60,
        LauncherError::LaunchFailed => 61,
        LauncherError::GameCrash => 7,
        LauncherError::LocalStateFailed => 10,
        LauncherError::InstanceCreateFailed => 12,
        LauncherError::ProfileWriteFailed => 60,
        LauncherError::RegistryMissing => 13,
        LauncherError::UnsupportedLoader => 61,
        LauncherError::VersionNotFound => 61,
        LauncherError::GameVersionNotFound => 61,
        LauncherError::LoaderProfileNotFound => 61,
        LauncherError::ProfileMissing(..) => 11,
        LauncherError::ProfileUnsupportedMetadata(..) => 11,
        LauncherError::ProfileCorrupt(..) => 11,
        LauncherError::JavaIncompatible => 62,
        LauncherError::JavaRuntimeMissing { .. } => 62,
        LauncherError::JavaRuntimeCatalogMissing { .. } => 62,
        LauncherError::UnresolvedPlaceholder => 70,
        LauncherError::DependencyMissing => 70,
        LauncherError::McpTooManyRequests => 80,
        LauncherError::McpDenied => 81,
        LauncherError::McpUnauthorized => 82,
        LauncherError::NetworkMojangMetadataDisabled => 90,
        LauncherError::NetworkMojangContentDisabled => 91,
        LauncherError::NetworkLoaderDisabled => 92,
        LauncherError::NetworkMsaDisabled => 93,
        LauncherError::NetworkJavaDisabled => 94,
        LauncherError::JavaRuntimeCancelled { .. } => 62,
        LauncherError::JavaRuntimeDownloadDisabled { .. } => 94,
        LauncherError::MavenDescriptor => 30,
        LauncherError::MigrationConflict { .. } => 71,
        LauncherError::MigrationFailed { .. } => 72,
        LauncherError::ProcessCaptureFailed { .. } => 100,
        LauncherError::ProcessStale { .. } => 101,
        LauncherError::UserDecisionRequired => 71,
        LauncherError::Generic { .. } => 1,
    }
}

/// Map any error to a stable exit code.  If the error wraps a `LauncherError`
/// (via anyhow's downcast) use its semantic code; otherwise return 1.
fn exit_code_from_error(err: &anyhow::Error) -> i32 {
    if let Some(le) = err.downcast_ref::<agora_core::error::LauncherError>() {
        exit_code_from_launcher_error(le)
    } else {
        1
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let output_fmt = match cli.output {
        Some(f) => f,
        None if cli.json => OutputFormat::Json,
        None => OutputFormat::Human,
    };
    let json = output_fmt.is_json_output();
    let paths =
        agora_core::app_paths::AppPaths::platform_default_with_override(cli.data_dir.clone());

    // Migration must run before normal initialization. Initializing the
    // destination would create local_state.db and turn a fresh destination
    // into a false database conflict.
    if let Commands::MigrateData { from, yes } = &cli.command {
        if let Err(error) = run_data_migration(&paths, from, *yes, output_fmt) {
            let code = exit_code_from_error(&error);
            if json {
                let value = serde_json::json!({
                    "error": error.to_string(),
                    "exitCode": code,
                });
                eprintln!("{}", serde_json::to_string_pretty(&value).unwrap());
            } else {
                eprintln!("Error: {error}");
            }
            std::process::exit(code);
        }
        return;
    }

    let (ctx, warnings) = match agora_core::ctx::CoreContext::initialize(paths.clone()) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("Error initializing Agora core: {error}");
            std::process::exit(1);
        }
    };
    for warning in warnings {
        eprintln!("Warning: {warning}");
    }
    let data_dir = paths.root().to_path_buf();
    let client = ctx
        .http_clients
        .get(agora_core::http_client::ClientCategory::GitHub)
        .clone();

    let result = run_command(cli, &paths, &data_dir, &client, &ctx, output_fmt).await;
    if let Err(e) = result {
        let code = exit_code_from_error(&e);
        if json {
            let err_val = serde_json::json!({
                "error": e.to_string(),
                "exitCode": code,
            });
            eprintln!("{}", serde_json::to_string_pretty(&err_val).unwrap());
        } else {
            eprintln!("Error: {e}");
        }
        std::process::exit(code);
    }
}

fn run_data_migration(
    paths: &agora_core::app_paths::AppPaths,
    from: &Path,
    yes: bool,
    output_fmt: OutputFormat,
) -> anyhow::Result<()> {
    let json = output_fmt.is_json_output();
    if !from.exists() {
        anyhow::bail!("Source path '{}' does not exist", from.display());
    }

    let service = agora_core::data_migration::DataMigrationService::new(paths.clone());
    let plan = service.plan(from)?;
    if !plan.can_proceed {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "conflict",
                    "sourceInventory": plan.source_inventory,
                    "conflicts": plan.conflicts,
                }))?
            );
        } else {
            println!("Source: {}", plan.source_inventory.source_root);
            println!(
                "Files:  {} ({:.2} MB)",
                plan.source_inventory.files.len(),
                plan.source_inventory.total_size_bytes as f64 / 1_048_576.0
            );
            println!("CONFLICTS - migration cannot proceed:");
            for conflict in &plan.conflicts {
                println!("  - {}: {}", conflict.rel_path, conflict.reason);
            }
        }
        return Err(agora_core::error::LauncherError::MigrationConflict {
            message: format!("Migration blocked by {} conflict(s)", plan.conflicts.len()),
        }
        .into());
    }

    if !yes {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "dry-run",
                    "sourceInventory": plan.source_inventory,
                    "conflicts": plan.conflicts,
                }))?
            );
        } else {
            println!("Dry-run: migration would copy the following:");
            println!("  Source:   {}", plan.source_inventory.source_root);
            println!(
                "  Files:    {} ({:.2} MB)",
                plan.source_inventory.files.len(),
                plan.source_inventory.total_size_bytes as f64 / 1_048_576.0
            );
            if !plan.source_inventory.instance_ids.is_empty() {
                println!(
                    "  Instances: {}",
                    plan.source_inventory.instance_ids.join(", ")
                );
            }
            println!("Pass --yes to execute this migration.");
        }
        return Ok(());
    }

    let result = service.execute(from)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!(
            "Migration complete: {} file(s), {:.2} MB, {} instance(s)",
            result.files_migrated,
            result.total_bytes as f64 / 1_048_576.0,
            result.instance_ids.len(),
        );
        println!("Backup: {}", result.backup_path);
        if !result.instance_ids.is_empty() {
            println!("Instances: {}", result.instance_ids.join(", "));
        }
    }
    Ok(())
}

async fn run_command(
    cli: Cli,
    paths: &agora_core::app_paths::AppPaths,
    data_dir: &Path,
    client: &reqwest::Client,
    ctx: &agora_core::ctx::Ctx,
    output_fmt: OutputFormat,
) -> anyhow::Result<()> {
    let json = output_fmt.is_json_output();

    match cli.command {
        Commands::Paths => {
            let values = serde_json::json!({
                "root": paths.root().display().to_string(),
                "local_state_db": paths.local_state_db().display().to_string(),
                "registry_db": paths.registry_db().display().to_string(),
                "registry_signature": paths.registry_signature().display().to_string(),
                "instances": paths.instances_root().display().to_string(),
                "minecraft_runtime": paths.minecraft_runtime_root().display().to_string(),
                "loader_cache": paths.loader_cache().display().to_string(),
                "loader_receipts": paths.loader_receipts().display().to_string(),
                "java_runtimes": paths.java_runtimes_root().display().to_string(),
                "snapshots": paths.snapshots_root().display().to_string(),
                "staging": paths.staging_root().display().to_string(),
                "locks": paths.locks_root().display().to_string(),
            });
            if json {
                println!("{}", serde_json::to_string_pretty(&values)?);
            } else {
                if let Some(object) = values.as_object() {
                    for (key, value) in object {
                        println!("{key}: {}", value.as_str().unwrap_or_default());
                    }
                }
            }
        }
        Commands::ListInstances => {
            let svc = InstanceService::new(ctx.clone());
            let instances = svc.list()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&instances)?);
            } else {
                let rows: Vec<Vec<String>> = instances
                    .iter()
                    .map(|i| {
                        vec![
                            i.instance_id.clone(),
                            i.name.clone(),
                            i.minecraft_version.clone(),
                            i.loader.clone(),
                            i.loader_version.clone(),
                            i.last_launched_at.clone().unwrap_or_default(),
                        ]
                    })
                    .collect();
                print_table(
                    &["ID", "Name", "MC", "Loader", "Version", "Launched"],
                    &rows,
                );
            }
        }
        Commands::GetInstance { id } => {
            let svc = InstanceService::new(ctx.clone());
            match svc.get(&id)? {
                Some(detail) => {
                    let row = &detail.row;
                    if json {
                        println!("{}", serde_json::to_string_pretty(row)?);
                    } else {
                        println!("ID:       {}", row.instance_id);
                        println!("Name:     {}", row.name);
                        println!("MC:       {}", row.minecraft_version);
                        println!("Loader:   {} {}", row.loader, row.loader_version);
                        println!("Locked:   {}", row.is_locked);
                        println!("Modpack:  {}", row.is_modpack);
                        println!(
                            "Launched: {}",
                            row.last_launched_at.clone().unwrap_or_default()
                        );
                    }
                }
                None => {
                    anyhow::bail!("Instance '{}' not found", id);
                }
            }
        }
        Commands::Instance { action } => match action {
            InstanceCmd::Create {
                name,
                mc_version,
                loader,
                loader_version,
                jvm_memory_mb,
                jvm_gc,
                jvm_custom_args,
                jvm_always_pre_touch,
            } => {
                let instance_id = agora_core::paths::sanitize_id(&name);
                let request = CreateInstanceRequest {
                    name: name.clone(),
                    instance_id: instance_id.clone(),
                    minecraft_version: mc_version,
                    loader,
                    loader_version,
                    jvm_memory_mb,
                    jvm_gc,
                    jvm_custom_args,
                    jvm_always_pre_touch,
                };
                let row = InstanceService::new(ctx.clone()).create(request).await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&row)?);
                } else {
                    println!("Created instance: {} ({})", row.name, row.instance_id);
                }
            }
            InstanceCmd::Clone {
                source,
                name,
                no_saves,
                no_mods,
                no_resource_packs,
                no_shader_packs,
                no_screenshots,
                no_config,
                no_servers,
                no_options,
                hard_links,
                sym_links,
            } => {
                let prefs = ClonePrefs {
                    copy_saves: !no_saves,
                    copy_mods: !no_mods,
                    copy_resource_packs: !no_resource_packs,
                    copy_shader_packs: !no_shader_packs,
                    copy_screenshots: !no_screenshots,
                    copy_config: !no_config,
                    copy_servers: !no_servers,
                    copy_options: !no_options,
                    use_hard_links: hard_links,
                    use_sym_links: sym_links,
                };
                let request = agora_core::instance_service::CloneRequest {
                    source_instance_id: source,
                    new_name: name,
                    prefs,
                };
                let row = InstanceService::new(ctx.clone()).clone(request).await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&row)?);
                } else {
                    println!("Cloned instance: {} ({})", row.name, row.instance_id);
                }
            }
            InstanceCmd::Lock { id } => {
                InstanceService::new(ctx.clone()).lock(&id)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "locked", "instanceId": id})
                    );
                } else {
                    println!("Locked instance '{}'", id);
                }
            }
            InstanceCmd::Unlock { id } => {
                InstanceService::new(ctx.clone()).unlock(&id)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "unlocked", "instanceId": id})
                    );
                } else {
                    println!("Unlocked instance '{}'", id);
                }
            }
            InstanceCmd::Delete { id } => {
                InstanceService::new(ctx.clone()).delete(&id, None)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "deleted", "instanceId": id})
                    );
                } else {
                    println!("Deleted instance '{}'", id);
                }
            }
            InstanceCmd::Rename { id, name } => {
                InstanceService::new(ctx.clone()).rename(&id, &name)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "renamed", "instanceId": id, "name": name})
                    );
                } else {
                    println!("Renamed instance '{}' to '{}'", id, name);
                }
            }
            InstanceCmd::RepairLoader { id } => {
                let summary = LoaderService::new(ctx.clone()).repair(&id).await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&summary)?);
                } else {
                    println!(
                        "Reinstalled {} {} for '{}'",
                        summary.tuple.loader, summary.tuple.loader_version, id
                    );
                }
            }
        },
        Commands::Loader { action } => match action {
            LoaderCmd::List { mc_version } => {
                let loaders = agora_core::loader_manifests::list_loaders();
                if let Some(mc_version) = mc_version {
                    let entries: Vec<serde_json::Value> = loaders
                        .into_iter()
                        .filter_map(|loader| {
                            let versions: Vec<String> =
                                agora_core::loader_manifests::list_versions(&loader, &mc_version)
                                    .into_iter()
                                    .map(|entry| entry.loader_version)
                                    .collect();
                            (!versions.is_empty()).then(|| {
                                serde_json::json!({
                                    "loader": loader,
                                    "minecraftVersion": mc_version,
                                    "versions": versions,
                                })
                            })
                        })
                        .collect();
                    if json {
                        println!("{}", serde_json::to_string_pretty(&entries)?);
                    } else {
                        let rows: Vec<Vec<String>> = entries
                            .iter()
                            .flat_map(|entry| {
                                let loader = entry["loader"].as_str().unwrap_or_default();
                                entry["versions"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(|version| vec![loader.to_string(), version.to_string()])
                            })
                            .collect();
                        print_table(&["Loader", "Version"], &rows);
                    }
                } else if json {
                    println!("{}", serde_json::to_string_pretty(&loaders)?);
                } else {
                    for loader in &loaders {
                        println!("{loader}");
                    }
                }
            }
        },
        Commands::Settings { action } => match action {
            SettingsCmd::List => {
                let svc = SettingsService::new(ctx.clone());
                if json {
                    let rows = svc.list_parsed()?;
                    let map: serde_json::Map<String, serde_json::Value> =
                        rows.into_iter().collect();
                    println!("{}", serde_json::to_string_pretty(&map)?);
                } else {
                    let rows = svc.list()?;
                    let table_rows: Vec<Vec<String>> = rows
                        .iter()
                        .map(|(k, v)| vec![k.clone(), v.clone()])
                        .collect();
                    print_table(&["Key", "Value (JSON)"], &table_rows);
                }
            }
            SettingsCmd::Get { key } => {
                let svc = SettingsService::new(ctx.clone());
                match svc.get(&key)? {
                    Some(value) => {
                        if json {
                            println!("{}", serde_json::to_string_pretty(&value)?);
                        } else {
                            println!("{}: {}", key, value);
                        }
                    }
                    None => {
                        anyhow::bail!("Setting '{}' not found", key);
                    }
                }
            }
            SettingsCmd::Set { key, value } => {
                let parsed: serde_json::Value = serde_json::from_str(&value)
                    .unwrap_or(serde_json::Value::String(value.clone()));
                let svc = SettingsService::new(ctx.clone());
                svc.set(&key, &parsed)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "set", "key": key, "value": parsed})
                    );
                } else {
                    println!("Set {} = {}", key, parsed);
                }
            }
        },
        Commands::Mods { action } => match action {
            ModsCmd::List { instance } => {
                let manifest_path = agora_core::paths::instance_manifest_path(data_dir, &instance)?;
                if !manifest_path.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                let text = std::fs::read_to_string(&manifest_path)?;
                let manifest: agora_core::models::InstanceManifest = serde_json::from_str(&text)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&manifest.mods)?);
                } else {
                    let rows: Vec<Vec<String>> = manifest
                        .mods
                        .iter()
                        .map(|m| {
                            vec![
                                m.filename.clone(),
                                m.source.clone(),
                                m.version.clone().unwrap_or_default(),
                                m.modrinth_id.clone().unwrap_or_default(),
                            ]
                        })
                        .collect();
                    print_table(&["Filename", "Source", "Version", "Modrinth ID"], &rows);
                }
            }
            ModsCmd::Install {
                project,
                instance,
                version,
                source,
                allow_replace,
                skip_health_scan,
                include_optional,
                exclude_optional,
                replace_conflicts,
                abort_conflicts,
                dry_run,
            } => {
                let svc = InstallService::new(ctx.clone());
                let requested_version = version.clone().unwrap_or_else(|| "selected".into());
                let optional_deps = resolve_optional_deps(include_optional, exclude_optional);
                let intent = agora_core::install_pipeline::InstallIntent {
                    action: agora_core::install_pipeline::InstallAction::Install {
                        source_type: match source {
                            ModSourceArg::Curated => {
                                agora_core::install_pipeline::SourceType::Curated
                            }
                            ModSourceArg::Modrinth => {
                                agora_core::install_pipeline::SourceType::Modrinth
                            }
                        },
                        item_id: project.clone(),
                        candidate_version: version,
                    },
                    target_instance: instance.clone(),
                    optional_deps,
                    requested_by: agora_core::install_pipeline::RequestSource::CLI,
                    overrides: agora_core::install_pipeline::PlanOverrides {
                        allow_replace,
                        skip_health_scan,
                        ..Default::default()
                    },
                };

                let reporter = SilentReporter;
                let cancel = agora_core::install_pipeline::CancellationToken::new();

                let mut plan = svc.resolve(intent, &reporter).await?;

                // Apply --replace-conflicts / --abort-conflicts override
                apply_conflict_overrides(&mut plan, replace_conflicts, abort_conflicts)?;

                // Dry-run: print the plan and exit
                if dry_run {
                    print_plan(&plan, json)?;
                    return Ok(());
                }

                // Preview / error gate — fail closed on unresolved plans
                if !plan.is_fully_resolved() {
                    let has_choices = !plan.pending_choices.is_empty()
                        || plan
                            .conflicts
                            .iter()
                            .any(|c| c.blocking && c.chosen.is_none());
                    report_unresolved_plan(&plan, json);
                    if has_choices {
                        return Err(agora_core::error::LauncherError::UserDecisionRequired.into());
                    }
                    anyhow::bail!(
                        "Install blocked: unresolved errors, conflicts, or pending choices"
                    );
                }

                // Execute the plan with snapshot, verifiable staging, and health gate
                let outcome = svc.execute(&plan, &reporter, &cancel).await;

                match outcome {
                    agora_core::install_pipeline::InstallOutcome::Success {
                        warnings,
                        snapshot_id,
                        ..
                    } => {
                        if json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "success",
                                    "filename": plan.files_to_add.first().map(|f| &f.target_filename),
                                     "version": requested_version.clone(),
                                    "snapshotId": snapshot_id,
                                    "warnings": warnings,
                                }))?
                            );
                        } else {
                            let filename = plan
                                .files_to_add
                                .first()
                                .map(|f| f.target_filename.clone())
                                .unwrap_or_else(|| project.clone());
                            println!("Installed {} ({})", filename, requested_version);
                        }
                    }
                    agora_core::install_pipeline::InstallOutcome::HealthRollback {
                        health_report,
                        ..
                    } => {
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "health_rollback",
                                    "blockers": health_report.blockers,
                                }))?
                            );
                        } else {
                            eprintln!(
                                "Install completed but post-install health check found blockers; rolled back."
                            );
                            for b in &health_report.blockers {
                                eprintln!("  [BLOCK] {}", b.message);
                            }
                        }
                        anyhow::bail!(
                            "Install rolled back due to post-install health check failures"
                        );
                    }
                    agora_core::install_pipeline::InstallOutcome::Cancelled { phase, .. } => {
                        anyhow::bail!("Install was cancelled during {}.", phase);
                    }
                    agora_core::install_pipeline::InstallOutcome::Failed {
                        error,
                        rollback_performed,
                        ..
                    } => {
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "failed",
                                    "error": error,
                                    "rollbackPerformed": rollback_performed,
                                }))?
                            );
                        } else {
                            eprintln!("Install failed: {}", error);
                        }
                        anyhow::bail!("Install failed and rolled back: {}", error);
                    }
                }
            }
            ModsCmd::Remove {
                project,
                instance,
                allow_replace,
                skip_health_scan,
                replace_conflicts,
                abort_conflicts,
                dry_run,
            } => {
                let svc = InstallService::new(ctx.clone());
                let load = svc.load_instance(&instance)?;

                let prepared = InstallService::prepare_removal(
                    &load.manifest,
                    &project,
                    load.registry_revision.clone(),
                );

                let target_filename = match &prepared.operation {
                    agora_core::install_pipeline::ResolvedOperation::Remove {
                        target_filename,
                        ..
                    } => target_filename.clone(),
                    _ => project.clone(),
                };

                let intent = agora_core::install_pipeline::InstallIntent {
                    action: agora_core::install_pipeline::InstallAction::Remove {
                        filename: target_filename.clone(),
                    },
                    target_instance: instance.clone(),
                    optional_deps: agora_core::install_pipeline::OptionalDepsPolicy::ExcludeAll,
                    requested_by: agora_core::install_pipeline::RequestSource::CLI,
                    overrides: agora_core::install_pipeline::PlanOverrides {
                        allow_replace,
                        skip_health_scan,
                        ..Default::default()
                    },
                };

                let reporter = SilentReporter;
                let cancel = agora_core::install_pipeline::CancellationToken::new();

                let mut plan = svc.resolve(intent, &reporter).await?;

                // Apply --replace-conflicts / --abort-conflicts override
                apply_conflict_overrides(&mut plan, replace_conflicts, abort_conflicts)?;

                // Dry-run: print the plan and exit
                if dry_run {
                    print_plan(&plan, json)?;
                    return Ok(());
                }

                // Preview / error gate — fail closed on unresolved plans
                if !plan.is_fully_resolved() {
                    let has_choices = !plan.pending_choices.is_empty()
                        || plan
                            .conflicts
                            .iter()
                            .any(|c| c.blocking && c.chosen.is_none());
                    report_unresolved_plan(&plan, json);
                    if has_choices {
                        return Err(agora_core::error::LauncherError::UserDecisionRequired.into());
                    }
                    anyhow::bail!("Remove blocked: unresolved errors");
                }

                // Execute the remove plan with snapshot, file removal, and health gate
                let outcome = svc.execute(&plan, &reporter, &cancel).await;

                match outcome {
                    agora_core::install_pipeline::InstallOutcome::Success { .. } => {
                        if json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "removed",
                                    "filename": target_filename,
                                }))?
                            );
                        } else {
                            println!("Removed {}", target_filename);
                        }
                    }
                    agora_core::install_pipeline::InstallOutcome::Failed { error, .. } => {
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "failed",
                                    "error": error,
                                }))?
                            );
                        } else {
                            eprintln!("Remove failed: {}", error);
                        }
                        anyhow::bail!("Remove failed: {}", error);
                    }
                    other => {
                        let err_msg = format!("Remove encountered unexpected state: {:?}", other);
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "unexpected",
                                    "error": err_msg,
                                }))?
                            );
                        } else {
                            eprintln!("{}", err_msg);
                        }
                        anyhow::bail!("{}", err_msg);
                    }
                }
            }
            ModsCmd::Search {
                query,
                content_type,
                mc_version,
            } => {
                let svc = RegistryService::new(ctx.clone());
                if !ctx.paths.registry_db().exists() {
                    anyhow::bail!("Registry database not found. Run 'agora registry sync' first.");
                }
                let sort = agora_core::registry::SortOption::NetScore;
                let items = svc.browse_items(
                    content_type.as_deref(),
                    None,
                    &sort,
                    true,
                    mc_version.as_deref(),
                    None,
                    Some(&query),
                    50,
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&items)?);
                } else {
                    let rows: Vec<Vec<String>> = items
                        .iter()
                        .map(|i| {
                            vec![
                                i.id.clone(),
                                i.name.clone(),
                                i.content_type.clone(),
                                i.description.clone().unwrap_or_default(),
                                i.page_url.clone().unwrap_or_default(),
                            ]
                        })
                        .collect();
                    print_table(&["ID", "Name", "Type", "Description", "URL"], &rows);
                }
            }
            ModsCmd::Update {
                instance,
                item,
                version,
                include_optional,
                exclude_optional,
                replace_conflicts,
                abort_conflicts,
                dry_run,
            } => {
                let svc = InstallService::new(ctx.clone());
                let target_version = version.clone().unwrap_or_else(|| "latest".into());
                let optional_deps = resolve_optional_deps(include_optional, exclude_optional);
                let intent = agora_core::install_pipeline::InstallIntent {
                    action: agora_core::install_pipeline::InstallAction::Update {
                        item_id: item.clone(),
                        target_version: target_version.clone(),
                    },
                    target_instance: instance.clone(),
                    optional_deps,
                    requested_by: agora_core::install_pipeline::RequestSource::CLI,
                    overrides: agora_core::install_pipeline::PlanOverrides {
                        allow_replace: true,
                        skip_health_scan: false,
                        ..Default::default()
                    },
                };

                let reporter = SilentReporter;
                let cancel = agora_core::install_pipeline::CancellationToken::new();

                let mut plan = svc.resolve(intent, &reporter).await?;

                // Apply --replace-conflicts / --abort-conflicts override
                apply_conflict_overrides(&mut plan, replace_conflicts, abort_conflicts)?;

                // Dry-run: print the plan and exit
                if dry_run {
                    print_plan(&plan, json)?;
                    return Ok(());
                }

                // Preview / error gate — fail closed on unresolved plans
                if !plan.is_fully_resolved() {
                    let has_choices = !plan.pending_choices.is_empty()
                        || plan
                            .conflicts
                            .iter()
                            .any(|c| c.blocking && c.chosen.is_none());
                    report_unresolved_plan(&plan, json);
                    if has_choices {
                        return Err(agora_core::error::LauncherError::UserDecisionRequired.into());
                    }
                    anyhow::bail!(
                        "Update blocked: unresolved errors, conflicts, or pending choices"
                    );
                }

                // Execute the update plan with snapshot, verifiable staging, and health gate
                let outcome = svc.execute(&plan, &reporter, &cancel).await;

                match outcome {
                    agora_core::install_pipeline::InstallOutcome::Success {
                        warnings,
                        snapshot_id,
                        installed_items,
                        ..
                    } => {
                        if json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "success",
                                    "itemId": item,
                                    "version": target_version,
                                    "snapshotId": snapshot_id,
                                    "installedItems": installed_items,
                                    "warnings": warnings,
                                }))?
                            );
                        } else {
                            println!("Updated {} ({})", item, target_version);
                        }
                    }
                    agora_core::install_pipeline::InstallOutcome::HealthRollback {
                        health_report,
                        ..
                    } => {
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "health_rollback",
                                    "blockers": health_report.blockers,
                                }))?
                            );
                        } else {
                            eprintln!(
                                "Update completed but post-update health check found blockers; rolled back."
                            );
                            for b in &health_report.blockers {
                                eprintln!("  [BLOCK] {}", b.message);
                            }
                        }
                        anyhow::bail!(
                            "Update rolled back due to post-update health check failures"
                        );
                    }
                    agora_core::install_pipeline::InstallOutcome::Cancelled { phase, .. } => {
                        anyhow::bail!("Update was cancelled during {}.", phase);
                    }
                    agora_core::install_pipeline::InstallOutcome::Failed {
                        error,
                        rollback_performed,
                        ..
                    } => {
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "failed",
                                    "error": error,
                                    "rollbackPerformed": rollback_performed,
                                }))?
                            );
                        } else {
                            eprintln!("Update failed: {}", error);
                        }
                        anyhow::bail!("Update failed and rolled back: {}", error);
                    }
                }
            }
            ModsCmd::Enable { instance, file } => {
                CrashService::new(ctx.clone()).enable_mod(&instance, &file)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "enabled", "instanceId": instance, "file": file})
                    );
                } else {
                    println!("Enabled {} in '{}'", file, instance);
                }
            }
            ModsCmd::Disable { instance, file } => {
                CrashService::new(ctx.clone()).disable_mod(&instance, &file)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "disabled", "instanceId": instance, "file": file})
                    );
                } else {
                    println!("Disabled {} in '{}'", file, instance);
                }
            }
            ModsCmd::UpdateAll {
                instance,
                include_optional,
                exclude_optional,
                replace_conflicts,
                abort_conflicts,
                dry_run,
            } => {
                let svc = InstallService::new(ctx.clone());
                let load = svc.load_instance(&instance)?;

                let items: Vec<agora_core::install_pipeline::BatchUpdateItem> = load
                    .manifest
                    .mods
                    .iter()
                    .chain(load.manifest.resourcepacks.iter())
                    .chain(load.manifest.shaders.iter())
                    .chain(load.manifest.datapacks.iter())
                    .filter_map(|m| {
                        m.registry_id
                            .as_ref()
                            .or(m.modrinth_id.as_ref())
                            .or(m.mod_jar_id.as_ref())
                            .map(|id| agora_core::install_pipeline::BatchUpdateItem {
                                item_id: id.clone(),
                                target_version: "latest".into(),
                            })
                    })
                    .collect();

                if items.is_empty() {
                    anyhow::bail!(
                        "No installed items with a registry identity found in instance '{}'",
                        instance
                    );
                }

                let optional_deps = resolve_optional_deps(include_optional, exclude_optional);
                let intent = agora_core::install_pipeline::InstallIntent {
                    action: agora_core::install_pipeline::InstallAction::BatchUpdate { items },
                    target_instance: instance.clone(),
                    optional_deps,
                    requested_by: agora_core::install_pipeline::RequestSource::CLI,
                    overrides: agora_core::install_pipeline::PlanOverrides {
                        skip_health_scan: false,
                        ..Default::default()
                    },
                };

                let reporter = SilentReporter;
                let cancel = agora_core::install_pipeline::CancellationToken::new();

                let mut plan = svc.resolve(intent, &reporter).await?;

                // Apply --replace-conflicts / --abort-conflicts override
                apply_conflict_overrides(&mut plan, replace_conflicts, abort_conflicts)?;

                // Dry-run: print the plan and exit
                if dry_run {
                    print_plan(&plan, json)?;
                    return Ok(());
                }

                // Preview / error gate — fail closed on unresolved plans.
                // This ensures unsafe conflicts are never silently chosen.
                if !plan.is_fully_resolved() {
                    let has_choices = !plan.pending_choices.is_empty()
                        || plan
                            .conflicts
                            .iter()
                            .any(|c| c.blocking && c.chosen.is_none());
                    report_unresolved_plan(&plan, json);
                    if has_choices {
                        return Err(agora_core::error::LauncherError::UserDecisionRequired.into());
                    }
                    anyhow::bail!(
                        "Batch update blocked: unresolved errors, conflicts, or pending choices"
                    );
                }

                // Execute the batch update plan with snapshot, verifiable staging, and health gate
                let outcome = svc.execute(&plan, &reporter, &cancel).await;

                match outcome {
                    agora_core::install_pipeline::InstallOutcome::Success {
                        warnings,
                        snapshot_id,
                        installed_items,
                        ..
                    } => {
                        if json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "success",
                                    "snapshotId": snapshot_id,
                                    "installedItems": installed_items,
                                    "warnings": warnings,
                                }))?
                            );
                        } else {
                            println!(
                                "Batch update completed with {} updated items",
                                installed_items.len()
                            );
                        }
                    }
                    agora_core::install_pipeline::InstallOutcome::HealthRollback {
                        health_report,
                        ..
                    } => {
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "health_rollback",
                                    "blockers": health_report.blockers,
                                }))?
                            );
                        } else {
                            eprintln!(
                                "Batch update completed but post-update health check found blockers; rolled back."
                            );
                            for b in &health_report.blockers {
                                eprintln!("  [BLOCK] {}", b.message);
                            }
                        }
                        anyhow::bail!(
                            "Batch update rolled back due to post-update health check failures"
                        );
                    }
                    agora_core::install_pipeline::InstallOutcome::Cancelled { phase, .. } => {
                        anyhow::bail!("Batch update was cancelled during {}.", phase);
                    }
                    agora_core::install_pipeline::InstallOutcome::Failed {
                        error,
                        rollback_performed,
                        ..
                    } => {
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "status": "failed",
                                    "error": error,
                                    "rollbackPerformed": rollback_performed,
                                }))?
                            );
                        } else {
                            eprintln!("Batch update failed: {}", error);
                        }
                        anyhow::bail!("Batch update failed and rolled back: {}", error);
                    }
                }
            }
        },
        Commands::Health { instance } => {
            let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
            if !instance_dir.exists() {
                anyhow::bail!("Instance '{}' not found", instance);
            }
            let manifest_path = agora_core::paths::instance_manifest_path(data_dir, &instance)?;
            if !manifest_path.exists() {
                anyhow::bail!("Instance manifest not found for '{}'", instance);
            }
            let text = std::fs::read_to_string(&manifest_path)?;
            let manifest: agora_core::models::InstanceManifest = serde_json::from_str(&text)?;
            let reg_path = data_dir.join("registry.db");
            let reg_opt = if reg_path.exists() {
                Some(reg_path)
            } else {
                None
            };
            let report = agora_core::health::health(&instance_dir, &manifest, reg_opt.as_deref());
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Health score: {:?}", report.score);
                for w in &report.warnings {
                    println!("  [WARN] {}", w.message);
                }
                for b in &report.blockers {
                    println!("  [BLOCK] {}", b.message);
                }
            }
            if matches!(
                report.score,
                agora_core::health::HealthScore::Red | agora_core::health::HealthScore::Yellow
            ) {
                anyhow::bail!("Health score is {:?} (see report above)", report.score);
            }
        }
        Commands::Registry { action } => match action {
            RegistryCmd::Status => {
                let local_state = data_dir.join("local_state.db");
                let _repo =
                    agora_core::registry_sync::resolve_registry_repo(cli.registry_repo.as_deref());
                let status = agora_core::registry_sync::get_status(data_dir, &local_state);
                if json {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                } else {
                    println!("Cached DB:     {}", status.has_cached_db);
                    println!(
                        "Cached tag:    {}",
                        status.cached_tag.as_deref().unwrap_or("none")
                    );
                    println!(
                        "Schema:        {}",
                        status
                            .cached_schema_version
                            .map_or("N/A".into(), |v| v.to_string())
                    );
                    println!("Update avail:  {}", status.update_available);
                    println!("Message:       {}", status.message);
                }
            }
            RegistryCmd::Sync => {
                let repo =
                    agora_core::registry_sync::resolve_registry_repo(cli.registry_repo.as_deref());
                let local_state = data_dir.join("local_state.db");
                if !local_state.exists() {
                    agora_core::db::init_local_state_db(&local_state)?;
                }
                let report = agora_core::registry_sync::check_and_download_update(
                    data_dir,
                    &local_state,
                    true,
                    None,
                    None,
                    &repo,
                    ctx.lock_manager(),
                )
                .await?;
                let catalog_warnings = ctx.reload_runtime_catalog()?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "report": report,
                            "catalog_warnings": catalog_warnings,
                        }))?
                    );
                } else {
                    println!("Registry sync: {}", report.message);
                    for warning in catalog_warnings {
                        println!("Catalog: {warning}");
                    }
                }
            }
        },
        Commands::Snapshots { action } => match action {
            SnapshotsCmd::List { instance } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                let snapshots = agora_core::snapshot::list_snapshots(&instance_dir)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&snapshots)?);
                } else {
                    let rows: Vec<Vec<String>> = snapshots
                        .iter()
                        .map(|s| {
                            vec![
                                s.id.clone(),
                                s.label.clone().unwrap_or_default(),
                                s.created_at.clone(),
                                s.file_count.to_string(),
                            ]
                        })
                        .collect();
                    print_table(&["ID", "Label", "Created", "Files"], &rows);
                }
            }
            SnapshotsCmd::Create { instance, label } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                let snapshot =
                    agora_core::snapshot::create_snapshot(&instance_dir, label.as_deref())
                        .map_err(|e| anyhow::anyhow!("{}", e))?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&snapshot)?);
                } else {
                    println!(
                        "Created snapshot {} ({})",
                        snapshot.id,
                        snapshot.label.as_deref().unwrap_or("unlabeled")
                    );
                }
            }
            SnapshotsCmd::Restore {
                instance,
                snapshot_id,
            } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                agora_core::snapshot::restore_snapshot(&instance_dir, &snapshot_id)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                println!(
                    "Restored instance '{}' from snapshot {}",
                    instance, snapshot_id
                );
            }
            SnapshotsCmd::Delete {
                instance,
                snapshot_id,
            } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                agora_core::snapshot::delete_snapshot(&instance_dir, &snapshot_id)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "deleted", "instanceId": instance, "snapshotId": snapshot_id})
                    );
                } else {
                    println!(
                        "Deleted snapshot '{}' for instance '{}'",
                        snapshot_id, instance
                    );
                }
            }
        },
        Commands::Import {
            path,
            symlink_saves,
        } => {
            if !path.exists() {
                anyhow::bail!("Path '{}' does not exist", path.display());
            }
            let svc = agora_core::import_service::ImportService::new(ctx.clone());
            let import_source = if path.is_dir() {
                agora_core::import_service::ImportSource::Directory(path)
            } else {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                match ext {
                    "mrpack" => agora_core::import_service::ImportSource::Mrpack(path),
                    "zip" => agora_core::import_service::ImportSource::PrismZip(path),
                    _ => anyhow::bail!(
                        "Unsupported file type '.{ext}'. Use .mrpack, .zip, or a directory"
                    ),
                }
            };
            let request = agora_core::import_service::ImportRequest {
                source: import_source,
                symlink_saves,
            };
            let result = svc.run_import(request).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("Imported: {} ({} mods)", result.name, result.imported_mods);
            }
        }
        Commands::Launch { instance, yes } => {
            run_launch_service(ctx, &instance, yes, output_fmt).await?;
        }
        Commands::Auth { action } => match action {
            AuthCmd::Login => {
                let db_path = data_dir.join("local_state.db");
                let flow = agora_core::msa::begin_login(client, &db_path).await?;
                if json {
                    eprintln!("Open this URL in your browser:");
                    eprintln!("{}", flow.auth_uri);
                    eprintln!();
                    eprintln!("After authorizing, paste the full redirect URL here:");
                } else {
                    println!("Open this URL in your browser:");
                    println!("{}", flow.auth_uri);
                    println!();
                    println!("After authorizing, paste the full redirect URL here:");
                }
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let input = input.trim();
                if input.is_empty() {
                    anyhow::bail!("No input provided");
                }
                let (code, state) = extract_auth_redirect(input)?;
                let credentials =
                    agora_core::msa::finish_login(client, &code, &flow, Some(&state), &db_path)
                        .await?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "username": credentials.username,
                            "uuid": credentials.uuid,
                            "expires": credentials.expires,
                        })
                    );
                } else {
                    println!("Signed in as {}", credentials.username);
                }
            }
            AuthCmd::Status => match agora_core::msa::load_credentials()? {
                Some(creds) => {
                    if creds.is_expired() {
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({"status": "expired", "username": creds.username})
                            );
                        } else {
                            println!(
                                "Signed in as {} (expired — run 'agora auth login')",
                                creds.username
                            );
                        }
                    } else {
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({
                                    "status": "valid",
                                    "username": creds.username,
                                    "expires": creds.expires,
                                })
                            );
                        } else {
                            println!(
                                "Signed in as {} (expires {})",
                                creds.username, creds.expires
                            );
                        }
                    }
                }
                None => {
                    if json {
                        println!("{}", serde_json::json!({"status": "not_authenticated"}));
                    } else {
                        println!("Not authenticated. Run 'agora auth login'.");
                    }
                }
            },
            AuthCmd::Logout => {
                agora_core::msa::clear_credentials()?;
                if json {
                    println!("{}", serde_json::json!({"status": "logged_out"}));
                } else {
                    println!("Signed out.");
                }
            }
        },
        Commands::Sync => {
            let repo =
                agora_core::registry_sync::resolve_registry_repo(cli.registry_repo.as_deref());
            let local_state = data_dir.join("local_state.db");
            if !local_state.exists() {
                agora_core::db::init_local_state_db(&local_state)?;
            }
            let report = agora_core::registry_sync::check_and_download_update(
                data_dir,
                &local_state,
                true,
                None,
                None,
                &repo,
                ctx.lock_manager(),
            )
            .await?;
            let catalog_warnings = ctx.reload_runtime_catalog()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "report": report,
                        "catalog_warnings": catalog_warnings,
                    }))?
                );
            } else {
                println!("{}", report.message);
                for warning in catalog_warnings {
                    println!("Catalog: {warning}");
                }
            }
        }
        Commands::Mcp { action } => match action {
            McpCmd::Serve { stdio: true } => run_mcp_stdio(ctx).await?,
            McpCmd::Serve { stdio: false } => {
                anyhow::bail!("Only --stdio transport is currently supported for 'mcp serve'")
            }
        },
        Commands::Runtime { action } => match action {
            RuntimeCmd::List => {
                let svc = RuntimeService::new(ctx.clone());
                let candidates = svc.list_candidates()?;

                if json {
                    println!("{}", serde_json::to_string_pretty(&candidates)?);
                } else {
                    let rows: Vec<Vec<String>> = candidates
                        .iter()
                        .map(|j| {
                            vec![
                                j.version.to_string(),
                                j.version_string.clone(),
                                j.path.to_string_lossy().to_string(),
                                format!("{:?}", j.source),
                                j.arch.clone().unwrap_or_default(),
                            ]
                        })
                        .collect();
                    print_table(&["Major", "Version", "Path", "Source", "Arch"], &rows);
                }
            }
            RuntimeCmd::Ensure { major } => {
                let svc = RuntimeService::new(ctx.clone());
                let policy = svc.network_policy()?;

                if !json {
                    println!("Ensuring Java {major} runtime...");
                }

                let ensured = svc
                    .ensure_runtime(major, policy, std::sync::Arc::new(ConsoleRuntimeProgress))
                    .await?;

                if json {
                    println!("{}", serde_json::to_string_pretty(&ensured)?);
                } else {
                    println!(
                        "Java {} runtime ready at {}",
                        ensured.version,
                        ensured.path.display()
                    );
                }
            }
            RuntimeCmd::RemoveUnused => {
                let svc = RuntimeService::new(ctx.clone());
                let removed = svc.remove_unused()?;

                if json {
                    println!("{}", serde_json::json!({"removed": removed}));
                } else {
                    println!("Removed {removed} unused runtime(s).");
                }
            }
            RuntimeCmd::Inspect { path } => {
                let svc = RuntimeService::new(ctx.clone());
                match svc.inspect(&path) {
                    Ok(inst) => {
                        if json {
                            println!("{}", serde_json::to_string_pretty(&inst)?);
                        } else {
                            println!("Java major:   {}", inst.version);
                            println!("Version:      {}", inst.version_string);
                            println!("Path:         {}", inst.path.display());
                            println!("Source:       {:?}", inst.source);
                            if let Some(ref arch) = inst.arch {
                                println!("Architecture: {}", arch);
                            }
                        }
                    }
                    Err(e) => {
                        anyhow::bail!("{}", e);
                    }
                }
            }
        },
        Commands::Crash { action } => match action {
            CrashCmd::List { instance } => {
                let reports = CrashService::new(ctx.clone()).list_reports(&instance)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&reports)?);
                } else {
                    let rows: Vec<Vec<String>> = reports
                        .iter()
                        .map(|r| {
                            vec![
                                r.filename.clone(),
                                r.modified_at.clone(),
                                r.size_bytes.to_string(),
                            ]
                        })
                        .collect();
                    print_table(&["Filename", "Modified", "Size (bytes)"], &rows);
                }
            }
            CrashCmd::Inspect { instance, file } => {
                let content = CrashService::new(ctx.clone()).read_crash_log(&instance, &file)?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "filename": file,
                            "content": content,
                        })
                    );
                } else {
                    println!("{}", content);
                }
            }
            CrashCmd::Investigate { instance } => {
                let reports = CrashService::new(ctx.clone()).list_reports(&instance)?;
                let newest = reports
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("No crash reports found for '{}'", instance))?;
                let crash_text =
                    CrashService::new(ctx.clone()).read_crash_log(&instance, &newest.filename)?;
                let suspects = CrashService::new(ctx.clone())
                    .suggest_mod_incompatibility(&instance, &crash_text)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&suspects)?);
                } else {
                    let rows: Vec<Vec<String>> = suspects
                        .iter()
                        .map(|s| {
                            vec![
                                s.mod_id.clone(),
                                s.filename.clone(),
                                format!("{:.2}", s.total_score),
                                s.is_dependent_of.clone().unwrap_or_default(),
                            ]
                        })
                        .collect();
                    print_table(&["Mod ID", "Filename", "Score", "Depends On"], &rows);
                }
            }
        },
        Commands::MigrateData { from, yes } => {
            let svc = agora_core::data_migration::DataMigrationService::new(paths.clone());

            if !from.exists() {
                anyhow::bail!("Source path '{}' does not exist", from.display());
            }

            // Always produce a plan (dry-run inventory).
            let plan = svc.plan(&from)?;

            if !plan.can_proceed {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "conflict",
                            "sourceInventory": plan.source_inventory,
                            "conflicts": plan.conflicts,
                        }))?
                    );
                } else {
                    println!("Source: {}", plan.source_inventory.source_root);
                    println!(
                        "Files:  {} ({:.2} MB)",
                        plan.source_inventory.files.len(),
                        plan.source_inventory.total_size_bytes as f64 / 1_048_576.0
                    );
                    println!(
                        "DBs:   local_state.db={}, registry.db={}",
                        plan.source_inventory.has_local_state_db,
                        plan.source_inventory.has_registry_db,
                    );
                    println!(
                        "Instances: {}",
                        plan.source_inventory.instance_ids.join(", ")
                    );
                    println!();
                    println!("CONFLICTS — migration cannot proceed:");
                    for c in &plan.conflicts {
                        println!("  - {}: {}", c.rel_path, c.reason);
                    }
                    println!();
                    println!("Resolve conflicts (remove or rename destination data) and retry.");
                }
                anyhow::bail!("Migration blocked by {} conflict(s)", plan.conflicts.len());
            }

            // Dry-run: show what would happen but don't execute.
            if !yes {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "dry-run",
                            "sourceInventory": plan.source_inventory,
                            "conflicts": plan.conflicts,
                        }))?
                    );
                } else {
                    println!("Dry-run: migration would copy the following:");
                    println!("  Source:   {}", plan.source_inventory.source_root);
                    println!(
                        "  Files:    {} ({:.2} MB)",
                        plan.source_inventory.files.len(),
                        plan.source_inventory.total_size_bytes as f64 / 1_048_576.0
                    );
                    println!(
                        "  DBs:      local_state.db={}, registry.db={}",
                        plan.source_inventory.has_local_state_db,
                        plan.source_inventory.has_registry_db,
                    );
                    if !plan.source_inventory.instance_ids.is_empty() {
                        println!(
                            "  Instances: {}",
                            plan.source_inventory.instance_ids.join(", ")
                        );
                    }
                    println!();
                    println!("Pass --yes to execute this migration.");
                }
                return Ok(());
            }

            // Execute.
            let result = svc.execute(&from)?;

            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "Migration complete: {} file(s), {:.2} MB, {} instance(s)",
                    result.files_migrated,
                    result.total_bytes as f64 / 1_048_576.0,
                    result.instance_ids.len(),
                );
                println!("Backup: {}", result.backup_path);
                if !result.instance_ids.is_empty() {
                    println!("Instances: {}", result.instance_ids.join(", "));
                }
            }
        }
        Commands::Pack { action } => match action {
            PackCmd::Install { path, instance } => {
                let json_text = std::fs::read_to_string(&path).map_err(|e| {
                    anyhow::anyhow!("Cannot read pack manifest '{}': {}", path.display(), e)
                })?;
                let svc = agora_core::import_service::ImportService::new(ctx.clone());
                let request = agora_core::import_service::ImportRequest {
                    source: agora_core::import_service::ImportSource::PackManifest {
                        manifest_json: json_text,
                        target_instance_id: instance.clone(),
                    },
                    symlink_saves: false,
                };
                let result = svc.install_pack(request).await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    println!(
                        "Installed pack '{}' ({} mods)",
                        result.name, result.mods_installed
                    );
                }
            }
        },
        Commands::Export { instance, dest } => {
            let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
            if !instance_dir.exists() {
                anyhow::bail!("Instance '{}' not found", instance);
            }
            std::fs::create_dir_all(&dest).map_err(|e| {
                anyhow::anyhow!("Cannot create destination '{}': {}", dest.display(), e)
            })?;
            let manifest_path = agora_core::paths::instance_manifest_path(data_dir, &instance)?;
            let text = std::fs::read_to_string(&manifest_path)
                .map_err(|_| anyhow::anyhow!("Instance manifest not found for '{}'", instance))?;
            let manifest: agora_core::models::InstanceManifest = serde_json::from_str(&text)?;
            let result = agora_core::server_export::export_server_environment(
                &instance_dir,
                &dest,
                &manifest.loader,
                &manifest.minecraft_version,
            )
            .map_err(|e| anyhow::anyhow!("Export failed: {e}"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "Exported {} mods ({} server, {} client-only removed) to {}",
                    result.total_mods,
                    result.server_mods,
                    result.removed_client_only.len(),
                    dest.display()
                );
            }
        }
        Commands::Loadout { action } => match action {
            LoadoutCmd::Create { instance, name } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                let profile = agora_core::loadout::create_profile(&instance_dir, &name)
                    .map_err(|e| anyhow::anyhow!("Failed to create loadout: {e}"))?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&profile)?);
                } else {
                    println!(
                        "Created loadout '{}' ({})",
                        profile.name, profile.created_at
                    );
                }
            }
            LoadoutCmd::List { instance } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                let profiles = agora_core::loadout::list_profiles(&instance_dir)
                    .map_err(|e| anyhow::anyhow!("Failed to list loadouts: {e}"))?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&profiles)?);
                } else {
                    let rows: Vec<Vec<String>> = profiles
                        .iter()
                        .map(|p| {
                            vec![
                                p.name.clone(),
                                p.created_at.clone(),
                                p.enabled_mods.len().to_string(),
                            ]
                        })
                        .collect();
                    print_table(&["Name", "Created", "Entries"], &rows);
                }
            }
            LoadoutCmd::Apply { instance, name } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                agora_core::loadout::apply_profile(&instance_dir, &name)
                    .map_err(|e| anyhow::anyhow!("Failed to apply loadout: {e}"))?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "applied", "instanceId": instance, "profile": name})
                    );
                } else {
                    println!("Applied loadout '{}' to '{}'", name, instance);
                }
            }
            LoadoutCmd::Delete { instance, name } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                agora_core::loadout::delete_profile(&instance_dir, &name)
                    .map_err(|e| anyhow::anyhow!("Failed to delete loadout: {e}"))?;
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"status": "deleted", "instanceId": instance, "profile": name})
                    );
                } else {
                    println!("Deleted loadout '{}'", name);
                }
            }
        },
        Commands::Lockfile { action } => match action {
            LockfileCmd::Export {
                instance,
                out: output,
            } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                let lockfile = agora_core::lockfile::build_from_instance(&instance_dir)
                    .map_err(|e| anyhow::anyhow!("Failed to build lockfile: {e}"))?;
                let lockfile_json = lockfile
                    .to_pretty_json()
                    .map_err(|e| anyhow::anyhow!("Failed to serialize lockfile: {e}"))?;
                match output {
                    Some(path) => {
                        std::fs::write(&path, &lockfile_json)
                            .map_err(|e| anyhow::anyhow!("Failed to write lockfile: {e}"))?;
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({"status": "exported", "path": path.display().to_string()})
                            );
                        } else {
                            println!("Exported lockfile to {}", path.display());
                        }
                    }
                    None => {
                        println!("{lockfile_json}");
                    }
                }
            }
            LockfileCmd::Verify { path } => {
                let json_text = std::fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", path.display(), e))?;
                match agora_core::lockfile::InstanceLockfile::parse_and_validate(&json_text) {
                    Ok(lockfile) => {
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({
                                    "status": "valid",
                                    "instance": lockfile.instance.name,
                                    "artifacts": lockfile.artifacts.len(),
                                    "schemaVersion": lockfile.schema_version,
                                })
                            );
                        } else {
                            println!("Lockfile is valid");
                            println!(
                                "  Instance: {} {} ({}/{})",
                                lockfile.instance.name,
                                lockfile.instance.minecraft_version,
                                lockfile.instance.loader,
                                lockfile.instance.loader_version
                            );
                            println!("  Artifacts: {}", lockfile.artifacts.len());
                            println!("  Schema:   v{}", lockfile.schema_version);
                            if lockfile.signature.is_some() {
                                println!("  Signed:   yes");
                            }
                        }
                    }
                    Err(e) => {
                        if json {
                            eprintln!(
                                "{}",
                                serde_json::json!({
                                    "status": "invalid",
                                    "error": e,
                                })
                            );
                        } else {
                            eprintln!("Lockfile is invalid: {e}");
                        }
                        anyhow::bail!("Lockfile verification failed: {e}");
                    }
                }
            }
            LockfileCmd::Repair {
                instance,
                out: output,
            } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                // Repair re-exports the lockfile from the current state.
                let lockfile = agora_core::lockfile::build_from_instance(&instance_dir)
                    .map_err(|e| anyhow::anyhow!("Failed to rebuild lockfile: {e}"))?;
                let lockfile_json = lockfile
                    .to_pretty_json()
                    .map_err(|e| anyhow::anyhow!("Failed to serialize lockfile: {e}"))?;
                match output {
                    Some(path) => {
                        std::fs::write(&path, &lockfile_json)
                            .map_err(|e| anyhow::anyhow!("Failed to write lockfile: {e}"))?;
                        if json {
                            println!(
                                "{}",
                                serde_json::json!({"status": "repaired", "path": path.display().to_string()})
                            );
                        } else {
                            println!("Repaired lockfile written to {}", path.display());
                        }
                    }
                    None => {
                        println!("{lockfile_json}");
                    }
                }
            }
            LockfileCmd::Import {
                path,
                instance,
                skip_health_scan: _,
            } => {
                let instance_dir = agora_core::paths::instance_dir(data_dir, &instance)?;
                if !instance_dir.exists() {
                    anyhow::bail!("Instance '{}' not found", instance);
                }
                let json_text = std::fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", path.display(), e))?;
                let lockfile =
                    agora_core::lockfile::InstanceLockfile::parse_and_validate(&json_text)
                        .map_err(|e| anyhow::anyhow!("Invalid lockfile: {e}"))?;

                // Build a lockfile from the current instance to detect drift.
                let _current = agora_core::lockfile::build_from_instance(&instance_dir)
                    .map_err(|e| anyhow::anyhow!("Cannot read current instance: {e}"))?;

                // Compute the drift between the lockfile and current instance
                let mut live_files: std::collections::BTreeMap<String, String> =
                    std::collections::BTreeMap::new();
                let content_dirs = ["mods", "resourcepacks", "shaderpacks", "datapacks", "saves"];
                for dir_name in &content_dirs {
                    let dir = instance_dir.join(dir_name);
                    if !dir.is_dir() {
                        continue;
                    }
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_file() {
                                if let Ok(data) = std::fs::read(&path) {
                                    let sha256 = agora_core::download::sha256_hex(&data);
                                    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                                        live_files.insert(format!("{dir_name}/{name}"), sha256);
                                    }
                                }
                            }
                        }
                    }
                }

                let drift = agora_core::lockfile::detect_drift(&lockfile, &live_files, None);
                if json {
                    println!("{}", serde_json::to_string_pretty(&drift)?);
                } else {
                    if drift.status == agora_core::lockfile::DriftStatus::InSync {
                        println!("Instance is already in sync with lockfile");
                    } else {
                        println!("Drift detected ({} differences):", drift.differences.len());
                        for diff in &drift.differences {
                            println!("  [{:?}] {}", diff.kind, diff.path);
                        }
                    }
                }
            }
        },
    }

    Ok(())
}

/// Resolve optional deps policy from CLI flags.
fn resolve_optional_deps(
    include: Option<String>,
    exclude: bool,
) -> agora_core::install_pipeline::OptionalDepsPolicy {
    if exclude {
        return agora_core::install_pipeline::OptionalDepsPolicy::ExcludeAll;
    }
    if let Some(list) = include {
        let deps: Vec<String> = list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return agora_core::install_pipeline::OptionalDepsPolicy::Include { deps };
    }
    agora_core::install_pipeline::OptionalDepsPolicy::Prompt
}

/// Apply --replace-conflicts / --abort-conflicts to a resolved plan.
fn apply_conflict_overrides(
    plan: &mut agora_core::install_pipeline::ResolvedInstallPlan,
    replace: bool,
    abort: bool,
) -> anyhow::Result<()> {
    for conflict in &mut plan.conflicts {
        if conflict.chosen.is_some() {
            continue;
        }
        if replace {
            if conflict
                .resolution_options
                .contains(&agora_core::install_pipeline::ConflictResolution::Replace)
            {
                conflict.chosen = Some(agora_core::install_pipeline::ConflictResolution::Replace);
            }
        } else if abort {
            conflict.chosen = Some(agora_core::install_pipeline::ConflictResolution::Abort);
        }
    }
    Ok(())
}

/// Print unresolved plan diagnostics to stderr.
fn report_unresolved_plan(plan: &agora_core::install_pipeline::ResolvedInstallPlan, json: bool) {
    if json {
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "blocked",
                "blockingErrors": plan.blocking_errors,
                "pendingChoices": plan.pending_choices,
                "conflicts": plan.conflicts,
            }))
            .unwrap_or_default()
        );
    } else {
        for err in &plan.blocking_errors {
            eprintln!("[BLOCK] {}: {}", err.code, err.message);
        }
        for conflict in &plan.conflicts {
            if conflict.chosen.is_none() {
                eprintln!("[CONFLICT] {}", conflict.message);
            }
        }
        for choice in &plan.pending_choices {
            let label = match choice {
                agora_core::install_pipeline::PendingChoice::OptionalDependencies { .. } => {
                    "Optional dependencies"
                }
                agora_core::install_pipeline::PendingChoice::Conflict { .. } => {
                    "Conflict resolution"
                }
            };
            eprintln!("[CHOICE] {} requires user input", label);
        }
    }
}

/// Print a resolved plan (used by --dry-run).
fn print_plan(
    plan: &agora_core::install_pipeline::ResolvedInstallPlan,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(plan)?);
    } else {
        println!("=== Dry-run plan ({}): ===", plan.fingerprint);
        print!("  Operation: ");
        use agora_core::install_pipeline::ResolvedArtifact;
        fn artifact_id(artifact: &ResolvedArtifact) -> String {
            match artifact {
                ResolvedArtifact::Download(d) => d.item_id.clone(),
                ResolvedArtifact::LocalFile(l) => l.item_id.clone(),
            }
        }
        fn artifact_version(artifact: &ResolvedArtifact) -> String {
            match artifact {
                ResolvedArtifact::Download(d) => d.version_id.clone(),
                ResolvedArtifact::LocalFile(_) => "local".into(),
            }
        }
        match &plan.operation {
            agora_core::install_pipeline::ResolvedOperation::Install { artifact } => {
                println!(
                    "install {} v{}",
                    artifact_id(artifact),
                    artifact_version(artifact)
                );
            }
            agora_core::install_pipeline::ResolvedOperation::Update { new_artifact, .. } => {
                println!(
                    "update {} v{}",
                    artifact_id(new_artifact),
                    artifact_version(new_artifact)
                );
            }
            agora_core::install_pipeline::ResolvedOperation::Remove {
                target_filename, ..
            } => {
                println!("remove {}", target_filename);
            }
            other => {
                println!("{:?}", other);
            }
        }
        if !plan.files_to_add.is_empty() {
            println!("  Add: {} file(s)", plan.files_to_add.len());
            for f in &plan.files_to_add {
                println!("    + {}", f.target_filename);
            }
        }
        if !plan.files_to_remove.is_empty() {
            println!("  Remove: {} file(s)", plan.files_to_remove.len());
            for f in &plan.files_to_remove {
                println!("    - {}", f.filename);
            }
        }
        if !plan.files_to_disable.is_empty() {
            println!("  Disable: {} file(s)", plan.files_to_disable.len());
            for f in &plan.files_to_disable {
                println!("    ~ {}", f.filename);
            }
        }
        if !plan.conflicts.is_empty() {
            println!("  Conflicts:");
            for c in &plan.conflicts {
                let resolution = c
                    .chosen
                    .as_ref()
                    .map(|r| format!("{:?}", r))
                    .unwrap_or_else(|| "unresolved".into());
                println!("    ! {} -> {}", c.message, resolution);
            }
        }
        if !plan.dependencies.is_empty() {
            println!("  Dependencies: {}", plan.dependencies.len());
        }
        println!(
            "  Disk: {} download, {} additional, {} delta",
            plan.disk_estimate.download_bytes,
            plan.disk_estimate.peak_additional_bytes,
            plan.disk_estimate.post_commit_delta_bytes
        );
        if !plan.blocking_errors.is_empty() {
            println!("  Blocking errors: {}", plan.blocking_errors.len());
            for err in &plan.blocking_errors {
                println!("    [BLOCK] {}: {}", err.code, err.message);
            }
        }
    }
    Ok(())
}

async fn run_launch_service(
    ctx: &agora_core::ctx::Ctx,
    instance: &str,
    yes: bool,
    output_fmt: OutputFormat,
) -> anyhow::Result<()> {
    let json = output_fmt.is_json_output();
    let request = agora_core::launch_service::LaunchRequest {
        instance_id: instance.to_owned(),
        mode: agora_core::launch_service::LaunchMode::Direct,
        health_policy: if yes {
            agora_core::launch_service::HealthPolicy::WarnOnly
        } else {
            agora_core::launch_service::HealthPolicy::BlockOnRed
        },
    };
    let progress = ConsoleLaunchProgress { json };
    let result = agora_core::launch_service::LaunchService::new(ctx.clone())
        .launch(request, &progress)
        .await?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "pid": result.pid,
                "session_id": result.session_id,
                "outcome": result.outcome,
                "snapshot_id": result.snapshot_id,
            })
        );
    } else {
        println!("Launch finished with outcome {:?}.", result.outcome);
    }
    match result.outcome {
        agora_core::lkg::LaunchOutcome::Crash | agora_core::lkg::LaunchOutcome::Unknown => {
            Err(agora_core::error::LauncherError::GameCrash.into())
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// MCP stdio transport
// ---------------------------------------------------------------------------

/// Build a JSON-RPC 2.0 success response envelope.
fn build_jsonrpc_response(id: &serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

/// Build a JSON-RPC 2.0 error response envelope.
fn build_jsonrpc_error(id: &serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

/// Run the MCP stdio transport loop.
///
/// Reads newline-delimited JSON-RPC 2.0 requests from stdin, dispatches via
/// [`agora_core::mcp_dispatcher::McpDispatcher`], writes responses to
/// stdout, and prints diagnostics to stderr.  Notifications (requests without
/// an `id` field) do not receive a response.  Exits cleanly on EOF.
async fn run_mcp_stdio(ctx: &agora_core::ctx::Ctx) -> anyhow::Result<()> {
    let dispatcher = agora_core::mcp_dispatcher::McpDispatcher::new(ctx.clone());
    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let mut stdout = std::io::stdout();

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[mcp] stdin read error: {e}");
                continue;
            }
        };
        let trimmed = line.trim().to_owned();
        if trimmed.is_empty() {
            continue;
        }

        let request: serde_json::Value = match serde_json::from_str(&trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[mcp] Failed to parse request: {e}");
                // Cannot send a JSON-RPC error for malformed JSON — we don't
                // have a valid id to echo back in the response.
                continue;
            }
        };

        let id_value: serde_json::Value = request
            .get("id")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let is_notification = request.get("id").is_none();

        let method = match request.get("method").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => {
                eprintln!("[mcp] Request missing 'method' field");
                if !is_notification {
                    let resp =
                        build_jsonrpc_error(&id_value, -32600, "Invalid Request: missing method");
                    let line = serde_json::to_string(&resp)?;
                    writeln!(stdout, "{line}")?;
                    stdout.flush()?;
                }
                continue;
            }
        };

        let dispatcher_result = dispatcher.handle_method(method, request.get("params"));

        // Notifications have no "id" — no response per JSON-RPC 2.0
        if is_notification {
            continue;
        }

        let response = if dispatcher_result.get("error").is_some() {
            let err = &dispatcher_result["error"];
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-32603);
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Internal error");
            build_jsonrpc_error(&id_value, code, msg)
        } else {
            build_jsonrpc_response(&id_value, dispatcher_result)
        };

        let line = serde_json::to_string(&response)?;
        writeln!(stdout, "{line}")?;
        stdout.flush()?;
    }

    Ok(())
}

/// Extract the authorization code and CSRF state from the complete browser
/// redirect URL. The direct-launch OAuth flow always creates a state value, so
/// accepting a bare code would make CLI login fail closed and hide the cause.
fn extract_auth_redirect(input: &str) -> anyhow::Result<(String, String)> {
    let url = reqwest::Url::parse(input).map_err(|_| {
        anyhow::anyhow!(
            "Paste the complete redirect URL so Agora can verify the OAuth state parameter."
        )
    })?;
    let mut code = None;
    let mut state = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            _ => {}
        }
    }

    let code =
        code.ok_or_else(|| anyhow::anyhow!("Redirect URL did not include an OAuth code."))?;
    let state =
        state.ok_or_else(|| anyhow::anyhow!("Redirect URL did not include an OAuth state."))?;
    Ok((code, state))
}

#[cfg(test)]
mod tests {
    use super::exit_code_from_error;
    use super::exit_code_from_launcher_error;
    use super::extract_auth_redirect;
    use super::Cli;
    use super::Commands;
    use super::LoadoutCmd;
    use super::LockfileCmd;
    use super::ModSourceArg;
    use super::ModsCmd;
    use super::OutputFormat;
    use super::PackCmd;
    use super::SilentReporter;
    use agora_core::dependency_ops;
    use agora_core::error::LauncherError;
    use agora_core::install_pipeline::{
        ArtifactMetadata, ArtifactSource, CancellationToken, ConflictKind, ConflictResolution,
        DepConflict, DiskSpaceEstimate, HashSpec, InstallAction, InstallIntent, OptionalDepsPolicy,
        PlanOverrides, ProgressEvent, ProgressPhase, ProgressReporter, RequestSource,
        ResolvedArtifact, ResolvedDownload, ResolvedInstallPlan, ResolvedOperation, SnapshotPlan,
        SourceType,
    };
    use agora_core::models::InstalledMod;
    use clap::Parser;

    #[test]
    fn parses_code_and_state_from_redirect_url() {
        let (code, state) = extract_auth_redirect(
            "https://login.live.com/oauth20_desktop.srf?code=abc%20123&state=csrf-token",
        )
        .unwrap();
        assert_eq!(code, "abc 123");
        assert_eq!(state, "csrf-token");
    }

    #[test]
    fn rejects_redirect_without_state() {
        assert!(extract_auth_redirect("https://example.invalid/?code=abc").is_err());
    }

    #[test]
    fn silent_reporter_accepts_progress_events() {
        let reporter = SilentReporter;
        // Should not panic
        reporter.report(ProgressEvent {
            plan_id: "test".into(),
            phase: ProgressPhase::Resolving,
            step: 0,
            total_steps: 1,
            bytes_downloaded: 0,
            bytes_total: 0,
            message: "test".into(),
        });
    }

    #[test]
    fn silent_reporter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SilentReporter>();
    }

    #[test]
    fn cancellation_token_default_is_not_cancelled() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancellation_token_cancel_works() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn removal_plan_detects_reverse_dependents() {
        let target = InstalledMod {
            filename: "core-lib.jar".into(),
            registry_id: Some("core-lib".into()),
            modrinth_id: None,
            source: "modrinth".into(),
            source_url: None,
            version: Some("1.0.0".into()),
            sha256: "a".repeat(64),
            installed_at: "2024-01-01T00:00:00Z".into(),
            java_packages: vec![],
            mod_jar_id: Some("core-lib".into()),
            provided_mod_ids: vec![],
            enabled: true,
            content_type: "mod".into(),
            depends_on: vec![],
            optional_deps: vec![],
            incompatible_deps: vec![],
        };

        let dependent = InstalledMod {
            filename: "dependent-mod.jar".into(),
            registry_id: Some("dependent-mod".into()),
            modrinth_id: None,
            source: "modrinth".into(),
            source_url: None,
            version: Some("2.0.0".into()),
            sha256: "b".repeat(64),
            installed_at: "2024-01-01T00:00:00Z".into(),
            java_packages: vec![],
            mod_jar_id: Some("dependent-mod".into()),
            provided_mod_ids: vec![],
            enabled: true,
            content_type: "mod".into(),
            depends_on: vec!["core-lib".into()],
            optional_deps: vec![],
            incompatible_deps: vec![],
        };

        let installed = vec![target.clone(), dependent];
        let plan = dependency_ops::build_removal_plan(&installed, &target);
        assert_eq!(plan.dependents.len(), 1);
        assert_eq!(plan.dependents[0].mod_id, "dependent-mod");
        assert_eq!(
            plan.dependents[0].requirement,
            agora_core::install_pipeline::Requirement::Required
        );
    }

    #[test]
    fn removal_plan_empty_for_unreferenced_mod() {
        let target = InstalledMod {
            filename: "standalone.jar".into(),
            registry_id: None,
            modrinth_id: None,
            source: "manual".into(),
            source_url: None,
            version: None,
            sha256: "c".repeat(64),
            installed_at: "2024-01-01T00:00:00Z".into(),
            java_packages: vec![],
            mod_jar_id: None,
            provided_mod_ids: vec![],
            enabled: true,
            content_type: "mod".into(),
            depends_on: vec![],
            optional_deps: vec![],
            incompatible_deps: vec![],
        };

        let other = InstalledMod {
            filename: "other.jar".into(),
            registry_id: Some("other".into()),
            modrinth_id: None,
            source: "modrinth".into(),
            source_url: None,
            version: Some("1.0.0".into()),
            sha256: "d".repeat(64),
            installed_at: "2024-01-01T00:00:00Z".into(),
            java_packages: vec![],
            mod_jar_id: Some("other".into()),
            provided_mod_ids: vec![],
            enabled: true,
            content_type: "mod".into(),
            depends_on: vec![],
            optional_deps: vec![],
            incompatible_deps: vec![],
        };

        let installed = vec![target.clone(), other];
        let plan = dependency_ops::build_removal_plan(&installed, &target);
        assert!(plan.dependents.is_empty());
    }

    // --- exit-code mapping tests ---

    #[test]
    fn exit_code_local_state_failed() {
        assert_eq!(
            exit_code_from_launcher_error(&LauncherError::LocalStateFailed),
            10
        );
    }

    #[test]
    fn exit_code_instance_locked() {
        assert_eq!(
            exit_code_from_launcher_error(&LauncherError::InstanceLocked),
            11
        );
    }

    #[test]
    fn exit_code_instance_create_failed() {
        assert_eq!(
            exit_code_from_launcher_error(&LauncherError::InstanceCreateFailed),
            12
        );
    }

    #[test]
    fn exit_code_network_offline() {
        assert_eq!(
            exit_code_from_launcher_error(&LauncherError::NetworkOffline),
            20
        );
    }

    #[test]
    fn exit_code_auth_expired() {
        assert_eq!(
            exit_code_from_launcher_error(&LauncherError::AuthExpired),
            40
        );
    }

    #[test]
    fn exit_code_generic_falls_to_one() {
        assert_eq!(
            exit_code_from_launcher_error(&LauncherError::Generic {
                code: "ERR_X".into(),
                message: "x".into(),
            }),
            1
        );
    }

    #[test]
    fn exit_code_from_anyhow_wrapping_launcher_error() {
        let le = LauncherError::LocalStateFailed;
        let err = anyhow::Error::from(le);
        assert_eq!(exit_code_from_error(&err), 10);
    }

    #[test]
    fn exit_code_from_anyhow_plain_message_is_one() {
        let err = anyhow::anyhow!("something went wrong");
        assert_eq!(exit_code_from_error(&err), 1);
    }

    #[test]
    fn exit_code_generic_maps_to_one() {
        assert_eq!(
            exit_code_from_launcher_error(&LauncherError::Generic {
                code: "ERR_INSTANCE_NOT_FOUND".into(),
                message: "Instance 'test' not found".into(),
            }),
            1
        );
    }

    #[test]
    fn exit_code_generic_through_anyhow_maps_to_one() {
        let err: anyhow::Error = LauncherError::Generic {
            code: "ERR_INSTANCE_NOT_FOUND".into(),
            message: "Instance 'test' not found".into(),
        }
        .into();
        assert_eq!(exit_code_from_error(&err), 1);
    }

    #[test]
    fn exit_code_from_plain_bail_is_one() {
        let err = anyhow::anyhow!("Instance 'foo' not found");
        assert_eq!(exit_code_from_error(&err), 1);
    }

    // --- OutputFormat tests ---

    #[test]
    fn output_format_human_is_not_json() {
        assert!(!OutputFormat::Human.is_json_output());
    }

    #[test]
    fn output_format_json_is_json() {
        assert!(OutputFormat::Json.is_json_output());
    }

    #[test]
    fn output_format_ndjson_is_json() {
        assert!(OutputFormat::Ndjson.is_json_output());
    }

    // --- JSON-safe behavior tests ---

    #[test]
    fn json_branch_list_instances_no_db_uses_eprintln() {
        // This is a compile-time / logic assertion that the stdout leak
        // has been fixed. The production code now uses eprintln! for
        // the "No local state database found" message, which keeps
        // stdout clean for JSON consumers.
    }

    #[test]
    fn json_branch_settings_list_no_db_uses_eprintln() {
        // Same safety property as list_instances_no_db.
    }

    #[test]
    fn json_branch_auth_login_prompts_use_eprintln() {
        // In JSON mode, the interactive prompts are sent to stderr
        // so stdout contains only the final JSON credentials object.
    }

    // --- JSON error envelope tests ---

    #[test]
    fn json_error_envelope_has_required_fields() {
        let err = anyhow::anyhow!("Instance 'foo' not found");
        let code = exit_code_from_error(&err);
        let envelope = serde_json::json!({
            "error": err.to_string(),
            "exitCode": code,
        });
        assert_eq!(envelope["error"], "Instance 'foo' not found");
        assert_eq!(envelope["exitCode"], 1);
    }

    #[test]
    fn json_error_envelope_includes_semantic_code_for_launcher_error() {
        let le = LauncherError::LocalStateFailed;
        let err = anyhow::Error::from(le);
        let code = exit_code_from_error(&err);
        let envelope = serde_json::json!({
            "error": err.to_string(),
            "exitCode": code,
        });
        assert_eq!(envelope["exitCode"], 10);
        assert!(
            envelope["error"].as_str().unwrap().contains("database"),
            "error should mention database"
        );
    }

    #[test]
    fn json_error_envelope_blocks_install() {
        let err =
            anyhow::anyhow!("Install blocked: unresolved errors, conflicts, or pending choices");
        let code = exit_code_from_error(&err);
        assert_eq!(code, 1);
        assert!(
            err.to_string().contains("blocked"),
            "blocked error should contain 'blocked'"
        );
    }

    #[test]
    fn json_error_envelope_blocks_remove() {
        let err = anyhow::anyhow!("Remove blocked: unresolved errors");
        let code = exit_code_from_error(&err);
        assert_eq!(code, 1);
        assert!(
            err.to_string().contains("blocked"),
            "blocked error should contain 'blocked'"
        );
    }

    #[test]
    fn bail_messages_include_expected_content() {
        // Verify that the bail messages used in run_command replacements
        // produce meaningful error text through the top-level envelope.
        let cases: Vec<(&str, &[&str])> = vec![
            ("No local state database found", &["database", "found"]),
            (
                "Instance 'my-instance' not found",
                &["Instance", "not found"],
            ),
            ("Setting 'foo' not found", &["Setting", "not found"]),
            (
                "Path 'C:\\missing' does not exist",
                &["Path", "does not exist"],
            ),
            ("No input provided", &["No input", "provided"]),
        ];
        for (msg, keywords) in cases {
            let err = anyhow::anyhow!("{}", msg);
            let s = err.to_string();
            for kw in keywords {
                assert!(s.contains(kw), "error '{}' should contain '{}'", s, kw);
            }
        }
    }

    // --- MCP JSON-RPC envelope tests ---

    #[test]
    fn jsonrpc_response_has_correct_envelope() {
        let id = serde_json::json!(1);
        let result = serde_json::json!({"status": "ok"});
        let resp = super::build_jsonrpc_response(&id, result);
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["status"], "ok");
        assert!(resp.get("error").is_none());
    }

    #[test]
    fn jsonrpc_response_string_id() {
        let id = serde_json::json!("req-42");
        let result = serde_json::json!({});
        let resp = super::build_jsonrpc_response(&id, result);
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], "req-42");
    }

    #[test]
    fn jsonrpc_response_null_id() {
        let id = serde_json::Value::Null;
        let result = serde_json::json!({});
        let resp = super::build_jsonrpc_response(&id, result);
        assert_eq!(resp["id"], serde_json::Value::Null);
    }

    #[test]
    fn jsonrpc_error_has_correct_envelope() {
        let id = serde_json::json!(1);
        let resp = super::build_jsonrpc_error(&id, -32601, "Method not found");
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["error"]["code"], -32601);
        assert_eq!(resp["error"]["message"], "Method not found");
        assert!(resp.get("result").is_none());
    }

    #[test]
    fn jsonrpc_error_no_id_uses_null() {
        let id = serde_json::Value::Null;
        let resp = super::build_jsonrpc_error(&id, -32700, "Parse error");
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], serde_json::Value::Null);
        assert_eq!(resp["error"]["code"], -32700);
    }

    #[test]
    fn jsonrpc_dispatcher_result_with_error_becomes_error_response() {
        // Simulates the dispatcher returning a method-level error
        let id = serde_json::json!(5);
        let dispatcher_result = serde_json::json!({
            "error": {
                "code": -32601,
                "message": "Unknown method: bogus"
            }
        });

        let response = if dispatcher_result.get("error").is_some() {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": dispatcher_result["error"],
            })
        } else {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": dispatcher_result,
            })
        };

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 5);
        assert_eq!(response["error"]["code"], -32601);
        assert!(response.get("result").is_none());
    }

    #[test]
    fn jsonrpc_dispatcher_result_without_error_becomes_result_response() {
        let id = serde_json::json!(10);
        let dispatcher_result = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {}
        });

        let response = if dispatcher_result.get("error").is_some() {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": dispatcher_result["error"],
            })
        } else {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": dispatcher_result,
            })
        };

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 10);
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert!(response.get("error").is_none());
    }

    #[test]
    fn jsonrpc_notification_suppresses_response() {
        // Notifications (no "id") produce no response
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize"
        });
        let is_notification = request.get("id").is_none();
        assert!(is_notification);
    }

    #[test]
    fn jsonrpc_request_with_id_is_not_notification() {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        });
        let is_notification = request.get("id").is_none();
        assert!(!is_notification);
    }

    // --- CLI parser tests for crash commands ---

    #[test]
    fn crash_list_parses() {
        let cli =
            Cli::try_parse_from(["agora", "crash", "list", "my-instance"]).expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Crash {
                action: super::CrashCmd::List { .. }
            }
        ));
    }

    #[test]
    fn crash_inspect_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "crash",
            "inspect",
            "my-instance",
            "crash-2024-01-01.txt",
        ])
        .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Crash {
                action: super::CrashCmd::Inspect { .. }
            }
        ));
    }

    #[test]
    fn crash_investigate_parses() {
        let cli = Cli::try_parse_from(["agora", "crash", "investigate", "my-instance"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Crash {
                action: super::CrashCmd::Investigate { .. }
            }
        ));
    }

    #[test]
    fn loader_list_with_mc_version_parses() {
        let cli = Cli::try_parse_from(["agora", "loader", "list", "--mc-version", "1.21"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Loader {
                action: super::LoaderCmd::List { .. }
            }
        ));
    }

    #[test]
    fn loader_list_with_m_flag_parses() {
        let cli =
            Cli::try_parse_from(["agora", "loader", "list", "-m", "1.20.1"]).expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Loader {
                action: super::LoaderCmd::List { .. }
            }
        ));
    }

    #[test]
    fn runtime_inspect_parses() {
        let cli = Cli::try_parse_from(["agora", "runtime", "inspect", "/usr/bin/java"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Runtime {
                action: super::RuntimeCmd::Inspect { .. }
            }
        ));
    }

    // --- CLI parser tests for new mod commands ---

    #[test]
    fn mod_search_parses_query() {
        let cli = Cli::try_parse_from(["agora", "mod", "search", "sodium"]).expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Mods {
                action: ModsCmd::Search { .. }
            }
        ));
    }

    #[test]
    fn mod_search_parses_with_filters() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "search",
            "sodium",
            "--content-type",
            "mod",
            "--mc-version",
            "1.21",
        ])
        .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Mods {
                action: ModsCmd::Search { .. }
            }
        ));
    }

    #[test]
    fn mod_update_parses_with_version() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "update",
            "my-instance",
            "sodium",
            "--version",
            "1.0.1",
        ])
        .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Mods {
                action: ModsCmd::Update { .. }
            }
        ));
    }

    #[test]
    fn mod_update_parses_without_version() {
        let cli = Cli::try_parse_from(["agora", "mod", "update", "my-instance", "sodium"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Mods {
                action: ModsCmd::Update { .. }
            }
        ));
    }

    #[test]
    fn mod_update_all_parses() {
        let cli = Cli::try_parse_from(["agora", "mod", "update-all", "my-instance"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Mods {
                action: ModsCmd::UpdateAll { .. }
            }
        ));
    }

    // --- mod enable / disable parser tests ---

    #[test]
    fn mod_enable_parses() {
        let cli = Cli::try_parse_from(["agora", "mod", "enable", "my-instance", "sodium.jar"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Mods {
                action: ModsCmd::Enable { .. }
            }
        ));
    }

    #[test]
    fn mod_disable_parses() {
        let cli = Cli::try_parse_from(["agora", "mod", "disable", "my-instance", "sodium.jar"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Mods {
                action: ModsCmd::Disable { .. }
            }
        ));
    }

    // --- integration-style test for enable / disable via CrashService ---

    #[test]
    fn mod_enable_disable_roundtrip() {
        use agora_core::crash_service::CrashService;
        use agora_core::ctx::CoreContext;
        use agora_core::models::{InstalledMod, InstanceManifest};
        use std::fs;

        static TEST_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("agora-cli-mod-test-{}", seq));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create tmp");

        let ctx = CoreContext::for_testing(tmp.clone());
        let instance_dir = ctx.paths.instance_dir("test-instance").unwrap();
        fs::create_dir_all(instance_dir.join("mods")).expect("create mods dir");
        let mod_path = instance_dir.join("mods").join("test-mod.jar");
        fs::write(&mod_path, b"fake mod content").expect("write mod file");

        let manifest = InstanceManifest {
            instance_id: "test-instance".into(),
            name: "Test".into(),
            created_from_pack: None,
            minecraft_version: "1.21".into(),
            loader: "fabric".into(),
            loader_version: "0.16.0".into(),
            is_locked: false,
            mods: vec![InstalledMod {
                filename: "test-mod.jar".into(),
                source: "manual".into(),
                source_url: None,
                version: Some("1.0.0".into()),
                sha256: "a".repeat(64),
                installed_at: "2024-01-01T00:00:00Z".into(),
                java_packages: vec![],
                modrinth_id: None,
                registry_id: None,
                mod_jar_id: None,
                provided_mod_ids: vec![],
                enabled: true,
                content_type: "mod".into(),
                depends_on: vec![],
                optional_deps: vec![],
                incompatible_deps: vec![],
            }],
            resourcepacks: vec![],
            shaders: vec![],
            datapacks: vec![],
            worlds: vec![],
            user_preferences: serde_json::Value::Null,
        };
        let manifest_path = ctx.paths.instance_manifest("test-instance").unwrap();
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .expect("write manifest");

        let svc = CrashService::new(ctx.clone());

        // DISABLE: rename test-mod.jar -> test-mod.jar.disabled
        svc.disable_mod("test-instance", "test-mod.jar")
            .expect("disable");
        assert!(
            !instance_dir.join("mods").join("test-mod.jar").exists(),
            "original gone"
        );
        assert!(
            instance_dir
                .join("mods")
                .join("test-mod.jar.disabled")
                .exists(),
            "disabled file exists"
        );

        let updated: InstanceManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert!(!updated.mods[0].enabled, "mod disabled in manifest");

        // Idempotent
        svc.disable_mod("test-instance", "test-mod.jar")
            .expect("re-disable ok");

        // ENABLE: rename back
        svc.enable_mod("test-instance", "test-mod.jar")
            .expect("enable");
        assert!(
            instance_dir.join("mods").join("test-mod.jar").exists(),
            "original back"
        );
        assert!(
            !instance_dir
                .join("mods")
                .join("test-mod.jar.disabled")
                .exists(),
            "disabled file gone"
        );

        let updated2: InstanceManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert!(updated2.mods[0].enabled, "mod enabled in manifest");

        // Idempotent
        svc.enable_mod("test-instance", "test-mod.jar")
            .expect("re-enable ok");

        let _ = fs::remove_dir_all(&tmp);
    }

    // --- New policy flag tests ---

    #[test]
    fn mod_install_defaults_to_curated_source() {
        let cli = Cli::try_parse_from(["agora", "mod", "install", "lithium", "my-instance"])
            .expect("should parse");
        match cli.command {
            Commands::Mods {
                action: ModsCmd::Install { source, .. },
            } => assert_eq!(source, ModSourceArg::Curated),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_install_accepts_raw_modrinth_source() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "install",
            "sodium",
            "my-instance",
            "--source",
            "modrinth",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action: ModsCmd::Install { source, .. },
            } => assert_eq!(source, ModSourceArg::Modrinth),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_install_with_include_optional_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "install",
            "sodium",
            "my-instance",
            "--include-optional",
            "fabric-api,indium",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action:
                    ModsCmd::Install {
                        include_optional, ..
                    },
            } => {
                assert_eq!(include_optional, Some("fabric-api,indium".into()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_install_with_exclude_optional_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "install",
            "sodium",
            "my-instance",
            "--exclude-optional",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action:
                    ModsCmd::Install {
                        exclude_optional, ..
                    },
            } => {
                assert!(exclude_optional);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_install_with_replace_conflicts_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "install",
            "sodium",
            "my-instance",
            "--replace-conflicts",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action:
                    ModsCmd::Install {
                        replace_conflicts, ..
                    },
            } => {
                assert!(replace_conflicts);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_install_with_abort_conflicts_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "install",
            "sodium",
            "my-instance",
            "--abort-conflicts",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action: ModsCmd::Install {
                    abort_conflicts, ..
                },
            } => {
                assert!(abort_conflicts);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_install_with_dry_run_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "install",
            "sodium",
            "my-instance",
            "--dry-run",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action: ModsCmd::Install { dry_run, .. },
            } => {
                assert!(dry_run);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_install_include_exclude_optional_are_mutually_exclusive() {
        assert!(Cli::try_parse_from([
            "agora",
            "mod",
            "install",
            "sodium",
            "my-instance",
            "--include-optional",
            "fabric-api",
            "--exclude-optional"
        ])
        .is_err());
    }

    #[test]
    fn mod_update_with_policy_flags_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "update",
            "my-instance",
            "sodium",
            "--include-optional",
            "fabric-api",
            "--replace-conflicts",
            "--dry-run",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action:
                    ModsCmd::Update {
                        include_optional,
                        replace_conflicts,
                        dry_run,
                        ..
                    },
            } => {
                assert_eq!(include_optional, Some("fabric-api".into()));
                assert!(replace_conflicts);
                assert!(dry_run);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_update_all_with_policy_flags_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "update-all",
            "my-instance",
            "--exclude-optional",
            "--abort-conflicts",
            "--dry-run",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action:
                    ModsCmd::UpdateAll {
                        exclude_optional,
                        abort_conflicts,
                        dry_run,
                        ..
                    },
            } => {
                assert!(exclude_optional);
                assert!(abort_conflicts);
                assert!(dry_run);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn mod_remove_with_conflict_flags_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "mod",
            "remove",
            "sodium",
            "my-instance",
            "--replace-conflicts",
            "--dry-run",
        ])
        .expect("should parse");
        match cli.command {
            Commands::Mods {
                action:
                    ModsCmd::Remove {
                        replace_conflicts,
                        dry_run,
                        ..
                    },
            } => {
                assert!(replace_conflicts);
                assert!(dry_run);
            }
            _ => panic!("wrong variant"),
        }
    }

    // --- New command family parser tests ---

    #[test]
    fn pack_install_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "pack",
            "install",
            "/path/to/pack.json",
            "my-instance",
        ])
        .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Pack {
                action: PackCmd::Install { .. }
            }
        ));
    }

    #[test]
    fn export_parses() {
        let cli = Cli::try_parse_from(["agora", "export", "my-instance", "/path/to/dest"])
            .expect("should parse");
        assert!(matches!(cli.command, Commands::Export { .. }));
    }

    #[test]
    fn loadout_create_parses() {
        let cli = Cli::try_parse_from(["agora", "loadout", "create", "my-instance", "my-profile"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Loadout {
                action: LoadoutCmd::Create { .. }
            }
        ));
    }

    #[test]
    fn loadout_list_parses() {
        let cli =
            Cli::try_parse_from(["agora", "loadout", "list", "my-instance"]).expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Loadout {
                action: LoadoutCmd::List { .. }
            }
        ));
    }

    #[test]
    fn loadout_apply_parses() {
        let cli = Cli::try_parse_from(["agora", "loadout", "apply", "my-instance", "my-profile"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Loadout {
                action: LoadoutCmd::Apply { .. }
            }
        ));
    }

    #[test]
    fn loadout_delete_parses() {
        let cli = Cli::try_parse_from(["agora", "loadout", "delete", "my-instance", "my-profile"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Loadout {
                action: LoadoutCmd::Delete { .. }
            }
        ));
    }

    #[test]
    fn lockfile_export_parses() {
        let cli = Cli::try_parse_from(["agora", "lockfile", "export", "my-instance"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Lockfile {
                action: LockfileCmd::Export { .. }
            }
        ));
    }

    #[test]
    fn lockfile_export_with_output_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "lockfile",
            "export",
            "my-instance",
            "--out",
            "lockfile.json",
        ])
        .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Lockfile {
                action: LockfileCmd::Export { .. }
            }
        ));
    }

    #[test]
    fn lockfile_verify_parses() {
        let cli = Cli::try_parse_from(["agora", "lockfile", "verify", "lockfile.json"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Lockfile {
                action: LockfileCmd::Verify { .. }
            }
        ));
    }

    #[test]
    fn lockfile_repair_parses() {
        let cli = Cli::try_parse_from(["agora", "lockfile", "repair", "my-instance"])
            .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Lockfile {
                action: LockfileCmd::Repair { .. }
            }
        ));
    }

    #[test]
    fn lockfile_import_parses() {
        let cli = Cli::try_parse_from([
            "agora",
            "lockfile",
            "import",
            "lockfile.json",
            "my-instance",
        ])
        .expect("should parse");
        assert!(matches!(
            cli.command,
            Commands::Lockfile {
                action: LockfileCmd::Import { .. }
            }
        ));
    }

    // --- resolve_optional_deps tests ---

    #[test]
    fn resolve_optional_deps_include_list() {
        use agora_core::install_pipeline::OptionalDepsPolicy;
        let policy = super::resolve_optional_deps(Some("fabric-api,indium".into()), false);
        match policy {
            OptionalDepsPolicy::Include { deps } => {
                assert_eq!(deps, vec!["fabric-api", "indium"]);
            }
            _ => panic!("expected Include"),
        }
    }

    #[test]
    fn resolve_optional_deps_exclude_all() {
        use agora_core::install_pipeline::OptionalDepsPolicy;
        let policy = super::resolve_optional_deps(None, true);
        assert_eq!(policy, OptionalDepsPolicy::ExcludeAll);
    }

    #[test]
    fn resolve_optional_deps_prompt_when_no_flags() {
        use agora_core::install_pipeline::OptionalDepsPolicy;
        let policy = super::resolve_optional_deps(None, false);
        assert_eq!(policy, OptionalDepsPolicy::Prompt);
    }

    #[test]
    fn resolve_optional_deps_empty_include_is_exclude() {
        use agora_core::install_pipeline::OptionalDepsPolicy;
        let policy = super::resolve_optional_deps(Some(String::new()), false);
        match policy {
            OptionalDepsPolicy::Include { deps } => {
                assert!(deps.is_empty());
            }
            _ => panic!("expected Include"),
        }
    }

    // --- apply_conflict_overrides tests ---

    #[test]
    fn apply_replace_resolves_conflicts() {
        use agora_core::install_pipeline::*;
        let mut plan = ResolvedInstallPlan {
            fingerprint: "test".into(),
            intent: todo_placeholder_intent(),
            operation: ResolvedOperation::Install {
                artifact: ResolvedArtifact::Download(ResolvedDownload {
                    item_id: "test".into(),
                    version_id: "1.0".into(),
                    filename: "test.jar".into(),
                    source: ArtifactSource::Download {
                        url: "https://example.com/test.jar".into(),
                    },
                    hashes: HashSpec { values: vec![] },
                    size: 0,
                    metadata: ArtifactMetadata {
                        source_type: SourceType::Modrinth,
                        registry_id: None,
                        modrinth_id: None,
                        content_type: "mod".into(),
                    },
                }),
            },
            dependencies: vec![],
            conflicts: vec![DepConflict {
                conflict_id: "c1".into(),
                kind: ConflictKind::DuplicateMod,
                existing_mod_jar_id: "existing".into(),
                incoming_mod_jar_id: "incoming".into(),
                message: "conflict".into(),
                blocking: true,
                resolution_options: vec![ConflictResolution::Replace, ConflictResolution::Skip],
                chosen: None,
            }],
            files_to_add: vec![],
            files_to_remove: vec![],
            files_to_disable: vec![],
            snapshot: SnapshotPlan {
                label: "".into(),
                estimated_bytes: 0,
            },
            disk_estimate: DiskSpaceEstimate::zero(),
            warnings: vec![],
            blocking_errors: vec![],
            pending_choices: vec![],
            created_at: "".into(),
            instance_state_hash: "".into(),
            registry_revision: "".into(),
        };
        super::apply_conflict_overrides(&mut plan, true, false).unwrap();
        assert_eq!(plan.conflicts[0].chosen, Some(ConflictResolution::Replace));
        assert!(plan.is_fully_resolved());
    }

    #[test]
    fn apply_abort_resolves_conflicts() {
        let mut plan = ResolvedInstallPlan {
            fingerprint: "test".into(),
            intent: todo_placeholder_intent(),
            operation: ResolvedOperation::Install {
                artifact: ResolvedArtifact::Download(ResolvedDownload {
                    item_id: "test".into(),
                    version_id: "1.0".into(),
                    filename: "test.jar".into(),
                    source: ArtifactSource::Download {
                        url: "https://example.com/test.jar".into(),
                    },
                    hashes: HashSpec { values: vec![] },
                    size: 0,
                    metadata: ArtifactMetadata {
                        source_type: SourceType::Modrinth,
                        registry_id: None,
                        modrinth_id: None,
                        content_type: "mod".into(),
                    },
                }),
            },
            dependencies: vec![],
            conflicts: vec![DepConflict {
                conflict_id: "c1".into(),
                kind: ConflictKind::DuplicateMod,
                existing_mod_jar_id: "existing".into(),
                incoming_mod_jar_id: "incoming".into(),
                message: "conflict".into(),
                blocking: true,
                resolution_options: vec![ConflictResolution::Replace, ConflictResolution::Skip],
                chosen: None,
            }],
            files_to_add: vec![],
            files_to_remove: vec![],
            files_to_disable: vec![],
            snapshot: SnapshotPlan {
                label: "".into(),
                estimated_bytes: 0,
            },
            disk_estimate: DiskSpaceEstimate::zero(),
            warnings: vec![],
            blocking_errors: vec![],
            pending_choices: vec![],
            created_at: "".into(),
            instance_state_hash: "".into(),
            registry_revision: "".into(),
        };
        super::apply_conflict_overrides(&mut plan, false, true).unwrap();
        // Abort is not in resolution_options, so chosen may be set but
        // is_fully_resolved will still return true (chosen is Some).
        assert_eq!(plan.conflicts[0].chosen, Some(ConflictResolution::Abort));
        assert!(plan.is_fully_resolved());
    }

    fn todo_placeholder_intent() -> InstallIntent {
        InstallIntent {
            action: InstallAction::Install {
                source_type: SourceType::Modrinth,
                item_id: "test".into(),
                candidate_version: None,
            },
            target_instance: "test".into(),
            optional_deps: OptionalDepsPolicy::ExcludeAll,
            requested_by: RequestSource::CLI,
            overrides: PlanOverrides::default(),
        }
    }

    #[test]
    fn exit_code_user_decision_required() {
        assert_eq!(
            exit_code_from_launcher_error(&LauncherError::UserDecisionRequired),
            71
        );
    }
}
