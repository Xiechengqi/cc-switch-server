import { useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Input } from "@/components/ui/input";
import { providersApi, type BuddyAccountModels, type BuddyProviderType } from "@/lib/api/providers";
import { managedAccountKeys } from "@/lib/query/accounts";
import { EditableModelInput, type ModelInputProps } from "./EditableModelInput";

export function useBuddyModelCatalog(
  providerType: string | undefined,
  accountId: string,
  authIdentityGeneration: number | undefined,
  enabled: boolean,
) {
  const supported = providerType === "workbuddy_oauth" || providerType === "codebuddy_oauth";
  const ready = Boolean(accountId && authIdentityGeneration != null);
  const scope = JSON.stringify([providerType, accountId, authIdentityGeneration]);
  const forceRefresh = useRef<{ scope: string } | null>(null);
  const query = useQuery({
    // Account mutation invalidations already target this prefix, including
    // reauthorization and deletion. Never retain another account's placeholder.
    queryKey: [...managedAccountKeys.all, "buddy-model-catalog", providerType, accountId, authIdentityGeneration],
    enabled: supported && ready && enabled,
    queryFn: ({ signal }) => providersApi.fetchBuddyAccountModels({
      providerType: providerType as BuddyProviderType,
      accountId,
      authIdentityGeneration: authIdentityGeneration!,
    }, forceRefresh.current?.scope === scope, signal),
    staleTime: 8 * 60_000,
    gcTime: 0,
    retry: false,
    refetchOnWindowFocus: false,
  });
  return {
    supported, ready, data: query.data, error: query.error, isFetching: query.isFetching,
    refresh: async () => {
      if (!supported || !ready || query.isFetching) return;
      const pending = { scope };
      forceRefresh.current = pending;
      try { await query.refetch(); } finally {
        if (forceRefresh.current === pending) forceRefresh.current = null;
      }
    },
  };
}

export type BuddyModelCatalogControl = ReturnType<typeof useBuddyModelCatalog>;

type BuddyModel = BuddyAccountModels["models"][number];

function BuddyReasoningDefault({ model, stale }: { model?: BuddyModel; stale?: boolean }) {
  const { t } = useTranslation();
  const caps = model?.raw?.capabilities;
  return <span className="block text-xs text-muted-foreground">
    {caps?.reasoningCapabilityKnown === true && caps.reasoning === false
      ? t("buddyModels.reasoningNone")
      : caps?.reasoningDefault ? t(stale ? "buddyModels.reasoningCachedDefault" : "buddyModels.reasoningDefault", { effort: caps.reasoningDefault })
      : t("buddyModels.reasoningUnknown")}
  </span>;
}

export function BuddyReasoningNotice({ catalog }: { catalog?: BuddyModelCatalogControl }) {
  const { t } = useTranslation();
  return catalog?.supported ? <p className="text-xs text-muted-foreground">{t("buddyModels.reasoningPolicy")}</p> : null;
}

/** Keeps the existing editable value; discovery never changes or saves it. */
export function BuddyUpstreamModelInput({ catalog, ...input }: ModelInputProps & {
  catalog?: BuddyModelCatalogControl;
}) {
  const { t } = useTranslation();
  if (!catalog?.supported) {
    const { onValueChange, ...field } = input;
    return <Input {...field} onChange={(event) => onValueChange(event.target.value)} />;
  }
  const models = catalog.data?.models ?? [];
  const selected = input.value.trim();
  const options = [
    { id: "auto", upstreamModel: "auto", displayName: t("buddyModels.auto"), raw: undefined },
    ...models.filter((model) => model.id !== "auto"),
  ];
  const missing = catalog.data && !catalog.error && !catalog.data.stale && selected
    && selected !== "auto" && !models.some((model) => model.id === selected);
  return <EditableModelInput {...input} ready={catalog.ready} isFetching={catalog.isFetching} refresh={catalog.refresh}
    labels={{ choose: t("buddyModels.choose"), search: t("buddyModels.search"), refresh: t("buddyModels.refresh"), noMatch: t("buddyModels.noMatch") }}
    options={options.map((model) => ({
      id: model.id, name: model.displayName || model.id,
      detail: model.id !== "auto" ? <>
        <span className="block text-xs text-muted-foreground">
          {t(model.raw?.capabilities?.images === true ? "buddyModels.imageInput"
            : model.raw?.capabilities?.images === false ? "buddyModels.textInput" : "buddyModels.unknownImageInput")}
        </span>
        <BuddyReasoningDefault model={model} stale={catalog.data?.stale} />
      </> : null,
    }))}>
    <p className="text-xs text-muted-foreground">{t("buddyModels.autoHint")}</p>
    <BuddyReasoningNotice catalog={catalog} />
    {selected === "auto" ? <p className="text-xs text-muted-foreground">{t("buddyModels.reasoningAuto")}</p>
      : <BuddyReasoningDefault model={models.find((model) => model.id === selected)} stale={catalog.data?.stale} />}
    <div role="status" className="text-xs text-muted-foreground">
      {!catalog.ready ? t("buddyModels.selectAccount") : catalog.isFetching ? t("buddyModels.loading")
        : catalog.error ? t("buddyModels.failed", { message: catalog.error.message })
        : catalog.data ? <>
          {t(catalog.data.stale ? "buddyModels.stale" : "buddyModels.updated", {
            time: new Date(catalog.data.fetchedAtMs).toLocaleString(),
          })}
          {!models.length ? <span className="block text-amber-700">{t("buddyModels.empty")}</span> : null}
        </> : null}
    </div>
    {missing ? <p className="text-xs text-amber-700">{t("buddyModels.missing")}</p> : null}
  </EditableModelInput>;
}
