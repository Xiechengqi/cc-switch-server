import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Search } from "lucide-react";

import { ClaudeIcon, CodexIcon, GeminiIcon } from "@/components/BrandIcons";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
  familySupportedApps,
  filterFamilies,
  groupFamilies,
  recommendedFamilyId,
  type FamilyAuthKind,
  type FamilyGroupId,
} from "./familyCatalog";

const APP_LABELS: Record<CoreProviderApp, string> = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
};

function AppLogo({
  app,
  muted = false,
}: {
  app: CoreProviderApp;
  muted?: boolean;
}) {
  const className = muted ? "opacity-30 grayscale" : undefined;
  if (app === "claude") return <ClaudeIcon size={14} className={className} />;
  if (app === "codex") return <CodexIcon size={14} className={className} />;
  return <GeminiIcon size={14} className={className} />;
}

function FamilyLogo({ family }: { family: ProviderFamilySpec }) {
  const profile = profileById(family.credentialProfileId);
  const preset = profile ? createDraftForProfile(profile) : undefined;
  return (
    <ProviderIcon
      icon={preset?.icon}
      name={family.label}
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
      return t("providerBundle.authKindCustom", { defaultValue: "Custom HTTP" });
    default:
      return t("providerBundle.authKindApiKey", { defaultValue: "API Key" });
  }
}

function groupLabel(
  groupId: FamilyGroupId,
  t: (key: string, options?: { defaultValue: string }) => string,
): string {
  switch (groupId) {
    case "official_oauth":
      return t("providerBundle.groupOfficialOauth", {
        defaultValue: "Official subscriptions",
      });
    case "official_key":
      return t("providerBundle.groupOfficialKey", {
        defaultValue: "Official API keys",
      });
    case "china_plan":
      return t("providerBundle.groupChinaPlan", {
        defaultValue: "China coding plans",
      });
    case "aggregator_cloud":
      return t("providerBundle.groupAggregator", {
        defaultValue: "Aggregators and cloud",
      });
    case "experimental_bridge":
      return t("providerBundle.groupExperimental", {
        defaultValue: "Experimental bridges",
      });
    default:
      return t("providerBundle.groupCustom", { defaultValue: "Custom" });
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
                  <AppLogo app={app} />
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
          {groups.map((group) => (
            <div key={group.groupId} className="space-y-2">
              <Label className="text-xs uppercase tracking-wide text-muted-foreground">
                {groupLabel(group.groupId, t)}
              </Label>
              <div
                role="radiogroup"
                className="grid grid-cols-1 gap-2 sm:grid-cols-2"
              >
                {group.families.map((family) => {
                  const selected = family.familyId === selectedFamilyId;
                  const supported = familySupportedApps(family);
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
                          <FamilyLogo family={family} />
                          <span className="truncate text-sm font-medium">
                            {family.label}
                          </span>
                        </span>
                        <span className="flex shrink-0 items-center gap-1">
                          {(["claude", "codex", "gemini"] as const).map(
                            (app) => (
                              <span key={app} title={APP_LABELS[app]}>
                                <AppLogo
                                  app={app}
                                  muted={!supported.includes(app)}
                                />
                              </span>
                            ),
                          )}
                        </span>
                      </div>
                      <div className="flex flex-wrap items-center gap-1.5">
                        <Badge variant="outline" className="h-5 px-1.5 text-[10px]">
                          {authKindLabel(familyAuthKind(family), t)}
                        </Badge>
                        {familyIsExperimental(family) ? (
                          <Badge
                            variant="secondary"
                            className="h-5 px-1.5 text-[10px]"
                          >
                            {t("serverProviderForm.identity.experimental")}
                          </Badge>
                        ) : null}
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      )}

      {selectedFamily ? (
        <p className="rounded-md border bg-muted/30 px-3 py-2 text-sm text-muted-foreground">
          {t("providerBundle.familySummary", {
            defaultValue:
              "Creates {{apps}} using {{auth}}. {{credential}}",
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
                      defaultValue: "Enter the shared credential before saving.",
                    }),
          })}
        </p>
      ) : null}
    </div>
  );
}
