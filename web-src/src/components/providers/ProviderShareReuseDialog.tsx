import { useEffect, useState } from "react";
import { Link2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  ClaudeIcon,
  CodexIcon,
  GeminiIcon,
} from "@/components/BrandIcons";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { ShareBindings, ShareReuseCandidate } from "@/lib/api";

interface ProviderShareReuseDialogProps {
  candidates: ShareReuseCandidate[] | null;
  onConfirm: (reuse: boolean, shareId: string) => void;
  onCancel: () => void;
}

function AppIcon({ app }: { app: keyof ShareBindings }) {
  if (app === "claude") return <ClaudeIcon size={16} />;
  if (app === "codex") return <CodexIcon size={16} />;
  return <GeminiIcon size={16} />;
}

export function ProviderShareReuseDialog({
  candidates,
  onConfirm,
  onCancel,
}: ProviderShareReuseDialogProps) {
  const { t } = useTranslation();
  const [reuse, setReuse] = useState(false);
  const [selectedShareId, setSelectedShareId] = useState("");

  useEffect(() => {
    if (candidates?.length) {
      setReuse(false);
      setSelectedShareId(candidates[0].shareId);
    }
  }, [candidates]);

  const selected = candidates?.find(
    (candidate) => candidate.shareId === selectedShareId,
  );

  return (
    <Dialog
      open={Boolean(candidates?.length)}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <DialogContent className="max-w-md">
        <DialogHeader className="space-y-2 border-b-0 bg-transparent pb-0">
          <DialogTitle className="flex items-center gap-2 text-lg">
            <Link2 className="h-5 w-5 text-primary" />
            {t("provider.share.reuseTitle", {
              defaultValue: "发现同一账号或 API key 的远程分享",
            })}
          </DialogTitle>
          <DialogDescription className="text-sm leading-relaxed">
            {t("provider.share.reuseDescription", {
              defaultValue:
                "可以将当前应用加入已有链接，也可以保留为新的独立链接。",
            })}
          </DialogDescription>
        </DialogHeader>

        {candidates && candidates.length > 1 ? (
          <div className="space-y-2 px-6 pt-3">
            <label className="text-sm font-medium">
              {t("provider.share.reuseTarget", {
                defaultValue: "已有分享",
              })}
            </label>
            <Select value={selectedShareId} onValueChange={setSelectedShareId}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {candidates.map((candidate) => (
                  <SelectItem key={candidate.shareId} value={candidate.shareId}>
                    {candidate.shareName}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        ) : null}

        {selected ? (
          <div className="mx-6 mt-3 flex items-center justify-between border-y py-3">
            <div className="min-w-0">
              <p className="truncate text-sm font-medium">{selected.shareName}</p>
              <p className="truncate text-xs text-muted-foreground">
                {selected.subdomain ?? selected.shareId}
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-1.5">
              {selected.apps.map((app) => (
                <span
                  key={app}
                  className="flex h-7 w-7 items-center justify-center rounded-md border bg-muted"
                  title={app}
                >
                  <AppIcon app={app} />
                </span>
              ))}
            </div>
          </div>
        ) : null}

        <label className="flex cursor-pointer items-start gap-2 px-6 pt-2">
          <Checkbox
            checked={reuse}
            onCheckedChange={(value) => setReuse(value === true)}
            className="mt-0.5"
          />
          <span className="text-sm leading-relaxed">
            {t("provider.share.reuseCheckbox", {
              defaultValue: "复用已有 Share URL 和远程分享配置",
            })}
          </span>
        </label>

        <DialogFooter className="border-t-0 bg-transparent pt-3">
          <Button variant="outline" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button
            onClick={() => onConfirm(reuse, selectedShareId)}
            disabled={!selectedShareId}
          >
            {t("common.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
