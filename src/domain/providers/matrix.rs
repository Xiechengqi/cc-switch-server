use serde::Serialize;

use crate::domain::providers::model::{AppKind, ProviderType};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMatrix {
    pub ok: bool,
    pub apps: Vec<&'static str>,
    pub provider_types: Vec<ProviderTypeSummary>,
    pub entries: Vec<ProviderMatrixEntry>,
    pub summary: ProviderMatrixSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTypeSummary {
    pub provider_type: ProviderType,
    pub provider_type_id: &'static str,
    pub label: &'static str,
    pub defaults: ProviderDefaults,
    pub template_env: Vec<&'static str>,
    pub credential_mode: &'static str,
    pub account_supported: bool,
    pub direct_config_supported: bool,
    pub managed_account_recommended: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_url: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website_url: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMatrixEntry {
    pub app: AppKind,
    pub provider_type: ProviderType,
    pub provider_type_id: &'static str,
    pub label: &'static str,
    pub defaults: ProviderDefaults,
    pub template_env: Vec<&'static str>,
    pub ui_visible: bool,
    pub visibility: &'static str,
    pub credential_mode: &'static str,
    pub account_supported: bool,
    pub direct_config_supported: bool,
    pub managed_account_recommended: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_url: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website_url: Option<&'static str>,
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDefaults {
    pub base_url: &'static str,
    pub api_format: &'static str,
    pub model: &'static str,
    pub key: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMatrixSummary {
    pub apps: usize,
    pub provider_types: usize,
    pub entries: usize,
    pub ui_visible_entries: usize,
    pub diagnostic_only_entries: usize,
}

pub fn provider_matrix() -> ProviderMatrix {
    let apps = all_apps();
    let provider_types = all_provider_types();
    let entries: Vec<_> = apps
        .iter()
        .flat_map(|app| {
            provider_types
                .iter()
                .map(move |provider_type| matrix_entry(*app, *provider_type))
        })
        .collect();
    let ui_visible_entries = entries.iter().filter(|entry| entry.ui_visible).count();
    let provider_type_summaries = provider_types
        .iter()
        .map(|provider_type| provider_type_summary(*provider_type))
        .collect();
    ProviderMatrix {
        ok: true,
        apps: apps.iter().map(|app| app.as_str()).collect(),
        provider_types: provider_type_summaries,
        summary: ProviderMatrixSummary {
            apps: apps.len(),
            provider_types: provider_types.len(),
            entries: entries.len(),
            ui_visible_entries,
            diagnostic_only_entries: entries.len() - ui_visible_entries,
        },
        entries,
    }
}

pub fn all_provider_types() -> &'static [ProviderType] {
    &[
        ProviderType::Claude,
        ProviderType::ClaudeAuth,
        ProviderType::ClaudeOAuth,
        ProviderType::Codex,
        ProviderType::CodexOAuth,
        ProviderType::Gemini,
        ProviderType::GeminiCli,
        ProviderType::OpenRouter,
        ProviderType::GitHubCopilot,
        ProviderType::DeepSeekAccount,
        ProviderType::KiroOAuth,
        ProviderType::CursorOAuth,
        ProviderType::CursorApiKey,
        ProviderType::AntigravityOAuth,
        ProviderType::AgyOAuth,
        ProviderType::OllamaCloud,
        ProviderType::AwsBedrock,
        ProviderType::Nvidia,
        ProviderType::DeepSeekApi,
    ]
}

pub fn ui_provider_types(app: AppKind) -> &'static [ProviderType] {
    match app {
        AppKind::Claude => &[
            ProviderType::Claude,
            ProviderType::ClaudeAuth,
            ProviderType::ClaudeOAuth,
            ProviderType::CodexOAuth,
            ProviderType::GeminiCli,
            ProviderType::OpenRouter,
            ProviderType::GitHubCopilot,
            ProviderType::DeepSeekAccount,
            ProviderType::KiroOAuth,
            ProviderType::CursorOAuth,
            ProviderType::CursorApiKey,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
            ProviderType::OllamaCloud,
            ProviderType::AwsBedrock,
            ProviderType::Nvidia,
            ProviderType::DeepSeekApi,
        ],
        AppKind::Codex => &[
            ProviderType::Codex,
            ProviderType::CodexOAuth,
            ProviderType::OpenRouter,
            ProviderType::CursorOAuth,
            ProviderType::CursorApiKey,
            ProviderType::OllamaCloud,
            ProviderType::Nvidia,
            ProviderType::DeepSeekApi,
            ProviderType::Claude,
            ProviderType::ClaudeAuth,
            ProviderType::ClaudeOAuth,
            ProviderType::Gemini,
            ProviderType::GeminiCli,
        ],
        AppKind::Gemini => &[
            ProviderType::Gemini,
            ProviderType::GeminiCli,
            ProviderType::OpenRouter,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
            ProviderType::Claude,
            ProviderType::ClaudeAuth,
            ProviderType::ClaudeOAuth,
            ProviderType::Codex,
            ProviderType::CodexOAuth,
            ProviderType::Nvidia,
            ProviderType::DeepSeekApi,
        ],
    }
}

fn all_apps() -> [AppKind; 3] {
    [AppKind::Claude, AppKind::Codex, AppKind::Gemini]
}

fn provider_type_summary(provider_type: ProviderType) -> ProviderTypeSummary {
    ProviderTypeSummary {
        provider_type,
        provider_type_id: provider_type.as_str(),
        label: provider_label(provider_type),
        defaults: provider_defaults(provider_type),
        template_env: provider_template_env(provider_type).to_vec(),
        credential_mode: credential_mode(provider_type),
        account_supported: account_supported(provider_type),
        direct_config_supported: direct_config_supported(provider_type),
        managed_account_recommended: managed_account_recommended(provider_type),
        api_key_url: provider_api_key_url(provider_type),
        website_url: provider_website_url(provider_type),
    }
}

fn matrix_entry(app: AppKind, provider_type: ProviderType) -> ProviderMatrixEntry {
    let ui_visible = ui_provider_types(app).contains(&provider_type);
    ProviderMatrixEntry {
        app,
        provider_type,
        provider_type_id: provider_type.as_str(),
        label: provider_label(provider_type),
        defaults: provider_defaults(provider_type),
        template_env: provider_template_env(provider_type).to_vec(),
        ui_visible,
        visibility: if ui_visible { "ui" } else { "diagnostic_only" },
        credential_mode: credential_mode(provider_type),
        account_supported: account_supported(provider_type),
        direct_config_supported: direct_config_supported(provider_type),
        managed_account_recommended: managed_account_recommended(provider_type),
        api_key_url: provider_api_key_url(provider_type),
        website_url: provider_website_url(provider_type),
        note: provider_note(app, provider_type, ui_visible),
    }
}

fn provider_api_key_url(provider_type: ProviderType) -> Option<&'static str> {
    match provider_type {
        ProviderType::Claude => Some("https://console.anthropic.com/settings/keys"),
        ProviderType::Codex => Some("https://platform.openai.com/api-keys"),
        ProviderType::Gemini => Some("https://aistudio.google.com/app/apikey"),
        ProviderType::OpenRouter => Some("https://openrouter.ai/keys"),
        ProviderType::CursorApiKey => Some("https://cursor.com/settings"),
        ProviderType::OllamaCloud => Some("https://ollama.com/settings/keys"),
        ProviderType::AwsBedrock => {
            Some("https://console.aws.amazon.com/iam/home#/security_credentials")
        }
        ProviderType::Nvidia => Some("https://build.nvidia.com/settings/api-keys"),
        ProviderType::DeepSeekApi => Some("https://platform.deepseek.com/api_keys"),
        ProviderType::ClaudeAuth
        | ProviderType::ClaudeOAuth
        | ProviderType::CodexOAuth
        | ProviderType::GeminiCli
        | ProviderType::GitHubCopilot
        | ProviderType::DeepSeekAccount
        | ProviderType::KiroOAuth
        | ProviderType::CursorOAuth
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth => None,
    }
}

fn provider_website_url(provider_type: ProviderType) -> Option<&'static str> {
    match provider_type {
        ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth => {
            Some("https://www.anthropic.com")
        }
        ProviderType::Codex | ProviderType::CodexOAuth => Some("https://openai.com"),
        ProviderType::Gemini | ProviderType::GeminiCli => Some("https://ai.google.dev"),
        ProviderType::OpenRouter => Some("https://openrouter.ai"),
        ProviderType::GitHubCopilot => Some("https://github.com/features/copilot"),
        ProviderType::DeepSeekAccount | ProviderType::DeepSeekApi => {
            Some("https://www.deepseek.com")
        }
        ProviderType::KiroOAuth => Some("https://kiro.dev"),
        ProviderType::CursorOAuth | ProviderType::CursorApiKey => Some("https://cursor.com"),
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            Some("https://antigravity.google")
        }
        ProviderType::OllamaCloud => Some("https://ollama.com"),
        ProviderType::AwsBedrock => Some("https://aws.amazon.com/bedrock"),
        ProviderType::Nvidia => Some("https://build.nvidia.com"),
    }
}

fn provider_label(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::Claude => "Claude API",
        ProviderType::ClaudeAuth => "Claude bearer relay",
        ProviderType::ClaudeOAuth => "Claude OAuth",
        ProviderType::Codex => "OpenAI/Codex",
        ProviderType::CodexOAuth => "OpenAI OAuth",
        ProviderType::Gemini => "Gemini API",
        ProviderType::GeminiCli => "Gemini OAuth/CLI",
        ProviderType::OpenRouter => "OpenRouter",
        ProviderType::GitHubCopilot => "GitHub Copilot",
        ProviderType::DeepSeekAccount => "DeepSeek Account",
        ProviderType::KiroOAuth => "Kiro OAuth",
        ProviderType::CursorOAuth => "Cursor OAuth",
        ProviderType::CursorApiKey => "Cursor API Key",
        ProviderType::AntigravityOAuth => "Antigravity OAuth",
        ProviderType::AgyOAuth => "Antigravity CLI",
        ProviderType::OllamaCloud => "Ollama Cloud",
        ProviderType::AwsBedrock => "AWS Bedrock",
        ProviderType::Nvidia => "Nvidia",
        ProviderType::DeepSeekApi => "DeepSeek API Key",
    }
}

fn provider_defaults(provider_type: ProviderType) -> ProviderDefaults {
    match provider_type {
        ProviderType::Claude => ProviderDefaults {
            base_url: "https://api.anthropic.com",
            api_format: "anthropic",
            model: "claude-sonnet-4-6",
            key: "ANTHROPIC_API_KEY",
            aws_region: None,
        },
        ProviderType::ClaudeAuth => ProviderDefaults {
            base_url: "https://api.anthropic.com",
            api_format: "anthropic",
            model: "claude-sonnet-4-6",
            key: "ANTHROPIC_AUTH_TOKEN",
            aws_region: None,
        },
        ProviderType::ClaudeOAuth => ProviderDefaults {
            base_url: "https://api.anthropic.com",
            api_format: "anthropic",
            model: "claude-sonnet-4-6",
            key: "ANTHROPIC_AUTH_TOKEN",
            aws_region: None,
        },
        ProviderType::Codex => ProviderDefaults {
            base_url: "https://api.openai.com/v1",
            api_format: "openai_responses",
            model: "gpt-5.5",
            key: "OPENAI_API_KEY",
            aws_region: None,
        },
        ProviderType::CodexOAuth => ProviderDefaults {
            base_url: "https://chatgpt.com/backend-api/codex",
            api_format: "openai_responses",
            model: "gpt-5.5",
            key: "OPENAI_API_KEY",
            aws_region: None,
        },
        ProviderType::Gemini => ProviderDefaults {
            base_url: "https://generativelanguage.googleapis.com",
            api_format: "gemini_native",
            model: "gemini-3.5-flash",
            key: "GEMINI_API_KEY",
            aws_region: None,
        },
        ProviderType::GeminiCli => ProviderDefaults {
            base_url: "https://generativelanguage.googleapis.com",
            api_format: "gemini_native",
            model: "gemini-3.5-flash",
            key: "GEMINI_API_KEY",
            aws_region: None,
        },
        ProviderType::OpenRouter => ProviderDefaults {
            base_url: "https://openrouter.ai/api",
            api_format: "openai_chat",
            model: "openrouter/auto",
            key: "API_KEY",
            aws_region: None,
        },
        ProviderType::GitHubCopilot => ProviderDefaults {
            base_url: "https://api.githubcopilot.com",
            api_format: "openai_chat",
            model: "gpt-5.5",
            key: "ANTHROPIC_AUTH_TOKEN",
            aws_region: None,
        },
        ProviderType::DeepSeekAccount => ProviderDefaults {
            base_url: "https://chat.deepseek.com",
            api_format: "anthropic",
            model: "deepseek-v4-pro",
            key: "ANTHROPIC_AUTH_TOKEN",
            aws_region: None,
        },
        ProviderType::KiroOAuth => ProviderDefaults {
            base_url: "https://q.us-east-1.amazonaws.com",
            api_format: "anthropic",
            model: "claude-sonnet-4-8",
            key: "ANTHROPIC_AUTH_TOKEN",
            aws_region: None,
        },
        ProviderType::CursorOAuth => ProviderDefaults {
            base_url: "https://api2.cursor.sh",
            api_format: "openai_chat",
            model: "composer-2.5",
            key: "ANTHROPIC_AUTH_TOKEN",
            aws_region: None,
        },
        ProviderType::CursorApiKey => ProviderDefaults {
            base_url: "https://api.cursor.com",
            api_format: "openai_chat",
            model: "composer-2.5",
            key: "ANTHROPIC_AUTH_TOKEN",
            aws_region: None,
        },
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => ProviderDefaults {
            base_url: "https://daily-cloudcode-pa.googleapis.com",
            api_format: "gemini_native",
            model: "gemini-3.5-flash-medium",
            key: "GEMINI_API_KEY",
            aws_region: None,
        },
        ProviderType::OllamaCloud => ProviderDefaults {
            base_url: "https://ollama.com",
            api_format: "openai_chat",
            model: "gpt-oss:20b",
            key: "OPENAI_API_KEY",
            aws_region: None,
        },
        ProviderType::AwsBedrock => ProviderDefaults {
            base_url: "https://bedrock-runtime.${AWS_REGION}.amazonaws.com",
            api_format: "anthropic",
            model: "global.anthropic.claude-opus-4-8",
            key: "AWS_ACCESS_KEY_ID",
            aws_region: Some("us-west-2"),
        },
        ProviderType::Nvidia => ProviderDefaults {
            base_url: "https://integrate.api.nvidia.com/v1",
            api_format: "openai_chat",
            model: "moonshotai/kimi-k2.5",
            key: "OPENAI_API_KEY",
            aws_region: None,
        },
        ProviderType::DeepSeekApi => ProviderDefaults {
            base_url: "https://api.deepseek.com",
            api_format: "openai_chat",
            model: "deepseek-v4-flash",
            key: "OPENAI_API_KEY",
            aws_region: None,
        },
    }
}

fn provider_template_env(provider_type: ProviderType) -> &'static [&'static str] {
    match provider_type {
        ProviderType::Claude => &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
        ],
        ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth => &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
        ],
        ProviderType::Codex | ProviderType::CodexOAuth => &["OPENAI_BASE_URL", "OPENAI_API_KEY"],
        ProviderType::Gemini | ProviderType::GeminiCli => {
            &["GOOGLE_GEMINI_BASE_URL", "GEMINI_API_KEY", "GEMINI_MODEL"]
        }
        ProviderType::OpenRouter => &["OPENAI_BASE_URL", "OPENAI_API_KEY"],
        ProviderType::GitHubCopilot | ProviderType::DeepSeekAccount | ProviderType::KiroOAuth => {
            &["ANTHROPIC_AUTH_TOKEN"]
        }
        ProviderType::CursorOAuth | ProviderType::CursorApiKey => {
            &["OPENAI_BASE_URL", "ANTHROPIC_AUTH_TOKEN"]
        }
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            &["GOOGLE_GEMINI_BASE_URL", "GEMINI_API_KEY", "GEMINI_MODEL"]
        }
        ProviderType::OllamaCloud => &["OPENAI_BASE_URL", "OPENAI_API_KEY"],
        ProviderType::AwsBedrock => &[
            "AWS_REGION",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
        ],
        ProviderType::Nvidia | ProviderType::DeepSeekApi => &["OPENAI_BASE_URL", "OPENAI_API_KEY"],
    }
}

fn credential_mode(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::Claude
        | ProviderType::Codex
        | ProviderType::Gemini
        | ProviderType::OpenRouter
        | ProviderType::CursorApiKey
        | ProviderType::OllamaCloud
        | ProviderType::Nvidia
        | ProviderType::DeepSeekApi => "api_key",
        ProviderType::ClaudeAuth => "bearer_token",
        ProviderType::AwsBedrock => "aws_credentials",
        ProviderType::ClaudeOAuth
        | ProviderType::CodexOAuth
        | ProviderType::GeminiCli
        | ProviderType::GitHubCopilot
        | ProviderType::DeepSeekAccount
        | ProviderType::KiroOAuth
        | ProviderType::CursorOAuth
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth => "oauth_or_manual_token",
    }
}

fn account_supported(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::ClaudeOAuth
            | ProviderType::CodexOAuth
            | ProviderType::GeminiCli
            | ProviderType::GitHubCopilot
            | ProviderType::DeepSeekAccount
            | ProviderType::KiroOAuth
            | ProviderType::CursorOAuth
            | ProviderType::CursorApiKey
            | ProviderType::AntigravityOAuth
            | ProviderType::AgyOAuth
            | ProviderType::OllamaCloud
            | ProviderType::AwsBedrock
            | ProviderType::Nvidia
            | ProviderType::DeepSeekApi
    )
}

fn direct_config_supported(_provider_type: ProviderType) -> bool {
    true
}

fn managed_account_recommended(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::ClaudeOAuth
            | ProviderType::CodexOAuth
            | ProviderType::GeminiCli
            | ProviderType::GitHubCopilot
            | ProviderType::DeepSeekAccount
            | ProviderType::KiroOAuth
            | ProviderType::CursorOAuth
            | ProviderType::AntigravityOAuth
            | ProviderType::AgyOAuth
    )
}

fn provider_note(app: AppKind, provider_type: ProviderType, ui_visible: bool) -> &'static str {
    if !ui_visible {
        return "diagnostic capability only; not offered by the Web provider form";
    }
    match (app, provider_type) {
        (_, ProviderType::AwsBedrock) => {
            "SigV4 converse request generation is wired; real AWS Bedrock forwarding remains unvalidated"
        }
        (_, ProviderType::GitHubCopilot) => {
            "managed-account token exchange and endpoint discovery are wired; capability remains fallback until real Copilot non-stream/stream validation"
        }
        (AppKind::Claude, ProviderType::KiroOAuth) => {
            "managed-account CodeWhisperer forwarder is wired for Claude; capability remains planned until real Kiro account validation"
        }
        (_, ProviderType::DeepSeekAccount) => {
            "Claude forwarder protocol bridge is wired; real upstream validation remains pending"
        }
        (AppKind::Codex | AppKind::Gemini, ProviderType::KiroOAuth) => {
            "diagnostic capability only; Kiro forwarding is Claude-only on server"
        }
        (_, ProviderType::CursorOAuth | ProviderType::CursorApiKey) => {
            "Cursor AgentService h2/protobuf static driver is available behind explicit opt-in; capability remains planned until real Cursor non-stream/stream validation"
        }
        (
            _,
            ProviderType::ClaudeOAuth
            | ProviderType::CodexOAuth
            | ProviderType::GeminiCli
            | ProviderType::AntigravityOAuth
            | ProviderType::AgyOAuth,
        ) => {
            "static adapter contract is available; import refresh credentials to enable native refresh/profile; browser login remains disabled"
        }
        _ => "",
    }
}
