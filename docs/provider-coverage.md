# Provider Coverage

Provider types from: `/data/projects/cc-switch`
Presets from: `/data/projects/cc-switch`

Note: server compatibility provider types are explicit cc-switch-server classifications for cc-switch presets that do not carry an upstream `providerType`.

## Provider Types

| ProviderType | Apps | Required | Present in source |
| --- | --- | --- | --- |
| `claude` | claude | yes | yes |
| `claude_auth` | claude | yes | yes |
| `claude_oauth` | claude | yes | yes |
| `codex` | codex | yes | yes |
| `codex_oauth` | claude, codex | yes | yes |
| `gemini` | gemini | yes | yes |
| `gemini_cli` | gemini, claude | yes | yes |
| `openrouter` | claude, codex, gemini | yes | yes |
| `github_copilot` | claude | yes | yes |
| `deepseek_account` | claude | yes | yes |
| `kiro_oauth` | claude | yes | yes |
| `cursor_oauth` | claude, codex | yes | yes |
| `cursor_apikey` | claude, codex | yes | yes |
| `antigravity_oauth` | claude, gemini | yes | yes |
| `agy_oauth` | claude, gemini | yes | yes |
| `ollama_cloud` | claude, codex | yes | yes |
| `aws_bedrock` | claude | no | NO |
| `nvidia` | claude, codex | no | NO |
| `deepseek_api` | claude, codex | no | NO |

## claude presets

| Name | providerType |
| --- | --- |
| Claude Official | `claude_oauth` |
| OpenAI OAuth | `codex_oauth` |
| Kiro OAuth | `kiro_oauth` |
| Ollama API Key | `ollama_cloud` |
| Cursor OAuth | `cursor_oauth` |
| Cursor API Key | `cursor_apikey` |
| Antigravity OAuth | `antigravity_oauth` |
| Antigravity CLI (agy) | `agy_oauth` |
| GitHub Copilot | `github_copilot` |
| DeepSeek Official | `deepseek_account` |
| AWS Bedrock (AKSK) |  |
| AWS Bedrock (API Key) |  |
| OpenRouter |  |
| Nvidia |  |
| DeepSeek(API Key) |  |

## codex presets

| Name | providerType |
| --- | --- |
| OpenAI OAuth | `codex_oauth` |
| Cursor API Key | `cursor_apikey` |
| Cursor OAuth | `cursor_oauth` |
| Ollama API Key | `ollama_cloud` |
| OpenRouter |  |
| Nvidia |  |
| DeepSeek(API Key) |  |

## gemini presets

| Name | providerType |
| --- | --- |
| Google Official | `google_gemini_oauth` |
| Antigravity OAuth | `antigravity_oauth` |
| Antigravity CLI (agy) | `agy_oauth` |
| OpenRouter |  |

## universal presets

| Name | providerType |
| --- | --- |
| NewAPI | `newapi` |
| 自定义网关 | `custom_gateway` |

## Server parity notes

### `claude_oauth` (Claude Official)

Server-native parity with desktop `cc-switch` forwarder and `claude_oauth_auth` (2026-07-07):

- Proxy hot path: `?beta=true`, `anthropic-beta` assembly (`claude-code-20250219`, `oauth-2025-04-20`, `interleaved-thinking-2025-05-14`), billing `system` injection, `cch=` body signing (`src/proxy/claude_oauth.rs`).
- OAuth web-paste: `code#state` parsing, platform token endpoint first, platform User-Agent (`axios/1.13.6`).
- Local callback uses `/api/accounts/login/callback` instead of desktop `localhost:54545` (deployment difference, not a capability gap).
