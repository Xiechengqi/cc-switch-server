/**
 * Coding Plan 供应商的 base_url 路由表。
 *
 * 与后端 `coding_plan` 检测逻辑保持一致：靠 URL 子串/模式识别供应商。
 * 新增供应商时改这一处即可。
 */
export interface CodingPlanProviderEntry {
  /** 与后端 QuotaTier 的 `codingPlanProvider` 取值对齐 */
  id: "kimi" | "zhipu" | "minimax" | "zenmux" | "volcengine";
  /** 下拉/展示用 */
  label: string;
  /** base_url 匹配规则 */
  pattern: RegExp;
}

export const CODING_PLAN_PROVIDERS: readonly CodingPlanProviderEntry[] = [
  { id: "kimi", label: "Kimi For Coding", pattern: /api\.kimi\.com\/coding/i },
  {
    id: "zhipu",
    label: "Zhipu GLM (智谱)",
    pattern: /bigmodel\.cn|api\.z\.ai/i,
  },
  {
    id: "minimax",
    label: "MiniMax",
    pattern: /api\.minimaxi?\.com|api\.minimax\.io/i,
  },
  {
    id: "zenmux",
    label: "ZenMux",
    pattern: /zenmux\./i,
  },
  {
    id: "volcengine",
    label: "火山方舟 (Volcengine)",
    pattern: /volces\.com\/api\/coding/i,
  },
] as const;

/** 根据 Base URL 自动检测 Coding Plan 供应商；未命中返回 null */
export function detectCodingPlanProvider(
  baseUrl: string | undefined | null,
): CodingPlanProviderEntry["id"] | null {
  if (!baseUrl) return null;
  for (const cp of CODING_PLAN_PROVIDERS) {
    if (cp.pattern.test(baseUrl)) return cp.id;
  }
  return null;
}
