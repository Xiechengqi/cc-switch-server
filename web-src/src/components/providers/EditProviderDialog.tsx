import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Save } from "lucide-react";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { Button } from "@/components/ui/button";
import { useUnsavedChangesGuard } from "@/hooks/useUnsavedChangesGuard";
import type {
  ProviderCredentialPatches,
  ProviderCustomBinding,
  ProviderResource,
} from "@/lib/api/providers";
import type { CoreProviderApp } from "@/server/providerRegistry";
import {
  ServerProviderForm,
  type ServerProviderFormValues,
} from "@/server/providers/editor/ServerProviderForm";
import type { Provider } from "@/types";

interface EditProviderDialogProps {
  open: boolean;
  provider: Provider | null;
  resource?: ProviderResource;
  onOpenChange: (open: boolean) => void;
  onSubmit: (payload: {
    provider: Provider;
    originalId?: string;
    profileId?: string;
    customBinding?: ProviderCustomBinding;
    credentialPatches?: ProviderCredentialPatches;
  }) => Promise<void> | void;
  appId: CoreProviderApp;
  isProxyTakeover?: boolean;
  onOpenShareSettings?: () => void;
}

export function EditProviderDialog({
  open,
  provider,
  resource,
  onOpenChange,
  onSubmit,
  appId,
  onOpenShareSettings,
}: EditProviderDialogProps) {
  const { t } = useTranslation();
  const [isFormSubmitting, setIsFormSubmitting] = useState(false);
  const [isFormDirty, setIsFormDirty] = useState(false);
  const [hasUnsavedChanges, setHasUnsavedChanges] = useState(false);
  const [isSubmitBlocked, setIsSubmitBlocked] = useState(false);
  const closePanel = useCallback(() => onOpenChange(false), [onOpenChange]);
  const closeGuard = useUnsavedChangesGuard({
    active: open,
    dirty: hasUnsavedChanges && !isFormSubmitting,
    onClose: closePanel,
  });

  useEffect(() => {
    setIsFormDirty(false);
    setHasUnsavedChanges(false);
    setIsSubmitBlocked(false);
  }, [open, provider?.id]);

  const initialData = useMemo(() => {
    if (!provider) return null;
    return {
      name: provider.name,
      notes: provider.notes,
      websiteUrl: provider.websiteUrl,
      settingsConfig: provider.settingsConfig,
      category: provider.category,
      meta: provider.meta,
      icon: provider.icon,
      iconColor: provider.iconColor,
    };
  }, [open, provider]);

  const handleSubmit = useCallback(
    async (values: ServerProviderFormValues) => {
      if (!provider) return;
      const settingsConfig = JSON.parse(values.settingsConfig) as Record<
        string,
        unknown
      >;
      await onSubmit({
        provider: {
          ...provider,
          name: values.name.trim(),
          notes: values.notes?.trim() || undefined,
          websiteUrl: values.websiteUrl?.trim() || undefined,
          settingsConfig,
          icon: values.icon?.trim() || undefined,
          iconColor: values.iconColor?.trim() || undefined,
          ...(values.presetCategory ? { category: values.presetCategory } : {}),
          ...(values.meta ? { meta: values.meta } : {}),
        },
        originalId: provider.id,
        profileId: values.profileId,
        customBinding: values.customBinding,
        credentialPatches: values.credentialPatches,
      });
      closePanel();
    },
    [closePanel, onSubmit, provider],
  );

  if (!provider || !initialData) return null;

  return (
    <>
      <FullScreenPanel
        isOpen={open}
        title={t("provider.editProvider")}
        onClose={closeGuard.requestClose}
        footer={
          <Button
            type="submit"
            form="provider-form"
            disabled={isFormSubmitting || !isFormDirty || isSubmitBlocked}
          >
            <Save className="mr-2 h-4 w-4" />
            {t("common.save")}
          </Button>
        }
      >
        <ServerProviderForm
          appId={appId}
          providerId={provider.id}
          resource={resource}
          submitLabel={t("common.save")}
          onSubmit={handleSubmit}
          onCancel={closeGuard.requestClose}
          onSubmittingChange={setIsFormSubmitting}
          onDirtyChange={setIsFormDirty}
          onUnsavedChange={setHasUnsavedChanges}
          onSubmitBlockedChange={setIsSubmitBlocked}
          initialData={initialData}
          showButtons={false}
          onOpenShareSettings={onOpenShareSettings}
        />
      </FullScreenPanel>
      <ConfirmDialog
        isOpen={closeGuard.confirmOpen}
        title={t("provider.unsavedChanges.title")}
        message={t("provider.unsavedChanges.editMessage")}
        confirmText={t("provider.unsavedChanges.discard")}
        cancelText={t("provider.unsavedChanges.keepEditing")}
        variant="destructive"
        zIndex="top"
        onConfirm={closeGuard.discardAndClose}
        onCancel={closeGuard.keepEditing}
      />
    </>
  );
}
