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
import { useManagedAccountsQuery, useSharesQuery } from "@/lib/query";
import { cn } from "@/lib/utils";
import { shareForBundle } from "./bundleShare";
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
            onClick={() => setEditing({ mode: "create" })}
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
          <Button onClick={() => setEditing({ mode: "create" })}>
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
                  onDuplicate={() => setEditing({ mode: "duplicate", bundle })}
                  onOpenShare={() =>
                    setEditing({
                      mode: "edit",
                      bundle,
                      initialSection: "share",
                    })
                  }
                  onDelete={() => setDeleting(bundle)}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
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
