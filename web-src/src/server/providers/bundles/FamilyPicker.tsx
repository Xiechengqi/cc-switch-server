import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Search } from "lucide-react";

import { ProviderIcon } from "@/components/ProviderIcon";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import {
  profileById,
  providerRegistry,
  type CoreProviderApp,
  type ProviderFamilySpec,
} from "@/server/providerRegistry";
import { createDraftForProfile } from "@/server/providers/editor/providerDraft";
import {
  familyAuthKind,
  familyIsExperimental,
  familyLabel,
  familySupportedApps,
  filterFamilies,
  groupFamilies,
  recommendedFamilyId,
  type FamilyAuthKind,
  type FamilyCategoryId,
} from "./familyCatalog";
import { APP_LABELS, AppLogo } from "./bundleApps";

function FamilyLogo({
  family,
  label,
}: {
  family: ProviderFamilySpec;
  label: string;
}) {
  const profile = profileById(family.credentialProfileId);
  const preset = profile ? createDraftForProfile(profile) : undefined;
  return (
    <ProviderIcon
      icon={preset?.icon}
      name={label}
      color={preset?.iconColor}
      size={16}
      className="shrink-0"
      showFallback
    />
  );
}

function authKindLabel(
  kind: FamilyAuthKind,
  t: (key: string, options?: { defaultValue: string }) => string,
): string {
  switch (kind) {
    case "oauth":
      return t("providerBundle.authKindOauth", { defaultValue: "OAuth" });
    case "aws":
      return t("providerBundle.authKindAws", { defaultValue: "AWS" });
    case "custom":
      return t("providerBundle.authKindCustom", {
        defaultValue: "Custom",
      });
    default:
      return t("providerBundle.authKindApiKey", { defaultValue: "API Key" });
  }
}

function categoryLabel(
  categoryId: FamilyCategoryId,
  t: (key: string, options?: { defaultValue: string }) => string,
): string {
  switch (categoryId) {
    case "custom":
      return t("providerBundle.categoryCustom", { defaultValue: "Custom" });
    case "subscription":
      return t("providerBundle.categorySubscription", {
        defaultValue: "Subscription accounts",
      });
    case "api_key":
      return t("providerBundle.categoryApiKey", {
        defaultValue: "API Key",
      });
  }
}

export function FamilyPicker({
  selectedFamilyId,
  onSelect,
  onAutoSelect,
}: {
  selectedFamilyId: string;
  onSelect: (familyId: string) => void;
  onAutoSelect?: (familyId: string) => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [appFilter, setAppFilter] = useState<CoreProviderApp | "all">("all");
  const visibleFamilies = useMemo(
    () => filterFamilies(providerRegistry.families, query, appFilter),
    [appFilter, query],
  );
  const groups = useMemo(
    () => groupFamilies(visibleFamilies),
    [visibleFamilies],
  );
  const selectedFamily =
    visibleFamilies.find((family) => family.familyId === selectedFamilyId) ??
    visibleFamilies.find(
      (family) => family.familyId === recommendedFamilyId(visibleFamilies),
    ) ??
    visibleFamilies[0];

  useEffect(() => {
    if (
      visibleFamilies.length === 0 ||
      visibleFamilies.some((family) => family.familyId === selectedFamilyId)
    ) {
      return;
    }
    (onAutoSelect ?? onSelect)(recommendedFamilyId(visibleFamilies));
  }, [onAutoSelect, onSelect, selectedFamilyId, visibleFamilies]);

  return (
    <div className="space-y-4">
      <div className="grid gap-3 md:grid-cols-[minmax(0,1fr)_auto]">
        <div className="relative">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t("providerBundle.familySearchPlaceholder", {
              defaultValue: "Search name, App, or auth method",
            })}
            className="pl-9"
            aria-label={t("providerBundle.familySearchPlaceholder", {
              defaultValue: "Search name, App, or auth method",
            })}
          />
        </div>
        <div className="flex flex-wrap gap-1.5">
          {(["all", "claude", "codex", "gemini"] as const).map((app) => (
            <button
              key={app}
              type="button"
              className={cn(
                "inline-flex h-9 items-center gap-1.5 rounded-md border px-2.5 text-xs font-medium",
                appFilter === app
                  ? "border-primary bg-primary text-primary-foreground"
                  : "border-border bg-background text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setAppFilter(app)}
            >
              {app === "all" ? (
                t("providerBundle.appFilterAll", { defaultValue: "All Apps" })
              ) : (
                <>
                  <AppLogo app={app} size={14} />
                  {APP_LABELS[app]}
                </>
              )}
            </button>
          ))}
        </div>
      </div>

      {groups.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          {t("providerBundle.familySearchEmpty", {
            defaultValue: "No provider types match this search.",
          })}
        </p>
      ) : (
        <div className="space-y-5">
          {groups.map((group) => {
            const categoryName = categoryLabel(group.groupId, t);
            // A single-card category whose one card already carries the category name
            // does not need a heading: the two would print the same word twice in a
            // row. The pinned Custom entry is the only group shaped like that, and
            // dropping its heading leaves it reading as the default sitting on top.
            const headingRepeatsCard =
              group.families.length === 1 &&
              familyLabel(group.families[0], t) === categoryName;
            return (
              <div key={group.groupId} className="space-y-2">
                {headingRepeatsCard ? null : (
                  <h3 className="text-sm font-semibold text-foreground">
                    {categoryName}
                  </h3>
                )}
                <div
                  role="radiogroup"
                  className="grid grid-cols-1 gap-2 sm:grid-cols-2"
                >
                  {group.families.map((family) => {
                    const selected = family.familyId === selectedFamilyId;
                    const supported = familySupportedApps(family);
                    const label = familyLabel(family, t);
                    // Same rule one level down: the auth badge is there to say how you
                    // sign in, which is worth nothing when it only echoes the name.
                    const authLabel = authKindLabel(familyAuthKind(family), t);
                    const experimental = familyIsExperimental(family);
                    const showAuthBadge = authLabel !== label;
                    return (
                      <button
                        key={family.familyId}
                        type="button"
                        role="radio"
                        aria-checked={selected}
                        onClick={() => onSelect(family.familyId)}
                        className={cn(
                          "flex min-h-[4.5rem] w-full flex-col gap-2 rounded-lg border px-3 py-2.5 text-left transition-colors",
                          selected
                            ? "border-primary bg-primary/5"
                            : "border-border bg-card hover:border-border-active",
                        )}
                      >
                        <div className="flex items-start justify-between gap-2">
                          <span className="flex min-w-0 items-center gap-2">
                            <FamilyLogo family={family} label={label} />
                            <span className="truncate text-sm font-medium">
                              {label}
                            </span>
                          </span>
                          <span className="flex shrink-0 items-center gap-1">
                            {(["claude", "codex", "gemini"] as const).map(
                              (app) => (
                                <span key={app} title={APP_LABELS[app]}>
                                  <AppLogo
                                    app={app}
                                    size={14}
                                    muted={!supported.includes(app)}
                                  />
                                </span>
                              ),
                            )}
                          </span>
                        </div>
                        {showAuthBadge || experimental ? (
                          <div className="flex flex-wrap items-center gap-1.5">
                            {showAuthBadge ? (
                              <Badge
                                variant="outline"
                                className="h-5 px-1.5 text-[10px]"
                              >
                                {authLabel}
                              </Badge>
                            ) : null}
                            {experimental ? (
                              <Badge
                                variant="secondary"
                                className="h-5 px-1.5 text-[10px]"
                              >
                                {t("serverProviderForm.identity.experimental")}
                              </Badge>
                            ) : null}
                          </div>
                        ) : null}
                      </button>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {selectedFamily ? (
        <p className="rounded-md border bg-muted/30 px-3 py-2 text-sm text-muted-foreground">
          {t("providerBundle.familySummary", {
            defaultValue: "Creates {{apps}} using {{auth}}. {{credential}}",
            apps: familySupportedApps(selectedFamily)
              .map((app) => APP_LABELS[app])
              .join(" + "),
            auth: authKindLabel(familyAuthKind(selectedFamily), t),
            credential:
              familyAuthKind(selectedFamily) === "oauth"
                ? t("providerBundle.familyNeedsAccount", {
                    defaultValue: "Bind a managed account before saving.",
                  })
                : familyAuthKind(selectedFamily) === "custom"
                  ? t("providerBundle.familyNeedsCustom", {
                      defaultValue:
                        "Each enabled App needs its own URL and credential.",
                    })
                  : t("providerBundle.familyNeedsSecret", {
                      defaultValue:
                        "Enter the shared credential before saving.",
                    }),
          })}
        </p>
      ) : null}
    </div>
  );
}
