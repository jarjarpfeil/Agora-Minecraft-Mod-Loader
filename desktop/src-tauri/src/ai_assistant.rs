use crate::error::LauncherResult;

pub use crate::mcp::MCP_SKILL_CONTENT;
pub use agora_core::ai_assistant::{
    build_context_message, build_system_prompt as core_build_system_prompt,
    chat_completion as core_chat_completion, clear_copilot_token, load_copilot_token,
    poll_copilot_flow, resolve_copilot_endpoint, start_copilot_flow, store_copilot_token,
    AiContext, ChatMessage, ChatResponse, CopilotDeviceFlowResponse, CopilotToken,
};

pub fn build_system_prompt() -> String {
    agora_core::ai_assistant::build_system_prompt(MCP_SKILL_CONTENT)
}

pub async fn chat_completion(
    messages: Vec<ChatMessage>,
    token: &CopilotToken,
) -> LauncherResult<ChatResponse> {
    agora_core::ai_assistant::chat_completion(messages, token).await
}

pub fn build_context_message_with_app(app: &tauri::AppHandle, context: &AiContext) -> String {
    let manifest_path = context
        .instance_id
        .as_ref()
        .and_then(|id| crate::paths::instance_manifest_path(app, id).ok());
    agora_core::ai_assistant::build_context_message_with_app(manifest_path, context)
}
