import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { providersApi } from "@/lib/api/providers";
import { Badge } from "@/components/ui/badge";
import { EditableModelInput, type ModelInputProps } from "./EditableModelInput";

export function useZenModelCatalog(familyId: string, enabled: boolean) {
  const supported = familyId === "family.opencode_zen";
  const force = useRef(false);
  const query = useQuery({
    queryKey: ["provider-model-catalog", "opencode-zen"],
    enabled: supported && enabled,
    queryFn: ({ signal }) => providersApi.fetchZenModels(force.current, signal),
    staleTime: 8 * 60_000,
    refetchInterval: supported && enabled ? 8 * 60_000 : false,
    gcTime: 0,
    retry: false,
    refetchOnWindowFocus: false,
  });
  return {
    supported, data: query.data, error: query.error, isFetching: query.isFetching,
    refresh: async () => {
      if (!supported || query.isFetching) return;
      force.current = true;
      try { await query.refetch(); } finally { force.current = false; }
    },
  };
}

export type ZenModelCatalogControl = ReturnType<typeof useZenModelCatalog>;

export function ZenUpstreamModelInput({ catalog, ...input }: ModelInputProps & { catalog: ZenModelCatalogControl }) {
  const { t } = useTranslation();
  const [showAll, setShowAll] = useState(false);
  const models = catalog.data?.models ?? [];
  const selectedId = input.value.trim().replace(/\([^()]+\)$/, "");
  const selected = models.find((m) => m.id === selectedId);
  const missing = input.value.trim() && catalog.data && !catalog.error && !catalog.data.stale && !selected;
  const selectedEfforts = !catalog.data?.stale && !catalog.error
    ? selected?.effectiveCapabilities?.reasoningEfforts ?? [] : [];
  const reason = (key: NonNullable<typeof selected>["unavailableReason"]) => key ? t(`zenModels.reason.${key}`) : "";
  return <EditableModelInput {...input} ready isFetching={catalog.isFetching} refresh={catalog.refresh}
    labels={{ choose: t("zenModels.choose"), search: t("zenModels.search"), refresh: t("zenModels.refresh"), noMatch: t("zenModels.noMatch") }}
    controls={<label className="flex cursor-pointer items-center gap-2 text-xs">
      <input type="checkbox" checked={showAll} onChange={(event) => setShowAll(event.target.checked)} />{t("zenModels.showAll")}
    </label>}
    options={models.filter((m) => showAll || m.selectable).map((m) => ({
      id: m.id, value: m.upstreamModel, name: m.displayName, disabled: !m.selectable,
      badge: m.pricing === "free" ? <Badge variant="secondary">{t("zenModels.free")}</Badge> : null,
      detail: <span className="block text-xs text-muted-foreground">
        {m.unavailableReason ? reason(m.unavailableReason)
          : m.contextWindow ? t("zenModels.context", { count: m.contextWindow }) : null}
        {m.pricingSource === "legacy_id" ? <span className="block">{t("zenModels.legacy")}</span> : null}
      </span>,
    }))}>
    <p className="text-xs text-muted-foreground">{t("zenModels.hint")}</p>
    <div role="status" className="text-xs text-muted-foreground">
      {catalog.isFetching ? t("zenModels.loading") : catalog.error ? t("zenModels.failed") : null}
      {catalog.data ? <span className="block">{t(catalog.data.stale || catalog.error ? "zenModels.stale" : "zenModels.updated", {
        time: new Date(catalog.data.fetchedAtMs).toLocaleString(),
      })}</span> : null}
      {catalog.data && !catalog.isFetching && !models.some((m) => m.selectable) ? <span className="block">{t("zenModels.empty")}</span> : null}
    </div>
    {selected?.selectable ? <div className="space-y-1 text-xs text-muted-foreground">
      {selected.maxOutputTokens ? <p>{t("zenModels.output", { count: selected.maxOutputTokens })}</p> : null}
      {selectedEfforts.length > 0 ? <p>{t("zenModels.efforts", { values: selectedEfforts.join(", ") })}</p> : null}
      {!catalog.data?.stale && !catalog.error && selected.effectiveCapabilities?.structuredOutput
        ? <p>{t("zenModels.structured")}</p> : null}
      {selected.capabilities?.tools === false ? <p>{t("zenModels.noTools")}</p> : null}
    </div> : null}
    {missing ? <p className="text-xs text-amber-700">{t("zenModels.missing")}</p> : null}
    {selected?.unavailableReason ? <p className="text-xs text-amber-700">{reason(selected.unavailableReason)}</p> : null}
  </EditableModelInput>;
}
