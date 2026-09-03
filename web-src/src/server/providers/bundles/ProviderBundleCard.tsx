import {
  Activity,
  Copy,
  GripVertical,
  Link,
  LoaderCircle,
  Pencil,
  Share2,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  DraggableAttributes,
  DraggableSyntheticListeners,
} from "@dnd-kit/core";

import AntigravityOauthQuotaFooter from "@/components/AntigravityOauthQuotaFooter";
import ClaudeOauthQuotaFooter from "@/components/ClaudeOauthQuotaFooter";
import CodeBuddyOauthQuotaFooter from "@/components/CodeBuddyOauthQuotaFooter";
import CodexOauthQuotaFooter from "@/components/CodexOauthQuotaFooter";
import CopilotQuotaFooter from "@/components/CopilotQuotaFooter";
import CursorOauthQuotaFooter from "@/components/CursorOauthQuotaFooter";
import GeminiOauthQuotaFooter from "@/components/GeminiOauthQuotaFooter";
import GrokOauthQuotaFooter from "@/components/GrokOauthQuotaFooter";
import KiroOauthQuotaFooter from "@/components/KiroOauthQuotaFooter";
import OllamaQuotaFooter from "@/components/OllamaQuotaFooter";
import { ProviderIcon } from "@/components/ProviderIcon";
import CodingPlanQuotaFooter from "@/components/providers/CodingPlanQuotaFooter";
import { ProviderHealthBadge } from "@/components/providers/ProviderHealthBadge";
import { ProviderShareStatusTag } from "@/components/providers/ProviderShareStatusTag";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { PROVIDER_TYPES } from "@/config/constants";
import type { ManagedAuthAccount } from "@/lib/api/auth";
import type { ProviderBundleView, ProviderResource } from "@/lib/api/providers";
import type { ShareRecord } from "@/lib/api/share";
import { useModelTest } from "@/hooks/useModelTest";
import { useStreamCheck } from "@/hooks/useStreamCheck";
import { useProviderHealth } from "@/lib/query/providerHealth";
import { cn } from "@/lib/utils";
import {
  modelPoliciesForProfile,
  profileById,
} from "@/server/providerRegistry";
import { providerResourceSupportsOperation } from "@/server/providerOperations";
import {
  canTestLinkProvider,
  canTestModelProvider,
  getProviderQuotaSource,
} from "@/utils/providerMetaUtils";
import { getProviderSharePhase } from "@/utils/shareUtils";
import {
  providerBundleDisplayTarget,
  providerBundlePrimaryResource,
  providerBundleTestResource,
} from "./bundleCard";
import { APP_LABELS, AppLogo } from "./bundleApps";

function BundleQuotaSummary({
  resource,
  cursorAccountLabel,
  cursorSubscriptionLevel,
}: {
  resource?: ProviderResource;
  cursorAccountLabel?: string;
  cursorSubscriptionLevel?: string;
}) {
  if (!resource) return null;
  if (resource.runtime?.codingPlan) {
    return <CodingPlanQuotaFooter resource={resource} inline />;
  }
  const provider = resource.provider;
  const app = resource.app;
  const quotaSource = getProviderQuotaSource(provider, app);

  if (quotaSource === "copilot") {
    return <CopilotQuotaFooter meta={provider.meta} inline />;
  }
  if (quotaSource === "codex_oauth") {
    return <CodexOauthQuotaFooter meta={provider.meta} inline />;
  }
  if (quotaSource === "grok_oauth") {
    return <GrokOauthQuotaFooter meta={provider.meta} inline />;
  }
  if (quotaSource === "claude_oauth") {
    return <ClaudeOauthQuotaFooter meta={provider.meta} inline />;
  }
  if (quotaSource === "codebuddy_oauth") {
    return <CodeBuddyOauthQuotaFooter meta={provider.meta} inline />;
  }
  if (quotaSource === "google_gemini_oauth") {
    return <GeminiOauthQuotaFooter meta={provider.meta} inline />;
  }
  if (quotaSource === "antigravity_oauth" || quotaSource === "agy_oauth") {
    return (
      <AntigravityOauthQuotaFooter
        meta={provider.meta}
        authProvider={
          quotaSource === "agy_oauth"
            ? PROVIDER_TYPES.AGY_OAUTH
            : PROVIDER_TYPES.ANTIGRAVITY_OAUTH
        }
        inline
      />
    );
  }
  if (quotaSource === "cursor_oauth" || quotaSource === "cursor_apikey") {
    return (
      <CursorOauthQuotaFooter
        meta={provider.meta}
        appId={app}
        providerId={provider.id}
        accountLabel={cursorAccountLabel}
        subscriptionLevel={cursorSubscriptionLevel}
        inline
      />
    );
  }
  if (quotaSource === "kiro_oauth") {
    return <KiroOauthQuotaFooter meta={provider.meta} inline />;
  }
  if (quotaSource === "ollama_cloud") {
    return <OllamaQuotaFooter resource={resource} inline />;
  }
  return null;
}

function supportsConnectivity(resource: ProviderResource): boolean {
  return (
    providerResourceSupportsOperation(resource, "connectivity") ??
    canTestLinkProvider(resource.provider, resource.app)
  );
}

function supportsModelTest(resource: ProviderResource): boolean {
  return (
    providerResourceSupportsOperation(resource, "test") ??
    canTestModelProvider(resource.provider, resource.app)
  );
}

function operationResource(
  bundle: ProviderBundleView,
  supports: (resource: ProviderResource) => boolean,
): ProviderResource | undefined {
  return bundle.enabledApps
    .map((app) => bundle.surfaces[app])
    .find((resource): resource is ProviderResource =>
      Boolean(resource && supports(resource)),
    );
}

interface ProviderBundleCardProps {
  bundle: ProviderBundleView;
  share?: ShareRecord;
  accounts: ManagedAuthAccount[];
  onEdit: () => void;
  onEditShare?: () => void;
  onDuplicate: () => void;
  sharePending?: boolean;
  shareActionDisabled?: boolean;
  onToggleShare: () => void;
  onDeleteShare: () => void;
  onDelete: () => void;
  dragHandleProps?: {
    attributes: DraggableAttributes;
    listeners: DraggableSyntheticListeners;
    isDragging: boolean;
  };
}

export function ProviderBundleCard({
  bundle,
  share,
  accounts,
  onEdit,
  onEditShare,
  onDuplicate,
  sharePending = false,
  shareActionDisabled = false,
  onToggleShare,
  onDeleteShare,
  onDelete,
  dragHandleProps,
}: ProviderBundleCardProps) {
  const { t } = useTranslation();
  const primaryResource = providerBundlePrimaryResource(bundle);
  const connectivityResource = operationResource(bundle, supportsConnectivity);
  const configuredModelResource = providerBundleTestResource(bundle);
  const modelResource =
    configuredModelResource && supportsModelTest(configuredModelResource)
      ? configuredModelResource
      : undefined;
  const connectivityApp =
    connectivityResource?.app ?? primaryResource?.app ?? "claude";
  const { checkProvider, isChecking } = useStreamCheck(connectivityApp);
  const claudeModelTest = useModelTest("claude");
  const codexModelTest = useModelTest("codex");
  const geminiModelTest = useModelTest("gemini");
  const modelTests = {
    claude: claudeModelTest,
    codex: codexModelTest,
    gemini: geminiModelTest,
  };
  const modelTest = modelTests[bundle.testApp];
  const healthResource = configuredModelResource ?? primaryResource;
  const { data: health } = useProviderHealth(
    healthResource?.provider.id ?? bundle.id,
    healthResource?.app ?? bundle.testApp,
  );
  const target = providerBundleDisplayTarget(bundle, accounts);
  const targetText =
    target.kind === "oauth_account"
      ? target.value
        ? t("provider.oauthAccountDisplay", {
            account: target.value,
            defaultValue: `OAuth account: ${target.value}`,
          })
        : t("provider.oauthAccountResolving", {
            defaultValue: "OAuth account",
          })
      : target.kind === "api_key_account"
        ? target.value
          ? t("provider.apiKeyAccountDisplay", {
              account: target.value,
              defaultValue: `API key account: ${target.value}`,
            })
          : t("provider.apiKeyAccountResolving", {
              defaultValue: "API key account",
            })
        : (target.value ??
          t("providerBundle.apiUrlUnavailable", {
            defaultValue: "API 地址未配置",
          }));
  const sharePhase = getProviderSharePhase(share);
  const isSharing = sharePhase === "sharing";
  const shareButtonLabel =
    sharePhase === "sharing"
      ? t("provider.share.sharing", { defaultValue: "分享中" })
      : sharePhase === "stopped"
        ? t("provider.share.resumeShort", { defaultValue: "开启分享" })
        : t("provider.share.enable", { defaultValue: "分享" });
  const connectivityId = connectivityResource?.provider.id ?? bundle.id;
  const modelTesting = Boolean(
    modelResource && modelTest.isTesting(modelResource.provider.id),
  );
  const modelSummaries = bundle.enabledApps.flatMap((app) => {
    const resource = bundle.surfaces[app];
    const policy = resource?.runtime?.modelPolicy;
    if (!policy) return [];
    const profile = resource.profileId
      ? profileById(resource.profileId)
      : undefined;
    return [
      {
        app,
        fixed: profile ? modelPoliciesForProfile(profile).length === 1 : false,
        signature:
          policy.mode === "single"
            ? `single:${policy.upstreamModel}`
            : "passthrough",
        label:
          policy.mode === "single"
            ? t("providerBundle.modelSummarySingle", {
                model: policy.upstreamModel,
              })
            : t("providerBundle.modelPassthrough"),
      },
    ];
  });
  const globalModelSummaries = modelSummaries.filter((item) => !item.fixed);
  const fixedModelSummaries = modelSummaries.filter((item) => item.fixed);
  const compactModelSummary =
    bundle.modelPolicyScope === "global" &&
    globalModelSummaries.length > 0 &&
    new Set(globalModelSummaries.map((item) => item.signature)).size <= 1
      ? [
          globalModelSummaries[0]?.label,
          ...fixedModelSummaries.map(
            (item) =>
              `${APP_LABELS[item.app]} (${t("providerBundle.modelProfileFixed")}): ${item.label}`,
          ),
        ]
          .filter(Boolean)
          .join(" · ")
      : modelSummaries
          .map(
            (item) =>
              `${APP_LABELS[item.app]}${item.fixed ? ` (${t("providerBundle.modelProfileFixed")})` : ""}: ${item.label}`,
          )
          .join(" · ");
  const iconButtonClass = "h-8 w-8 p-1";

  return (
    <article
      className={cn(
        "group relative overflow-hidden rounded-xl border bg-card p-4 text-card-foreground transition-all duration-300",
        isSharing
          ? "border-violet-500/60 shadow-sm shadow-violet-500/10"
          : "border-border hover:border-border-active hover:shadow-sm",
        dragHandleProps?.isDragging &&
          "z-10 scale-[1.01] cursor-grabbing border-primary shadow-lg",
      )}
    >
      <div
        className={cn(
          "pointer-events-none absolute inset-0 bg-gradient-to-r from-violet-500/10 to-transparent transition-opacity duration-500",
          isSharing ? "opacity-100" : "opacity-0",
        )}
      />
      <div className="relative flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-1 items-center gap-2">
          {dragHandleProps ? (
            <button
              type="button"
              className={cn(
                "-ml-1.5 flex shrink-0 cursor-grab items-center justify-center p-1.5 text-muted-foreground/50 transition-colors hover:text-muted-foreground active:cursor-grabbing",
                dragHandleProps.isDragging && "cursor-grabbing",
              )}
              aria-label={t("provider.dragHandle", {
                defaultValue: "拖拽排序",
              })}
              {...dragHandleProps.attributes}
              {...dragHandleProps.listeners}
            >
              <GripVertical className="h-4 w-4" />
            </button>
          ) : null}
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-border bg-muted transition-transform duration-300 group-hover:scale-105">
            <ProviderIcon
              icon={bundle.icon}
              name={bundle.name}
              color={bundle.iconColor}
              size={20}
              showFallback
            />
          </div>

          <div className="min-w-0 flex-1 space-y-1">
            <div className="flex min-h-7 min-w-0 flex-wrap items-center gap-2">
              <h2 className="truncate text-base font-semibold leading-none">
                {bundle.name}
              </h2>
              <div className="flex shrink-0 items-center gap-1">
                {bundle.supportedApps.map((app) => (
                  <span
                    key={app}
                    className={cn(
                      "flex h-5 w-5 items-center justify-center",
                      bundle.enabledApps.includes(app)
                        ? "opacity-100"
                        : "opacity-30 grayscale",
                    )}
                    title={`${APP_LABELS[app]}${bundle.enabledApps.includes(app) ? "" : " (disabled)"}`}
                  >
                    <AppLogo app={app} size={17} />
                  </span>
                ))}
              </div>
              {share ? (
                onEditShare ? (
                  <button
                    type="button"
                    className="inline-flex"
                    title={t("providerBundle.editShare", {
                      defaultValue: "编辑远程分享",
                    })}
                    onClick={onEditShare}
                  >
                    <ProviderShareStatusTag share={share} />
                  </button>
                ) : (
                  <ProviderShareStatusTag share={share} />
                )
              ) : null}
              {health ? <ProviderHealthBadge health={health} /> : null}
              <Badge variant="outline" className="h-5 px-1.5 text-[10px]">
                {bundle.modelPolicyScope === "global"
                  ? t("providerBundle.modelScopeGlobal")
                  : t("providerBundle.modelScopePerApp")}
              </Badge>
            </div>

            <button
              type="button"
              disabled={target.kind !== "api_url" || !target.value}
              className={cn(
                "inline-flex max-w-full items-center overflow-hidden text-left text-sm",
                target.kind === "api_url" && target.value
                  ? "cursor-pointer text-blue-500 transition-colors hover:underline dark:text-blue-400"
                  : "cursor-default text-muted-foreground",
              )}
              title={targetText}
              onClick={() => {
                if (target.kind === "api_url" && target.value) {
                  window.open(target.value, "_blank", "noopener,noreferrer");
                }
              }}
            >
              <span className="min-w-0 truncate">{targetText}</span>
            </button>
            {compactModelSummary ? (
              <p
                className="truncate text-xs text-muted-foreground"
                title={compactModelSummary}
              >
                {compactModelSummary}
              </p>
            ) : null}
          </div>
        </div>

        <div className="flex w-full min-w-0 flex-col gap-2 sm:ml-auto sm:w-auto sm:max-w-[58%]">
          <div className="flex min-h-5 min-w-0 max-w-full flex-wrap items-center justify-end gap-x-1 gap-y-1">
            <BundleQuotaSummary
              resource={primaryResource}
              cursorAccountLabel={
                target.kind === "api_key_account" ||
                target.kind === "oauth_account"
                  ? (target.value ?? undefined)
                  : undefined
              }
              cursorSubscriptionLevel={target.subscriptionLevel ?? undefined}
            />
          </div>

          <div className="flex min-w-0 flex-wrap items-center justify-end gap-1.5">
            {sharePhase === "stopped" ? (
              <>
                <Button
                  type="button"
                  size="sm"
                  variant="default"
                  className="min-w-[4.5rem] bg-violet-500 px-2.5 hover:bg-violet-600 dark:bg-violet-600 dark:hover:bg-violet-700"
                  title={t("provider.share.resume", {
                    defaultValue: "重新开启分享",
                  })}
                  disabled={sharePending || shareActionDisabled}
                  onClick={onToggleShare}
                >
                  {sharePending ? (
                    <LoaderCircle className="h-4 w-4 animate-spin" />
                  ) : (
                    <Share2 className="h-4 w-4" />
                  )}
                  {shareButtonLabel}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className="min-w-[4.5rem] px-2.5 text-destructive hover:text-destructive"
                  title={t("provider.share.delete", {
                    defaultValue: "删除分享",
                  })}
                  disabled={sharePending || shareActionDisabled}
                  onClick={onDeleteShare}
                >
                  {sharePending ? (
                    <LoaderCircle className="h-4 w-4 animate-spin" />
                  ) : (
                    <Trash2 className="h-4 w-4" />
                  )}
                  {t("provider.share.deleteShort", {
                    defaultValue: "删除分享",
                  })}
                </Button>
              </>
            ) : (
              <Button
                type="button"
                size="sm"
                variant={isSharing ? "secondary" : "default"}
                className={cn(
                  "min-w-[4.5rem] px-2.5",
                  isSharing
                    ? "bg-violet-100 text-violet-600 hover:bg-violet-200 dark:bg-violet-900/50 dark:text-violet-400 dark:hover:bg-violet-900/70"
                    : "bg-violet-500 hover:bg-violet-600 dark:bg-violet-600 dark:hover:bg-violet-700",
                )}
                title={
                  isSharing
                    ? t("provider.share.stop", {
                        defaultValue: "点击停止分享",
                      })
                    : t("provider.share.sectionTitle", {
                        defaultValue: "远程分享",
                      })
                }
                disabled={sharePending || shareActionDisabled}
                onClick={onToggleShare}
              >
                {sharePending ? (
                  <LoaderCircle className="h-4 w-4 animate-spin" />
                ) : (
                  <Share2 className="h-4 w-4" />
                )}
                {shareButtonLabel}
              </Button>
            )}

            <div className="flex items-center gap-1">
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className={iconButtonClass}
                title={t("common.edit")}
                onClick={onEdit}
              >
                <Pencil className="h-4 w-4" />
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className={iconButtonClass}
                title={t("provider.duplicate", { defaultValue: "复制" })}
                onClick={onDuplicate}
              >
                <Copy className="h-4 w-4" />
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className={cn(
                  iconButtonClass,
                  !connectivityResource &&
                    "cursor-not-allowed text-muted-foreground opacity-40",
                )}
                disabled={!connectivityResource || isChecking(connectivityId)}
                title={
                  connectivityResource
                    ? t("provider.testLink", { defaultValue: "测试链接" })
                    : t("provider.testLinkUnavailable", {
                        defaultValue: "当前供应商不支持连通性测试",
                      })
                }
                onClick={() => {
                  if (connectivityResource) {
                    void checkProvider(connectivityId, bundle.name);
                  }
                }}
              >
                {isChecking(connectivityId) ? (
                  <LoaderCircle className="h-4 w-4 animate-spin" />
                ) : (
                  <Link className="h-4 w-4" />
                )}
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className={cn(
                  iconButtonClass,
                  !modelResource &&
                    "cursor-not-allowed text-muted-foreground opacity-40",
                )}
                disabled={!modelResource || modelTesting}
                title={t("providerBundle.testModel")}
                onClick={() => {
                  if (!modelResource) return;
                  void modelTest.testProvider(
                    modelResource.provider.id,
                    `${bundle.name} / ${APP_LABELS[modelResource.app]}`,
                  );
                }}
              >
                {modelTesting ? (
                  <LoaderCircle className="h-4 w-4 animate-spin" />
                ) : (
                  <Activity className="h-4 w-4" />
                )}
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className={cn(
                  iconButtonClass,
                  "hover:text-red-500 dark:hover:text-red-400",
                )}
                title={t("common.delete")}
                onClick={onDelete}
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          </div>
        </div>
      </div>
    </article>
  );
}
