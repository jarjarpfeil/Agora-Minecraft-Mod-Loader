use crate::error::LauncherResult;

pub use agora_core::auth::{
    log_line, start_device_flow, poll_device_flow, get_github_user,
    AGORA_OAUTH_CLIENT_ID, DeviceFlowResponse, GithubProfile,
};

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
