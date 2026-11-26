import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { AuthCenterPanel } from "./AuthCenterPanel";
import { ProviderRuntimeDefaultsPanel } from "./ProviderRuntimeDefaultsPanel";
import { LanguageSettings } from "./LanguageSettings";
import { normalizeLanguage } from "@/i18n";
import { ThemeSettings } from "./ThemeSettings";
import { ModelTestConfigPanel } from "@/components/usage/ModelTestConfigPanel";
import { UsageDashboard } from "@/components/usage/UsageDashboard";
import { invokeCommand, jsonFetch } from "@/lib/runtime";

type Policy = {
  autoUpgradeEnabled: boolean;
  autoUpgradeCheckIntervalMinutes: number;
};
type Workspace = Policy & {
  upgrade: { taskId: string; status: string } | null;
};
const commands = {
  restart: "restart_server_service",
  rollback: "rollback_server_service",
  upgrade: "start_admin_upgrade",
} as const;

function TenantServiceSettings() {
  const { t } = useTranslation();
  const [policy, setPolicy] = useState<Policy | null>(null);
  const [status, setStatus] = useState("");
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const read = async () => {
      try {
        const data = await jsonFetch<Workspace>("/web-api/market/workspace", {
          signal: controller.signal,
        });
        if (controller.signal.aborted) return;
        setPolicy(
          (current) =>
            current ?? {
              autoUpgradeEnabled: data.autoUpgradeEnabled,
              autoUpgradeCheckIntervalMinutes:
                data.autoUpgradeCheckIntervalMinutes,
            },
        );
        setStatus(data.upgrade?.status ?? "");
      } catch (cause) {
        if (!controller.signal.aborted) setError(String(cause));
      } finally {
        if (!controller.signal.aborted) timer = setTimeout(read, 5000);
      }
    };
    void read();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, []);
  async function perform(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setNotice("");
    setError("");
    try {
      await fn();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="grid gap-4 rounded-lg border p-4">
      <h3 className="font-semibold">{t("lease.settings")}</h3>
      <div className="flex flex-wrap gap-2">
        {(Object.keys(commands) as (keyof typeof commands)[]).map((action) => (
          <Button
            key={action}
            variant="outline"
            disabled={busy || status === "running"}
            onClick={() => {
              if (window.confirm(t(`lease.confirm_${action}`)))
                void perform(async () => {
                  await invokeCommand(commands[action], {});
                  setNotice(
                    t("lease.running", { action: t(`lease.${action}`) }),
                  );
                });
            }}
          >
            {t(`lease.${action}`)}
          </Button>
        ))}
      </div>
      {status && (
        <p role="status">
          {t("lease.upgrade")} ·{" "}
          {t(
            status === "success"
              ? "lease.done"
              : status === "failed"
                ? "lease.failed"
                : "lease.running",
            { action: t("lease.upgrade") },
          )}
        </p>
      )}
      {policy && (
        <form
          className="flex flex-wrap items-center gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void perform(async () => {
              await jsonFetch("/web-api/market/upgrade-policy", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify(policy),
              });
              setNotice(t("lease.done"));
            });
          }}
        >
          <label>
            <input
              type="checkbox"
              checked={policy.autoUpgradeEnabled}
              onChange={(e) =>
                setPolicy({ ...policy, autoUpgradeEnabled: e.target.checked })
              }
            />{" "}
            {t("lease.automatic")}
          </label>
          <label>
            {t("lease.interval")}{" "}
            <input
              className="w-20 rounded border px-2"
              type="number"
              min={5}
              max={1440}
              required
              value={policy.autoUpgradeCheckIntervalMinutes}
              onChange={(e) =>
                setPolicy({
                  ...policy,
                  autoUpgradeCheckIntervalMinutes: Number(e.target.value),
                })
              }
            />
          </label>
          <Button type="submit" disabled={busy}>
            {t("lease.save")}
          </Button>
        </form>
      )}
      <div>
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => {
            if (window.confirm(t("lease.rotateConfirm")))
              void perform(async () => {
                const result = await jsonFetch<{ token: string }>(
                  "/web-api/market/token",
                  { method: "POST" },
                );
                setToken(result.token);
              });
          }}
        >
          {t("lease.rotate")}
        </Button>
        {token && (
          <input
            className="mt-2 block w-full rounded border p-2 font-mono"
            aria-label="API Token"
            readOnly
            value={token}
            onFocus={(e) => e.target.select()}
          />
        )}
      </div>
      {notice && <p role="status">{notice}</p>}
      {error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
    </section>
  );
}

export function TenantSettings({ initialTab }: { initialTab: string }) {
  const { t, i18n } = useTranslation();
  return (
    <Tabs
      defaultValue={
        initialTab === "auth" || initialTab === "usage" ? initialTab : "general"
      }
    >
      <TabsList>
        <TabsTrigger value="general">{t("settings.tabGeneral")}</TabsTrigger>
        <TabsTrigger value="auth">{t("settings.tabAuth")}</TabsTrigger>
        <TabsTrigger value="usage">{t("usage.title")}</TabsTrigger>
      </TabsList>
      <TabsContent value="general" className="grid gap-5">
        <TenantServiceSettings />
        <ProviderRuntimeDefaultsPanel />
        <ModelTestConfigPanel />
        <LanguageSettings
          value={normalizeLanguage(i18n.language)}
          onChange={(language) => {
            void i18n.changeLanguage(language);
            try {
              localStorage.setItem("language", language);
            } catch {
              /* Browser storage may be unavailable. */
            }
          }}
        />
        <ThemeSettings />
      </TabsContent>
      <TabsContent value="auth">
        <AuthCenterPanel serverMode />
      </TabsContent>
      <TabsContent value="usage">
        <UsageDashboard />
      </TabsContent>
    </Tabs>
  );
}
