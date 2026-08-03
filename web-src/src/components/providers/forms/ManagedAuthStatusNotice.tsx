import { Loader2, RefreshCw, TriangleAlert } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";

interface ManagedAuthStatusNoticeProps {
  className?: string;
  title: string;
  error?: string | null;
  isError: boolean;
  isFetching: boolean;
  onRetry: () => void;
}

export function ManagedAuthStatusNotice({
  className,
  title,
  error,
  isError,
  isFetching,
  onRetry,
}: ManagedAuthStatusNoticeProps) {
  const { t } = useTranslation();

  return (
    <div className={`space-y-4 ${className || ""}`}>
      <div className="flex items-center justify-between">
        <Label>{title}</Label>
        <Badge variant={isError ? "destructive" : "secondary"}>
          {isError ? t("common.error") : t("common.loading")}
        </Badge>
      </div>
      {isError ? (
        <div
          className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-destructive/40 p-3 text-sm text-destructive"
          role="alert"
        >
          <span className="flex min-w-0 items-center gap-2">
            <TriangleAlert className="h-4 w-4 shrink-0" />
            {error ||
              t("common.authStatusLoadFailed", {
                defaultValue: "Failed to load authentication status.",
              })}
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={isFetching}
            onClick={onRetry}
          >
            {isFetching ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <RefreshCw className="mr-2 h-4 w-4" />
            )}
            {t("common.retry")}
          </Button>
        </div>
      ) : (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("common.loading")}
        </div>
      )}
    </div>
  );
}
