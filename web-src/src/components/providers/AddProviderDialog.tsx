import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus } from "lucide-react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { Button } from "@/components/ui/button";
import { useUnsavedChangesGuard } from "@/hooks/useUnsavedChangesGuard";
import type {
  ProviderCredentialPatches,
  ProviderCustomBinding,
} from "@/lib/api/providers";
import type { CoreProviderApp } from "@/server/providerRegistry";
import {
  ServerProviderForm,
  type ServerProviderFormValues,
} from "@/server/providers/editor/ServerProviderForm";
import type { Provider } from "@/types";

interface AddProviderDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  appId: CoreProviderApp;
  onSubmit: (
    provider: Omit<Provider, "id"> & {
      profileId?: string;
      customBinding?: ProviderCustomBinding;
      credentialPatches?: ProviderCredentialPatches;
    },
  ) => Promise<void> | void;
}

export function AddProviderDialog({
  open,
  onOpenChange,
  appId,
  onSubmit,
}: AddProviderDialogProps) {
  const { t } = useTranslation();
  const [isFormSubmitting, setIsFormSubmitting] = useState(false);
  const [isFormDirty, setIsFormDirty] = useState(false);
  const closePanel = useCallback(() => onOpenChange(false), [onOpenChange]);
  const closeGuard = useUnsavedChangesGuard({
    active: open,
    dirty: isFormDirty && !isFormSubmitting,
    onClose: closePanel,
  });

  const handleSubmit = useCallback(
    async (values: ServerProviderFormValues) => {
      const settingsConfig = JSON.parse(values.settingsConfig) as Record<
        string,
        unknown
      >;
      await onSubmit({
        name: values.name.trim(),
        notes: values.notes?.trim() || undefined,
        websiteUrl: values.websiteUrl?.trim() || undefined,
        settingsConfig,
        icon: values.icon?.trim() || undefined,
        iconColor: values.iconColor?.trim() || undefined,
        ...(values.presetCategory ? { category: values.presetCategory } : {}),
        ...(values.meta ? { meta: values.meta } : {}),
        ...(values.profileId ? { profileId: values.profileId } : {}),
        ...(values.customBinding
          ? { customBinding: values.customBinding }
          : {}),
        ...(values.credentialPatches
          ? { credentialPatches: values.credentialPatches }
          : {}),
      });
      closePanel();
    },
    [closePanel, onSubmit],
  );

  return (
    <>
      <FullScreenPanel
        isOpen={open}
        title={t("provider.addNewProvider")}
        onClose={closeGuard.requestClose}
        contentClassName="pt-3"
        footer={
          <>
            <span className="mr-auto min-w-0 truncate text-xs text-muted-foreground">
              {t("provider.addFooterHint")}
            </span>
            <Button variant="outline" onClick={closeGuard.requestClose}>
              {t("common.cancel")}
            </Button>
            <Button
              type="submit"
              form="provider-form"
              disabled={isFormSubmitting}
            >
              <Plus className="mr-2 h-4 w-4" />
              {t("common.add")}
            </Button>
          </>
        }
      >
        <ServerProviderForm
          appId={appId}
          submitLabel={t("common.add")}
          onSubmit={handleSubmit}
          onCancel={closeGuard.requestClose}
          onSubmittingChange={setIsFormSubmitting}
          onDirtyChange={setIsFormDirty}
          onUnsavedChange={setIsFormDirty}
          showButtons={false}
        />
      </FullScreenPanel>
      <ConfirmDialog
        isOpen={closeGuard.confirmOpen}
        title="放弃未保存的更改？"
        message="当前供应商配置尚未保存。关闭后，这些更改将丢失。"
        confirmText="放弃更改"
        cancelText="继续编辑"
        variant="destructive"
        zIndex="top"
        onConfirm={closeGuard.discardAndClose}
        onCancel={closeGuard.keepEditing}
      />
    </>
  );
}
