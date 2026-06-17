use crate::error::{LauncherError, LauncherResult};
use std::path::PathBuf;

/// Resolve the official Mojang launcher executable path.
///
/// Priority:
/// 1. User override (passed in; in production this comes from `user_settings`).
/// 2. OS-specific discovery.
/// 3. Error `MojangNotFound`.
pub fn resolve_launcher_path(user_override: Option<&str>) -> LauncherResult<PathBuf> {
    if let Some(p) = user_override {
        let path = PathBuf::from(p);
        if path.exists() {
            return Ok(path);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(p) = discover_windows() {
            return Ok(p);
        }
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

    Err(LauncherError::MojangNotFound)
}

#[cfg(target_os = "windows")]
fn discover_windows() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("C:\\Program Files (x86)\\Minecraft Launcher\\MinecraftLauncher.exe"),
        PathBuf::from("C:\\Program Files\\Minecraft Launcher\\MinecraftLauncher.exe"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(target_os = "macos")]
fn discover_macos() -> Option<PathBuf> {
    let p = PathBuf::from("/Applications/Minecraft.app/Contents/MacOS/launcher");
    if p.exists() { Some(p) } else { None }
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
