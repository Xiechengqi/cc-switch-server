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
  Share2,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { ClaudeIcon, CodexIcon, GeminiIcon } from "@/components/BrandIcons";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { ProviderBundleView } from "@/lib/api/providers";
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
  shared,
  onEdit,
  onDelete,
}: {
  bundle: ProviderBundleView;
  shared: boolean;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const family = familyById(bundle.familyId);
  const routeBase = `${window.location.origin}/r/${bundle.routeKey}`;
  return (
    <article className="rounded-lg border border-border/70 bg-background p-4 shadow-sm transition-colors hover:border-border">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-3">
          <div
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md border bg-muted/40"
            style={bundle.iconColor ? { color: bundle.iconColor } : undefined}
          >
            <ProviderIcon
              icon={bundle.icon}
              name={bundle.name}
              size={23}
              showFallback
            />
          </div>
          <div className="min-w-0">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <h2 className="truncate text-sm font-semibold">{bundle.name}</h2>
              <div className="flex shrink-0 items-center gap-1.5">
                {bundle.supportedApps.map((app) => (
                  <span
                    key={app}
                    className={cn(
                      "flex h-6 w-6 items-center justify-center rounded border",
                      bundle.enabledApps.includes(app)
                        ? "border-border bg-background"
                        : "border-transparent bg-muted opacity-35 grayscale",
                    )}
                    title={`${app}${bundle.enabledApps.includes(app) ? "" : " (disabled)"}`}
                  >
                    <AppLogo app={app} />
                  </span>
                ))}
              </div>
            </div>
            <p className="mt-1 truncate text-xs text-muted-foreground">
              {family?.label ?? bundle.familyId}
            </p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1">
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

      <div className="mt-4 flex flex-wrap items-center gap-2">
        <Badge variant="outline" className="gap-1.5 font-mono font-normal">
          <Route className="h-3.5 w-3.5" />
          {bundle.routeKey}
        </Badge>
        <Badge
          variant={bundle.credentialConfigured ? "secondary" : "destructive"}
          className="gap-1.5"
        >
          <KeyRound className="h-3.5 w-3.5" />
          {bundle.credentialConfigured
            ? t("providerBundle.credentialReady", {
                defaultValue: "凭据已配置",
              })
            : t("providerBundle.credentialMissing", {
                defaultValue: "缺少凭据",
              })}
        </Badge>
        {shared ? (
          <Badge variant="secondary" className="gap-1.5">
            <Share2 className="h-3.5 w-3.5" />
            {t("provider.share.stateActive", { defaultValue: "分享已启用" })}
          </Badge>
        ) : null}
      </div>

      <div className="mt-4 flex min-w-0 items-center gap-2 rounded-md bg-muted/40 px-3 py-2">
        <code className="min-w-0 flex-1 truncate text-xs">{routeBase}</code>
        <Button
          type="button"
          size="icon"
          variant="ghost"
          title={t("common.copy")}
          onClick={() => void copyText(routeBase)}
        >
          <Copy className="h-4 w-4" />
        </Button>
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
  const shareIdsByBundle = useMemo(
    () =>
      new Map(
        bundles.map((bundle) => [
          bundle.id,
          shareForBundle(sharesQuery.data, bundle.id)?.id,
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
        <div className="grid gap-3 lg:grid-cols-2">
          {bundles.map((bundle) => (
            <BundleCard
              key={bundle.id}
              bundle={bundle}
              shared={Boolean(shareIdsByBundle.get(bundle.id))}
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
