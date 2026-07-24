export interface ProxyStatus {
  running: boolean;
  address: string;
  port: number;
  active_connections: number;
  total_requests: number;
  success_requests: number;
  failed_requests: number;
  success_rate: number;
  uptime_seconds: number;
  current_provider: string | null;
  current_provider_id: string | null;
  last_request_at: string | null;
  last_error: string | null;
  active_targets?: ActiveTarget[];
}

export interface ActiveTarget {
  app_type: string;
  provider_name: string;
  provider_id: string;
}

export interface ProxyServerInfo {
  address: string;
  port: number;
  started_at: string;
}

export interface ProxyTakeoverStatus {
  claude: boolean;
  "claude-desktop"?: boolean;
  codex: boolean;
  gemini: boolean;
  opencode: boolean;
  openclaw: boolean;
  hermes: boolean;
  // 「意图位已开启 但当前还没 current provider」的待接管态。
  // 后端 derive 计算：enabled && !has_current_provider。
  // 前端可据此显示「代理已就绪，添加 provider 后自动启用接管」类提示。
  claude_pending?: boolean;
  codex_pending?: boolean;
  gemini_pending?: boolean;
}

export type ProviderHealthStatus =
  | "unknown"
  | "healthy"
  | "degraded"
  | "unhealthy";

export type ProviderProbeSupport = "supported" | "unsupported";

export interface ProviderHealth {
  provider_id: string;
  app_type: string;
  status: ProviderHealthStatus;
  probe_support: ProviderProbeSupport;
  available: boolean;
  is_healthy: boolean;
  consecutive_successes: number;
  consecutive_failures: number;
  confirmation_pending: boolean;
  last_success_at: string | null;
  last_failure_at: string | null;
  last_error: string | null;
  updated_at: string;
  checked_at: string | null;
  stale_at: string | null;
  source: string | null;
  latency_ms: number | null;
  model: string | null;
  status_code: number | null;
  error_category: string | null;
}

export interface ProxyUsageRecord {
  provider_id: string;
  app_type: string;
  endpoint: string;
  request_tokens: number | null;
  response_tokens: number | null;
  status_code: number;
  latency_ms: number;
  error: string | null;
  timestamp: string;
}
