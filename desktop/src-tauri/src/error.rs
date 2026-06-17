use serde::Serialize;

/// Standardized launcher error codes matching the human-centric taxonomy.
#[derive(Debug, Clone, Serialize)]
pub enum LauncherError {
    /// ERR_NETWORK_OFFLINE — You're offline. Using cached data.
    NetworkOffline,
    /// ERR_REGISTRY_DOWNLOAD_FAILED — Could not download the latest registry.
    RegistryDownloadFailed,
    /// ERR_REGISTRY_SIGNATURE_INVALID — Registry signature check failed.
    RegistrySignatureInvalid,
    /// ERR_SCHEMA_TOO_NEW — This registry requires a newer launcher version.
    SchemaTooNew,
    /// ERR_ZIP_BOMB — Installation aborted: this archive exceeds safety limits.
    ZipBomb,
    /// ERR_OVERRIDE_SECURITY_VIOLATION — Pack override contains forbidden files.
    OverrideSecurityViolation,
    /// ERR_HASH_MISMATCH — Downloaded file does not match its expected hash.
    HashMismatch,
    /// ERR_UNTRUSTED_SOURCE — Download rejected: URL is not from an allowed source.
    UntrustedSource,
    /// ERR_DISKFULL — Not enough disk space to complete this operation.
    DiskFull,
    /// ERR_AUTH_EXPIRED — Your GitHub session has expired.
    AuthExpired,
    /// ERR_AUTH_REQUIRED — This feature requires GitHub sign-in.
    AuthRequired,
    /// ERR_MODRINTH_DISABLED — Modrinth integration is disabled.
    ModrinthDisabled,
    /// ERR_INSTANCE_LOCKED — This instance is locked.
    InstanceLocked,
    /// ERR_SANDBOX_UNAVAILABLE — Dev Mode builds require a sandbox runtime.
    SandboxUnavailable,
    /// ERR_MOJANG_NOT_FOUND — Minecraft Launcher not found.
    MojangNotFound,
    /// ERR_LAUNCH_FAILED — Could not start Minecraft.
    LaunchFailed,
    /// ERR_LOCAL_STATE_FAILED — Local state database error.
    LocalStateFailed,
    /// ERR_INSTANCE_CREATE_FAILED — Could not create instance.
    InstanceCreateFailed,
    /// ERR_PROFILE_WRITE_FAILED — Could not update Mojang launcher profiles.
    ProfileWriteFailed,
    /// ERR_REGISTRY_MISSING — Cached registry database is missing.
    RegistryMissing,
    /// ERR_UNSUPPORTED_LOADER — This modloader version is not yet verified.
    UnsupportedLoader,
    /// ERR_VERSION_NOT_FOUND — Requested mod version not found.
    VersionNotFound,
    /// ERR_DEPENDENCY_MISSING — A mod requires a dependency.
    DependencyMissing,
    /// ERR_MCP_TOO_MANY_REQUESTS — AI client sent too many requests.
    McpTooManyRequests,
    /// ERR_MCP_DENIED — AI tool request denied based on approval preferences.
    McpDenied,
    /// ERR_MCP_UNAUTHORIZED — AI client connection rejected.
    McpUnauthorized,
    /// Catch-all for errors that do not yet have a dedicated code.
    Generic { code: String, message: String },
}

impl LauncherError {
    pub fn code(&self) -> String {
        match self {
            LauncherError::NetworkOffline => "ERR_NETWORK_OFFLINE".to_string(),
            LauncherError::RegistryDownloadFailed => "ERR_REGISTRY_DOWNLOAD_FAILED".to_string(),
            LauncherError::RegistrySignatureInvalid => "ERR_REGISTRY_SIGNATURE_INVALID".to_string(),
            LauncherError::SchemaTooNew => "ERR_SCHEMA_TOO_NEW".to_string(),
            LauncherError::ZipBomb => "ERR_ZIP_BOMB".to_string(),
            LauncherError::OverrideSecurityViolation => "ERR_OVERRIDE_SECURITY_VIOLATION".to_string(),
            LauncherError::HashMismatch => "ERR_HASH_MISMATCH".to_string(),
            LauncherError::UntrustedSource => "ERR_UNTRUSTED_SOURCE".to_string(),
            LauncherError::DiskFull => "ERR_DISKFULL".to_string(),
            LauncherError::AuthExpired => "ERR_AUTH_EXPIRED".to_string(),
            LauncherError::AuthRequired => "ERR_AUTH_REQUIRED".to_string(),
            LauncherError::ModrinthDisabled => "ERR_MODRINTH_DISABLED".to_string(),
            LauncherError::InstanceLocked => "ERR_INSTANCE_LOCKED".to_string(),
            LauncherError::SandboxUnavailable => "ERR_SANDBOX_UNAVAILABLE".to_string(),
            LauncherError::MojangNotFound => "ERR_MOJANG_NOT_FOUND".to_string(),
            LauncherError::LaunchFailed => "ERR_LAUNCH_FAILED".to_string(),
            LauncherError::LocalStateFailed => "ERR_LOCAL_STATE_FAILED".to_string(),
            LauncherError::InstanceCreateFailed => "ERR_INSTANCE_CREATE_FAILED".to_string(),
            LauncherError::ProfileWriteFailed => "ERR_PROFILE_WRITE_FAILED".to_string(),
            LauncherError::RegistryMissing => "ERR_REGISTRY_MISSING".to_string(),
            LauncherError::UnsupportedLoader => "ERR_UNSUPPORTED_LOADER".to_string(),
            LauncherError::VersionNotFound => "ERR_VERSION_NOT_FOUND".to_string(),
            LauncherError::DependencyMissing => "ERR_DEPENDENCY_MISSING".to_string(),
            LauncherError::McpTooManyRequests => "ERR_MCP_TOO_MANY_REQUESTS".to_string(),
            LauncherError::McpDenied => "ERR_MCP_DENIED".to_string(),
            LauncherError::McpUnauthorized => "ERR_MCP_UNAUTHORIZED".to_string(),
            LauncherError::Generic { code, .. } => code.clone(),
        }
    }
}

impl std::fmt::Display for LauncherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LauncherError::NetworkOffline => write!(f, "You're offline. Using cached data."),
            LauncherError::RegistryDownloadFailed => {
                write!(f, "Could not download the latest registry. Using cached version.")
            }
            LauncherError::RegistrySignatureInvalid => {
                write!(f, "Registry signature check failed. The database may be compromised.")
            }
            LauncherError::SchemaTooNew => {
                write!(f, "This registry requires a newer launcher version. Please update the app.")
            }
            LauncherError::ZipBomb => {
                write!(f, "Installation aborted: this archive exceeds safety limits.")
            }
            LauncherError::OverrideSecurityViolation => {
                write!(f, "Installation aborted: pack override contains forbidden files.")
            }
            LauncherError::HashMismatch => {
                write!(f, "Downloaded file does not match its expected hash. It may be corrupted or tampered with.")
            }
            LauncherError::UntrustedSource => {
                write!(f, "Download rejected: URL is not from an allowed source.")
            }
            LauncherError::DiskFull => {
                write!(f, "Not enough disk space to complete this operation.")
            }
            LauncherError::AuthExpired => {
                write!(f, "Your GitHub session has expired. Sign in again to continue.")
            }
            LauncherError::AuthRequired => {
                write!(f, "This feature requires GitHub sign-in.")
            }
            LauncherError::ModrinthDisabled => {
                write!(f, "Modrinth integration is disabled. Enable it in Settings or install this mod manually.")
            }
            LauncherError::InstanceLocked => {
                write!(f, "This instance is locked. Unlock it to add or remove mods.")
            }
            LauncherError::SandboxUnavailable => {
                write!(f, "Dev Mode builds require Docker, Podman, or Firecracker.")
            }
            LauncherError::MojangNotFound => {
                write!(f, "Minecraft Launcher not found. Please install it or set its path in Settings.")
            }
            LauncherError::LaunchFailed => {
                write!(f, "Could not start Minecraft. Check the logs for details.")
            }
            LauncherError::LocalStateFailed => {
                write!(f, "Local state database error. The app may be misconfigured or out of disk space.")
            }
            LauncherError::InstanceCreateFailed => {
                write!(f, "Could not create the instance.")
            }
            LauncherError::ProfileWriteFailed => {
                write!(f, "Could not update the Mojang launcher profiles. The file may be locked or corrupt.")
            }
            LauncherError::RegistryMissing => {
                write!(f, "The cached registry database is missing. Please connect to the internet and restart.")
            }
            LauncherError::UnsupportedLoader => {
                write!(f, "This modloader version is not yet verified by the curation team.")
            }
            LauncherError::VersionNotFound => {
                write!(f, "Requested mod version not found. Install the closest compatible version?")
            }
            LauncherError::DependencyMissing => {
                write!(f, "A mod requires a dependency. Try installing it?")
            }
            LauncherError::McpTooManyRequests => {
                write!(f, "AI client sent too many requests. Approve or deny pending requests first.")
            }
            LauncherError::McpDenied => {
                write!(f, "AI tool request denied based on your saved approval preferences.")
            }
            LauncherError::McpUnauthorized => {
                write!(f, "AI client connection rejected: invalid or missing token.")
            }
            LauncherError::Generic { message, .. } => write!(f, "{}", message),
        }
    }
}

impl std::error::Error for LauncherError {}

pub type LauncherResult<T> = Result<T, LauncherError>;
