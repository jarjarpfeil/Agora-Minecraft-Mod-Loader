use crate::error::{LauncherError, LauncherResult};
use std::io::Write;
use std::path::PathBuf;

/// Append a diagnostic line to `agora-mojang.log` in the OS temp dir.
///
/// On Windows the Tauri exe detaches from the launching terminal (especially
/// under `npm run tauri:dev`), so stderr vanishes. File logging lets us inspect
/// every candidate path / command tried during launcher discovery to diagnose
/// `ERR_MOJANG_NOT_FOUND` reports.
fn log_line(line: &str) {
    let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let entry = format!("[{stamp}] {line}\n");
    let path = std::env::temp_dir().join("agora-mojang.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(entry.as_bytes());
        let _ = f.flush();
    }
}

/// Resolve the official Mojang launcher executable path.
///
/// Priority:
/// 1. User override (passed in; in production this comes from `user_settings`).
/// 2. OS-specific discovery.
/// 3. Error `MojangNotFound`.
pub fn resolve_launcher_path(user_override: Option<&str>) -> LauncherResult<PathBuf> {
    log_line(&format!(
        "resolve_launcher_path called, user_override={:?}",
        user_override
    ));

    if let Some(p) = user_override {
        let path = PathBuf::from(p);
        let exists = path.exists();
        log_line(&format!(
            "user_override path: {} (exists={})",
            path.display(),
            exists
        ));
        if exists {
            log_line(&format!("using user override: {}", path.display()));
            return Ok(path);
        }
    }

    #[cfg(target_os = "windows")]
    {
        log_line("trying discover_windows()");
        if let Some(p) = discover_windows() {
            log_line(&format!("discover_windows found: {}", p.display()));
            return Ok(p);
        }
        log_line("discover_windows returned None");
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(p) = discover_macos() {
            return Ok(p);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(p) = discover_linux() {
            return Ok(p);
        }
    }

    log_line("ERROR: no launcher found — returning MojangNotFound");
    Err(LauncherError::MojangNotFound)
}

#[cfg(target_os = "windows")]
fn discover_windows() -> Option<PathBuf> {
    // The launcher binary has been renamed over the years. Legacy Mojang
    // installer and per-user install ship `MinecraftLauncher.exe`; the newer
    // Xbox app install (default location C:\XboxGames\) ships `Minecraft.exe`
    // inside a `Content` subfolder. Probe both names against every known root.
    const EXE_NAMES: &[&str] = &["MinecraftLauncher.exe", "Minecraft.exe"];

    // 1. Hardcoded legacy / per-user / Xbox app install roots. The Xbox app
    //    location is configurable via the Xbox app itself; the default is
    //    `C:\XboxGames\Minecraft Launcher\Content\` (note: Content subdir).
    let mut legacy_roots: Vec<PathBuf> = vec![
        PathBuf::from("C:\\Program Files (x86)\\Minecraft Launcher"),
        PathBuf::from("C:\\Program Files\\Minecraft Launcher"),
    ];
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        legacy_roots.push(PathBuf::from(&local_appdata).join("Programs\\Minecraft Launcher"));
    }
    // Xbox app default install root, with Content subdir appended.
    legacy_roots.push(PathBuf::from("C:\\XboxGames\\Minecraft Launcher\\Content"));

    for root in &legacy_roots {
        let root_exists = root.exists();
        log_line(&format!(
            "[win root] {} (exists={})",
            root.display(),
            root_exists
        ));
        if !root_exists {
            continue;
        }
        for exe in EXE_NAMES {
            let p = root.join(exe);
            let exists = p.exists();
            log_line(&format!(
                "[win root candidate] {} (exists={})",
                p.display(),
                exists
            ));
            if exists {
                return Some(p);
            }
        }
    }

    // 2. Registry-installed Mojang launcher (HKLM\SOFTWARE\Mojang\Launcher\InstallPath).
    log_line("[win] trying registry discovery (HKLM\\SOFTWARE\\Mojang\\Launcher)");
    if let Some(p) = discover_via_registry() {
        log_line(&format!("[win registry] found: {}", p.display()));
        return Some(p);
    }
    log_line("[win registry] no result");

    // 3. Microsoft Store (MSIX) version — via Get-AppxPackage.
    log_line("[win] trying AppX discovery (Get-AppxPackage)");
    if let Some(p) = discover_via_appx() {
        log_line(&format!("[win appx] found: {}", p.display()));
        return Some(p);
    }
    log_line("[win appx] no result");

    None
}

#[cfg(target_os = "windows")]
fn discover_via_registry() -> Option<PathBuf> {
    let output = std::process::Command::new("reg")
        .args([
            "query",
            "HKLM\\SOFTWARE\\Mojang\\Launcher",
            "/v",
            "InstallPath",
        ])
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            log_line(&format!("[registry] failed to spawn reg.exe: {e}"));
            return None;
        }
    };
    log_line(&format!(
        "[registry] reg.exe exit_status={:?} stdout_bytes={} stderr_bytes={}",
        output.status,
        output.stdout.len(),
        output.stderr.len()
    ));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log_line(&format!("[registry] reg.exe failed; stderr: {stderr}"));
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    log_line(&format!("[registry] full stdout:\n{stdout}"));
    // Registry output contains a line like:
    //     InstallPath    REG_SZ    C:\Program Files\Minecraft Launcher
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("InstallPath") {
            // Rest looks like: "    REG_SZ    C:\path"
            let after_type = rest.split("REG_SZ").nth(1)?;
            let path_str = after_type.trim();
            log_line(&format!("[registry] parsed InstallPath: {path_str:?}"));
            if path_str.is_empty() {
                continue;
            }
            // Probe both known exe names — the newer Xbox-installed launcher
            // uses Minecraft.exe where the legacy Mojang installer shipped
            // MinecraftLauncher.exe.
            for exe in &["MinecraftLauncher.exe", "Minecraft.exe"] {
                let candidate = PathBuf::from(path_str).join(exe);
                let exists = candidate.exists();
                log_line(&format!(
                    "[registry] candidate exe: {} (exists={})",
                    candidate.display(),
                    exists
                ));
                if exists {
                    return Some(candidate);
                }
            }
            // Also try a `Content` subdirectory (Xbox app layout).
            let content_dir = PathBuf::from(path_str).join("Content");
            if content_dir.exists() {
                for exe in &["MinecraftLauncher.exe", "Minecraft.exe"] {
                    let candidate = content_dir.join(exe);
                    let exists = candidate.exists();
                    log_line(&format!(
                        "[registry] content candidate exe: {} (exists={})",
                        candidate.display(),
                        exists
                    ));
                    if exists {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    log_line("[registry] no InstallPath line matched");
    None
}

#[cfg(target_os = "windows")]
fn discover_via_appx() -> Option<PathBuf> {
    // Get-AppxPackage returns objects with an InstallLocation property when
    // querying Microsoft's "Minecraft Launcher" package. The legacy and the
    // Xbox-installed binary differ in name (MinecraftLauncher.exe vs
    // Minecraft.exe); probe both inside each InstallLocation returned.
    let script = "Get-AppxPackage -Name '*Minecraft*' | \
                  Where-Object { $_.InstallLocation -ne $null } | \
                  ForEach-Object { $_.InstallLocation }";
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            log_line(&format!("[appx] failed to spawn powershell: {e}"));
            return None;
        }
    };
    log_line(&format!(
        "[appx] powershell exit_status={:?} stdout_bytes={} stderr_bytes={}",
        output.status,
        output.stdout.len(),
        output.stderr.len()
    ));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log_line(&format!("[appx] powershell failed; stderr: {stderr}"));
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    log_line(&format!("[appx] full stdout:\n{stdout}"));
    for line in stdout.lines() {
        let install_loc = PathBuf::from(line.trim());
        if install_loc.as_os_str().is_empty() {
            continue;
        }
        let dir_exists = install_loc.exists();
        log_line(&format!(
            "[appx] InstallLocation: {} (exists={})",
            install_loc.display(),
            dir_exists
        ));
        if !dir_exists {
            continue;
        }
        // Probe both exe names directly inside InstallLocation.
        for exe in &["MinecraftLauncher.exe", "Minecraft.exe"] {
            let candidate = install_loc.join(exe);
            let candidate_exists = candidate.exists();
            log_line(&format!(
                "[appx] candidate: {} (exists={})",
                candidate.display(),
                candidate_exists
            ));
            if candidate_exists {
                return Some(candidate);
            }
        }
        // Also probe a `Content` subdirectory (Xbox app layout).
        let content_dir = install_loc.join("Content");
        if content_dir.exists() {
            for exe in &["MinecraftLauncher.exe", "Minecraft.exe"] {
                let candidate = content_dir.join(exe);
                let candidate_exists = candidate.exists();
                log_line(&format!(
                    "[appx] content candidate: {} (exists={})",
                    candidate.display(),
                    candidate_exists
                ));
                if candidate_exists {
                    return Some(candidate);
                }
            }
        }
    }
    log_line("[appx] no candidate matched");
    None
}

#[cfg(target_os = "macos")]
fn discover_macos() -> Option<PathBuf> {
    let p = PathBuf::from("/Applications/Minecraft.app/Contents/MacOS/launcher");
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn discover_linux() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("/usr/bin/minecraft-launcher"),
        PathBuf::from("/opt/minecraft-launcher/minecraft-launcher"),
        PathBuf::from("/snap/minecraft-launcher/current/bin/minecraft-launcher"),
    ];
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".local/bin/minecraft-launcher");
        if p.exists() {
            return Some(p);
        }
    }
    None
}
