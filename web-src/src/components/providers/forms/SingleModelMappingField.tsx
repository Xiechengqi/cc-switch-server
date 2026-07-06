import { useTranslation } from "react-i18next";
import type { ReactNode } from "react";
import { FormLabel } from "@/components/ui/form";
import { Button } from "@/components/ui/button";
import { Download, Loader2 } from "lucide-react";
import { ModelInputWithFetch } from "./shared";
import type { FetchedModel } from "@/lib/api/model-fetch";

interface SingleModelMappingFieldProps {
  id: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  fetchedModels?: FetchedModel[];
  isLoading?: boolean;
  onFetchModels?: () => void;
  input?: ReactNode;
  className?: string;
}

export function SingleModelMappingField({
  id,
  value,
  onChange,
  placeholder,
  fetchedModels = [],
  isLoading = false,
  onFetchModels,
  input,
  className,
}: SingleModelMappingFieldProps) {
  const { t } = useTranslation();

  return (
    <div
      className={
        className ??
        "space-y-3 rounded-md border border-border-default bg-muted/40 p-3"
      }
    >
      <div className="space-y-1">
        <div className="flex items-center justify-between gap-3">
          <FormLabel>
            {t("providerForm.modelMappingLabel", {
              defaultValue: "模型映射",
            })}
          </FormLabel>
          {onFetchModels && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={onFetchModels}
              disabled={isLoading}
              className="h-7 gap-1"
            >
              {isLoading ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Download className="h-3.5 w-3.5" />
              )}
              {t("providerForm.fetchModels", { defaultValue: "获取模型" })}
            </Button>
          )}
        </div>
        <p className="text-xs leading-relaxed text-muted-foreground">
          {t("providerForm.singleModelMappingSummary", {
            defaultValue:
              "不管客户端请求什么模型，都会统一转发为下面这个真实模型。",
          })}
        </p>
      </div>
      <div className="space-y-2">
        <FormLabel htmlFor={id}>
          {t("providerForm.singleUpstreamModelLabel", {
            defaultValue: "真实模型",
          })}
        </FormLabel>
        {input ?? (
          <ModelInputWithFetch
            id={id}
            value={value}
            onChange={onChange}
            placeholder={
              placeholder ??
              t("providerForm.singleUpstreamModelPlaceholder", {
                defaultValue: "例如: composer-2.5",
              })
            }
            fetchedModels={fetchedModels}
            isLoading={isLoading}
          />
        )}
      </div>
    </div>
  );
}
