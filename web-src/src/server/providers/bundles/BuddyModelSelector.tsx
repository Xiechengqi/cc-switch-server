import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ChevronDown, LoaderCircle, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { providersApi, type BuddyProviderType } from "@/lib/api/providers";
import { managedAccountKeys } from "@/lib/query/accounts";

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

/** Keeps the existing editable value; discovery never changes or saves it. */
export function UpstreamModelInput({ catalog, value, onValueChange, className, ...input }: {
  catalog?: BuddyModelCatalogControl;
  value: string;
  onValueChange: (value: string) => void;
  id?: string;
  "aria-label"?: string;
  placeholder?: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const field = <Input {...input} className={className} value={value}
    onChange={(event) => onValueChange(event.target.value)} />;
  if (!catalog?.supported) return field;
  const models = catalog.data?.models ?? [];
  const selected = value.trim();
  const needle = search.trim().toLowerCase();
  const options = [
    { id: "auto", displayName: t("buddyModels.auto") },
    ...models.filter((model) => model.id !== "auto"),
  ].filter((model) => `${model.id} ${model.displayName ?? ""}`.toLowerCase().includes(needle));
  const missing = catalog.data && !catalog.error && !catalog.data.stale && selected
    && selected !== "auto" && !models.some((model) => model.id === selected);
  return <div className="min-w-0 flex-1 basis-56 space-y-1.5">
    <div className="flex items-center gap-1.5">
      {field}
      <Popover open={open} onOpenChange={(next) => { setOpen(next); if (!next) setSearch(""); }}>
        <PopoverTrigger asChild>
          <Button type="button" variant="outline" size="sm" disabled={!catalog.ready}
            aria-label={t("buddyModels.choose")}>
            {catalog.isFetching ? <LoaderCircle className="h-4 w-4 animate-spin" /> : <ChevronDown className="h-4 w-4" />}
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-80 max-w-[90vw] space-y-2 p-2">
          <Input aria-label={t("buddyModels.search")} placeholder={t("buddyModels.search")}
            value={search} onChange={(event) => setSearch(event.target.value)} />
          <div className="max-h-64 overflow-y-auto" role="listbox" aria-label={t("buddyModels.choose")}
            onKeyDown={(event) => {
              if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
              const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="option"]'));
              if (!buttons.length) return;
              event.preventDefault();
              const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
              const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
                : (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
              buttons[next].focus();
            }}>
            {options.map((model) => <button key={model.id} type="button" role="option"
              aria-selected={model.id === selected}
              className="block w-full rounded px-2 py-1.5 text-left text-sm hover:bg-accent focus:bg-accent"
              onClick={() => { onValueChange(model.id); setOpen(false); setSearch(""); }}>
              <span className="block">{model.displayName || model.id}</span>
              <span className="block font-mono text-xs text-muted-foreground">{model.id}</span>
            </button>)}
            {!options.length ? <p className="p-2 text-xs text-muted-foreground">{t("buddyModels.noMatch")}</p> : null}
          </div>
        </PopoverContent>
      </Popover>
      <Button type="button" variant="ghost" size="sm" disabled={!catalog.ready || catalog.isFetching}
        aria-label={t("buddyModels.refresh")} onClick={() => void catalog.refresh()}>
        <RefreshCw className="h-4 w-4" />
      </Button>
    </div>
    <p className="text-xs text-muted-foreground">{t("buddyModels.autoHint")}</p>
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
  </div>;
}
