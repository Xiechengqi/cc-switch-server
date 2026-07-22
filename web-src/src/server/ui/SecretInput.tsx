import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Input, type InputProps } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export function SecretInput({ className, type, ...props }: InputProps) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(false);
  const isPasswordField = type === "password" || type === undefined;

  return (
    <div className="relative">
      <Input
        {...props}
        type={isPasswordField && !visible ? "password" : "text"}
        className={cn(isPasswordField && "pr-10", className)}
      />
      {isPasswordField ? (
        <button
          type="button"
          tabIndex={-1}
          aria-label={visible ? t("common.hide") : t("common.show")}
          className="absolute inset-y-0 right-0 inline-flex w-9 items-center justify-center text-muted-foreground transition-colors hover:text-foreground"
          onClick={() => setVisible((current) => !current)}
        >
          {visible ? (
            <EyeOff className="h-4 w-4" />
          ) : (
            <Eye className="h-4 w-4" />
          )}
        </button>
      ) : null}
    </div>
  );
}
