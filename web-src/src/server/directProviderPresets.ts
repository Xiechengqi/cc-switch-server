import type { ProviderCategory, ProviderMeta } from "@/types";

export interface ServerProviderPreset {
  name: string;
  websiteUrl: string;
  notes: string;
  settingsConfig: Record<string, unknown>;
  category?: ProviderCategory;
  apiKeyField?: ProviderMeta["apiKeyField"];
  icon?: string;
  iconColor?: string;
}

/**
 * Server-owned creation defaults keyed by the authoritative Provider Registry
 * profileId. Values contain no credentials and are reviewed with the Registry
 * contract; they are not imported from a desktop preset catalog.
 */
export const serverProviderPresets: Readonly<
  Record<string, ServerProviderPreset>
> = {
  "claude.official_oauth": {
    "name": "Claude Official",
    "websiteUrl": "https://www.anthropic.com/claude-code",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "anthropic",
    "iconColor": "#D4915D"
  },
  "claude.anthropic_api_key": {
    "name": "Anthropic API Key",
    "websiteUrl": "https://console.anthropic.com/",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://api.anthropic.com"
      },
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "apiKeyField": "ANTHROPIC_API_KEY",
    "icon": "anthropic",
    "iconColor": "#D4915D"
  },
  "claude.openai_oauth": {
    "name": "OpenAI OAuth",
    "websiteUrl": "https://chatgpt.com/codex",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://chatgpt.com/backend-api/codex",
        "ANTHROPIC_MODEL": "gpt-5.6-sol",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "gpt-5.4",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "gpt-5.6-sol",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "gpt-5.6-sol"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gpt-5.6-sol"
      }
    },
    "category": "official",
    "icon": "openai",
    "iconColor": "#00A67E"
  },
  "claude.google_oauth": {
    "name": "Google Gemini OAuth",
    "websiteUrl": "https://codeassist.google/",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://cloudcode-pa.googleapis.com",
        "ANTHROPIC_MODEL": "gemini-3.1-pro-preview"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gemini-3.1-pro-preview"
      }
    },
    "category": "official",
    "icon": "gemini",
    "iconColor": "#4285F4"
  },
  "claude.grok_oauth": {
    "name": "Grok OAuth",
    "websiteUrl": "https://x.ai",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://api.x.ai/v1",
        "ANTHROPIC_MODEL": "grok-4.6",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "grok-4.6",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "grok-4.6",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "grok-4.6"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "grok-4.6"
      }
    },
    "category": "official",
    "icon": "grok",
    "iconColor": "#111827"
  },
  "claude.kimi_code": {
    "name": "Kimi OAuth",
    "websiteUrl": "https://kimi.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://api.kimi.com/coding/v1",
        "ANTHROPIC_MODEL": "kimi-for-coding"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "kimi-for-coding"
      }
    },
    "category": "official",
    "icon": "kimi",
    "iconColor": "#111827"
  },
  "claude.qoder_cosy": {
    "name": "Qoder OAuth",
    "websiteUrl": "https://qoder.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_MODEL": "auto"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "auto"
      }
    },
    "category": "official",
    "icon": "qoder"
  },
  "claude.kiro_oauth": {
    "name": "Kiro OAuth",
    "websiteUrl": "https://kiro.dev",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://q.us-east-1.amazonaws.com",
        "ANTHROPIC_MODEL": "claude-sonnet-4-8",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku-4-5",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "claude-sonnet-4-8",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-8",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "claude-opus-4-8",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-4-8"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "claude-sonnet-4-8"
      }
    },
    "category": "official",
    "icon": "kiro"
  },
  "claude.amazon_q_oauth": {
    "name": "Amazon Q Developer",
    "websiteUrl": "https://aws.amazon.com/q/developer/",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_MODEL": "auto"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "auto"
      }
    },
    "category": "official",
    "icon": "aws",
    "iconColor": "#FF9900"
  },
  "claude.grok_web_session": {
    "name": "Grok Web Session",
    "websiteUrl": "https://grok.com/",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_MODEL": "fast"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "fast"
      }
    },
    "category": "official",
    "icon": "grok",
    "iconColor": "#111827"
  },
  "claude.perplexity_web_session": {
    "name": "Perplexity Web Session",
    "websiteUrl": "https://www.perplexity.ai/",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_MODEL": "pplx-auto"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "pplx-auto"
      }
    },
    "category": "official",
    "icon": "perplexity",
    "iconColor": "#20808D"
  },
  "claude.ollama_cloud": {
    "name": "Ollama API Key",
    "websiteUrl": "https://ollama.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://ollama.com",
        "ANTHROPIC_MODEL": "kimi-k2.7-code",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "kimi-k2.7-code",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "kimi-k2.7-code",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "kimi-k2.7-code",
        "ANTHROPIC_DEFAULT_FABLE_MODEL": "kimi-k2.7-code"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "kimi-k2.7-code"
      }
    },
    "category": "third_party",
    "icon": "ollama"
  },
  "claude.cursor_oauth": {
    "name": "Cursor OAuth",
    "websiteUrl": "https://cursor.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://api2.cursor.sh",
        "ANTHROPIC_MODEL": "default",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "default",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "Claude Haiku",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "default",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Claude Sonnet",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "default",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "Claude Opus",
        "ANTHROPIC_DEFAULT_FABLE_MODEL": "default",
        "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME": "Claude Fable"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "default"
      }
    },
    "category": "official",
    "icon": "cursor"
  },
  "claude.cursor_api_key": {
    "name": "Cursor API Key",
    "websiteUrl": "https://cursor.com/dashboard/cloud-agents",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://api.cursor.com",
        "ANTHROPIC_MODEL": "default",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "default",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "Claude Haiku",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "default",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Claude Sonnet",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "default",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "Claude Opus",
        "ANTHROPIC_DEFAULT_FABLE_MODEL": "default",
        "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME": "Claude Fable"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "default"
      }
    },
    "category": "official",
    "icon": "cursor"
  },
  "claude.antigravity_oauth": {
    "name": "Antigravity OAuth",
    "websiteUrl": "https://antigravity.google",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://daily-cloudcode-pa.googleapis.com",
        "ANTHROPIC_MODEL": "claude-sonnet-4-6",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "gemini-3.5-flash-low",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "Gemini 3.5 Flash (Low)",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-6",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Claude Sonnet 4.6",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-4-6-thinking",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "Claude Opus 4.6 (Thinking)"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "claude-sonnet-4-6"
      }
    },
    "category": "official",
    "icon": "gemini",
    "iconColor": "#1A73E8"
  },
  "claude.antigravity_cli": {
    "name": "Antigravity CLI (agy)",
    "websiteUrl": "https://antigravity.google",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://daily-cloudcode-pa.googleapis.com",
        "ANTHROPIC_MODEL": "claude-sonnet-4-6",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "gemini-3.5-flash-low",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "Gemini 3.5 Flash (Low)",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-6",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Claude Sonnet 4.6",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-4-6-thinking",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "Claude Opus 4.6 (Thinking)"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "claude-sonnet-4-6"
      }
    },
    "category": "official",
    "icon": "gemini",
    "iconColor": "#111827"
  },
  "claude.github_copilot": {
    "name": "GitHub Copilot",
    "websiteUrl": "https://github.com/features/copilot",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://api.githubcopilot.com",
        "ANTHROPIC_MODEL": "claude-sonnet-5",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku-4.5",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-5",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-sonnet-5"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "claude-sonnet-5"
      }
    },
    "category": "third_party",
    "icon": "github",
    "iconColor": "#000000"
  },
  "claude.deepseek_account": {
    "name": "DeepSeek Official",
    "websiteUrl": "https://chat.deepseek.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://chat.deepseek.com",
        "ANTHROPIC_MODEL": "deepseek-v4-flash",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "deepseek-v4-flash",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "deepseek-v4-flash",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-v4-pro"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "deepseek-v4-flash"
      }
    },
    "category": "cn_official",
    "icon": "deepseek",
    "iconColor": "#1E88E5"
  },
  "claude.aws_bedrock_aksk": {
    "name": "AWS Bedrock (AKSK)",
    "websiteUrl": "https://aws.amazon.com/bedrock/",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://bedrock-runtime.us-east-1.amazonaws.com",
        "AWS_REGION": "us-east-1",
        "ANTHROPIC_MODEL": "global.anthropic.claude-opus-4-8",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "global.anthropic.claude-sonnet-5",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "global.anthropic.claude-opus-4-8",
        "CLAUDE_CODE_USE_BEDROCK": "1"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "global.anthropic.claude-opus-4-8"
      }
    },
    "category": "cloud_provider",
    "icon": "aws",
    "iconColor": "#FF9900"
  },
  "claude.aws_bedrock_api_key": {
    "name": "AWS Bedrock (API Key)",
    "websiteUrl": "https://aws.amazon.com/bedrock/",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://bedrock-runtime.us-east-1.amazonaws.com",
        "AWS_REGION": "us-east-1",
        "ANTHROPIC_MODEL": "global.anthropic.claude-opus-4-8",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "global.anthropic.claude-sonnet-5",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "global.anthropic.claude-opus-4-8",
        "CLAUDE_CODE_USE_BEDROCK": "1"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "global.anthropic.claude-opus-4-8"
      }
    },
    "category": "cloud_provider",
    "icon": "aws",
    "iconColor": "#FF9900"
  },
  "claude.openrouter": {
    "name": "OpenRouter",
    "websiteUrl": "https://openrouter.ai",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://openrouter.ai/api",
        "ANTHROPIC_MODEL": "anthropic/claude-sonnet-4.6",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "anthropic/claude-haiku-4.5",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "anthropic/claude-sonnet-4.6",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "anthropic/claude-opus-4.7"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "anthropic/claude-sonnet-4.6"
      }
    },
    "category": "aggregator",
    "icon": "openrouter",
    "iconColor": "#6566F1"
  },
  "claude.nvidia": {
    "name": "Nvidia",
    "websiteUrl": "https://build.nvidia.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://integrate.api.nvidia.com",
        "ANTHROPIC_MODEL": "moonshotai/kimi-k2.5",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "moonshotai/kimi-k2.5",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "moonshotai/kimi-k2.5",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "moonshotai/kimi-k2.5"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "moonshotai/kimi-k2.5"
      }
    },
    "category": "aggregator",
    "icon": "nvidia",
    "iconColor": "#000000"
  },
  "claude.deepseek_api": {
    "name": "DeepSeek(API Key)",
    "websiteUrl": "https://platform.deepseek.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
        "ANTHROPIC_MODEL": "deepseek-v4-flash",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": "deepseek-v4-flash",
        "ANTHROPIC_DEFAULT_SONNET_MODEL": "deepseek-v4-flash",
        "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-v4-pro"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "deepseek-v4-flash"
      }
    },
    "category": "cn_official",
    "icon": "deepseek",
    "iconColor": "#1E88E5"
  },
  "codex.openai_oauth": {
    "name": "OpenAI OAuth",
    "websiteUrl": "https://chatgpt.com/codex",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model = \"gpt-5.6-sol\"",
      "modelMapping": {
        "mode": "passthrough"
      },
      "env": {}
    },
    "category": "official",
    "icon": "openai",
    "iconColor": "#00A67E"
  },
  "codex.openai_api_key": {
    "name": "OpenAI API Key",
    "websiteUrl": "https://platform.openai.com/",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model = \"gpt-5.4\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true",
      "modelMapping": {
        "mode": "passthrough"
      },
      "env": {}
    },
    "category": "official",
    "icon": "openai",
    "iconColor": "#00A67E"
  },
  "codex.github_copilot": {
    "name": "GitHub Copilot",
    "websiteUrl": "https://github.com/features/copilot",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"github_copilot\"\nmodel = \"gpt-5.5\"\ndisable_response_storage = true\n\n[model_providers.github_copilot]\nname = \"GitHub Copilot\"\nbase_url = \"https://api.githubcopilot.com\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gpt-5.5"
      },
      "env": {
        "OPENAI_MODEL": "gpt-5.5"
      }
    },
    "category": "third_party",
    "icon": "github",
    "iconColor": "#000000"
  },
  "codex.grok_oauth": {
    "name": "Grok OAuth",
    "websiteUrl": "https://x.ai",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"custom\"\nmodel = \"grok-4.6\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"Grok\"\nbase_url = \"https://api.x.ai/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelCatalog": {
        "models": [
          {
            "model": "grok-4.6"
          },
          {
            "model": "grok-4.5"
          },
          {
            "model": "grok-4.3"
          },
          {
            "model": "grok-4.20-multi-agent"
          },
          {
            "model": "grok-3-mini"
          }
        ]
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "grok-4.6"
      },
      "env": {
        "OPENAI_MODEL": "grok-4.6"
      }
    },
    "category": "official",
    "icon": "grok",
    "iconColor": "#111827"
  },
  "codex.kimi_code": {
    "name": "Kimi OAuth",
    "websiteUrl": "https://kimi.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"kimi_code\"\nmodel = \"kimi-for-coding\"\ndisable_response_storage = true\n\n[model_providers.kimi_code]\nname = \"Kimi Code\"\nbase_url = \"https://api.kimi.com/coding/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "kimi-for-coding"
      },
      "env": {
        "OPENAI_MODEL": "kimi-for-coding"
      }
    },
    "category": "official",
    "icon": "kimi",
    "iconColor": "#111827"
  },
  "codex.qoder_cosy": {
    "name": "Qoder OAuth",
    "websiteUrl": "https://qoder.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "auto"
      },
      "env": {
        "OPENAI_MODEL": "auto"
      }
    },
    "category": "official",
    "icon": "qoder"
  },
  "codex.kiro_oauth": {
    "name": "Kiro OAuth",
    "websiteUrl": "https://kiro.dev",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"kiro\"\nmodel = \"claude-sonnet-4-8\"\ndisable_response_storage = true\n\n[model_providers.kiro]\nname = \"Kiro\"\nbase_url = \"https://q.us-east-1.amazonaws.com\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "claude-sonnet-4-8"
      },
      "env": {
        "OPENAI_MODEL": "claude-sonnet-4-8"
      }
    },
    "category": "official",
    "icon": "kiro"
  },
  "codex.amazon_q_oauth": {
    "name": "Amazon Q Developer",
    "websiteUrl": "https://aws.amazon.com/q/developer/",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {
        "OPENAI_MODEL": "auto"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "auto"
      }
    },
    "category": "official",
    "icon": "aws",
    "iconColor": "#FF9900"
  },
  "codex.grok_web_session": {
    "name": "Grok Web Session",
    "websiteUrl": "https://grok.com/",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {
        "OPENAI_MODEL": "fast"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "fast"
      }
    },
    "category": "official",
    "icon": "grok",
    "iconColor": "#111827"
  },
  "codex.perplexity_web_session": {
    "name": "Perplexity Web Session",
    "websiteUrl": "https://www.perplexity.ai/",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {
        "OPENAI_MODEL": "pplx-auto"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "pplx-auto"
      }
    },
    "category": "official",
    "icon": "perplexity",
    "iconColor": "#20808D"
  },
  "codex.cursor_api_key": {
    "name": "Cursor API Key",
    "websiteUrl": "https://cursor.com/dashboard/cloud-agents",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"custom\"\nmodel = \"gpt-5.5\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"cursor\"\nbase_url = \"https://api.cursor.com\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelCatalog": {
        "models": [
          {
            "model": "gpt-5.5",
            "upstreamModel": "default",
            "displayName": "GPT-5.5",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-low",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 Low",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-medium",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 Medium",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-high",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 High",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-xhigh",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 XHigh",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-minimal",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 Minimal",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.4",
            "upstreamModel": "default",
            "displayName": "GPT-5.4",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.4-mini",
            "upstreamModel": "default",
            "displayName": "GPT-5.4 Mini",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.4-nano",
            "upstreamModel": "default",
            "displayName": "GPT-5.4 Nano",
            "contextWindow": 128000
          }
        ]
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "default"
      },
      "env": {
        "OPENAI_MODEL": "default"
      }
    },
    "category": "official",
    "icon": "cursor"
  },
  "codex.cursor_oauth": {
    "name": "Cursor OAuth",
    "websiteUrl": "https://cursor.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"custom\"\nmodel = \"gpt-5.5\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"cursor\"\nbase_url = \"https://api2.cursor.sh\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelCatalog": {
        "models": [
          {
            "model": "gpt-5.5",
            "upstreamModel": "default",
            "displayName": "GPT-5.5",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-low",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 Low",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-medium",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 Medium",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-high",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 High",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-xhigh",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 XHigh",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.5-minimal",
            "upstreamModel": "default",
            "displayName": "GPT-5.5 Minimal",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.4",
            "upstreamModel": "default",
            "displayName": "GPT-5.4",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.4-mini",
            "upstreamModel": "default",
            "displayName": "GPT-5.4 Mini",
            "contextWindow": 128000
          },
          {
            "model": "gpt-5.4-nano",
            "upstreamModel": "default",
            "displayName": "GPT-5.4 Nano",
            "contextWindow": 128000
          }
        ]
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "default"
      },
      "env": {
        "OPENAI_MODEL": "default"
      }
    },
    "category": "official",
    "icon": "cursor"
  },
  "codex.ollama_cloud": {
    "name": "Ollama API Key",
    "websiteUrl": "https://ollama.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"custom\"\nmodel = \"kimi-k2.7-code\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"ollama\"\nbase_url = \"https://ollama.com\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelCatalog": {
        "models": [
          {
            "model": "kimi-k2.7-code",
            "displayName": "Kimi K2.7 Code",
            "contextWindow": 262144
          }
        ]
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "kimi-k2.7-code"
      },
      "env": {
        "OPENAI_MODEL": "kimi-k2.7-code"
      }
    },
    "category": "third_party",
    "icon": "ollama"
  },
  "codex.openrouter": {
    "name": "OpenRouter",
    "websiteUrl": "https://openrouter.ai",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"custom\"\nmodel = \"gpt-5.4\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"openrouter\"\nbase_url = \"https://openrouter.ai/api/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gpt-5.4"
      },
      "env": {
        "OPENAI_MODEL": "gpt-5.4"
      }
    },
    "category": "aggregator",
    "icon": "openrouter",
    "iconColor": "#6566F1"
  },
  "codex.nvidia": {
    "name": "Nvidia",
    "websiteUrl": "https://build.nvidia.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"custom\"\nmodel = \"moonshotai/kimi-k2.5\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"nvidia\"\nbase_url = \"https://integrate.api.nvidia.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelCatalog": {
        "models": [
          {
            "model": "moonshotai/kimi-k2.5",
            "displayName": "Kimi K2.5",
            "contextWindow": 262144
          }
        ]
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "moonshotai/kimi-k2.5"
      },
      "env": {
        "OPENAI_MODEL": "moonshotai/kimi-k2.5"
      }
    },
    "category": "aggregator",
    "icon": "nvidia",
    "iconColor": "#000000"
  },
  "codex.deepseek_api": {
    "name": "DeepSeek(API Key)",
    "websiteUrl": "https://platform.deepseek.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "model_provider = \"custom\"\nmodel = \"deepseek-v4-flash\"\nmodel_reasoning_effort = \"high\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"deepseek\"\nbase_url = \"https://api.deepseek.com\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
      "modelCatalog": {
        "models": [
          {
            "model": "deepseek-v4-flash",
            "displayName": "DeepSeek V4 Flash",
            "contextWindow": 1000000
          },
          {
            "model": "deepseek-v4-pro",
            "displayName": "DeepSeek V4 Pro",
            "contextWindow": 1000000
          }
        ]
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "deepseek-v4-flash"
      },
      "env": {
        "OPENAI_MODEL": "deepseek-v4-flash"
      }
    },
    "category": "cn_official",
    "icon": "deepseek",
    "iconColor": "#1E88E5"
  },
  "gemini.google_oauth": {
    "name": "Google Official",
    "websiteUrl": "https://ai.google.dev/",
    "notes": "Google 官方 Gemini API (OAuth)",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "gemini",
    "iconColor": "#4285F4"
  },
  "gemini.google_api_key": {
    "name": "Google Gemini API Key",
    "websiteUrl": "https://ai.google.dev/",
    "notes": "Google Gemini API Key",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "gemini",
    "iconColor": "#4285F4"
  },
  "gemini.antigravity_oauth": {
    "name": "Antigravity OAuth",
    "websiteUrl": "https://antigravity.google",
    "notes": "Antigravity OAuth",
    "settingsConfig": {
      "env": {
        "GOOGLE_GEMINI_BASE_URL": "https://daily-cloudcode-pa.googleapis.com",
        "GEMINI_MODEL": "gemini-3.5-flash-medium"
      },
      "config": {
        "general": {
          "previewFeatures": true
        }
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gemini-3.5-flash-medium"
      }
    },
    "category": "official",
    "icon": "gemini",
    "iconColor": "#1A73E8"
  },
  "gemini.antigravity_cli": {
    "name": "Antigravity CLI (agy)",
    "websiteUrl": "https://antigravity.google",
    "notes": "Antigravity CLI (agy)",
    "settingsConfig": {
      "env": {
        "GOOGLE_GEMINI_BASE_URL": "https://daily-cloudcode-pa.googleapis.com",
        "GEMINI_MODEL": "gemini-3.5-flash-medium"
      },
      "config": {
        "general": {
          "previewFeatures": true
        }
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gemini-3.5-flash-medium"
      }
    },
    "category": "official",
    "icon": "gemini",
    "iconColor": "#111827"
  },
  "gemini.github_copilot": {
    "name": "GitHub Copilot",
    "websiteUrl": "https://github.com/features/copilot",
    "notes": "",
    "settingsConfig": {
      "env": {
        "GEMINI_MODEL": "gemini-3.5-flash"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gemini-3.5-flash"
      }
    },
    "category": "official",
    "icon": "github",
    "iconColor": "#000000"
  },
  "gemini.grok_oauth": {
    "name": "Grok OAuth",
    "websiteUrl": "https://x.ai",
    "notes": "Grok OAuth",
    "settingsConfig": {
      "env": {
        "GOOGLE_GEMINI_BASE_URL": "https://api.x.ai/v1",
        "GEMINI_MODEL": "grok-4.6"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "grok-4.6"
      }
    },
    "category": "official",
    "icon": "grok",
    "iconColor": "#111827"
  },
  "gemini.kimi_code": {
    "name": "Kimi OAuth",
    "websiteUrl": "https://kimi.com",
    "notes": "Kimi OAuth",
    "settingsConfig": {
      "env": {
        "GOOGLE_GEMINI_BASE_URL": "https://api.kimi.com/coding/v1",
        "GEMINI_MODEL": "kimi-for-coding"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "kimi-for-coding"
      }
    },
    "category": "official",
    "icon": "kimi",
    "iconColor": "#111827"
  },
  "gemini.qoder_cosy": {
    "name": "Qoder OAuth",
    "websiteUrl": "https://qoder.com",
    "notes": "Qoder OAuth",
    "settingsConfig": {
      "env": {
        "GEMINI_MODEL": "auto"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "auto"
      }
    },
    "category": "official",
    "icon": "qoder"
  },
  "gemini.cursor_api_key": {
    "name": "Cursor API Key",
    "websiteUrl": "https://cursor.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "GEMINI_MODEL": "default"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "default"
      }
    },
    "category": "official",
    "icon": "cursor"
  },
  "gemini.cursor_oauth": {
    "name": "Cursor OAuth",
    "websiteUrl": "https://cursor.com",
    "notes": "",
    "settingsConfig": {
      "env": {
        "GEMINI_MODEL": "default"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "default"
      }
    },
    "category": "official",
    "icon": "cursor"
  },
  "gemini.openrouter": {
    "name": "OpenRouter",
    "websiteUrl": "https://openrouter.ai",
    "notes": "OpenRouter",
    "settingsConfig": {
      "env": {
        "GOOGLE_GEMINI_BASE_URL": "https://openrouter.ai/api",
        "GEMINI_MODEL": "gemini-3.5-flash"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gemini-3.5-flash"
      }
    },
    "category": "aggregator",
    "icon": "openrouter",
    "iconColor": "#6566F1"
  },
  "claude.kimi_coding_api_key": {
    "name": "Kimi API Key",
    "websiteUrl": "https://kimi.com",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "kimi",
    "iconColor": "#6366F1"
  },
  "codex.kimi_coding_api_key": {
    "name": "Kimi API Key",
    "websiteUrl": "https://kimi.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "kimi",
    "iconColor": "#6366F1"
  },
  "claude.zhipu_glm_cn": {
    "name": "Zhipu (China)",
    "websiteUrl": "https://bigmodel.cn",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "zhipu",
    "iconColor": "#0F62FE"
  },
  "codex.zhipu_glm_cn": {
    "name": "Zhipu (China)",
    "websiteUrl": "https://bigmodel.cn",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "zhipu",
    "iconColor": "#0F62FE"
  },
  "claude.zhipu_glm_global": {
    "name": "Zhipu (Global)",
    "websiteUrl": "https://z.ai",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "zhipu",
    "iconColor": "#0F62FE"
  },
  "codex.zhipu_glm_global": {
    "name": "Zhipu (Global)",
    "websiteUrl": "https://z.ai",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "zhipu",
    "iconColor": "#0F62FE"
  },
  "claude.bailian_coding_plan_cn": {
    "name": "Bailian",
    "websiteUrl": "https://bailian.console.aliyun.com",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "bailian"
  },
  "codex.bailian_coding_plan_cn": {
    "name": "Bailian",
    "websiteUrl": "https://bailian.console.aliyun.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "bailian"
  },
  "claude.bailian_coding_plan_global": {
    "name": "Bailian",
    "websiteUrl": "https://www.alibabacloud.com/product/coding",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "bailian"
  },
  "codex.bailian_coding_plan_global": {
    "name": "Bailian",
    "websiteUrl": "https://www.alibabacloud.com/product/coding",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "bailian"
  },
  "claude.minimax_cn": {
    "name": "MiniMax (China)",
    "websiteUrl": "https://platform.minimaxi.com",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "minimax",
    "iconColor": "#FF6B6B"
  },
  "codex.minimax_cn": {
    "name": "MiniMax (China)",
    "websiteUrl": "https://platform.minimaxi.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "minimax",
    "iconColor": "#FF6B6B"
  },
  "claude.minimax_global": {
    "name": "MiniMax (Global)",
    "websiteUrl": "https://platform.minimax.io",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "minimax",
    "iconColor": "#FF6B6B"
  },
  "codex.minimax_global": {
    "name": "MiniMax (Global)",
    "websiteUrl": "https://platform.minimax.io",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "minimax",
    "iconColor": "#FF6B6B"
  },
  "claude.volcengine_coding_plan": {
    "name": "Volcengine",
    "websiteUrl": "https://www.volcengine.com",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "doubao",
    "iconColor": "#1E37FC"
  },
  "codex.volcengine_coding_plan": {
    "name": "Volcengine",
    "websiteUrl": "https://www.volcengine.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "doubao",
    "iconColor": "#1E37FC"
  },
  "claude.xiaomi_mimo_token_plan": {
    "name": "Xiaomi MiMo (China)",
    "websiteUrl": "https://platform.xiaomimimo.com",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "xiaomimimo",
    "iconColor": "#FF6900"
  },
  "codex.xiaomi_mimo_token_plan": {
    "name": "Xiaomi MiMo (China)",
    "websiteUrl": "https://platform.xiaomimimo.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "xiaomimimo",
    "iconColor": "#FF6900"
  },
  "claude.xiaomi_mimo_token_plan_sgp": {
    "name": "Xiaomi MiMo (Singapore)",
    "websiteUrl": "https://platform.xiaomimimo.com",
    "notes": "",
    "settingsConfig": {
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "xiaomimimo",
    "iconColor": "#FF6900"
  },
  "codex.xiaomi_mimo_token_plan_sgp": {
    "name": "Xiaomi MiMo (Singapore)",
    "websiteUrl": "https://platform.xiaomimimo.com",
    "notes": "",
    "settingsConfig": {
      "auth": {},
      "config": "",
      "env": {},
      "modelMapping": {
        "mode": "passthrough"
      }
    },
    "category": "official",
    "icon": "xiaomimimo",
    "iconColor": "#FF6900"
  },
  "claude.custom_http": {
    "name": "Custom",
    "websiteUrl": "",
    "notes": "",
    "settingsConfig": {
      "env": {
        "ANTHROPIC_MODEL": "claude-sonnet-4-6"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "claude-sonnet-4-6"
      }
    },
    "category": "custom"
  },
  "codex.custom_http": {
    "name": "Custom",
    "websiteUrl": "",
    "notes": "",
    "settingsConfig": {
      "env": {
        "OPENAI_MODEL": "gpt-5.4"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gpt-5.4"
      }
    },
    "category": "custom"
  },
  "gemini.custom_http": {
    "name": "Custom",
    "websiteUrl": "",
    "notes": "",
    "settingsConfig": {
      "env": {
        "GEMINI_MODEL": "gemini-3.5-flash"
      },
      "modelMapping": {
        "mode": "single",
        "upstreamModel": "gemini-3.5-flash"
      }
    },
    "category": "custom"
  }
};

export function serverProviderPresetForProfile(
  profileId: string,
): ServerProviderPreset | undefined {
  return serverProviderPresets[profileId];
}
