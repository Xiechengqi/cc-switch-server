/* Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V3 */
import type { Ref } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import { shareApi, type ShareUserGrantMap } from "@/lib/api/share";
import {
  capabilityOptions,
  capabilitySelectable,
  capabilityStatus,
  capabilityPricingText,
  capabilityScopeLabel,
  capabilityAmount,
  OPERATION_LABELS,
  COVERAGE_LABELS,
  emptyCapabilityPolicy,
  type ShareCapabilityPolicy,
  type ShareOperation,
  type CapabilityConsent,
  type CapabilityReviewHandle,
} from "@/lib/share-capabilities";
import { useCapabilityReview } from "@/lib/use-share-capabilities";

/** Native Provider Bundle view. Runtime evidence comes from the same local domain service as admission. */
export function ShareCapabilitiesEditor({
  shareId,
  revision,
  policy,
  savedPolicy,
  grants,
  tokenLimit,
  disabled,
  onChange,
  reviewRef,
}: {
  shareId?: string;
  revision?: number;
  policy?: ShareCapabilityPolicy;
  savedPolicy?: ShareCapabilityPolicy;
  grants: ShareUserGrantMap;
  tokenLimit: number;
  disabled?: boolean;
  reviewRef?: Ref<CapabilityReviewHandle>;
  onChange: (
    policy: ShareCapabilityPolicy,
    consent?: CapabilityConsent,
  ) => void;
}) {
  const { i18n } = useTranslation();
  const lang = i18n.language.startsWith("zh") ? 0 : 1;
  const review = useCapabilityReview({
    shareId,
    revision,
    policy,
    savedPolicy,
    grants,
    tokenLimit,
    request: shareApi.previewCapabilities,
    reviewRef,
    lang,
  });
  const { preview } = review;
  const options = capabilityOptions(preview, policy);
  const main = options.filter((option) => option.available || option.selected);
  const setup = options.filter(
    (option) => !option.available && !option.selected && option.configurable,
  );
  const canEdit =
    !!shareId &&
    review.current &&
    preview?.ready &&
    !review.loading &&
    !disabled;
  const toggle = (operation: ShareOperation, enabled: boolean) => {
    const current = policy ?? emptyCapabilityPolicy();
    const operationBindings = { ...current.operationBindings };
    delete operationBindings[operation];
    onChange({
      ...current,
      operationBindings,
      allowedOperations: enabled
        ? [...current.allowedOperations, operation]
        : current.allowedOperations.filter((op) => op !== operation),
    });
  };
  return (
    <section
      className="min-w-0 space-y-3 rounded-lg border p-4 [overflow-wrap:anywhere]"
      aria-label={lang === 0 ? "扩展能力" : "Extended capabilities"}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h4 className="font-medium">
            {lang === 0 ? "扩展能力" : "Extended capabilities"}
          </h4>
          <p className="mt-1 text-xs text-muted-foreground">
            {lang === 0
              ? "开放生成能力与供应商工具。图片、文档、音频和视频理解输入不受这些开关影响。"
              : "Allow generation capabilities and Provider-hosted tools. Image, document, audio and video understanding inputs are unaffected."}
          </p>
        </div>
        {shareId ? (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            disabled={disabled || review.loading}
            onClick={review.refresh}
          >
            {lang === 0 ? "刷新" : "Refresh"}
          </Button>
        ) : null}
      </div>
      {!shareId ? (
        <p className="text-sm text-muted-foreground">
          {lang === 0
            ? "新 Share 默认不开放扩展能力。创建后可按实际供应商绑定开启。"
            : "New Shares have extensions disabled. Configure them after creation using the actual Provider bindings."}
        </p>
      ) : (
        <>
          {review.error ? (
            <p role="alert" className="text-sm text-destructive">
              {review.error}
            </p>
          ) : null}
          {review.loading ? (
            <p role="status" className="text-xs text-muted-foreground">
              {lang === 0
                ? "正在检查当前供应商能力…"
                : "Checking current Provider capabilities…"}
            </p>
          ) : null}
          {preview && !preview.ready ? (
            <p role="alert" className="text-xs text-warning">
              {lang === 0
                ? "能力变更暂不可保存，请检查 Router 协议与存储状态。"
                : "Capability changes are unavailable. Check the Router protocol and storage."}
            </p>
          ) : null}
          {preview?.unavailableBindings?.length ? (
            <p role="alert" className="text-xs text-warning">
              {lang === 0
                ? "部分供应商绑定无法读取，已有选择保留。请先处理供应商配置。"
                : "Some Provider bindings are unavailable. Existing choices are retained; check Provider configuration."}
            </p>
          ) : null}
          {!policy ? (
            <div className="space-y-2">
              <p className="text-sm">
                {lang === 0
                  ? "此 Share 尚未设置独立能力权限。"
                  : "Independent capability permissions have not been configured."}
              </p>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={!canEdit}
                onClick={() =>
                  onChange(preview?.policy ?? emptyCapabilityPolicy())
                }
              >
                {lang === 0
                  ? "设置扩展能力权限"
                  : "Configure extension permissions"}
              </Button>
              <p className="text-xs text-muted-foreground">
                {lang === 0
                  ? "保存时会确认允许和关闭的能力。"
                  : "Saving reviews which extensions will be allowed or disabled."}
              </p>
            </div>
          ) : (
            <>
              {preview && !options.length ? (
                <p className="py-2 text-sm text-muted-foreground">
                  {lang === 0
                    ? "当前绑定没有可配置的扩展能力。"
                    : "The current bindings have no configurable extensions."}
                </p>
              ) : null}
              <div className="divide-y">
                {main.map((option) => (
                  <div key={option.operation} className="py-3">
                    <label className="flex items-center justify-between gap-3 text-sm">
                      <span className="flex min-w-0 items-center gap-2 font-normal">
                        <input
                          type="checkbox"
                          checked={option.selected}
                          disabled={
                            !canEdit || (!option.available && !option.selected)
                          }
                          onChange={(event) =>
                            toggle(option.operation, event.target.checked)
                          }
                          className="h-4 min-h-4 w-4 shrink-0 p-0 accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 disabled:cursor-not-allowed"
                        />
                        {OPERATION_LABELS[option.operation][lang]}
                      </span>
                      <span className="text-xs font-normal text-muted-foreground">
                        {!option.available
                          ? lang === 0
                            ? "暂不可用"
                            : "Unavailable"
                          : option.rows.some(
                                (row) =>
                                  capabilitySelectable(row) &&
                                  row.accountSupport !== "supported",
                              )
                            ? lang === 0
                              ? "账号待验证"
                              : "Verification pending"
                            : lang === 0
                              ? "账号已验证"
                              : "Account verified"}
                      </span>
                    </label>
                    <details className="mt-1 pl-5 text-xs text-muted-foreground">
                      <summary className="cursor-pointer">
                        {lang === 0 ? "支持与计量详情" : "Support and metering"}
                      </summary>
                      <div className="mt-2 space-y-3">
                        {option.rows
                          .filter(
                            (row) => row.implemented !== false || row.selected || option.selected,
                          )
                          .map((row) => (
                            <div key={row.app} className="space-y-1">
                              <p>
                                {row.providerLabel ||
                                  (lang === 0
                                    ? "当前供应商"
                                    : "Current Provider")}{" "}
                                · {row.app} · {capabilityStatus(row, lang)}
                              </p>
                              <p>
                                {row.model
                                  ? `${lang === 0 ? "固定模型：" : "Fixed model: "}${row.model}`
                                  : lang === 0
                                    ? "按实际请求选择模型"
                                    : "Model selected by each request"}
                              </p>
                              <p>{capabilityPricingText(row, lang)}</p>
                              {option.selected &&
                              (option.rows.filter(capabilitySelectable).length >
                                1 ||
                                option.rows.some(
                                  (item) =>
                                    capabilitySelectable(item) &&
                                    !item.selected,
                                )) ? (
                                <label className="flex items-center gap-2">
                                  <input
                                    type="checkbox"
                                    className="h-4 min-h-4 w-4 shrink-0 p-0 accent-primary"
                                    checked={row.selected ?? false}
                                    disabled={
                                      !canEdit || !capabilitySelectable(row)
                                    }
                                    aria-label={`${OPERATION_LABELS[option.operation][lang]} ${row.app}`}
                                    onChange={(event) => {
                                      if (!preview) return;
                                      const bindings =
                                        preview.policy.operationBindings[
                                          option.operation
                                        ] ?? [];
                                      onChange({
                                        ...preview.policy,
                                        operationBindings: {
                                          ...preview.policy.operationBindings,
                                          [option.operation]: event.target
                                            .checked
                                            ? [
                                                ...bindings.filter(
                                                  (binding) =>
                                                    binding.app !== row.app,
                                                ),
                                                {
                                                  app: row.app,
                                                  fingerprint:
                                                    row.bindingFingerprint,
                                                },
                                              ]
                                            : bindings.filter(
                                                (binding) =>
                                                  binding.app !== row.app,
                                              ),
                                        },
                                      });
                                    }}
                                  />
                                  {lang === 0
                                    ? `允许 ${row.app} 入口`
                                    : `Allow ${row.app} access`}
                                </label>
                              ) : null}
                              {option.selected && row.selected
                                ? [
                                    ...new Map(
                                      row.scopes.map((scope) => [
                                        JSON.stringify([
                                          scope.quotaUnit,
                                          scope.reason,
                                        ]),
                                        scope,
                                      ]),
                                    ).values(),
                                  ].map((scope) => (
                                    <p
                                      key={`${scope.quotaUnit}-${scope.reason}`}
                                    >
                                      {COVERAGE_LABELS[scope.reason]?.[lang] ??
                                        (lang === 0
                                          ? "计量状态尚未明确"
                                          : "Measurement status unknown")}
                                    </p>
                                  ))
                                : null}
                            </div>
                          ))}
                        {!option.rows.length ? (
                          <p>
                            {lang === 0
                              ? "原绑定不可用，可取消此能力。"
                              : "The previous binding is unavailable. You can disable this capability."}
                          </p>
                        ) : null}
                      </div>
                    </details>
                  </div>
                ))}
              </div>
              {setup.length ? (
                <details className="text-xs text-muted-foreground">
                  <summary className="cursor-pointer">
                    {lang === 0
                      ? "需先配置的能力"
                      : "Capabilities requiring setup"}
                  </summary>
                  <div className="mt-2 space-y-1">
                    {setup.map((option) => (
                      <p key={option.operation}>
                        {OPERATION_LABELS[option.operation][lang]} ·{" "}
                        {lang === 0
                          ? "需先在供应商设置中开启"
                          : "Enable in Provider settings first"}
                      </p>
                    ))}
                  </div>
                </details>
              ) : null}
              {policy.allowedOperations.length ? (
                <p className="text-xs text-muted-foreground">
                  {lang === 0
                    ? "用户默认跟随 Share；可在下方用户授权与配额中进一步限制。"
                    : "Users follow the Share by default. Restrict individuals in user access and quotas below."}
                </p>
              ) : null}
            </>
          )}
        </>
      )}
      <p className="text-xs text-muted-foreground">
        {lang === 0
          ? "与其他修改一起保存，保留已消耗额度。"
          : "Saved with your other changes. Consumed quota is retained."}
      </p>
      {preview?.incidents?.length ? (
        <div
          role="alert"
          className="space-y-1 rounded border border-destructive p-2 text-xs"
        >
          <p>
            {lang === 0
              ? "异常资源执行已隔离相关调用。请修复执行路径并重新核对绑定。"
              : "Unexpected resource execution quarantined affected calls. Repair the execution path and review its binding."}
          </p>
          <details>
            <summary>{lang === 0 ? "异常证据" : "Incident evidence"}</summary>
            {preview.incidents.map((item) => (
              <p key={`${item.requestId}-${item.resource}`}>
                {capabilityScopeLabel(item.scope, lang)} · {item.resource} ·{" "}
                {item.bindingFingerprint.slice(0, 12)}
              </p>
            ))}
          </details>
        </div>
      ) : null}
      {preview?.recentUsage?.length ? (
        <details className="text-xs">
          <summary className="cursor-pointer">
            {lang === 0
              ? "最近执行与计量明细"
              : "Recent execution and metering"}
          </summary>
          <div className="mt-2 max-h-64 space-y-2 overflow-y-auto">
            {preview.recentUsage.map((item) => (
              <div
                key={`${item.requestId}-${item.attempt}`}
                className="border-t pt-2"
              >
                <p>
                  {capabilityScopeLabel(item.scope, lang)} · #{item.attempt} ·{" "}
                  {item.meteringStatus === "not_in_usd_ledger"
                    ? lang === 0
                      ? "未记录美元扣减"
                      : "No USD quota debit recorded"
                    : `${item.meteringStatus} · ${capabilityAmount(item.settledPicos)}`}
                </p>
                {item.estimated ? (
                  <p>
                    {lang === 0
                      ? "资源数量包含估算"
                      : "Resource quantities include estimates"}
                  </p>
                ) : null}
                {item.unpricedReason ? (
                  <p>
                    {lang === 0
                      ? "含未定价用量："
                      : "Includes unpriced usage: "}
                    {item.unpricedReason}
                  </p>
                ) : null}
                {item.pendingRiskPicos && item.pendingRiskPicos !== "0" ? (
                  <p>
                    {lang === 0
                      ? "待结算风险预占："
                      : "Pending risk reservation: "}
                    {capabilityAmount(item.pendingRiskPicos)}
                  </p>
                ) : null}
                {item.dimensions.map((dimension) => (
                  <p key={dimension.dimension}>
                    {dimension.dimension} · {dimension.quantity} ·{" "}
                    {dimension.pricing === "not_in_usd_ledger"
                      ? lang === 0
                        ? "未记录美元扣减"
                        : "No USD quota debit recorded"
                      : dimension.pricing === "missing"
                        ? lang === 0
                          ? "未定价"
                          : "Unpriced"
                        : capabilityAmount(dimension.amountPicos)}
                  </p>
                ))}
                {item.pricingRevision ? (
                  <p className="break-all text-muted-foreground">
                    {lang === 0 ? "冻结价目版本：" : "Frozen tariff revision: "}
                    {item.pricingRevision}
                  </p>
                ) : null}
              </div>
            ))}
          </div>
        </details>
      ) : null}
      <Dialog
        open={!!review.prompt}
        onOpenChange={(open) => {
          if (!open) review.cancel();
        }}
      >
        <DialogContent className="max-h-[85dvh] min-w-0 w-[calc(100vw_-_2rem)] overflow-hidden rounded-lg transition-none [overflow-wrap:anywhere] sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>
              {lang === 0 ? "确认扩展能力变更" : "Review extension changes"}
            </DialogTitle>
          </DialogHeader>
          <div className="min-h-0 space-y-4 overflow-y-auto px-6 py-4">
          {review.prompt?.migration ? (
            <p className="text-sm">
              {lang === 0
                ? "保存后开始独立控制扩展能力，未选择的能力将关闭。"
                : "Saving activates independent permissions. Unselected extensions will be disabled."}
            </p>
          ) : null}
          <p className="text-sm">
            {lang === 0 ? "允许：" : "Allow: "}
            {review.prompt?.preview.policy.allowedOperations
              .map((op) => OPERATION_LABELS[op][lang])
              .join(lang === 0 ? "、" : ", ") ||
              (lang === 0 ? "无扩展能力" : "No extensions")}
          </p>
          {review.prompt?.groups.map((group) => (
            <div className="space-y-2 border-t pt-3 text-sm" key={group.key}>
              <p>
                {COVERAGE_LABELS[group.reason]?.[lang] ??
                  (lang === 0
                    ? "部分资源无法完整计量或扣减额度。"
                    : "Some resources cannot be fully measured or debited.")}
              </p>
              <p className="text-xs text-muted-foreground">
                {group.operations
                  .map((op) => OPERATION_LABELS[op][lang])
                  .join(lang === 0 ? "、" : ", ")}
              </p>
              <details className="text-xs">
                <summary className="cursor-pointer">
                  {lang === 0
                    ? "受影响的用户与访问范围"
                    : "Affected users and access"}
                </summary>
                {group.scopes.map((scope) => (
                  <p key={scope} className="break-all">
                    {capabilityScopeLabel(scope, lang)}
                  </p>
                ))}
              </details>
              <label className="flex items-start gap-2">
                <input
                  type="checkbox"
                  className="mt-0.5 h-4 min-h-4 w-4 shrink-0 p-0 accent-primary"
                  checked={review.accepted.includes(group.key)}
                  onChange={(event) =>
                    review.setAccepted((current) =>
                      event.target.checked
                        ? [...current, group.key]
                        : current.filter((key) => key !== group.key),
                    )
                  }
                />
                {group.reason === "runtime_binding_changed"
                  ? lang === 0
                    ? "为当前绑定重新授权"
                    : "Authorize the current binding"
                  : lang === 0
                    ? "我了解并允许上述未覆盖消耗"
                    : "I understand and allow this uncovered usage"}
              </label>
            </div>
          ))}
          <DialogDescription className="text-xs">
            {lang === 0
              ? "美元为 API 等值额度；估算和预占不能保证上游执行不会超量。"
              : "USD is API-equivalent quota. Estimates and reservations do not guarantee an upstream execution limit."}
          </DialogDescription>
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={review.cancel}>
              {lang === 0 ? "返回修改" : "Back"}
            </Button>
            <Button
              type="button"
              disabled={review.prompt?.groups.some(
                (group) => !review.accepted.includes(group.key),
              )}
              onClick={review.confirm}
            >
              {lang === 0 ? "确认并继续保存" : "Confirm and save"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
