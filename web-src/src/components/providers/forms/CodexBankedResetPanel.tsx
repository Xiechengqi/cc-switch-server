import React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Gift, Loader2, RefreshCw, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { codexBankedResetApi, subscriptionApi } from "@/lib/api";
import type { CodexBankedResetCredit } from "@/lib/api";

interface CodexBankedResetPanelProps {
  accountId?: string | null;
  workspaceId?: string | null;
}

function resetQueryKey(
  accountId: string | null | undefined,
  workspaceId: string | null | undefined,
) {
  return [
    "codex_banked_reset",
    "status",
    accountId ?? "default-account",
    workspaceId ?? "default-workspace",
  ] as const;
}

export const CodexBankedResetPanel: React.FC<CodexBankedResetPanelProps> = ({
  accountId,
  workspaceId,
}) => {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [selectedCreditId, setSelectedCreditId] = React.useState<string>("");
  const [consumeConfirmOpen, setConsumeConfirmOpen] = React.useState(false);

  const queryKey = React.useMemo(
    () => resetQueryKey(accountId, workspaceId),
    [accountId, workspaceId],
  );

  const statusQuery = useQuery({
    queryKey,
    queryFn: async () => {
      const status = await codexBankedResetApi.getCodexBankedResetStatus(
        accountId,
        false,
      );
      if (workspaceId && status.workspaceId !== workspaceId) {
        throw new Error(t("codexBankedReset.workspaceChanged"));
      }
      return status;
    },
    staleTime: 60_000,
    retry: false,
  });

  const refreshMutation = useMutation({
    mutationFn: async (target: {
      accountId: string | null | undefined;
      workspaceId: string | null | undefined;
    }) => {
      const status = await codexBankedResetApi.getCodexBankedResetStatus(
        target.accountId,
        true,
      );
      if (target.workspaceId && status.workspaceId !== target.workspaceId) {
        throw new Error(t("codexBankedReset.workspaceChanged"));
      }
      return status;
    },
    onSuccess: (status, target) => {
      queryClient.setQueryData(
        resetQueryKey(
          target.accountId,
          status.workspaceId ?? target.workspaceId,
        ),
        status,
      );
    },
  });

  const availableCredits = React.useMemo(() => {
    return (statusQuery.data?.credits ?? []).filter((credit) => {
      const status = credit.status?.toLowerCase();
      return status === "available";
    });
  }, [statusQuery.data?.credits]);

  React.useEffect(() => {
    if (availableCredits.length === 0) {
      setSelectedCreditId("");
      return;
    }
    if (!availableCredits.some((credit) => credit.id === selectedCreditId)) {
      setSelectedCreditId(availableCredits[0].id);
    }
  }, [availableCredits, selectedCreditId]);

  const consumeMutation = useMutation({
    mutationFn: (creditId: string) =>
      codexBankedResetApi.consumeCodexBankedReset(accountId, creditId),
    onSuccess: async (result) => {
      const success = result.code === "reset" || !result.code;
      toast[success ? "success" : "error"](consumeMessage(result.code));
      await queryClient.invalidateQueries({ queryKey });
      if (success) {
        await subscriptionApi.refreshOauthQuota(
          "codex_oauth",
          accountId ?? null,
        );
        await queryClient.invalidateQueries({
          queryKey: ["codex_oauth", "quota", accountId ?? "default"],
        });
      }
    },
    onError: (error) => {
      toast.error(error instanceof Error ? error.message : String(error));
    },
  });

  const handleConsume = () => {
    if (!selectedCreditId) return;
    setConsumeConfirmOpen(true);
  };

  const handleConsumeConfirm = () => {
    if (!selectedCreditId) {
      setConsumeConfirmOpen(false);
      return;
    }
    consumeMutation.mutate(selectedCreditId);
    setConsumeConfirmOpen(false);
  };

  const selectedCredit = availableCredits.find(
    (credit) => credit.id === selectedCreditId,
  );
  const selectedCreditIndex = availableCredits.findIndex(
    (credit) => credit.id === selectedCreditId,
  );
  const selectedCreditTimeSummary = selectedCredit
    ? creditTimeSummary(selectedCredit, t, i18n.language)
    : null;
  const availableCount =
    statusQuery.data?.availableCount ?? availableCredits.length;

  const consumeMessage = (code?: string | null) => {
    if (code === "nothing_to_reset")
      return t("codexBankedReset.nothingToReset");
    if (code === "already_redeemed")
      return t("codexBankedReset.alreadyRedeemed");
    if (code === "no_credit") return t("codexBankedReset.noCredit");
    return t("codexBankedReset.consumeSuccess");
  };

  const refreshTargetsCurrentAccount =
    (refreshMutation.variables?.accountId ?? null) === (accountId ?? null) &&
    (refreshMutation.variables?.workspaceId ?? null) === (workspaceId ?? null);
  const isRefreshing =
    statusQuery.isFetching ||
    (refreshMutation.isPending && refreshTargetsCurrentAccount);

  return (
    <section className="space-y-3 rounded-md border border-border-default bg-muted/20 p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-2">
          <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-emerald-500 text-white">
            <Gift className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <Label className="text-sm font-medium">
              {t("codexBankedReset.title")}
            </Label>
            <p className="mt-1 text-xs text-muted-foreground">
              {t("codexBankedReset.description")}
            </p>
          </div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-8 w-8 shrink-0"
          onClick={() => refreshMutation.mutate({ accountId, workspaceId })}
          disabled={isRefreshing}
          title={t("common.refresh")}
          aria-label={t("common.refresh")}
        >
          <RefreshCw
            className={`h-4 w-4 ${isRefreshing ? "animate-spin" : ""}`}
          />
        </Button>
      </div>

      {statusQuery.isLoading ? (
        <div className="flex items-center justify-center gap-2 py-5 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("common.loading")}
        </div>
      ) : statusQuery.error ? (
        <div className="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          {statusQuery.error instanceof Error
            ? statusQuery.error.message
            : String(statusQuery.error)}
        </div>
      ) : (
        <div className="space-y-3 rounded-md border border-border-default bg-background p-3">
          <div className="flex items-center justify-between gap-3">
            <div>
              <div className="text-xs text-muted-foreground">
                {t("codexBankedReset.available")}
              </div>
              <div className="mt-1 flex items-end gap-1">
                <span className="text-3xl font-semibold leading-none">
                  {availableCount}
                </span>
                <span className="text-xs text-muted-foreground">
                  {t("codexBankedReset.availableUnit")}
                </span>
              </div>
            </div>
            <RotateCcw className="h-5 w-5 text-emerald-600" />
          </div>

          {availableCredits.length > 0 ? (
            <div className="space-y-2">
              <Label className="text-xs text-muted-foreground">
                {t("codexBankedReset.selectedCredit")}
              </Label>
              <Select
                value={selectedCreditId}
                onValueChange={setSelectedCreditId}
              >
                <SelectTrigger>
                  <SelectValue placeholder={t("codexBankedReset.selectCredit")}>
                    {selectedCredit
                      ? creditLabel(selectedCredit, selectedCreditIndex, t)
                      : undefined}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent
                  position="item-aligned"
                  className="w-[22rem] max-w-[calc(100vw-2rem)]"
                >
                  {availableCredits.map((credit, index) => {
                    const summary = creditTimeSummary(
                      credit,
                      t,
                      i18n.language,
                    );
                    return (
                      <SelectItem
                        key={credit.id}
                        value={credit.id}
                        className="py-2"
                      >
                        <div className="flex min-w-0 flex-col gap-0.5">
                          <span className="truncate">
                            {creditLabel(credit, index, t)}
                          </span>
                          {summary && (
                            <span className="truncate text-xs text-muted-foreground">
                              {summary}
                            </span>
                          )}
                        </div>
                      </SelectItem>
                    );
                  })}
                </SelectContent>
              </Select>
              {selectedCredit && (
                <div className="rounded-md bg-muted px-3 py-2 text-xs text-muted-foreground">
                  <p>
                    {selectedCredit.description ||
                      t("codexBankedReset.creditFallbackDescription")}
                  </p>
                  {selectedCreditTimeSummary && (
                    <p className="mt-1 font-mono">
                      {selectedCreditTimeSummary}
                    </p>
                  )}
                </div>
              )}
            </div>
          ) : (
            <p className="rounded-md border border-dashed border-border-default px-3 py-4 text-sm text-muted-foreground">
              {t("codexBankedReset.noCredits")}
            </p>
          )}

          <Button
            type="button"
            className="w-full"
            onClick={handleConsume}
            disabled={!selectedCreditId || consumeMutation.isPending}
          >
            {consumeMutation.isPending ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <RotateCcw className="mr-2 h-4 w-4" />
            )}
            {t("codexBankedReset.useReset")}
          </Button>
        </div>
      )}
      <ConfirmDialog
        isOpen={consumeConfirmOpen}
        title={t("codexBankedReset.useReset")}
        message={t("codexBankedReset.consumeConfirm")}
        confirmText={t("codexBankedReset.useReset")}
        cancelText={t("common.cancel")}
        variant="destructive"
        onConfirm={handleConsumeConfirm}
        onCancel={() => setConsumeConfirmOpen(false)}
      />
    </section>
  );
};

function creditLabel(
  credit: CodexBankedResetCredit,
  index: number,
  t: ReturnType<typeof useTranslation>["t"],
) {
  const safeIndex = index >= 0 ? index : 0;
  return `${credit.title || t("codexBankedReset.creditFallbackTitle")} #${
    safeIndex + 1
  }`;
}

function creditTimeSummary(
  credit: CodexBankedResetCredit,
  t: ReturnType<typeof useTranslation>["t"],
  locale: string,
) {
  const grantedAt = formatCreditDate(credit.grantedAt, locale);
  const expiresAt = formatCreditDate(credit.expiresAt, locale);
  const parts: string[] = [];
  if (grantedAt) {
    parts.push(`${t("codexBankedReset.creditGrantedAt")} ${grantedAt}`);
  }
  if (expiresAt) {
    parts.push(`${t("codexBankedReset.creditExpiresAt")} ${expiresAt}`);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

function formatCreditDate(
  value: string | number | null | undefined,
  locale: string,
) {
  if (!value) return null;
  const text = String(value).trim();
  if (!text) return null;
  const numeric = Number(text);
  const date = Number.isFinite(numeric)
    ? new Date(numeric > 10_000_000_000 ? numeric : numeric * 1000)
    : new Date(text);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat(locale, {
    year: "2-digit",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

export default CodexBankedResetPanel;
