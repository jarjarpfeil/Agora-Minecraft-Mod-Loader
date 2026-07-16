use crate::error::{LauncherError, LauncherResult};
use crate::loader_manifests;
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;

const MOD_DOWNLOAD_ALLOWLIST: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "api.github.com",
    "cdn.modrinth.com",
    "api.modrinth.com",
];

/// Raw SHA-256 hex digest of a byte slice.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Lowercase SHA-1 hex digest of a byte slice.
pub fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Produce a canonical serialization of a JSON value for stable hashing.
///
/// Sorts object keys recursively, drops `time` and `releaseTime` fields (which
/// Mojang changes when re-releasing the same version), and uses a deterministic
/// serializer with no extra whitespace. This is the same algorithm the compiler
/// generator uses when computing `version_json_sha256`.
pub fn canonical_version_json(value: &serde_json::Value) -> String {
    fn strip_and_sort(val: &serde_json::Value) -> serde_json::Value {
        match val {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map
                    .iter()
                    .filter(|(key, _)| key.as_str() != "time" && key.as_str() != "releaseTime")
                    .map(|(key, value)| (key.clone(), strip_and_sort(value)))
                    .collect();
                entries.sort_by(|(a, _), (b, _)| a.cmp(b));
                serde_json::Value::Object(entries.into_iter().collect())
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(strip_and_sort).collect())
            }
            other => other.clone(),
        }
    }
    let cleaned = strip_and_sort(value);
    serde_json::to_string(&cleaned).expect("canonical_version_json serialization must not fail")
}

/// Download a mod artifact using the dedicated GitHub/Modrinth allowlist.
/// Both the initial URL and every redirect must remain HTTPS on port 443.
pub async fn download_mod_bytes(url: &str) -> LauncherResult<Vec<u8>> {
    let parsed = reqwest::Url::parse(url).map_err(|_| LauncherError::UntrustedSource)?;
    ensure_allowed_mod_url(&parsed)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if ensure_allowed_mod_url(attempt.url()).is_ok() {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .user_agent("AgoraLauncher/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".into(),
            message: format!("Failed to build mod download client: {e}"),
        })?;
    let response = client
        .get(parsed.clone())
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;
    if !response.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK".into(),
            message: format!("HTTP {} for {}", response.status(), parsed),
        });
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|_| LauncherError::NetworkOffline)
}

fn ensure_allowed_mod_url(url: &reqwest::Url) -> LauncherResult<()> {
    let host = url.host_str().ok_or(LauncherError::UntrustedSource)?;
    if url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && MOD_DOWNLOAD_ALLOWLIST.contains(&host)
    {
        Ok(())
    } else {
        Err(LauncherError::UntrustedSource)
    }
}

/// Deterministic SHA-256 of a JSON payload after stripping volatile keys.
///
/// Fabric rewrites `time`/`releaseTime` on every request, so the raw response hash
/// is unstable. This mirrors `_stable_json_sha256` in `scripts/fetch_loader_manifests.py`.
pub fn stable_json_sha256(data: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(data).ok()?;
    let mut obj = value;
    if let serde_json::Value::Object(map) = &mut obj {
        map.remove("time");
        map.remove("releaseTime");
    }
    let canonical = serde_json::to_string(&obj).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Some(hex::encode(hasher.finalize()))
}

/// Compute the expected hash for a loader file. Profile JSONs (Fabric/Quilt)
/// use the stable normalized hash; installer jars use the raw hash.
pub fn compute_loader_hash(loader: &str, _file_name: &str, file_type: &str, data: &[u8]) -> String {
    if file_type == "profile_json" && (loader == "fabric" || loader == "quilt") {
        if let Some(stable) = stable_json_sha256(data) {
            return stable;
        }
    }
    sha256_hex(data)
}

/// Download bytes from a URL using a redirect-safe client.
///
/// Redirects are only followed when the target host is on the embedded loader
/// domain allowlist, preventing SSRF via compromised/malicious pinned hosts.
pub async fn download_bytes(url: &str) -> LauncherResult<Vec<u8>> {
    loader_manifests::ensure_allowed_domain(url).map_err(|error| {
        eprintln!(
            "[loader-download] rejected stage=initial-allowlist url={}",
            crate::network::sanitized_url_for_log(url)
        );
        error
    })?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            if url.scheme() == "https" && url.port_or_known_default() == Some(443) {
                if let Some(host) = url.host_str() {
                    if loader_manifests::is_allowed_host(host) {
                        return attempt.follow();
                    }
                }
            }
            eprintln!(
                "[loader-download] rejected stage=redirect url={}",
                crate::network::sanitized_url_for_log(url.as_str())
            );
            attempt.stop()
        }))
        .user_agent("AgoraLoaderManifestBot/1.0")
        .build()
        .map_err(|e| LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|_| LauncherError::NetworkOffline)?;

    if !resp.status().is_success() {
        return Err(LauncherError::Generic {
            code: "ERR_NETWORK".to_string(),
            message: format!("HTTP {} for {}", resp.status(), url),
        });
    }

    loader_manifests::ensure_allowed_domain(resp.url().as_str()).map_err(|error| {
        eprintln!(
            "[loader-download] rejected stage=final-url url={}",
            crate::network::sanitized_url_for_log(resp.url().as_str())
        );
        error
    })?;

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|_| LauncherError::NetworkOffline)
}

/// Download a loader file and verify its hash against the pinned value.
pub async fn download_verified(
    loader: &str,
    file_name: &str,
    file_type: &str,
    url: &str,
    expected_sha: &str,
) -> LauncherResult<Vec<u8>> {
    loader_manifests::ensure_allowed_domain(url).map_err(|error| {
        eprintln!(
            "[loader-download] rejected stage=verified-initial loader={loader} file={file_name} url={}",
            crate::network::sanitized_url_for_log(url)
        );
        error
    })?;
    let data = download_bytes(url).await?;
    let actual = compute_loader_hash(loader, file_name, file_type, &data);

    if actual != loader_manifests::strip_sha_prefix(expected_sha) {
        return Err(LauncherError::HashMismatch);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn stable_json_sha256_is_order_independent() {
        // Verify that reordering keys does not change the hash.
        let input1 = br#"{"a":1,"b":2}"#;
        let input2 = br#"{"b":2,"a":1}"#;
        let hash1 = stable_json_sha256(input1).unwrap();
        let hash2 = stable_json_sha256(input2).unwrap();
        assert_eq!(hash1, hash2, "reordering keys must produce the same hash");
    }

    #[test]
    fn stable_json_sha256_matches_python_canonicalization() {
        // This test vector must correspond to what the Python script
        // (`scripts/fetch_loader_manifests.py`, `_stable_json_sha256`)
        // computes for the same input.
        let input = br#"{"id":"1.21","mainClass":"net.minecraft.client.main.Main","time":"2024-06-13T15:00:00+00:00","releaseTime":"2024-06-13T15:00:00+00:00","type":"release"}"#;
        // Python sorts keys with `sort_keys=True` after dropping time/releaseTime.
        // The expected canonical is: {"id":"1.21","mainClass":"net.minecraft.client.main.Main","type":"release"}
        let hash = stable_json_sha256(input).unwrap();
        // Compute expected: SHA-256 of '{"id":"1.21","mainClass":"net.minecraft.client.main.Main","type":"release"}'
        let canonical =
            br#"{"id":"1.21","mainClass":"net.minecraft.client.main.Main","type":"release"}"#;
        let mut hasher = Sha256::new();
        hasher.update(canonical);
        let expected = hex::encode(hasher.finalize());
        assert_eq!(
            hash, expected,
            "stable_json_sha256 must match Python-style canonicalization"
        );
    }

    #[test]
    fn canonical_version_json_rust_python_parity() {
        // This hardcoded test vector ensures Rust and Python produce the
        // same canonical JSON hash for a realistic version.json.
        // Python (scripts/fetch_loader_manifests.py _stable_json_sha256):
        //   1. Parse JSON
        //   2. Drop "time" and "releaseTime" keys
        //   3. Serialize with sort_keys=True, separators=(',', ':'), ensure_ascii=True
        //   4. SHA-256 of UTF-8 bytes
        //
        // Rust (crate::download::canonical_version_json):
        //   1. Parse JSON
        //   2. Recursively strip "time" and "releaseTime"
        //   3. Sort object keys recursively
        //   4. Serialize with serde_json::to_string (compact, no whitespace)
        //   5. SHA-256 of UTF-8 bytes
        let version_json = serde_json::json!({
            "id": "forge-1.21-47.1.0",
            "inheritsFrom": "1.21",
            "time": "2024-07-01T12:00:00Z",
            "releaseTime": "2024-07-01T12:00:00Z",
            "mainClass": "net.minecraftforge.Main",
            "libraries": [{"name": "net.minecraftforge:forge:47.1.0"}],
            "type": "release"
        });
        let canonical = super::canonical_version_json(&version_json);
        let hash = super::sha256_hex(canonical.as_bytes());

        // Verify structure: no time/releaseTime, keys sorted
        assert!(!canonical.contains("\"time\""), "must drop time");
        assert!(
            !canonical.contains("\"releaseTime\""),
            "must drop releaseTime"
        );
        // id should appear before inheritsFrom (sorted)
        assert!(
            canonical.find("\"id\"").unwrap() < canonical.find("\"inheritsFrom\"").unwrap(),
            "id must sort before inheritsFrom"
        );

        // Verify this is deterministic
        let canonical2 = super::canonical_version_json(&version_json);
        let hash2 = super::sha256_hex(canonical2.as_bytes());
        assert_eq!(hash, hash2, "canonical JSON hash must be deterministic");

        // Cross-language parity: the Python script with the same input
        // should produce the same hash. This is the expected value from
        // running: _stable_json_sha256(json.dumps({"id":"forge-...","inheritsFrom":"1.21","mainClass":"net.minecraftforge.Main","libraries":[{"name":"net.minecraftforge:forge:47.1.0"}],"type":"release"}))
        // with time/releaseTime removed and sort_keys=True.
        let expected_python_canonical = r#"{"id":"forge-1.21-47.1.0","inheritsFrom":"1.21","libraries":[{"name":"net.minecraftforge:forge:47.1.0"}],"mainClass":"net.minecraftforge.Main","type":"release"}"#;
        assert_eq!(
            canonical, expected_python_canonical,
            "Rust canonical JSON must match Python"
        );
    }
}
