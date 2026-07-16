/// Standardized launcher error codes matching the human-centric taxonomy.
#[derive(Debug, Clone)]
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
    /// ERR_GAME_VERSION_NOT_FOUND — The Minecraft version is not in the Mojang
    /// manifest. Distinct from `VersionNotFound`, which refers to *mod* versions.
    GameVersionNotFound,
    /// ERR_LOADER_PROFILE_NOT_FOUND — The modloader profile JSON (Fabric/Quilt
    /// partial version, or Forge/NeoForge install profile) could not be
    /// resolved or merged with the base Minecraft version.
    LoaderProfileNotFound,
    /// ERR_PROFILE_MISSING — The loader profile JSON is missing from the
    /// Mojang launcher's version directory.
    ProfileMissing(crate::installed_profile::ProfileIssue),
    /// ERR_PROFILE_UNSUPPORTED_METADATA — The installed profile is structurally
    /// valid but contains metadata or artifacts that this launcher version
    /// does not support (unknown rules, unverifiable generated libraries, etc.).
    ProfileUnsupportedMetadata(crate::installed_profile::ProfileIssue),
    /// ERR_PROFILE_CORRUPT — The installed profile is malformed, corrupted,
    /// or fails security validation.
    ProfileCorrupt(crate::installed_profile::ProfileIssue),
    /// ERR_JAVA_INCOMPATIBLE — The selected Java runtime is older than the
    /// major version required by this Minecraft version's metadata.
    JavaIncompatible,
    /// ERR_JAVA_RUNTIME_MISSING — No Java runtime could be found for the
    /// required major version across all candidate sources (system, Mojang,
    /// managed).
    JavaRuntimeMissing { major: u32, component: String },
    /// ERR_JAVA_RUNTIME_CATALOG_MISSING — The runtime catalog has no entry
    /// for the requested (major, os, arch) tuple. The platform is not
    /// supported by the curated catalog.
    JavaRuntimeCatalogMissing {
        major: u32,
        os: String,
        arch: String,
    },
    /// ERR_UNRESOLVED_PLACEHOLDER — A `${...}` token in the JVM or game
    /// arguments could not be substituted before spawn.
    UnresolvedPlaceholder,
    /// ERR_DEPENDENCY_MISSING — A mod requires a dependency.
    DependencyMissing,
    /// ERR_MCP_TOO_MANY_REQUESTS — AI client sent too many requests.
    McpTooManyRequests,
    /// ERR_MCP_DENIED — AI tool request denied based on approval preferences.
    McpDenied,
    /// ERR_MCP_UNAUTHORIZED — AI client connection rejected.
    McpUnauthorized,
    /// ERR_NETWORK_MOJANG_METADATA_DISABLED — Mojang metadata fetches are disabled in Privacy settings.
    NetworkMojangMetadataDisabled,
    /// ERR_NETWORK_MOJANG_CONTENT_DISABLED — Mojang content downloads are disabled in Privacy settings.
    NetworkMojangContentDisabled,
    /// ERR_NETWORK_LOADER_DISABLED — Modloader metadata/content downloads are disabled in Privacy settings.
    NetworkLoaderDisabled,
    /// ERR_NETWORK_MSA_DISABLED — Microsoft account authentication is disabled in Privacy settings.
    NetworkMsaDisabled,
    /// ERR_NETWORK_JAVA_DISABLED — Java runtime downloads are disabled in Privacy settings.
    NetworkJavaDisabled,
    /// ERR_JAVA_RUNTIME_CANCELLED — The Java runtime provisioning was cancelled by the user.
    JavaRuntimeCancelled { major: u32, component: String },
    /// ERR_JAVA_RUNTIME_DOWNLOAD_DISABLED — Java runtime downloads are disabled,
    /// with structured details for the frontend to show suggested actions.
    JavaRuntimeDownloadDisabled { major: u32, component: String },
    /// ERR_MAVEN_DESCRIPTOR — A Maven descriptor string is malformed or unsafe.
    MavenDescriptor,
    /// ERR_PROCESS_CAPTURE_FAILED — Could not capture OS identity for a just‑spawned
    /// process. The child was killed and the launch aborted.
    ProcessCaptureFailed { pid: u32, detail: String },
    /// ERR_PROCESS_STALE — The tracked OS process no longer matches its captured
    /// identity (PID was reused, the process died, or the executable changed).
    /// The stale record has been detached and the caller should treat it as idle.
    ProcessStale { pid: u32, detail: String },
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
            LauncherError::OverrideSecurityViolation => {
                "ERR_OVERRIDE_SECURITY_VIOLATION".to_string()
            }
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
            LauncherError::GameVersionNotFound => "ERR_GAME_VERSION_NOT_FOUND".to_string(),
            LauncherError::LoaderProfileNotFound => "ERR_LOADER_PROFILE_NOT_FOUND".to_string(),
            LauncherError::ProfileMissing(..) => "ERR_PROFILE_MISSING".to_string(),
            LauncherError::ProfileUnsupportedMetadata(..) => {
                "ERR_PROFILE_UNSUPPORTED_METADATA".to_string()
            }
            LauncherError::ProfileCorrupt(..) => "ERR_PROFILE_CORRUPT".to_string(),
            LauncherError::JavaIncompatible => "ERR_JAVA_INCOMPATIBLE".to_string(),
            LauncherError::JavaRuntimeMissing { .. } => "ERR_JAVA_RUNTIME_MISSING".to_string(),
            LauncherError::JavaRuntimeCatalogMissing { .. } => {
                "ERR_JAVA_RUNTIME_CATALOG_MISSING".to_string()
            }
            LauncherError::UnresolvedPlaceholder => "ERR_UNRESOLVED_PLACEHOLDER".to_string(),
            LauncherError::DependencyMissing => "ERR_DEPENDENCY_MISSING".to_string(),
            LauncherError::McpTooManyRequests => "ERR_MCP_TOO_MANY_REQUESTS".to_string(),
            LauncherError::McpDenied => "ERR_MCP_DENIED".to_string(),
            LauncherError::McpUnauthorized => "ERR_MCP_UNAUTHORIZED".to_string(),
            LauncherError::NetworkMojangMetadataDisabled => {
                "ERR_NETWORK_MOJANG_METADATA_DISABLED".to_string()
            }
            LauncherError::NetworkMojangContentDisabled => {
                "ERR_NETWORK_MOJANG_CONTENT_DISABLED".to_string()
            }
            LauncherError::NetworkLoaderDisabled => "ERR_NETWORK_LOADER_DISABLED".to_string(),
            LauncherError::NetworkMsaDisabled => "ERR_NETWORK_MSA_DISABLED".to_string(),
            LauncherError::NetworkJavaDisabled => "ERR_NETWORK_JAVA_DISABLED".to_string(),
            LauncherError::MavenDescriptor => "ERR_MAVEN_DESCRIPTOR".to_string(),
            LauncherError::ProcessCaptureFailed { .. } => "ERR_PROCESS_CAPTURE_FAILED".to_string(),
            LauncherError::ProcessStale { .. } => "ERR_PROCESS_STALE".to_string(),
            LauncherError::JavaRuntimeCancelled { .. } => "ERR_JAVA_RUNTIME_CANCELLED".to_string(),
            LauncherError::JavaRuntimeDownloadDisabled { .. } => {
                "ERR_JAVA_RUNTIME_DOWNLOAD_DISABLED".to_string()
            }
            LauncherError::Generic { code, .. } => code.clone(),
        }
    }
}

impl std::fmt::Display for LauncherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LauncherError::NetworkOffline => write!(f, "You're offline. Using cached data."),
            LauncherError::RegistryDownloadFailed => {
                write!(
                    f,
                    "Could not download the latest registry. Using cached version."
                )
            }
            LauncherError::RegistrySignatureInvalid => {
                write!(
                    f,
                    "Registry signature check failed. The database may be compromised."
                )
            }
            LauncherError::SchemaTooNew => {
                write!(
                    f,
                    "This registry requires a newer launcher version. Please update the app."
                )
            }
            LauncherError::ZipBomb => {
                write!(
                    f,
                    "Installation aborted: this archive exceeds safety limits."
                )
            }
            LauncherError::OverrideSecurityViolation => {
                write!(
                    f,
                    "Installation aborted: pack override contains forbidden files."
                )
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
                write!(
                    f,
                    "Your GitHub session has expired. Sign in again to continue."
                )
            }
            LauncherError::AuthRequired => {
                write!(f, "This feature requires GitHub sign-in.")
            }
            LauncherError::ModrinthDisabled => {
                write!(f, "Modrinth integration is disabled. Enable it in Settings or install this mod manually.")
            }
            LauncherError::InstanceLocked => {
                write!(
                    f,
                    "This instance is locked. Unlock it to add or remove mods."
                )
            }
            LauncherError::SandboxUnavailable => {
                write!(f, "Dev Mode builds require Docker, Podman, or Firecracker.")
            }
            LauncherError::MojangNotFound => {
                write!(
                    f,
                    "Minecraft Launcher not found. Please install it or set its path in Settings."
                )
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
                write!(
                    f,
                    "This modloader version is not yet verified by the curation team."
                )
            }

            LauncherError::VersionNotFound => {
                write!(
                    f,
                    "Requested mod version not found. Install the closest compatible version?"
                )
            }
            LauncherError::GameVersionNotFound => {
                write!(
                    f,
                    "This Minecraft version was not found in the official version manifest."
                )
            }
            LauncherError::LoaderProfileNotFound => {
                write!(
                    f,
                    "The modloader profile for this instance could not be resolved or merged with the base Minecraft version."
                )
            }
            LauncherError::ProfileMissing(ref issue) => {
                write!(f, "Profile missing: {}", issue.reasons.join("; "))
            }
            LauncherError::ProfileUnsupportedMetadata(ref issue) => {
                write!(
                    f,
                    "Profile metadata not supported: {}",
                    issue.reasons.join("; ")
                )
            }
            LauncherError::ProfileCorrupt(ref issue) => {
                write!(f, "Profile corrupted: {}", issue.reasons.join("; "))
            }
            LauncherError::JavaIncompatible => {
                write!(
                    f,
                    "The selected Java runtime is older than the version required by this Minecraft version."
                )
            }
            LauncherError::JavaRuntimeMissing { major, component } => {
                write!(
                    f,
                    "No Java {major} runtime found (component: {component}). Install a compatible JDK/JRE."
                )
            }
            LauncherError::JavaRuntimeCatalogMissing { major, os, arch } => {
                write!(
                    f,
                    "No catalog entry for Java {major} on {os}/{arch}. This platform is not supported by the curated catalog."
                )
            }
            LauncherError::UnresolvedPlaceholder => {
                write!(
                    f,
                    "The launch arguments reference a placeholder that could not be substituted."
                )
            }
            LauncherError::DependencyMissing => {
                write!(f, "A mod requires a dependency. Try installing it?")
            }
            LauncherError::McpTooManyRequests => {
                write!(
                    f,
                    "AI client sent too many requests. Approve or deny pending requests first."
                )
            }
            LauncherError::McpDenied => {
                write!(
                    f,
                    "AI tool request denied based on your saved approval preferences."
                )
            }
            LauncherError::McpUnauthorized => {
                write!(
                    f,
                    "AI client connection rejected: invalid or missing token."
                )
            }
            LauncherError::NetworkMojangMetadataDisabled => {
                write!(
                    f,
                    "Mojang metadata fetches are disabled in Privacy settings."
                )
            }
            LauncherError::NetworkMojangContentDisabled => {
                write!(
                    f,
                    "Mojang content downloads are disabled in Privacy settings."
                )
            }
            LauncherError::NetworkLoaderDisabled => {
                write!(
                    f,
                    "Modloader metadata and content downloads are disabled in Privacy settings."
                )
            }
            LauncherError::NetworkMsaDisabled => {
                write!(
                    f,
                    "Microsoft account authentication is disabled in Privacy settings."
                )
            }
            LauncherError::NetworkJavaDisabled => {
                write!(
                    f,
                    "Java runtime downloads are disabled in Privacy settings."
                )
            }
            LauncherError::MavenDescriptor => {
                write!(f, "Invalid Maven descriptor string.")
            }
            LauncherError::ProcessCaptureFailed { pid, detail } => {
                write!(f, "Could not capture OS identity for PID {pid}: {detail}")
            }
            LauncherError::ProcessStale { pid, detail } => {
                write!(f, "Process PID {pid} is stale: {detail}")
            }
            LauncherError::JavaRuntimeCancelled { major, component } => {
                write!(
                    f,
                    "Java {major} runtime provisioning was cancelled (component: {component})."
                )
            }
            LauncherError::JavaRuntimeDownloadDisabled { major, component } => {
                write!(
                    f,
                    "Java {major} runtime download is disabled (component: {component}). \
                     Enable runtime downloads in Privacy settings or choose a local Java installation."
                )
            }
            LauncherError::Generic { message, .. } => write!(f, "{}", message),
        }
    }
}

impl serde::Serialize for LauncherError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let suggested_action = match self {
            LauncherError::MojangNotFound => {
                Some("Install the Minecraft Launcher or set its path in Settings.")
            }
            LauncherError::HashMismatch => {
                Some("The downloaded file may be corrupted or from an untrusted source. Try again.")
            }
            LauncherError::NetworkOffline => Some("Check your internet connection and try again."),
            LauncherError::AuthExpired => Some("Sign in again via Settings to continue."),
            LauncherError::AuthRequired => Some("Sign in via Settings to use this feature."),
            LauncherError::GameVersionNotFound => {
                Some("Check that the instance targets a released Minecraft version and update the registry.")
            }
            LauncherError::LoaderProfileNotFound => {
                Some("Re-create the instance or select a supported modloader version.")
            }
            LauncherError::JavaIncompatible => {
                Some("Install a newer Java runtime or select a compatible one in Settings.")
            }
            LauncherError::JavaRuntimeMissing { .. } => {
                None // structured suggested_actions used instead
            }
            LauncherError::JavaRuntimeCatalogMissing { .. } => {
                None  // structured suggested_actions used instead
            }
            LauncherError::JavaRuntimeCancelled { .. } => None,
            LauncherError::JavaRuntimeDownloadDisabled { .. } => None,
            LauncherError::UnresolvedPlaceholder => {
                Some("This Minecraft version's arguments are not fully supported yet. Update Agora.")
            }
            _ => None,
        };

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("code", &self.code())?;
        map.serialize_entry("message", &self.to_string())?;

        // For profile issue variants, serialize structured recoverable_issue details.
        match self {
            LauncherError::ProfileMissing(ref issue)
            | LauncherError::ProfileUnsupportedMetadata(ref issue)
            | LauncherError::ProfileCorrupt(ref issue) => {
                let details = serde_json::json!({
                    "recoverable_issue": {
                        "kind": issue.kind,
                        "profile_path": issue.profile_path,
                        "reasons": issue.reasons,
                    },
                    "suggested_actions": issue.suggested_actions(),
                });
                map.serialize_entry("details", &details)?;
            }
            LauncherError::JavaRuntimeMissing { major, component } => {
                let details = serde_json::json!({
                    "major": major,
                    "component": component,
                    "suggested_actions": [
                        "download_runtime",
                        "choose_java",
                        "cancel",
                    ],
                });
                map.serialize_entry("details", &details)?;
            }
            LauncherError::JavaRuntimeCatalogMissing { major, os, arch } => {
                let details = serde_json::json!({
                    "major": major,
                    "os": os,
                    "arch": arch,
                    "suggested_actions": [
                        "choose_java",
                        "cancel",
                    ],
                });
                map.serialize_entry("details", &details)?;
            }
            LauncherError::JavaRuntimeCancelled { major, component } => {
                let details = serde_json::json!({
                    "major": major,
                    "component": component,
                    "suggested_actions": ["cancel"],
                });
                map.serialize_entry("details", &details)?;
            }
            LauncherError::JavaRuntimeDownloadDisabled { major, component } => {
                let details = serde_json::json!({
                    "major": major,
                    "component": component,
                    "suggested_actions": [
                        "choose_java",
                        "open_privacy",
                        "cancel",
                    ],
                });
                map.serialize_entry("details", &details)?;
            }
            _ => {
                map.serialize_entry("details", &None::<String>)?;
            }
        }

        map.serialize_entry("suggested_action", &suggested_action)?;
        map.end()
    }
}

impl std::error::Error for LauncherError {}

pub type LauncherResult<T> = Result<T, LauncherError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_generic_error() {
        let err = LauncherError::Generic {
            code: "ERR_TEST".into(),
            message: "test message".into(),
        };
        let val = serde_json::to_value(err).unwrap();
        assert_eq!(val.get("code").unwrap().as_str().unwrap(), "ERR_TEST");
        assert_eq!(
            val.get("message").unwrap().as_str().unwrap(),
            "test message"
        );
        assert_eq!(val.get("details"), Some(&serde_json::Value::Null));
        assert_eq!(val.get("suggested_action"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn test_serialize_hash_mismatch_has_suggested_action() {
        let err = LauncherError::HashMismatch;
        let val = serde_json::to_value(err).unwrap();
        assert!(val.get("suggested_action").unwrap().as_str().is_some());
    }

    #[test]
    fn test_serialize_mojang_not_found_has_suggested_action() {
        let err = LauncherError::MojangNotFound;
        let val = serde_json::to_value(err).unwrap();
        assert!(val.get("suggested_action").unwrap().as_str().is_some());
    }

    #[test]
    fn test_serialize_network_offline_has_suggested_action() {
        let err = LauncherError::NetworkOffline;
        let val = serde_json::to_value(err).unwrap();
        assert!(val.get("suggested_action").unwrap().as_str().is_some());
    }

    #[test]
    fn test_serialize_auth_required_has_suggested_action() {
        let err = LauncherError::AuthRequired;
        let val = serde_json::to_value(err).unwrap();
        assert!(val.get("suggested_action").unwrap().as_str().is_some());
    }

    #[test]
    fn test_serialize_local_state_failed() {
        let err = LauncherError::LocalStateFailed;
        let val = serde_json::to_value(err).unwrap();
        assert!(val.get("code").unwrap().as_str().is_some());
        assert_eq!(val.get("details"), Some(&serde_json::Value::Null));
        assert_eq!(val.get("suggested_action"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn test_serialize_roundtrip_preserves_code() {
        let err = LauncherError::Generic {
            code: "ERR_RT".into(),
            message: "roundtrip".into(),
        };
        let val = serde_json::to_value(err).unwrap();
        let code: &str = val.get("code").unwrap().as_str().unwrap();
        assert_eq!(code, "ERR_RT");
    }

    #[test]
    fn test_all_variants_serialize_without_panic() {
        let variants: Vec<LauncherError> = vec![
            LauncherError::NetworkOffline,
            LauncherError::RegistryDownloadFailed,
            LauncherError::RegistrySignatureInvalid,
            LauncherError::SchemaTooNew,
            LauncherError::ZipBomb,
            LauncherError::OverrideSecurityViolation,
            LauncherError::HashMismatch,
            LauncherError::UntrustedSource,
            LauncherError::DiskFull,
            LauncherError::AuthExpired,
            LauncherError::AuthRequired,
            LauncherError::ModrinthDisabled,
            LauncherError::InstanceLocked,
            LauncherError::SandboxUnavailable,
            LauncherError::MojangNotFound,
            LauncherError::LaunchFailed,
            LauncherError::LocalStateFailed,
            LauncherError::InstanceCreateFailed,
            LauncherError::ProfileWriteFailed,
            LauncherError::RegistryMissing,
            LauncherError::UnsupportedLoader,
            LauncherError::VersionNotFound,
            LauncherError::GameVersionNotFound,
            LauncherError::LoaderProfileNotFound,
            LauncherError::JavaIncompatible,
            LauncherError::JavaRuntimeMissing {
                major: 21,
                component: "client".into(),
            },
            LauncherError::JavaRuntimeCatalogMissing {
                major: 21,
                os: "linux".into(),
                arch: "x64".into(),
            },
            LauncherError::UnresolvedPlaceholder,
            LauncherError::DependencyMissing,
            LauncherError::McpTooManyRequests,
            LauncherError::McpDenied,
            LauncherError::McpUnauthorized,
            LauncherError::NetworkMojangMetadataDisabled,
            LauncherError::NetworkMojangContentDisabled,
            LauncherError::NetworkLoaderDisabled,
            LauncherError::NetworkMsaDisabled,
            LauncherError::NetworkJavaDisabled,
            LauncherError::JavaRuntimeCancelled {
                major: 21,
                component: "client".into(),
            },
            LauncherError::JavaRuntimeDownloadDisabled {
                major: 21,
                component: "client".into(),
            },
            LauncherError::MavenDescriptor,
            LauncherError::ProcessCaptureFailed {
                pid: 42,
                detail: "test".into(),
            },
            LauncherError::ProcessStale {
                pid: 42,
                detail: "test".into(),
            },
            LauncherError::ProfileMissing(crate::installed_profile::ProfileIssue {
                kind: crate::installed_profile::ProfileIssueKind::MissingProfile,
                profile_path: None,
                reasons: vec!["test".into()],
            }),
            LauncherError::ProfileUnsupportedMetadata(crate::installed_profile::ProfileIssue {
                kind: crate::installed_profile::ProfileIssueKind::UnsupportedProfileMetadata,
                profile_path: None,
                reasons: vec!["test".into()],
            }),
            LauncherError::ProfileCorrupt(crate::installed_profile::ProfileIssue {
                kind: crate::installed_profile::ProfileIssueKind::CorruptProfile,
                profile_path: None,
                reasons: vec!["test".into()],
            }),
            LauncherError::Generic {
                code: "ERR_X".into(),
                message: "x".into(),
            },
        ];
        for err in variants {
            let val = serde_json::to_value(err).unwrap();
            assert!(val.get("code").is_some());
        }
    }

    #[test]
    fn test_serialize_profile_missing_structured_details() {
        use crate::installed_profile::{ProfileIssue, ProfileIssueKind};

        let issue = ProfileIssue {
            kind: ProfileIssueKind::MissingProfile,
            profile_path: None,
            reasons: vec!["File not found".into()],
        };
        let err = LauncherError::ProfileMissing(issue);
        let val = serde_json::to_value(err).unwrap();

        assert_eq!(
            val.get("code").unwrap().as_str().unwrap(),
            "ERR_PROFILE_MISSING"
        );
        let details = val.get("details").unwrap().as_object().unwrap();
        let recoverable = details
            .get("recoverable_issue")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(
            recoverable.get("kind").unwrap().as_str().unwrap(),
            "MissingProfile"
        );
        assert_eq!(
            recoverable.get("reasons").unwrap().as_array().unwrap()[0]
                .as_str()
                .unwrap(),
            "File not found"
        );
        let suggested_actions = details
            .get("suggested_actions")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(!suggested_actions.is_empty());
        assert_eq!(val.get("suggested_action"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn test_serialize_profile_unsupported_structured_details() {
        use crate::installed_profile::{ProfileIssue, ProfileIssueKind};

        let issue = ProfileIssue {
            kind: ProfileIssueKind::UnsupportedProfileMetadata,
            profile_path: Some(std::path::PathBuf::from("/fake/path.json")),
            reasons: vec!["Unknown feature".into()],
        };
        let err = LauncherError::ProfileUnsupportedMetadata(issue);
        let val = serde_json::to_value(err).unwrap();

        assert_eq!(
            val.get("code").unwrap().as_str().unwrap(),
            "ERR_PROFILE_UNSUPPORTED_METADATA"
        );
        let details = val.get("details").unwrap().as_object().unwrap();
        let recoverable = details
            .get("recoverable_issue")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(
            recoverable.get("kind").unwrap().as_str().unwrap(),
            "UnsupportedProfileMetadata"
        );
        assert!(recoverable.get("profile_path").is_some());
    }

    #[test]
    fn test_serialize_profile_corrupt_structured_details() {
        use crate::installed_profile::{ProfileIssue, ProfileIssueKind};

        let issue = ProfileIssue {
            kind: ProfileIssueKind::CorruptProfile,
            profile_path: None,
            reasons: vec!["Truncated JSON".into()],
        };
        let err = LauncherError::ProfileCorrupt(issue);
        let val = serde_json::to_value(err).unwrap();

        assert_eq!(
            val.get("code").unwrap().as_str().unwrap(),
            "ERR_PROFILE_CORRUPT"
        );
        let details = val.get("details").unwrap().as_object().unwrap();
        let recoverable = details
            .get("recoverable_issue")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(
            recoverable.get("kind").unwrap().as_str().unwrap(),
            "CorruptProfile"
        );
    }

    #[test]
    fn test_profile_missing_suggested_actions() {
        use crate::installed_profile::{ProfileIssue, ProfileIssueKind};
        let issue = ProfileIssue {
            kind: ProfileIssueKind::MissingProfile,
            profile_path: None,
            reasons: vec!["gone".into()],
        };
        let err = LauncherError::ProfileMissing(issue);
        let val = serde_json::to_value(err).unwrap();
        let actions = val["details"]["suggested_actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actions, vec!["reinstall_loader", "use_delegated_launch"]);
    }

    #[test]
    fn test_profile_unsupported_suggested_actions() {
        use crate::installed_profile::{ProfileIssue, ProfileIssueKind};
        let issue = ProfileIssue {
            kind: ProfileIssueKind::UnsupportedProfileMetadata,
            profile_path: None,
            reasons: vec!["weird".into()],
        };
        let err = LauncherError::ProfileUnsupportedMetadata(issue);
        let val = serde_json::to_value(err).unwrap();
        let actions = val["details"]["suggested_actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            vec!["reinstall_loader", "use_delegated_launch", "dismiss"]
        );
    }

    #[test]
    fn test_java_runtime_missing_structured_details() {
        let err = LauncherError::JavaRuntimeMissing {
            major: 21,
            component: "java-runtime-gamma".into(),
        };
        let val = serde_json::to_value(err).unwrap();
        assert_eq!(
            val.get("code").unwrap().as_str().unwrap(),
            "ERR_JAVA_RUNTIME_MISSING"
        );
        let details = val.get("details").unwrap().as_object().unwrap();
        assert_eq!(details.get("major").unwrap().as_u64(), Some(21));
        assert_eq!(
            details.get("component").unwrap().as_str(),
            Some("java-runtime-gamma")
        );
        let actions = details
            .get("suggested_actions")
            .unwrap()
            .as_array()
            .unwrap();
        let action_strs: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(
            action_strs,
            vec!["download_runtime", "choose_java", "cancel"]
        );
    }

    #[test]
    fn test_java_runtime_catalog_missing_structured_details() {
        let err = LauncherError::JavaRuntimeCatalogMissing {
            major: 21,
            os: "linux".into(),
            arch: "x64".into(),
        };
        let val = serde_json::to_value(err).unwrap();
        assert_eq!(
            val.get("code").unwrap().as_str().unwrap(),
            "ERR_JAVA_RUNTIME_CATALOG_MISSING"
        );
        let details = val.get("details").unwrap().as_object().unwrap();
        assert_eq!(details.get("major").unwrap().as_u64(), Some(21));
        assert_eq!(details.get("os").unwrap().as_str(), Some("linux"));
        let actions = details
            .get("suggested_actions")
            .unwrap()
            .as_array()
            .unwrap();
        let action_strs: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(action_strs, vec!["choose_java", "cancel"]);
    }

    #[test]
    fn test_java_runtime_missing_suggested_action_is_none() {
        let err = LauncherError::JavaRuntimeMissing {
            major: 17,
            component: "client".into(),
        };
        let val = serde_json::to_value(err).unwrap();
        assert_eq!(val.get("suggested_action"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn test_profile_corrupt_suggested_actions_includes_delegated() {
        use crate::installed_profile::{ProfileIssue, ProfileIssueKind};
        let issue = ProfileIssue {
            kind: ProfileIssueKind::CorruptProfile,
            profile_path: None,
            reasons: vec!["bad data".into()],
        };
        let err = LauncherError::ProfileCorrupt(issue);
        let val = serde_json::to_value(err).unwrap();
        let actions = val["details"]["suggested_actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            vec!["reinstall_loader", "use_delegated_launch", "dismiss"]
        );
    }
}
