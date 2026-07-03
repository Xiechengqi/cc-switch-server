use serde::Serialize;

/// Downstream response shape expected by the app route that entered Cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorResponseFormat {
    AnthropicMessages,
    OpenAiResponses,
    OpenAiChatCompletions,
    GeminiGenerateContent,
}
