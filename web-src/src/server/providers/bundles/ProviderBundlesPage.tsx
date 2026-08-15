import {
  useCallback,
  useMemo,
  useState,
  type ComponentProps,
  type CSSProperties,
  type ReactNode,
} from "react";
import {
  closestCenter,
  DndContext,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  arrayMove,
  sortableKeyboardCoordinates,
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { LoaderCircle, Plus, RefreshCw } from "lucide-react";
import { toast } from "sonner";

import { ClaudeIcon, CodexIcon, GeminiIcon } from "@/components/BrandIcons";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import type { ProviderBundleView } from "@/lib/api/providers";
import { providersApi } from "@/lib/api/providers";
import type { ShareRecord } from "@/lib/api/share";
import {
  shareKeys,
  useManagedAccountsQuery,
  useSharesQuery,
} from "@/lib/query";
import { cn } from "@/lib/utils";
import {
  isProviderShareDeleteConfirmSkipped,
  setProviderShareDeleteConfirmSkipped,
} from "@/utils/providerShareDeleteConfirm";
import { getProviderSharePhase } from "@/utils/shareUtils";
import {
  deleteBundleShare,
  shareForBundle,
  toggleBundleShare,
} from "./bundleShare";
import { ProviderBundleCard } from "./ProviderBundleCard";
import { ProviderBundleEditor } from "./ProviderBundleEditor";

export const providerBundleKeys = {
  all: ["provider-bundles"] as const,
};

interface ProviderBundlesPageProps {
  onOpenShareSettings?: () => void;
  toolbarActions?: ReactNode;
}

type BundleEditorRequest =
  | { mode: "create" }
  | { mode: "edit"; bundle: ProviderBundleView; initialSection?: "share" }
  | { mode: "duplicate"; bundle: ProviderBundleView };

function SortableProviderBundleCard(
  props: ComponentProps<typeof ProviderBundleCard>,
) {
  const {
    setNodeRef,
    attributes,
    listeners,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: props.bundle.id });
  const style: CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  return (
    <div ref={setNodeRef} style={style}>
      <ProviderBundleCard
        {...props}
        dragHandleProps={{ attributes, listeners, isDragging }}
      />
    </div>
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
  const accountsQuery = useManagedAccountsQuery();
  const [editing, setEditing] = useState<BundleEditorRequest | null>(null);
  const [deleting, setDeleting] = useState<ProviderBundleView | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [sortPending, setSortPending] = useState(false);
  const [sharePendingBundleId, setSharePendingBundleId] = useState<
    string | null
  >(null);
  const [shareDeleting, setShareDeleting] = useState<{
    bundle: ProviderBundleView;
    share: ShareRecord;
  } | null>(null);
  const bundles = bundlesQuery.data ?? [];
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: 8 },
    }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );
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
  const handleDragEnd = useCallback(
    async ({ active, over }: DragEndEvent) => {
      if (sortPending || !over || active.id === over.id) return;
      const oldIndex = bundles.findIndex((bundle) => bundle.id === active.id);
      const newIndex = bundles.findIndex((bundle) => bundle.id === over.id);
      if (oldIndex < 0 || newIndex < 0) return;

      const reordered = arrayMove(bundles, oldIndex, newIndex);
      queryClient.setQueryData<ProviderBundleView[]>(
        providerBundleKeys.all,
        reordered,
      );
      setSortPending(true);
      try {
        await providersApi.updateBundleSortOrder(
          reordered.map((bundle, sortIndex) => ({
            id: bundle.id,
            sortIndex,
          })),
        );
        await queryClient.invalidateQueries({
          queryKey: providerBundleKeys.all,
        });
        toast.success(
          t("provider.sortUpdated", { defaultValue: "排序已更新" }),
        );
      } catch (error) {
        queryClient.setQueryData<ProviderBundleView[]>(
          providerBundleKeys.all,
          bundles,
        );
        await queryClient.invalidateQueries({
          queryKey: providerBundleKeys.all,
        });
        toast.error(
          error instanceof Error
            ? error.message
            : t("provider.sortUpdateFailed", {
                defaultValue: "排序更新失败",
              }),
        );
      } finally {
        setSortPending(false);
      }
    },
    [bundles, queryClient, sortPending, t],
  );

  const handleToggleShare = async (bundle: ProviderBundleView) => {
    if (sharePendingBundleId) return;
    const share = sharesByBundle.get(bundle.id);
    const stopping = getProviderSharePhase(share) === "sharing";
    setSharePendingBundleId(bundle.id);
    try {
      const result = await toggleBundleShare(bundle.id, share);
      await queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      toast.success(
        result === "disabled"
          ? t("share.toast.disableSuccess", { defaultValue: "分享已关闭" })
          : t("share.toast.enableSuccess", { defaultValue: "分享已开启" }),
      );
    } catch (error) {
      void queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      toast.error(
        t(stopping ? "share.toast.disableError" : "share.toast.enableError", {
          defaultValue: stopping
            ? "关闭分享失败：{{error}}"
            : "开启分享失败：{{error}}",
          error: error instanceof Error ? error.message : String(error),
        }),
      );
    } finally {
      setSharePendingBundleId(null);
    }
  };

  const handleDeleteShare = async (
    bundle: ProviderBundleView,
    share: ShareRecord,
  ) => {
    if (sharePendingBundleId) return;
    setSharePendingBundleId(bundle.id);
    try {
      await deleteBundleShare(share);
      await queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      toast.success(
        t("share.toast.deleteSuccess", { defaultValue: "分享已删除" }),
      );
    } catch (error) {
      void queryClient.invalidateQueries({ queryKey: shareKeys.list() });
      toast.error(
        t("share.toast.deleteError", {
          defaultValue: "删除分享失败：{{error}}",
          error: error instanceof Error ? error.message : String(error),
        }),
      );
    } finally {
      setSharePendingBundleId(null);
    }
  };

  const handleShareDeleteRequest = (
    bundle: ProviderBundleView,
    share: ShareRecord,
  ) => {
    if (isProviderShareDeleteConfirmSkipped()) {
      void handleDeleteShare(bundle, share);
      return;
    }
    setShareDeleting({ bundle, share });
  };

  const confirmShareDelete = (remember: boolean) => {
    const target = shareDeleting;
    if (!target) return;
    if (remember) {
      setProviderShareDeleteConfirmSkipped(true);
    }
    setShareDeleting(null);
    void handleDeleteShare(target.bundle, target.share);
  };

  if (editing) {
    return (
      <ProviderBundleEditor
        bundle={editing.mode === "create" ? undefined : editing.bundle}
        duplicate={editing.mode === "duplicate"}
        initialSection={
          editing.mode === "edit" ? editing.initialSection : undefined
        }
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
      <div className="sticky top-0 z-30 mb-4 flex min-h-14 shrink-0 items-center gap-3 bg-background/95 backdrop-blur-md">
        <a
          href="https://tokenswitch.org"
          target="_blank"
          rel="noreferrer"
          className="hidden shrink-0 text-lg font-semibold text-foreground transition-colors hover:text-primary sm:block"
        >
          CC Switch Server
        </a>
        {toolbarActions}
        <div className="ml-auto flex shrink-0 items-center gap-1">
          <Button
            type="button"
            size="icon"
            variant="outline"
            title={t("common.refresh")}
            disabled={bundlesQuery.isFetching}
            onClick={() =>
              void Promise.all([
                bundlesQuery.refetch(),
                sharesQuery.refetch(),
                accountsQuery.refetch(),
              ])
            }
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
            size="icon"
            className="h-8 w-8 shrink-0 rounded-full bg-orange-500 text-white shadow-lg shadow-orange-500/30 hover:bg-orange-600 dark:bg-orange-500 dark:shadow-orange-500/40 dark:hover:bg-orange-600"
            title={t("providers.addProvider", { defaultValue: "添加供应商" })}
            onClick={() => setEditing({ mode: "create" })}
          >
            <Plus className="h-5 w-5" />
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
        <div className="flex flex-1 flex-col items-center justify-center rounded-lg border border-dashed border-border p-10 text-center">
          <div className="mb-4 flex items-center gap-3 opacity-70">
            <ClaudeIcon size={28} />
            <CodexIcon size={28} />
            <GeminiIcon size={28} />
          </div>
          <h3 className="text-lg font-semibold">{t("provider.noProviders")}</h3>
          <p className="mt-2 max-w-lg text-sm text-muted-foreground">
            {t("provider.noProvidersDescription")}
          </p>
          <p className="mt-1 max-w-lg text-sm text-muted-foreground">
            {t("provider.noProvidersDescriptionSnippet")}
          </p>
          <Button
            className="mt-6"
            onClick={() => setEditing({ mode: "create" })}
          >
            <Plus className="mr-2 h-4 w-4" />
            {t("providers.addProvider", { defaultValue: "添加供应商" })}
          </Button>
        </div>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={(event) => void handleDragEnd(event)}
        >
          <SortableContext
            items={bundles.map((bundle) => bundle.id)}
            strategy={verticalListSortingStrategy}
          >
            <div className="space-y-3">
              {bundles.map((bundle) => (
                <SortableProviderBundleCard
                  key={bundle.id}
                  bundle={bundle}
                  share={sharesByBundle.get(bundle.id)}
                  accounts={accountsQuery.data ?? []}
                  onEdit={() => setEditing({ mode: "edit", bundle })}
                  onEditShare={() =>
                    setEditing({
                      mode: "edit",
                      bundle,
                      initialSection: "share",
                    })
                  }
                  onDuplicate={() => setEditing({ mode: "duplicate", bundle })}
                  sharePending={sharePendingBundleId === bundle.id}
                  shareActionDisabled={
                    sharesQuery.isLoading ||
                    sharesQuery.isError ||
                    sharePendingBundleId !== null
                  }
                  onToggleShare={() => void handleToggleShare(bundle)}
                  onDeleteShare={() => {
                    const share = sharesByBundle.get(bundle.id);
                    if (share) handleShareDeleteRequest(bundle, share);
                  }}
                  onDelete={() => setDeleting(bundle)}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
      )}

      <ConfirmDialog
        isOpen={shareDeleting !== null}
        title={t("provider.share.deleteConfirmTitle", {
          defaultValue: "删除分享",
        })}
        message={t("provider.share.deleteConfirmMessage", {
          defaultValue: "删除后该分享将从 Share 页移除，且无法恢复。",
        })}
        confirmText={t("provider.share.deleteShort", {
          defaultValue: "删除分享",
        })}
        checkboxLabel={t("provider.share.deleteRemember", {
          defaultValue: "记住选择，之后不再询问",
        })}
        onConfirm={confirmShareDelete}
        onCancel={() => setShareDeleting(null)}
      />

      <ConfirmDialog
        isOpen={deleting !== null}
        title={t("confirm.deleteProvider", { defaultValue: "删除供应商" })}
        message={t("confirm.deleteProviderMessage", {
          defaultValue: "确定删除 {{name}}？此操作无法撤销。",
          name: deleting?.name ?? "",
        })}
        confirmText={deletePending ? t("common.loading") : t("common.delete")}
        variant="destructive"
        confirmDisabled={deletePending}
        onConfirm={confirmDelete}
        onCancel={() => {
          if (!deletePending) setDeleting(null);
        }}
      />
    </div>
  );
}
