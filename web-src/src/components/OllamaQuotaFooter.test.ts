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
      "free · owner@example.com · Session 0% · Weekly 0% · Activity $0.0",
    );
    expect(formatOllamaCloudModels(snapshot(), t)).toBe(
      "Session: gpt-oss:120b 1 · Weekly: gpt-oss:120b 6",
    );
  });

  it("formats activity cost with one fractional digit", () => {
    const value = snapshot();
    const usage = value.usage.data;
    expect(usage?.activity).toBeDefined();
    if (usage?.activity) usage.activity.cost = "1.26";
    expect(formatOllamaCloudSummary(value, t)).toContain("Activity $1.3");
  });

  it("prefers email and falls back to one account identifier", () => {
    const value = snapshot();
    const account = value.account.data;
    expect(account).toBeDefined();
    delete account?.email;
    expect(formatOllamaCloudSummary(value, t)).toContain("free · owner ·");

    delete account?.name;
    expect(formatOllamaCloudSummary(value, t)).toContain("free · account-1 ·");

    delete account?.id;
    expect(formatOllamaCloudSummary(value, t)).toMatch(/^free · Session/);
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
