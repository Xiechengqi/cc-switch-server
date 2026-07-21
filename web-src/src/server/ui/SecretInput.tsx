import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";

import { Input, type InputProps } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export function SecretInput({ className, type, ...props }: InputProps) {
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
          aria-label={visible ? "隐藏" : "显示"}
          className="absolute inset-y-0 right-0 inline-flex w-9 items-center justify-center text-muted-foreground transition-colors hover:text-foreground"
          onClick={() => setVisible((current) => !current)}
        >
          {visible ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
        </button>
      ) : null}
    </div>
  );
}
