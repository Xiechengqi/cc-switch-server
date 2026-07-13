import { useCallback, useEffect, useMemo, useState } from "react";
import { CloudOff, ShieldAlert } from "lucide-react";
import { useTranslation } from "react-i18next";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { PayoutNetwork, PayoutToken } from "@/lib/api";
import {
  useClearPayoutProfileMutation,
  usePayoutProfileQuery,
  useSavePayoutProfileMutation,
} from "@/lib/query";

const NETWORKS: ReadonlyArray<{ id: PayoutNetwork; label: string }> = [
  { id: "eip155:56", label: "BSC" },
  { id: "eip155:8453", label: "Base" },
  { id: "eip155:42161", label: "Arbitrum One" },
];

export interface PayoutProfileFormState {
  dirty: boolean;
  canSave: boolean;
  isSaving: boolean;
  save: () => void;
}

interface PayoutProfileSettingsPanelProps {
  hideSaveButton?: boolean;
  onFormStateChange?: (state: PayoutProfileFormState | null) => void;
}

export function PayoutProfileSettingsPanel({
  hideSaveButton = false,
  onFormStateChange,
}: PayoutProfileSettingsPanelProps) {
  const { t } = useTranslation();
  const { data, isLoading } = usePayoutProfileQuery();
  const saveMutation = useSavePayoutProfileMutation();
  const clearMutation = useClearPayoutProfileMutation();
  const [address, setAddress] = useState("");
  const [token, setToken] = useState<PayoutToken | "">("");
  const [networks, setNetworks] = useState<PayoutNetwork[]>([]);
  const [clearOpen, setClearOpen] = useState(false);
  const [loadedRevision, setLoadedRevision] = useState<number | null>(null);

  useEffect(() => {
    if (!data || data.revision === loadedRevision) return;
    setAddress(data.profile?.address ?? "");
    setToken(data.profile?.token ?? "");
    setNetworks(data.profile?.networks ?? []);
    setLoadedRevision(data.revision);
  }, [data, loadedRevision]);

  const addressValid = /^0x[0-9a-fA-F]{40}$/.test(address);
  const canSave = addressValid && token !== "" && networks.length > 0;
  const dirty =
    address !== (data?.profile?.address ?? "") ||
    token !== (data?.profile?.token ?? "") ||
    networks.join(",") !== (data?.profile?.networks ?? []).join(",");
  const pending = saveMutation.isPending || clearMutation.isPending;
  const syncState = useMemo(() => {
    if (!data) return "idle" as const;
    if (data.sync.lastError) return "failed" as const;
    if (
      data.revision > 0 &&
      data.sync.lastSyncedRevision === data.revision
    ) {
      return "synced" as const;
    }
    return data.revision > 0 ? ("pending" as const) : ("idle" as const);
  }, [data]);

  const toggleNetwork = (network: PayoutNetwork, checked: boolean) => {
    setNetworks((current) =>
      checked
        ? NETWORKS.map((item) => item.id).filter(
            (id) => id === network || current.includes(id),
          )
        : current.filter((id) => id !== network),
    );
  };

  const handleSave = useCallback(() => {
    if (!canSave || !token) return;
    saveMutation.mutate({ address, token, networks });
  }, [address, canSave, networks, saveMutation, token]);

  const handleClear = () => {
    setClearOpen(false);
    clearMutation.mutate();
  };

  useEffect(() => {
    if (!onFormStateChange) return;
    onFormStateChange({
      dirty,
      canSave,
      isSaving: pending,
      save: handleSave,
    });
    return () => onFormStateChange(null);
  }, [canSave, dirty, handleSave, onFormStateChange, pending]);

  if (isLoading && !data) {
    return <p className="text-sm text-muted-foreground">{t("common.loading")}</p>;
  }

  return (
    <div className="space-y-5">
      <Alert className="border-amber-500/40 bg-amber-500/5">
        <ShieldAlert className="h-4 w-4 text-amber-600" />
        <AlertTitle>
          {t("settings.share.payout.publicTitle", {
            defaultValue: "公开收款信息",
          })}
        </AlertTitle>
        <AlertDescription className="space-y-1 text-muted-foreground">
          <p>
            {t("settings.share.payout.publicDescription", {
              defaultValue:
                "该地址、Token 和网络会公开给 Router 及未来接入的 Market。",
            })}
          </p>
          <p className="font-medium text-destructive">
            {t("settings.share.payout.secretWarning", {
              defaultValue: "切勿填写私钥、助记词或钱包密码。",
            })}
          </p>
          <p>
            {t("settings.share.payout.unverifiedWarning", {
              defaultValue: "地址仅为自行声明，当前未验证钱包所有权。",
            })}
          </p>
        </AlertDescription>
      </Alert>

      <div className="grid gap-4 md:grid-cols-2">
        <div className="space-y-2">
          <Label>{t("settings.share.payout.addressType", { defaultValue: "地址类型" })}</Label>
          <Input value="EVM" disabled />
        </div>
        <div className="space-y-2">
          <Label htmlFor="settings-payout-token">
            {t("settings.share.payout.token", { defaultValue: "收款 Token" })}
          </Label>
          <Select value={token} onValueChange={(value) => setToken(value as PayoutToken)} disabled={pending}>
            <SelectTrigger id="settings-payout-token">
              <SelectValue placeholder={t("settings.share.payout.selectToken", { defaultValue: "请选择 Token" })} />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="USDC">USDC</SelectItem>
              <SelectItem value="USDT">USDT</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div className="space-y-2">
        <Label htmlFor="settings-payout-address">
          {t("settings.share.payout.address", { defaultValue: "EVM 收款地址" })}
        </Label>
        <Input
          id="settings-payout-address"
          value={address}
          onChange={(event) => setAddress(event.target.value)}
          placeholder="0x..."
          autoComplete="off"
          spellCheck={false}
          className="font-mono"
          disabled={pending}
        />
        {address && !addressValid ? (
          <p className="text-xs text-destructive">
            {t("settings.share.payout.invalidAddress", {
              defaultValue: "请输入 0x 开头的 40 位十六进制 EVM 地址。",
            })}
          </p>
        ) : null}
      </div>

      <fieldset className="space-y-3">
        <legend className="text-sm font-medium">
          {t("settings.share.payout.networks", { defaultValue: "支持的收款网络" })}
        </legend>
        <div className="flex flex-wrap gap-4">
          {NETWORKS.map((network) => (
            <label key={network.id} className="flex cursor-pointer items-center gap-2 text-sm">
              <Checkbox
                checked={networks.includes(network.id)}
                onCheckedChange={(checked) => toggleNetwork(network.id, checked === true)}
                disabled={pending}
              />
              <span>{network.label}</span>
              <span className="font-mono text-xs text-muted-foreground">{network.id}</span>
            </label>
          ))}
        </div>
      </fieldset>

      <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-muted/20 p-3">
        <div className="flex min-w-0 items-center gap-2 text-sm">
          {syncState === "synced" ? <Badge variant="secondary">{t("settings.share.payout.synced", { defaultValue: "已同步 Router" })}</Badge> : null}
          {syncState === "pending" ? <Badge variant="outline">{t("settings.share.payout.pending", { defaultValue: "等待同步 Router" })}</Badge> : null}
          {syncState === "failed" ? (
            <>
              <CloudOff className="h-4 w-4 shrink-0 text-destructive" />
              <span className="break-all text-destructive" title={data?.sync.lastError ?? undefined}>
                {t("settings.share.payout.syncFailed", { defaultValue: "Router 同步失败，本地配置已保存" })}
              </span>
            </>
          ) : null}
          {syncState === "idle" ? <span className="text-muted-foreground">{t("settings.share.payout.notConfigured", { defaultValue: "尚未配置" })}</span> : null}
        </div>
        <div className="flex gap-2">
          {data?.configured ? (
            <Button type="button" variant="outline" disabled={pending} onClick={() => setClearOpen(true)}>
              {t("settings.share.payout.clear", { defaultValue: "清除" })}
            </Button>
          ) : null}
          {!hideSaveButton ? (
            <Button type="button" disabled={!canSave || !dirty || pending} onClick={handleSave}>
              {t("settings.share.payout.save", { defaultValue: "保存收款信息" })}
            </Button>
          ) : null}
        </div>
      </div>

      <ConfirmDialog
        isOpen={clearOpen}
        title={t("settings.share.payout.clearTitle", { defaultValue: "清除收款信息" })}
        message={t("settings.share.payout.clearDescription", {
          defaultValue: "公开收款地址将被清除，并向 Router 同步删除状态。确定继续吗？",
        })}
        confirmText={t("settings.share.payout.clearConfirm", { defaultValue: "确认清除" })}
        onConfirm={handleClear}
        onCancel={() => setClearOpen(false)}
      />
    </div>
  );
}
