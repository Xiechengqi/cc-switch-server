import React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { authCheckin, authGetCheckin, authSetCheckinPolicy, type ManagedAuthAccount } from "@/lib/api/auth";
import { oauthQuotaRootKey } from "@/lib/query/oauthQuotaKeys";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";

export function WorkBuddyCheckinControl({ account, editable = false }: { account: ManagedAuthAccount; editable?: boolean }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const key = ["account-checkin", account.id, account.authIdentityGeneration];
  const query = useQuery({
    queryKey: key,
    queryFn: () => authGetCheckin(account.id, account.authIdentityGeneration),
    enabled: account.provider === "workbuddy_oauth",
    staleTime: 30_000,
    refetchInterval: 30_000,
    retry: false,
  });
  const [draftTime, setDraftTime] = React.useState<string>();
  const view = query.data;
  const time = draftTime ?? view?.time ?? "09:00";
  const validTime = /^(?:[01]\d|2[0-3]):[0-5]\d$/.test(time);
  const policy = useMutation({
    mutationFn: (enabled: boolean) => authSetCheckinPolicy(account.id, account.authIdentityGeneration, view!.revision, enabled, time),
    onSuccess: (result) => { client.setQueryData(key, result); setDraftTime(undefined); },
    onError: () => { void client.invalidateQueries({ queryKey: key }); },
  });
  const run = useMutation({
    mutationFn: () => authCheckin(account.id, account.authIdentityGeneration),
    onSuccess: async (result) => {
      client.setQueryData(key, result);
      await Promise.all([
        client.invalidateQueries({ queryKey: ["managed-auth-status", account.provider] }),
        client.invalidateQueries({ queryKey: ["managed-auth-accounts"] }),
        client.invalidateQueries({ queryKey: oauthQuotaRootKey(account.provider) }),
        client.invalidateQueries({ queryKey: ["subscription"] }),
        client.invalidateQueries({ queryKey: ["providers"] }),
        client.invalidateQueries({ queryKey: ["share"] }),
      ]);
    },
    onError: () => { void client.invalidateQueries({ queryKey: key }); },
  });
  if (account.provider !== "workbuddy_oauth") return null;
  if (!view) return <p className="text-xs text-muted-foreground" role="status">
    {t(query.isError ? "workBuddyCheckin.loadError" : "workBuddyCheckin.loading")}
    {query.isError ? <Button type="button" size="sm" variant="ghost" onClick={() => void query.refetch()}>{t("workBuddyCheckin.reload")}</Button> : null}
  </p>;
  if (!view.supported) return <p className="text-xs text-muted-foreground">{t("workBuddyCheckin.unsupported")}</p>;
  const busy = policy.isPending || run.isPending;
  const waiting = view.nextRetryAt != null && view.nextRetryAt > Date.now();
  const status = view.todayCheckedIn ? "checked" : view.reason === "needs_relogin" ? "authentication_required"
    : view.pending ? "pending" : view.lastResult?.status ?? "unconfirmed";
  const statusToday = view.lastResult && new Intl.DateTimeFormat("en-CA", { timeZone: "Asia/Shanghai" }).format(view.lastResult.atMs)
    === new Intl.DateTimeFormat("en-CA", { timeZone: "Asia/Shanghai" }).format(Date.now());
  const shownStatus = !view.todayCheckedIn && !view.pending && !view.reason && view.lastResult && !statusToday ? "unconfirmed" : status;
  return <div className="space-y-2 text-xs" data-account-checkin={account.id}>
    <p role="status" className="text-muted-foreground">
      {t("workBuddyCheckin.title")} · {t(`workBuddyCheckin.status.${shownStatus}`)}
      {view.quotaPending ? ` · ${t("workBuddyCheckin.quotaPending")}` : ""}
      {!editable ? ` · ${t(view.enabled ? "workBuddyCheckin.scheduled" : "workBuddyCheckin.disabled", { time: view.time })}` : ""}
    </p>
    {editable ? <>
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-2">
          <Switch checked={view.enabled} disabled={busy || !validTime} onCheckedChange={(enabled) => policy.mutate(enabled)} aria-label={t("workBuddyCheckin.auto")} />
          {t("workBuddyCheckin.auto")}
        </label>
        <Input type="time" value={time} onChange={(e) => setDraftTime(e.target.value)} disabled={busy}
          aria-label={t("workBuddyCheckin.time")} className="h-8 w-28" />
        <span className="text-muted-foreground">{t("workBuddyCheckin.timezone")}</span>
        <Button type="button" size="sm" variant="outline" disabled={busy || !validTime || time === view.time}
          onClick={() => policy.mutate(view.enabled)}>{t("workBuddyCheckin.save")}</Button>
        <Button type="button" size="sm" variant="outline" disabled={busy || waiting || view.todayCheckedIn || view.reason === "needs_relogin"}
          onClick={() => run.mutate()}>{t("workBuddyCheckin.now")}</Button>
      </div>
      <p className="text-muted-foreground">{t("workBuddyCheckin.hint")}</p>
      {policy.isError || run.isError ? <p role="alert" className="text-destructive">{t("workBuddyCheckin.actionError")}</p> : null}
    </> : null}
  </div>;
}
