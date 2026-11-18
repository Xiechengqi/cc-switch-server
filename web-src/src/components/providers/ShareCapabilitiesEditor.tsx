import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { shareApi, type ShareUserGrantMap } from "@/lib/api/share";
import {
  capabilityAmount,
  SHARE_OPERATIONS,
  OPERATION_LABELS,
  COVERAGE_LABELS,
  emptyCapabilityPolicy,
  type CapabilityPreview,
  type ShareCapabilityPolicy,
  type CapabilityConsent,
} from "@/lib/share-capabilities";

/** Server-native capability editor. All execution/coverage facts come from the local domain service. */
export function ShareCapabilitiesEditor({
  shareId,
  revision,
  policy,
  savedPolicy,
  grants,
  tokenLimit,
  disabled,
  onChange,
  onGrantOperationsChange,
}: {
  shareId?: string;
  revision?: number;
  policy?: ShareCapabilityPolicy;
  savedPolicy?: ShareCapabilityPolicy;
  grants: ShareUserGrantMap;
  tokenLimit: number;
  disabled?: boolean;
  onChange: (
    policy: ShareCapabilityPolicy,
    consent?: CapabilityConsent,
  ) => void;
  onGrantOperationsChange: (
    email: string,
    operations?: ShareCapabilityPolicy["allowedOperations"],
  ) => void;
}) {
  const { i18n } = useTranslation();
  const lang = i18n.language.startsWith("zh") ? 0 : 1;
  const [preview, setPreview] = useState<CapabilityPreview>();
  const [error, setError] = useState("");
  const [accept, setAccept] = useState<string[]>([]);
  const [refresh, setRefresh] = useState(0);
  const [bulkEmails, setBulkEmails] = useState<string[]>([]);
  const [bulkOps, setBulkOps] = useState<
    ShareCapabilityPolicy["allowedOperations"]
  >([]);
  const ops = policy?.allowedOperations ?? [];
  const inheritedGrants = Object.values(grants).filter(
    (grant) => grant.active && grant.policy.allowedOperations === undefined,
  ).length;
  const key = JSON.stringify([
    shareId,
    revision,
    ops,
    policy?.operationBindings,
    Object.entries(grants)
      .filter(([, g]) => g.active)
      .map(([e, g]) => [e, g.policy]),
    tokenLimit,
    refresh,
  ]);
  useEffect(() => {
    setPreview(undefined);
    setError("");
    setAccept([]);
    if (!shareId || revision == null) return;
    const abort = new AbortController();
    const timer = setTimeout(() => {
      void shareApi
        .previewCapabilities(
          shareId,
          {
            expectedConfigRevision: revision,
            replacePolicies: true,
            policy: policy,
            policies: Object.fromEntries(
              Object.entries(grants)
                .filter(([, g]) => g.active)
                .map(([email, g]) => [email, g.policy]),
            ),
            shareTokenLimit: tokenLimit,
          },
          abort.signal,
        )
        .then((value) => {
          if (!abort.signal.aborted) setPreview(value);
        })
        .catch((err) => {
          if (!abort.signal.aborted) setError(String(err));
        });
    }, 200);
    return () => {
      abort.abort();
      clearTimeout(timer);
    };
    // Credentials and audit records do not alter the execution intent.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const required =
    preview?.requiredAcknowledgements.filter(
      (a) =>
        !savedPolicy?.coverageAcknowledgements?.some(
          (old) => old.condition === a.condition,
        ),
    ) ?? [];
  return (
    <div
      className="space-y-3 rounded-lg border p-4"
      aria-label={lang === 0 ? "扩展能力" : "Extended capabilities"}
    >
      <h4 className="font-medium">
        {lang === 0 ? "扩展能力" : "Extended capabilities"}
      </h4>
      <p className="text-xs text-muted-foreground">
        {lang === 0
          ? "独立于 Token、美元额度。理解图片、文档、音频与视频输入不受这些生成开关影响。"
          : "Independent of Token and USD quotas. Understanding image, document, audio and video inputs remains available."}
      </p>
      {!shareId ? (
        <p className="text-xs">
          {lang === 0
            ? "新 Share 默认关闭扩展生成。创建后可根据实际绑定预览并开启。"
            : "New Shares start with extensions disabled. Preview the actual binding after creation to enable operations."}
        </p>
      ) : null}
      <p className="text-xs text-muted-foreground">
        {lang === 0
          ? `${inheritedGrants} 个有效授权继承 Share 能力，保存后的权限变化将对这些授权生效。`
          : `${inheritedGrants} active grants inherit Share capabilities and will follow the saved permissions.`}
      </p>
      {!policy && preview?.policy.allowedOperations.length ? (
        <button
          type="button"
          disabled={disabled}
          className="text-left text-xs underline"
          onClick={() => onChange(preview.policy)}
        >
          {lang === 0
            ? "采用原 Grok 显式许可作为迁移草稿"
            : "Start with explicitly permitted legacy Grok operations"}
        </button>
      ) : null}
      {shareId && !savedPolicy ? (
        <p className="text-xs text-amber-700">
          {lang === 0
            ? "当前为旧兼容策略，确认后采用独立能力控制。"
            : "Legacy authorization is active. Confirm to adopt independent capability control."}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      ) : null}
      {preview && !preview.ready ? (
        <p role="alert" className="text-xs text-amber-700">
          {lang === 0
            ? "需要新协议协商与权威存储就绪。"
            : "Protocol negotiation and authoritative storage must be ready."}
        </p>
      ) : null}
      {SHARE_OPERATIONS.map((op) => {
        const rows = preview?.rows.filter((row) => row.operation === op) ?? [];
        return (
          <div key={op} className="space-y-1 border-t pt-2">
            <label className="flex gap-2 text-sm">
              <input
                type="checkbox"
                checked={ops.includes(op)}
                disabled={
                  disabled ||
                  !preview?.ready ||
                  (!rows.some((row) => row.controllable) && !ops.includes(op))
                }
                onChange={(e) => {
                  const bindings = { ...policy?.operationBindings };
                  delete bindings[op];
                  onChange({
                    ...emptyCapabilityPolicy(),
                    operationBindings: bindings,
                    allowedOperations: e.target.checked
                      ? [...ops, op]
                      : ops.filter((v) => v !== op),
                  });
                }}
              />
              {OPERATION_LABELS[op][lang]}
            </label>
            {rows.map((row) => (
              <p key={row.app} className="ml-5 text-xs text-muted-foreground">
                {ops.includes(op) ? (
                  <input
                    type="checkbox"
                    aria-label={`${op} ${row.app}`}
                    disabled={disabled || !row.controllable}
                    checked={row.selected ?? false}
                    onChange={(event) => {
                      const bindings =
                        preview?.policy.operationBindings[op] ?? [];
                      onChange({
                        ...preview!.policy,
                        operationBindings: {
                          ...preview!.policy.operationBindings,
                          [op]: event.target.checked
                            ? [
                                ...bindings.filter((b) => b.app !== row.app),
                                {
                                  app: row.app,
                                  fingerprint: row.bindingFingerprint,
                                },
                              ]
                            : bindings.filter((b) => b.app !== row.app),
                        },
                      });
                    }}
                  />
                ) : null}
                {row.app} ·{" "}
                {row.execution === "supported"
                  ? lang === 0
                    ? "账号证据支持"
                    : "Supported by account evidence"
                  : row.execution === "unsupported"
                    ? lang === 0
                      ? "当前账号不支持"
                      : "Unsupported by the current account"
                    : row.controllable
                      ? lang === 0
                        ? "可控制；账号待实测"
                        : "Controllable; live verification pending"
                      : lang === 0
                        ? "当前未实现或未开启"
                        : "Not implemented or enabled"}{" "}
                · {row.nativeUnit} ·{" "}
                {row.pricing === "explicit_free"
                  ? lang === 0
                    ? "资源价格明确为零"
                    : "Resource rate explicitly zero"
                  : row.pricing === "priced"
                    ? lang === 0
                      ? "资源价已配置"
                      : "Resource price configured"
                    : lang === 0
                      ? "价格或模型范围未覆盖"
                      : "Model range or price incomplete"}
              </p>
            ))}
          </div>
        );
      })}
      <details>
        <summary className="cursor-pointer text-sm">
          {lang === 0 ? "用户有效能力与权限范围" : "User capability scopes"}
        </summary>
        <div className="mt-2 space-y-3">
          {Object.entries(grants)
            .filter(([, g]) => g.active)
            .map(([email, g]) => (
              <div key={email} className="text-xs">
                <span>{email}</span>
                {g.manager === "routerShareMarket" ? (
                  <span>
                    {" "}
                    · {lang === 0 ? "市场合同固定" : "Frozen Market contract"}
                  </span>
                ) : (
                  <>
                    <select
                      aria-label={`${email} capabilities`}
                      className="ml-2 rounded border p-1"
                      value={
                        g.policy.allowedOperations === undefined
                          ? "inherit"
                          : "explicit"
                      }
                      disabled={disabled}
                      onChange={(e) =>
                        onGrantOperationsChange(
                          email,
                          e.target.value === "inherit" ? undefined : [],
                        )
                      }
                    >
                      <option value="inherit">
                        {lang === 0 ? "继承 Share" : "Inherit Share"}
                      </option>
                      <option value="explicit">
                        {lang === 0 ? "仅允许勾选项" : "Explicit subset"}
                      </option>
                    </select>
                    {g.policy.allowedOperations !== undefined ? (
                      <div className="mt-1 flex flex-wrap gap-2">
                        {ops.map((op) => (
                          <label key={op} className="flex gap-1">
                            <input
                              type="checkbox"
                              checked={g.policy.allowedOperations?.includes(op)}
                              disabled={disabled}
                              onChange={(e) =>
                                onGrantOperationsChange(
                                  email,
                                  e.target.checked
                                    ? [...g.policy.allowedOperations!, op]
                                    : g.policy.allowedOperations!.filter(
                                        (v) => v !== op,
                                      ),
                                )
                              }
                            />
                            {OPERATION_LABELS[op][lang]}
                          </label>
                        ))}
                      </div>
                    ) : null}
                  </>
                )}
              </div>
            ))}
        </div>
      </details>
      <details>
        <summary className="cursor-pointer text-xs">
          {lang === 0 ? "批量修改能力权限" : "Batch capability permissions"}
        </summary>
        <div className="my-2 flex flex-wrap gap-3 text-xs">
          {Object.entries(grants)
            .filter(([, g]) => g.active && g.manager !== "routerShareMarket")
            .map(([email]) => (
              <label key={email} className="flex gap-1">
                <input
                  type="checkbox"
                  checked={bulkEmails.includes(email)}
                  disabled={disabled}
                  onChange={(e) =>
                    setBulkEmails((current) =>
                      e.target.checked
                        ? [...current, email]
                        : current.filter((v) => v !== email),
                    )
                  }
                />
                {email}
              </label>
            ))}
        </div>
        <div className="my-2 flex flex-wrap gap-3 text-xs">
          {ops.map((op) => (
            <label key={op} className="flex gap-1">
              <input
                type="checkbox"
                checked={bulkOps.includes(op)}
                disabled={disabled}
                onChange={(e) =>
                  setBulkOps((current) =>
                    e.target.checked
                      ? [...current, op]
                      : current.filter((v) => v !== op),
                  )
                }
              />
              {OPERATION_LABELS[op][lang]}
            </label>
          ))}
        </div>
        <div className="flex gap-3 text-xs">
          <button
            type="button"
            disabled={disabled || !bulkEmails.length}
            className="underline"
            onClick={() =>
              bulkEmails
                .filter(
                  (e) =>
                    grants[e]?.active &&
                    grants[e].manager !== "routerShareMarket",
                )
                .forEach((email) => onGrantOperationsChange(email, undefined))
            }
          >
            {lang === 0 ? "选中用户继承 Share" : "Selected users inherit Share"}
          </button>
          <button
            type="button"
            disabled={disabled || !bulkEmails.length}
            className="underline"
            onClick={() =>
              bulkEmails
                .filter(
                  (e) =>
                    grants[e]?.active &&
                    grants[e].manager !== "routerShareMarket",
                )
                .forEach((email) =>
                  onGrantOperationsChange(
                    email,
                    bulkOps.filter((op) => ops.includes(op)),
                  ),
                )
            }
          >
            {lang === 0
              ? "选中用户仅允许勾选项（空表示禁用）"
              : "Limit selected users to checked operations (empty disables)"}
          </button>
        </div>
      </details>
      {preview?.blockers?.some(
        (b) => b.blocker === "VALUE_QUOTA_RESOURCE_POLICY_REQUIRED",
      ) ? (
        <button
          type="button"
          className="text-left text-xs underline"
          disabled={disabled}
          onClick={() => onChange(policy ?? emptyCapabilityPolicy())}
        >
          {lang === 0
            ? "为选中能力采用资源价卡并重新预览（保留缺价策略）"
            : "Use resource tariff and preview again (preserves missing-price policy)"}
        </button>
      ) : null}
      {preview?.blockers?.map((b, index) => (
        <p
          key={`${b.scope}-${index}`}
          role="alert"
          className="text-xs text-destructive"
        >
          {b.scope}:{" "}
          {b.blocker === "VALUE_QUOTA_RESOURCE_POLICY_REQUIRED"
            ? lang === 0
              ? "此美元授权需采用资源价卡。请调整能力选择后重新预览。"
              : "This USD grant requires the resource tariff. Refresh the capability selection and preview."
            : b.blocker}
        </p>
      ))}
      <details>
        <summary className="cursor-pointer text-xs">
          {lang === 0 ? "计量与额度覆盖详情" : "Measurement and quota coverage"}
        </summary>
        {preview?.rows
          .filter((row) => ops.includes(row.operation))
          .map((row) => (
            <div key={`${row.app}-${row.operation}`} className="my-2 text-xs">
              <span>
                {row.app} · {OPERATION_LABELS[row.operation][lang]} ·{" "}
                {lang === 0
                  ? "估算数量与软预占"
                  : "Estimated quantities and soft reservation"}
              </span>
              {row.scopes.map((scope) => (
                <p key={scope.scope}>
                  {scope.scope} · {scope.quotaUnit} · {scope.coverage} ·{" "}
                  {COVERAGE_LABELS[scope.reason]?.[lang] ?? scope.reason}
                </p>
              ))}
            </div>
          ))}
      </details>
      {required.length ? (
        <fieldset className="max-h-60 space-y-2 overflow-y-auto rounded bg-amber-50 p-3 text-xs">
          <legend>
            {lang === 0
              ? "未覆盖消耗，需要明确接受"
              : "Explicit acceptance of uncovered usage"}
          </legend>
          {required.map((a) => {
            const scope = preview?.rows
              .flatMap((row) => row.scopes)
              .find((s) => s.condition === a.condition);
            return (
              <label key={a.condition} className="flex gap-2">
                <input
                  type="checkbox"
                  checked={accept.includes(a.condition)}
                  disabled={disabled}
                  onChange={(e) =>
                    setAccept((current) =>
                      e.target.checked
                        ? [...current, a.condition]
                        : current.filter((c) => c !== a.condition),
                    )
                  }
                />
                <span>
                  {a.scope} · {OPERATION_LABELS[a.operation][lang]} ·{" "}
                  {a.quotaUnit}:{" "}
                  {COVERAGE_LABELS[scope?.reason ?? ""]?.[lang] ??
                    scope?.reason}
                </span>
              </label>
            );
          })}
        </fieldset>
      ) : null}
      <div className="flex gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={disabled || !shareId}
          onClick={() => setRefresh((v) => v + 1)}
        >
          {lang === 0 ? "刷新预览" : "Refresh preview"}
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={
            disabled ||
            !preview?.ready ||
            preview.expiresAt <= Date.now() ||
            (preview?.blockers?.length ?? 0) > 0 ||
            required.some((a) => !accept.includes(a.condition))
          }
          onClick={() => {
            if (preview?.previewCredential)
              onChange(preview.policy, {
                credential: preview.previewCredential,
                acceptUncovered: accept,
              });
          }}
        >
          {lang === 0 ? "确认能力配置" : "Confirm capabilities"}
        </Button>
      </div>
      {preview?.incidents?.length ? (
        <div role="alert" className="rounded border border-red-300 p-2 text-xs">
          <p>
            {lang === 0
              ? "发现未计划的资源执行，受影响的用户与运行时已隔离。需修复控制路径后升级执行版本或重新绑定，再预览确认；历史用量与证据保留。"
              : "Unplanned resource execution quarantined the affected user and runtime. Repair the control path, upgrade its execution version or rebind, then preview again. Historical usage and evidence are retained."}
          </p>
          {preview.incidents.map((item) => (
            <p key={`${item.requestId}-${item.resource}`}>
              {item.scope} · {item.resource} ·{" "}
              {item.bindingFingerprint.slice(0, 12)}
            </p>
          ))}
        </div>
      ) : null}
      {preview?.recentUsage?.length ? (
        <details>
          <summary className="text-xs">
            {lang === 0
              ? "近期执行的冻结计量明细"
              : "Frozen metering for recent executions"}
          </summary>
          <ul className="max-h-64 space-y-2 overflow-auto text-xs">
            {preview.recentUsage.map((row) => (
              <li
                key={`${row.requestId}-${row.attempt}`}
                className="rounded border p-2"
              >
                <p>
                  {row.scope} · #{row.attempt} ·{" "}
                  {row.meteringStatus === "not_in_usd_ledger"
                    ? lang === 0
                      ? "未记录美元扣减"
                      : "No USD quota debit recorded"
                    : row.meteringStatus}
                  : {capabilityAmount(row.settledPicos)}
                  {row.unpricedReason
                    ? lang === 0
                      ? "，另有未计价用量"
                      : ", additional unpriced usage"
                    : null}
                </p>
                {row.dimensions.map((d) => (
                  <p key={d.dimension}>
                    {d.dimension} × {d.quantity}:{" "}
                    {capabilityAmount(d.amountPicos)} · {d.pricing}
                    {row.estimated
                      ? lang === 0
                        ? "，估算"
                        : ", estimated"
                      : null}
                  </p>
                ))}
                <p className="break-all">{row.pricingRevision}</p>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
      <p className="text-xs text-muted-foreground">
        {lang === 0
          ? "确认后与 Share 一起保存；不会清零历史用量。"
          : "Confirm, then save the Share. Historical usage is preserved."}
      </p>
    </div>
  );
}
