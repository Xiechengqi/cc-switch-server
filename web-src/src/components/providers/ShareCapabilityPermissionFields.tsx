import { useTranslation } from "react-i18next";
import {
  capabilityPermissionMode,
  capabilityPermissionChange,
  OPERATION_LABELS,
  type ShareOperation,
  type CapabilityPermissionMode,
} from "@/lib/share-capabilities";

export function ShareCapabilityPermissionFields({
  value,
  available,
  disabled,
  onChange,
}: {
  value?: ShareOperation[];
  available: readonly ShareOperation[];
  disabled?: boolean;
  onChange: (operations: ShareOperation[] | undefined) => void;
}) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith("zh");
  const mode = capabilityPermissionMode(value);
  return (
    <fieldset disabled={disabled} className="min-w-0 space-y-2 sm:col-span-2">
      <legend className="mb-2 text-sm font-medium">
        {zh ? "扩展能力" : "Extended capabilities"}
      </legend>
      <select
        className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
        aria-label={zh ? "扩展能力权限" : "Extension permission"}
        value={mode}
        onChange={(event) =>
          onChange(
            capabilityPermissionChange(
              event.target.value as CapabilityPermissionMode,
              value,
              available,
            ),
          )
        }
      >
        <option value="inherit">
          {zh ? "跟随 Share 设置（默认）" : "Follow Share settings (default)"}
        </option>
        <option value="custom" disabled={!available.length}>
          {zh ? "只允许指定能力" : "Allow selected extensions"}
        </option>
        <option value="disabled">
          {zh ? "不允许扩展能力" : "Disable extensions"}
        </option>
      </select>
      {mode === "custom" ? (
        <div className="flex flex-wrap gap-x-4 gap-y-2 text-sm">
          {available.map((operation) => (
            <label key={operation} className="flex items-center gap-2">
              <input
                type="checkbox"
                className="h-4 min-h-4 w-4 shrink-0 p-0 accent-primary"
                checked={value?.includes(operation) ?? false}
                onChange={(event) =>
                  onChange(
                    event.target.checked
                      ? [...(value ?? []), operation]
                      : (value?.filter((op) => op !== operation) ?? []),
                  )
                }
              />
              {OPERATION_LABELS[operation][zh ? 0 : 1]}
            </label>
          ))}
        </div>
      ) : null}
      <p className="text-xs text-muted-foreground">
        {mode === "inherit"
          ? zh
            ? "此用户跟随 Share 以后对扩展能力的调整。"
            : "This user follows future Share extension changes."
          : mode === "disabled"
            ? zh
              ? "基础模型交互及理解输入仍按原权限和额度使用。"
              : "Basic model interaction and understanding inputs still use existing access and quotas."
            : zh
              ? "仅开放所选项，并始终受 Share 权限约束。"
              : "Only the selected extensions are allowed, within Share permissions."}
      </p>
    </fieldset>
  );
}
