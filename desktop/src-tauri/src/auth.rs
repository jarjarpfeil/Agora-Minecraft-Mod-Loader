use crate::error::{LauncherError, LauncherResult};

pub use agora_core::auth::{
    get_github_user, poll_device_flow, start_device_flow, DeviceFlowResponse, GithubProfile,
    AGORA_OAUTH_CLIENT_ID,
};

pub fn log_line(line: &str) {
    eprintln!("[auth] {line}");
}

pub fn store_token(_app: &tauri::AppHandle, token: &str) -> LauncherResult<()> {
    agora_core::auth::store_token(token)
}

pub fn get_token<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> Option<String> {
    agora_core::auth::get_token()
}

pub fn clear_token<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> Result<(), String> {
    agora_core::auth::clear_token()
}

pub fn is_authenticated<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> bool {
    agora_core::auth::is_authenticated()
}

/// Fetch the GitHub profile for the stored token. If the token has expired
/// (GitHub returns 401), clear it automatically so the launcher recognises
/// itself as signed out.
pub async fn get_validated_github_profile<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> LauncherResult<GithubProfile> {
    let token = get_token(app).ok_or(LauncherError::AuthRequired)?;
    match get_github_user(token).await {
        Ok(profile) => Ok(profile),
        Err(LauncherError::AuthExpired) => {
            let _ = clear_token(app);
            Err(LauncherError::AuthExpired)
        }
        Err(e) => Err(e),
    }
}
