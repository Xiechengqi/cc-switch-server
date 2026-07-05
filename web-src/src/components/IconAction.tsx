import { Loader2 } from "lucide-react";
import { ReactNode } from "react";

import { useI18n } from "@/lib/i18n";

interface IconActionProps {
  title: string;
  disabledTitle?: string;
  children: ReactNode;
  busy?: boolean;
  disabled?: boolean;
  danger?: boolean;
  wrap?: boolean;
  onClick: () => void;
}

export function IconAction({
  title,
  disabledTitle,
  children,
  busy,
  disabled,
  danger,
  wrap = true,
  onClick,
}: IconActionProps) {
  const { tx } = useI18n();
  const translatedTitle = tx(disabled && disabledTitle ? disabledTitle : title);
  const button = (
    <button
      className={danger ? "icon-button danger" : "icon-button"}
      type="button"
      title={wrap ? undefined : translatedTitle}
      aria-label={translatedTitle}
      onClick={onClick}
      disabled={busy || disabled}
    >
      {busy ? <Loader2 size={15} /> : children}
    </button>
  );
  if (!wrap) return button;
  return (
    <span className="icon-action-wrap" title={translatedTitle}>
      {button}
    </span>
  );
}
