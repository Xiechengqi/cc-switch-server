import { useMemo, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Copy,
  KeyRound,
  LoaderCircle,
  Pencil,
  Plus,
  RefreshCw,
  Route,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { ClaudeIcon, CodexIcon, GeminiIcon } from "@/components/BrandIcons";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ProviderShareStatusTag } from "@/components/providers/ProviderShareStatusTag";
import { Button } from "@/components/ui/button";
import type { ProviderBundleView } from "@/lib/api/providers";
import type { ShareRecord } from "@/lib/api/share";
import { providersApi } from "@/lib/api/providers";
import { copyText } from "@/lib/clipboard";
import { useSharesQuery } from "@/lib/query";
import type { CoreProviderApp } from "@/server/providerRegistry";
import { familyById } from "@/server/providerRegistry";
import { cn } from "@/lib/utils";
import { shareForBundle } from "./bundleShare";
import { ProviderBundleEditor } from "./ProviderBundleEditor";

export const providerBundleKeys = {
  all: ["provider-bundles"] as const,
};

interface ProviderBundlesPageProps {
  onOpenShareSettings?: () => void;
  toolbarActions?: ReactNode;
}

function AppLogo({ app }: { app: CoreProviderApp }) {
  if (app === "claude") return <ClaudeIcon size={17} />;
  if (app === "codex") return <CodexIcon size={17} />;
  return <GeminiIcon size={17} />;
}

function BundleCard({
  bundle,
  share,
  onEdit,
  onDelete,
}: {
  bundle: ProviderBundleView;
  share?: ShareRecord;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const family = familyById(bundle.familyId);
  const routeBase = `${window.location.origin}/r/${bundle.routeKey}`;
  return (
    <article className="group relative overflow-hidden rounded-xl border border-border bg-card p-4 text-card-foreground transition-all duration-300 hover:border-border-active hover:shadow-sm">
      <div className="relative flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-border bg-muted transition-transform duration-300 group-hover:scale-105">
            <ProviderIcon
              icon={bundle.icon}
              name={bundle.name}
              color={bundle.iconColor}
              size={20}
              showFallback
            />
          </div>
          <div className="min-w-0 flex-1 space-y-1">
            <div className="flex min-h-7 min-w-0 flex-wrap items-center gap-2">
              <h2 className="truncate text-base font-semibold leading-none">
                {bundle.name}
              </h2>
              <div className="flex shrink-0 items-center gap-1">
                {bundle.supportedApps.map((app) => (
                  <span
                    key={app}
                    className={cn(
                      "flex h-5 w-5 items-center justify-center",
                      bundle.enabledApps.includes(app)
                        ? "opacity-100"
                        : "opacity-30 grayscale",
                    )}
                    title={`${app}${bundle.enabledApps.includes(app) ? "" : " (disabled)"}`}
                  >
                    <AppLogo app={app} />
                  </span>
                ))}
              </div>
              <span className="inline-flex items-center rounded-md bg-muted px-1.5 py-0.5 text-[10px] font-semibold text-muted-foreground">
                {family?.label ?? bundle.familyId}
              </span>
              <span
                className={cn(
                  "inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] font-semibold",
                  bundle.credentialConfigured
                    ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300"
                    : "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300",
                )}
              >
                <KeyRound className="h-3 w-3" />
                {bundle.credentialConfigured
                  ? t("providerBundle.credentialReady", {
                      defaultValue: "凭据已配置",
                    })
                  : t("providerBundle.credentialMissing", {
                      defaultValue: "缺少凭据",
                    })}
              </span>
              {share ? <ProviderShareStatusTag share={share} /> : null}
            </div>
            <button
              type="button"
              className="inline-flex max-w-full items-center gap-1.5 overflow-hidden text-left text-sm text-blue-500 transition-colors hover:underline dark:text-blue-400"
              title={`${routeBase} - ${t("common.copy")}`}
              onClick={() => void copyText(routeBase)}
            >
              <Route className="h-3.5 w-3.5 shrink-0" />
              <code className="min-w-0 truncate">{routeBase}</code>
              <Copy className="h-3.5 w-3.5 shrink-0" />
            </button>
          </div>
        </div>
        <div className="flex shrink-0 justify-end gap-1 opacity-0 transition-opacity duration-200 group-hover:opacity-100 group-focus-within:opacity-100 max-sm:opacity-100">
          <Button
            type="button"
            size="icon"
            variant="ghost"
            title={t("common.edit")}
            onClick={onEdit}
          >
            <Pencil className="h-4 w-4" />
          </Button>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            title={t("common.delete")}
            onClick={onDelete}
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </article>
  );
}

export function ProviderBundlesPage({
  onOpenShareSettings,
  toolbarActions,
}: ProviderBundlesPageProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const bundlesQuery = useQuery({
    queryKey: providerBundleKeys.all,
    queryFn: providersApi.getBundles,
  });
  const sharesQuery = useSharesQuery();
  const [editing, setEditing] = useState<ProviderBundleView | "new" | null>(
    null,
  );
  const [deleting, setDeleting] = useState<ProviderBundleView | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const bundles = bundlesQuery.data ?? [];
  const sharesByBundle = useMemo(
    () =>
      new Map(
        bundles.map((bundle) => [
          bundle.id,
          shareForBundle(sharesQuery.data, bundle.id),
        ]),
      ),
    [bundles, sharesQuery.data],
  );

  if (editing) {
    return (
      <ProviderBundleEditor
        bundle={editing === "new" ? undefined : editing}
        onCancel={() => setEditing(null)}
        onSaved={() => setEditing(null)}
        onOpenShareSettings={onOpenShareSettings}
      />
    );
  }

  const confirmDelete = async () => {
    if (!deleting) return;
    setDeletePending(true);
    try {
      const preview = await providersApi.getBundleDeletePreview(deleting.id);
      if (preview.blocked) {
        toast.error(
          t("providerBundle.deleteBlocked", {
            defaultValue: "请先删除或解绑该供应商的远程分享",
          }),
        );
        return;
      }
      await providersApi.deleteBundle(deleting.id, preview.revision);
      await queryClient.invalidateQueries({ queryKey: providerBundleKeys.all });
      setDeleting(null);
      toast.success(
        t("providerBundle.deleted", { defaultValue: "供应商已删除" }),
      );
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setDeletePending(false);
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col pb-12">
      <div className="sticky top-0 z-30 mb-4 flex min-h-14 shrink-0 items-center gap-3 border-b border-border/60 bg-background/95 backdrop-blur-md">
        <a
          href="https://tokenswitch.org"
          target="_blank"
          rel="noreferrer"
          className="hidden shrink-0 text-lg font-semibold text-foreground transition-colors hover:text-primary sm:block"
        >
          CC Switch Server
        </a>
        <span
          className="hidden h-5 w-px shrink-0 bg-border sm:block"
          aria-hidden
        />
        <div className="flex min-w-0 items-baseline gap-2">
          <h1 className="shrink-0 text-base font-semibold">
            {t("providerBundle.title", { defaultValue: "供应商" })}
          </h1>
          <p className="hidden truncate text-xs text-muted-foreground md:block">
            {t("providerBundle.count", {
              defaultValue: "{{count}} 个供应商节点",
              count: bundles.length,
            })}
          </p>
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {toolbarActions}
          {toolbarActions ? (
            <span className="mx-1 h-5 w-px shrink-0 bg-border" aria-hidden />
          ) : null}
          <Button
            type="button"
            size="icon"
            variant="outline"
            title={t("common.refresh")}
            disabled={bundlesQuery.isFetching}
            onClick={() => void bundlesQuery.refetch()}
          >
            <RefreshCw
              className={cn(
                "h-4 w-4",
                bundlesQuery.isFetching && "animate-spin",
              )}
            />
          </Button>
          <Button
            type="button"
            className="shrink-0"
            title={t("providers.addProvider", { defaultValue: "添加供应商" })}
            onClick={() => setEditing("new")}
          >
            <Plus className="h-4 w-4 sm:mr-2" />
            <span className="hidden sm:inline">
              {t("providers.addProvider", { defaultValue: "添加供应商" })}
            </span>
          </Button>
        </div>
      </div>

      {bundlesQuery.isLoading ? (
        <div className="flex flex-1 items-center justify-center py-20 text-muted-foreground">
          <LoaderCircle className="mr-2 h-5 w-5 animate-spin" />
          {t("common.loading")}
        </div>
      ) : bundlesQuery.isError ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 py-20 text-sm text-destructive">
          <span>{bundlesQuery.error.message}</span>
          <Button variant="outline" onClick={() => void bundlesQuery.refetch()}>
            {t("common.retry")}
          </Button>
        </div>
      ) : bundles.length === 0 ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-4 py-20 text-center">
          <div className="flex items-center gap-3 opacity-70">
            <ClaudeIcon size={28} />
            <CodexIcon size={28} />
            <GeminiIcon size={28} />
          </div>
          <Button onClick={() => setEditing("new")}>
            <Plus className="mr-2 h-4 w-4" />
            {t("providers.addProvider", { defaultValue: "添加供应商" })}
          </Button>
        </div>
      ) : (
        <div className="space-y-3">
          {bundles.map((bundle) => (
            <BundleCard
              key={bundle.id}
              bundle={bundle}
              share={sharesByBundle.get(bundle.id)}
              onEdit={() => setEditing(bundle)}
              onDelete={() => setDeleting(bundle)}
            />
          ))}
        </div>
      )}

      <ConfirmDialog
        isOpen={deleting !== null}
        title={t("confirm.deleteProvider", { defaultValue: "删除供应商" })}
        message={t("confirm.deleteProviderMessage", {
          defaultValue: "确定删除 {{name}}？此操作无法撤销。",
          name: deleting?.name ?? "",
        })}
        confirmText={deletePending ? t("common.loading") : t("common.delete")}
        variant="destructive"
        onConfirm={() => void confirmDelete()}
        onCancel={() => {
          if (!deletePending) setDeleting(null);
        }}
      />
    </div>
  );
}
