import { useEffect, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { AlertTriangle, Info, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";

interface ConfirmDialogProps {
  isOpen: boolean;
  title: string;
  message: string;
  /** 完整展示的关键文本（如邮箱），避免在 message 里被截断 */
  highlight?: string;
  confirmText?: string;
  cancelText?: string;
  variant?: "destructive" | "info";
  zIndex?: "base" | "nested" | "alert" | "top";
  /** 可选勾选项：提供 label 即显示，勾选状态经 onConfirm 参数回传 */
  checkboxLabel?: string;
  checkboxDefaultChecked?: boolean;
  confirmDisabled?: boolean;
  onConfirm: (checkboxChecked: boolean) => void | Promise<void>;
  onCancel: () => void;
}

export function ConfirmDialog({
  isOpen,
  title,
  message,
  highlight,
  confirmText,
  cancelText,
  variant = "destructive",
  zIndex = "alert",
  checkboxLabel,
  checkboxDefaultChecked = false,
  confirmDisabled = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const { t } = useTranslation();
  const [checkboxChecked, setCheckboxChecked] = useState(
    checkboxDefaultChecked,
  );
  const [confirming, setConfirming] = useState(false);

  useEffect(() => {
    if (isOpen) {
      setCheckboxChecked(checkboxDefaultChecked);
    } else {
      setConfirming(false);
    }
  }, [isOpen, checkboxDefaultChecked]);

  const handleConfirm = async () => {
    if (confirming || confirmDisabled) return;
    setConfirming(true);
    try {
      await onConfirm(checkboxLabel ? checkboxChecked : false);
    } catch (error) {
      console.error("[ConfirmDialog] Confirmation failed", error);
    } finally {
      setConfirming(false);
    }
  };

  const IconComponent = variant === "info" ? Info : AlertTriangle;
  const iconClass =
    variant === "info" ? "h-5 w-5 text-blue-500" : "h-5 w-5 text-destructive";

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open && !confirming) {
          onCancel();
        }
      }}
    >
      <DialogContent className="max-w-sm" zIndex={zIndex}>
        <DialogHeader className="space-y-3 border-b-0 bg-transparent pb-0">
          <DialogTitle className="flex items-center gap-2 text-lg font-semibold">
            <IconComponent className={iconClass} />
            {title}
          </DialogTitle>
          <DialogDescription className="whitespace-pre-line text-sm leading-relaxed">
            {message}
          </DialogDescription>
        </DialogHeader>
        {highlight ? (
          <p className="break-all px-6 text-sm font-medium leading-relaxed text-foreground">
            {highlight}
          </p>
        ) : null}
        {checkboxLabel ? (
          <label className="flex cursor-pointer select-none items-start gap-2 px-6 pt-3">
            <Checkbox
              checked={checkboxChecked}
              onCheckedChange={(value) => setCheckboxChecked(value === true)}
              className="mt-0.5"
            />
            <span className="text-sm leading-relaxed">{checkboxLabel}</span>
          </label>
        ) : null}
        <DialogFooter className="flex gap-2 border-t-0 bg-transparent pt-2 sm:justify-end">
          <Button variant="outline" onClick={onCancel} disabled={confirming}>
            {cancelText || t("common.cancel")}
          </Button>
          <Button
            variant={variant === "info" ? "default" : "destructive"}
            disabled={confirming || confirmDisabled}
            onClick={() => void handleConfirm()}
          >
            {confirming ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
            {confirmText || t("common.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
