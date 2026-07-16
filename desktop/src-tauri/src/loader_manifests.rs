//! Loader manifest helpers.
//!
//! Delegates to `agora_core::loader_manifests` which provides the `LoaderCatalog`
//! type with runtime registry override and embedded fallback.

pub use agora_core::loader_manifests::{
    ensure_allowed_domain,
    find_entry,
    is_allowed_host,
    list_loaders,
    list_mc_versions,
    list_versions,
    strip_sha_prefix,
    LoaderEntry,
};

#[cfg(test)]
mod tests {
    use crate::error::LauncherError;
    use crate::loader_manifests::{
        ensure_allowed_domain, is_allowed_host, list_loaders, list_mc_versions,
    };

    // --- is_allowed_domain / ensure_allowed_domain (SSRF prevention) ---

    #[test]
    fn test_allowed_domain_fabric() {
        assert!(is_allowed_host("maven.fabricmc.net"));
        assert!(is_allowed_host("meta.fabricmc.net"));
    }

    #[test]
    fn test_allowed_domain_neoforge() {
        assert!(is_allowed_host("maven.neoforged.net"));
        assert!(is_allowed_host("neoforged.net"));
    }

    #[test]
    fn test_ensure_allowed_domain_accepts_valid() {
        assert!(ensure_allowed_domain("https://meta.fabricmc.net/v2/versions/loader/1.21/0.19.3/profile/json").is_ok());
        assert!(ensure_allowed_domain("https://maven.neoforged.net/releases/net/neoforged/neoforge/21.0.167/neoforge-21.0.167-installer.jar").is_ok());
        assert!(ensure_allowed_domain("https://files.minecraftforge.net/net/minecraftforge/forge/1.21-51.0.33/forge-1.21-51.0.33-installer.jar").is_ok());
    }

    #[test]
    fn test_ensure_allowed_domain_rejects_localhost() {
        assert!(matches!(
            ensure_allowed_domain("http://127.0.0.1/evil"),
            Err(LauncherError::UntrustedSource)
        ));
    }

    #[test]
    fn test_ensure_allowed_domain_rejects_metadata_ip() {
        assert!(matches!(
            ensure_allowed_domain("http://169.254.169.254/latest/meta-data"),
            Err(LauncherError::UntrustedSource)
        ));
    }

    #[test]
    fn test_ensure_allowed_domain_rejects_random_host() {
        assert!(matches!(
            ensure_allowed_domain("https://evil.example.com"),
            Err(LauncherError::UntrustedSource)
        ));
    }

    #[test]
    fn test_ensure_allowed_domain_rejects_file_scheme() {
        assert!(matches!(
            ensure_allowed_domain("file:///etc/passwd"),
            Err(LauncherError::UntrustedSource)
        ));
    }

    #[test]
    fn test_ensure_allowed_domain_rejects_malformed() {
        assert!(ensure_allowed_domain("not a url").is_err());
    }

    #[test]
    fn test_ensure_allowed_domain_rejects_empty() {
        assert!(ensure_allowed_domain("").is_err());
    }

    #[test]
    fn test_allowed_domain_case_insensitive() {
        assert!(!is_allowed_host("META.FABRICMC.NET"));
    }

    // --- manifest pure-state tests ---

    #[test]
    fn test_manifest_has_nonempty_allowlist() {
        let catalog = agora_core::loader_manifests::LoaderCatalog::embedded();
        assert!(!catalog.domain_allowlist.is_empty(), "domain_allowlist must not be empty");
    }

    #[test]
    fn test_manifest_has_known_loaders() {
        let catalog = agora_core::loader_manifests::LoaderCatalog::embedded();
        let loaders: Vec<String> = catalog.loaders.keys().cloned().collect();
        assert!(loaders.contains(&"fabric".to_string()), "fabric loader must be present");
        assert!(loaders.contains(&"quilt".to_string()), "quilt loader must be present");
        assert!(loaders.contains(&"neoforge".to_string()), "neoforge loader must be present");
        assert!(loaders.contains(&"forge".to_string()), "forge loader must be present");
    }
}
