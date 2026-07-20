import { useEffect, useMemo, useState } from "react";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import type {
  ShareTokenPeriod,
  ShareUserGrant,
  ShareUserGrantMap,
  ShareUserPolicy,
} from "@/lib/api/share";
import { isValidShareEmail } from "@/utils/shareFormUtils";

type PolicyDraft = {
  email: string;
  parallelLimit: string;
  tokenLimit: string;
  tokenPeriod: ShareTokenPeriod;
  expiresAt: string;
};

type ShareUserGrantsEditorProps = {
  value: ShareUserGrantMap;
  ownerEmail: string;
  defaultPolicy: ShareUserPolicy;
  protectedEmails?: ReadonlySet<string>;
  disabled?: boolean;
  onChange: (value: ShareUserGrantMap) => void;
};

function toLocalDateTime(value: number | undefined) {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function policyDraft(email: string, policy: ShareUserPolicy): PolicyDraft {
  return {
    email,
    parallelLimit: policy.parallelLimit == null ? "" : String(policy.parallelLimit),
    tokenLimit: policy.tokenLimit == null ? "" : String(policy.tokenLimit),
    tokenPeriod: policy.tokenPeriod ?? "lifetime",
    expiresAt: toLocalDateTime(policy.expiresAt),
  };
}

function displayLimit(value: number | undefined, unlimited: string) {
  return value == null ? unlimited : value.toLocaleString();
}

function displayExpiry(value: number | undefined, permanent: string) {
  return value == null ? permanent : new Date(value).toLocaleString();
}

export function ShareUserGrantsEditor({
  value,
  ownerEmail,
  defaultPolicy,
  protectedEmails,
  disabled,
  onChange,
}: ShareUserGrantsEditorProps) {
  const { t } = useTranslation();
  const normalizedOwner = ownerEmail.trim().toLowerCase();
  const [editingEmail, setEditingEmail] = useState<string | null>(null);
  const [draft, setDraft] = useState<PolicyDraft | null>(null);

  const grants = useMemo(
    () =>
      Object.values(value)
        .filter((grant) => grant.active !== false)
        .sort((left, right) => {
          if (left.role === "owner") return -1;
          if (right.role === "owner") return 1;
          return left.email.localeCompare(right.email);
        }),
    [value],
  );

  useEffect(() => {
    if (!normalizedOwner || value[normalizedOwner]) return;
    onChange({
      ...value,
      [normalizedOwner]: {
        email: normalizedOwner,
        role: "owner",
        active: true,
        policy: { ...defaultPolicy },
      },
    });
  }, [defaultPolicy, normalizedOwner, onChange, value]);

  const openAdd = () => {
    setEditingEmail(null);
    setDraft(policyDraft("", defaultPolicy));
  };

  const openEdit = (grant: ShareUserGrant) => {
    setEditingEmail(grant.email);
    setDraft(policyDraft(grant.email, grant.policy));
  };

  const saveDraft = () => {
    if (!draft) return;
    const email = draft.email.trim().toLowerCase();
    const parallelLimit = draft.parallelLimit.trim()
      ? Number(draft.parallelLimit)
      : undefined;
    const tokenLimit = draft.tokenLimit.trim()
      ? Number(draft.tokenLimit)
      : undefined;
    const expiresAt = draft.expiresAt
      ? new Date(draft.expiresAt).getTime()
      : undefined;
    if (
      !isValidShareEmail(email) ||
      (editingEmail == null && Boolean(value[email]?.active)) ||
      parallelLimit === 0 ||
      tokenLimit === 0 ||
      (parallelLimit != null && (!Number.isInteger(parallelLimit) || parallelLimit < 1)) ||
      (tokenLimit != null && (!Number.isInteger(tokenLimit) || tokenLimit < 1)) ||
      (expiresAt != null && !Number.isFinite(expiresAt))
    ) {
      return;
    }
    const previous = value[editingEmail ?? email];
    const next: ShareUserGrant = {
      ...previous,
      email,
      role: email === normalizedOwner ? "owner" : "shareto",
      active: true,
      policy: {
        parallelLimit,
        tokenLimit,
        tokenPeriod: draft.tokenPeriod,
        expiresAt,
      },
    };
    const updated = { ...value };
    if (editingEmail && editingEmail !== email) delete updated[editingEmail];
    updated[email] = next;
    onChange(updated);
    setDraft(null);
  };

  const unlimited = t("share.unlimited", { defaultValue: "无限" });
  const permanent = t("share.permanent", { defaultValue: "永久" });
  const periodLabels: Record<ShareTokenPeriod, string> = {
    lifetime: t("share.userLimit.periodLifetime", { defaultValue: "累计" }),
    day: t("share.userLimit.periodDay", { defaultValue: "每天" }),
    week: t("share.userLimit.periodWeek", { defaultValue: "每周" }),
    calendarMonth: t("share.userLimit.periodMonth", { defaultValue: "每月" }),
  };

  return (
    <div className="space-y-2 md:col-span-2">
      <div className="flex items-center justify-between gap-3">
        <div>
          <Label>{t("share.userLimit.title", { defaultValue: "用户限制" })}</Label>
          <p className="mt-1 text-xs text-muted-foreground">
            {t("share.userLimit.hint", {
              defaultValue: "总 Share 限制始终生效；每个用户还受自己的限制约束。",
            })}
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={disabled}
          onClick={openAdd}
        >
          <Plus className="mr-1.5 h-4 w-4" />
          {t("share.userLimit.add", { defaultValue: "添加用户" })}
        </Button>
      </div>

      <div className="overflow-x-auto rounded-md border border-border-default">
        <Table className="min-w-[720px]">
          <TableHeader>
            <TableRow>
              <TableHead className="h-9 px-3">Email</TableHead>
              <TableHead className="h-9 px-3">{t("share.parallelLimit", { defaultValue: "并发" })}</TableHead>
              <TableHead className="h-9 px-3">Token</TableHead>
              <TableHead className="h-9 px-3">{t("share.expiration", { defaultValue: "到期" })}</TableHead>
              <TableHead className="h-9 w-20 px-3" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {grants.map((grant) => (
              <TableRow key={grant.email}>
                <TableCell className="px-3 py-2">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="truncate">{grant.email}</span>
                    {grant.role === "owner" ? <Badge variant="secondary">Owner</Badge> : null}
                  </div>
                </TableCell>
                <TableCell className="px-3 py-2">{displayLimit(grant.policy.parallelLimit, unlimited)}</TableCell>
                <TableCell className="px-3 py-2">
                  {displayLimit(grant.policy.tokenLimit, unlimited)} · {periodLabels[grant.policy.tokenPeriod]}
                </TableCell>
                <TableCell className="px-3 py-2">{displayExpiry(grant.policy.expiresAt, permanent)}</TableCell>
                <TableCell className="px-3 py-2">
                  <div className="flex justify-end gap-1">
                    <Button type="button" variant="ghost" size="icon" disabled={disabled} onClick={() => openEdit(grant)} title={t("common.edit", { defaultValue: "编辑" })}>
                      <Pencil className="h-4 w-4" />
                    </Button>
                    {grant.role !== "owner" && !protectedEmails?.has(grant.email) ? (
                      <Button type="button" variant="ghost" size="icon" disabled={disabled} onClick={() => {
                        const updated = { ...value };
                        delete updated[grant.email];
                        onChange(updated);
                      }} title={t("common.delete", { defaultValue: "删除" })}>
                        <Trash2 className="h-4 w-4 text-destructive" />
                      </Button>
                    ) : null}
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      <Dialog open={draft != null} onOpenChange={(open) => !open && setDraft(null)}>
        {/* Provider editing uses a z-[60] FullScreenPanel. */}
        <DialogContent className="max-w-xl" zIndex="top">
          <DialogHeader>
            <DialogTitle>
              {editingEmail
                ? t("share.userLimit.edit", { defaultValue: "编辑用户限制" })
                : t("share.userLimit.add", { defaultValue: "添加用户" })}
            </DialogTitle>
          </DialogHeader>
          {draft ? (
            <div className="grid gap-4 overflow-y-auto px-6 py-5 sm:grid-cols-2">
              <div className="space-y-2 sm:col-span-2">
                <Label htmlFor="share-user-email">Email</Label>
                <Input id="share-user-email" type="email" disabled={editingEmail != null} value={draft.email} onChange={(event) => setDraft({ ...draft, email: event.target.value })} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-user-parallel">{t("share.parallelLimit", { defaultValue: "并发限额" })}</Label>
                <Input id="share-user-parallel" type="number" min={1} placeholder={unlimited} value={draft.parallelLimit} onChange={(event) => setDraft({ ...draft, parallelLimit: event.target.value })} />
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-user-token">{t("share.tokenLimit", { defaultValue: "Token 限额" })}</Label>
                <Input id="share-user-token" type="number" min={1} placeholder={unlimited} value={draft.tokenLimit} onChange={(event) => setDraft({ ...draft, tokenLimit: event.target.value })} />
              </div>
              <div className="space-y-2">
                <Label>{t("share.userLimit.period", { defaultValue: "Token 周期" })}</Label>
                <Select value={draft.tokenPeriod} onValueChange={(tokenPeriod: ShareTokenPeriod) => setDraft({ ...draft, tokenPeriod })}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent className="z-[120]">
                    {(Object.keys(periodLabels) as ShareTokenPeriod[]).map((period) => <SelectItem key={period} value={period}>{periodLabels[period]}</SelectItem>)}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-2">
                <Label htmlFor="share-user-expiry">{t("share.expiration", { defaultValue: "到期时间" })}</Label>
                <Input id="share-user-expiry" type="datetime-local" value={draft.expiresAt} onChange={(event) => setDraft({ ...draft, expiresAt: event.target.value })} />
              </div>
            </div>
          ) : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDraft(null)}>{t("common.cancel", { defaultValue: "取消" })}</Button>
            <Button type="button" onClick={saveDraft}>{t("common.save", { defaultValue: "保存" })}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
