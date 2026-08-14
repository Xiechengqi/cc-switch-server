import type { TFunction } from "i18next";
import { describe, expect, it } from "vitest";

import type { OllamaCloudSnapshot } from "@/lib/api/providers";
import {
  formatOllamaCloudModels,
  formatOllamaCloudSummary,
} from "./OllamaQuotaFooter";

const translations: Record<string, string> = {
  "provider.ollama.session": "Session",
  "provider.ollama.weekly": "Weekly",
  "provider.ollama.cost": "Activity ${{value}}",
  "provider.ollama.modelsMore": "+{{count}} more",
};
const t = ((key: string, options?: Record<string, unknown>) => {
  let value = translations[key] ?? key;
  for (const [name, replacement] of Object.entries(options ?? {})) {
    value = value.replace(`{{${name}}}`, String(replacement));
  }
  return value;
}) as TFunction;

function snapshot(): OllamaCloudSnapshot {
  return {
    providerKey: { app: "codex", providerId: "ollama" },
    providerRevision: 1,
    credentialSourceKey: { app: "codex", providerId: "ollama" },
    credentialGeneration: 2,
    source: "live",
    status: "complete",
    account: {
      state: "available",
      observedAtMs: 10,
      data: {
        id: "account-1",
        email: "owner@example.com",
        name: "owner",
        plan: "free",
      },
    },
    usage: {
      state: "available",
      observedAtMs: 10,
      data: {
        limits: [
          {
            kind: "session",
            utilization: 0,
            models: [{ name: "gpt-oss:120b", requestCount: 1 }],
            modelsTruncated: false,
          },
          {
            kind: "weekly",
            utilization: 0,
            models: [{ name: "gpt-oss:120b", requestCount: 6 }],
            modelsTruncated: false,
          },
        ],
        activity: { cost: "0.00000", models: [], modelsTruncated: false },
      },
    },
  };
}

describe("Ollama quota formatting", () => {
  it("keeps zero utilization and upstream model names visible", () => {
    expect(formatOllamaCloudSummary(snapshot(), t)).toBe(
      "free · owner · owner@example.com · Session 0% · Weekly 0% · Activity $0.00000",
    );
    expect(formatOllamaCloudModels(snapshot(), t)).toBe(
      "Session: gpt-oss:120b 1 · Weekly: gpt-oss:120b 6",
    );
  });

  it("keeps stale data visible and surfaces a partial endpoint failure", () => {
    const value = snapshot();
    value.status = "partial";
    value.account = {
      state: "error",
      errorKind: "authentication",
      reason: "Ollama Cloud rejected this API key",
    };
    expect(formatOllamaCloudSummary(value, t)).toContain("Session 0%");
    expect(formatOllamaCloudSummary(value, t)).toContain(
      "Ollama Cloud rejected this API key",
    );

    value.status = "stale";
    value.usage.state = "stale";
    expect(formatOllamaCloudSummary(value, t)).toContain("Weekly 0%");
  });
});
