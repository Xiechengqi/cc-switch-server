import { useEffect, useState } from "react";
import { AlertTriangle, Info } from "lucide-react";

import { useI18n } from "@/lib/i18n";

interface ConfirmDialogProps {
  isOpen: boolean;
  title: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  variant?: "destructive" | "info";
  zIndex?: "base" | "nested" | "alert" | "top";
  checkboxLabel?: string;
  checkboxDefaultChecked?: boolean;
  onConfirm: (checkboxChecked: boolean) => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  isOpen,
  title,
  message,
  confirmText,
  cancelText,
  variant = "destructive",
  zIndex = "alert",
  checkboxLabel,
  checkboxDefaultChecked = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const { t } = useI18n();
  const [checkboxChecked, setCheckboxChecked] = useState(checkboxDefaultChecked);

  useEffect(() => {
    if (isOpen) {
      setCheckboxChecked(checkboxDefaultChecked);
    }
  }, [isOpen, checkboxDefaultChecked]);

  useEffect(() => {
    if (!isOpen) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onCancel();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [isOpen, onCancel]);

  if (!isOpen) return null;

  const IconComponent = variant === "info" ? Info : AlertTriangle;
  const dialogClass = ["confirm-dialog", `confirm-dialog-${variant}`, `confirm-dialog-z-${zIndex}`].join(" ");

  return (
    <div className={dialogClass} role="presentation" onMouseDown={onCancel}>
      <section
        className="confirm-dialog-panel"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="confirm-dialog-title"
        aria-describedby="confirm-dialog-description"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="confirm-dialog-header">
          <IconComponent size={20} />
          <h2 id="confirm-dialog-title">{title}</h2>
        </header>
        <p id="confirm-dialog-description" className="confirm-dialog-message">
          {message}
        </p>
        {checkboxLabel ? (
          <label className="confirm-dialog-checkbox">
            <input
              type="checkbox"
              checked={checkboxChecked}
              onChange={(event) => setCheckboxChecked(event.target.checked)}
            />
            <span>{checkboxLabel}</span>
          </label>
        ) : null}
        <footer className="confirm-dialog-footer">
          <button className="secondary-button" type="button" onClick={onCancel}>
            {cancelText || t("common.cancel")}
          </button>
          <button
            className={variant === "info" ? "primary-button" : "danger-button"}
            type="button"
            onClick={() => onConfirm(checkboxLabel ? checkboxChecked : false)}
          >
            {confirmText || t("common.confirm")}
          </button>
        </footer>
      </section>
    </div>
  );
}
