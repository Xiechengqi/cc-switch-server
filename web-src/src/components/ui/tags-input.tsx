import * as React from "react";
import { Crown, X } from "lucide-react";
import { cn } from "@/lib/utils";

export interface EmailTagsInputProps {
  value: string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
  placeholder?: string;
  invalid?: boolean;
  inputId?: string;
  onPromote?: (email: string) => void;
  promotableEmails?: string[];
  promoteLabel?: string;
}

function parseEmails(raw: string): string[] {
  return raw
    .split(/[\s,;]+/)
    .map((value) => value.trim().toLowerCase())
    .filter(Boolean);
}

export function EmailTagsInput({
  value,
  onChange,
  disabled,
  placeholder,
  invalid,
  inputId,
  onPromote,
  promotableEmails,
  promoteLabel,
}: EmailTagsInputProps) {
  const [draft, setDraft] = React.useState("");
  const inputRef = React.useRef<HTMLInputElement>(null);
  const promotableSet = React.useMemo(
    () => new Set(promotableEmails ?? []),
    [promotableEmails],
  );

  const commit = (raw: string) => {
    const parts = parseEmails(raw);
    setDraft("");
    if (!parts.length) return;
    const next = [...value];
    for (const part of parts) {
      if (!next.includes(part)) next.push(part);
    }
    if (next.length !== value.length) onChange(next);
  };

  const removeAt = (idx: number) => {
    onChange(value.filter((_, i) => i !== idx));
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" || event.key === ",") {
      event.preventDefault();
      commit(draft);
    } else if (event.key === "Backspace" && draft === "" && value.length) {
      event.preventDefault();
      removeAt(value.length - 1);
    }
  };

  return (
    <div
      className={cn(
        "flex min-h-9 w-full flex-wrap items-center gap-1.5 rounded-md border border-border-default bg-background px-2 py-1.5 text-sm shadow-sm transition-colors focus-within:ring-2 focus-within:ring-blue-500/20 dark:focus-within:ring-blue-400/20",
        invalid && "border-destructive focus-within:ring-destructive/20",
        disabled && "cursor-not-allowed opacity-50",
      )}
      onClick={() => inputRef.current?.focus()}
    >
      {value.map((email, idx) => {
        const canPromote =
          !disabled && Boolean(onPromote) && promotableSet.has(email);
        return (
          <span
            key={email}
            className="inline-flex max-w-full items-center gap-1 rounded-md bg-secondary px-2 py-0.5 text-xs font-medium text-secondary-foreground"
          >
            <span className="min-w-0 truncate">{email}</span>
            {canPromote ? (
              <button
                type="button"
                className="rounded-sm p-0.5 text-muted-foreground hover:bg-background/70 hover:text-amber-600"
                aria-label={`${promoteLabel ?? "Set as owner"}: ${email}`}
                title={promoteLabel ?? "Set as owner"}
                onClick={(event) => {
                  event.stopPropagation();
                  onPromote?.(email);
                }}
              >
                <Crown className="h-3 w-3" />
              </button>
            ) : null}
            {disabled ? null : (
              <button
                type="button"
                className="rounded-sm p-0.5 hover:bg-background/70"
                aria-label={`Remove ${email}`}
                onClick={(event) => {
                  event.stopPropagation();
                  removeAt(idx);
                }}
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </span>
        );
      })}
      <input
        ref={inputRef}
        id={inputId}
        value={draft}
        disabled={disabled}
        className="h-6 min-w-[8rem] flex-1 bg-transparent text-foreground placeholder:text-muted-foreground focus:outline-none disabled:cursor-not-allowed"
        placeholder={value.length ? "" : placeholder}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={handleKeyDown}
        onBlur={() => commit(draft)}
        onPaste={(event) => {
          const text = event.clipboardData.getData("text");
          if (/[\s,;]/.test(text)) {
            event.preventDefault();
            commit(text);
          }
        }}
      />
    </div>
  );
}
