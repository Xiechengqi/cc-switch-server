import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft } from "lucide-react";

import { IconPicker } from "@/components/IconPicker";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { getIconMetadata } from "@/icons/extracted/metadata";

interface ProviderIconControlProps {
  icon?: string;
  iconColor?: string;
  providerName: string;
  onChange: (icon: string, iconColor?: string) => void;
}

export function ProviderIconControl({
  icon,
  iconColor,
  providerName,
  onChange,
}: ProviderIconControlProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const selectIcon = (nextIcon: string) => {
    onChange(nextIcon, getIconMetadata(nextIcon)?.defaultColor);
  };

  return (
    <div className="flex justify-center">
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogTrigger asChild>
          <button
            type="button"
            className="flex h-20 w-20 items-center justify-center rounded-lg border-2 border-muted bg-muted/30 p-3 transition-colors hover:border-primary hover:bg-muted/50"
            title={
              icon
                ? t("providerIcon.clickToChange")
                : t("providerIcon.clickToSelect")
            }
            aria-label={
              icon
                ? t("providerIcon.clickToChange")
                : t("providerIcon.clickToSelect")
            }
          >
            <ProviderIcon
              icon={icon}
              name={providerName || "Provider"}
              color={iconColor}
              size={48}
            />
          </button>
        </DialogTrigger>
        <DialogContent
          variant="fullscreen"
          zIndex="top"
          overlayClassName="bg-[hsl(var(--background))] backdrop-blur-0"
          className="p-0 sm:rounded-none"
        >
          <div className="flex h-full flex-col">
            <div className="flex shrink-0 items-center gap-4 border-b border-border-default bg-muted/40 px-6 py-4">
              <DialogClose asChild>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  title={t("common.back")}
                  aria-label={t("common.back")}
                >
                  <ArrowLeft className="h-4 w-4" />
                </Button>
              </DialogClose>
              <DialogTitle>{t("providerIcon.selectIcon")}</DialogTitle>
            </div>
            <div className="flex-1 overflow-y-auto px-6 py-6">
              <div className="space-y-4">
                <IconPicker
                  value={icon}
                  onValueChange={selectIcon}
                  color={iconColor}
                />
                <div className="flex justify-end">
                  <DialogClose asChild>
                    <Button type="button" variant="outline">
                      {t("common.done")}
                    </Button>
                  </DialogClose>
                </div>
              </div>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
