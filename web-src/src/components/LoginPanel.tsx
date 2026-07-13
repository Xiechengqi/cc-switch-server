import { FormEvent, useEffect, useMemo, useState } from "react";
import { KeyRound, Loader2, Mail, Shield } from "lucide-react";

import { AuthLanguageSwitcher } from "@/components/AuthLanguageSwitcher";
import { AuthPasswordInput } from "@/components/AuthPasswordInput";
import { ShareRouterSelector } from "@/components/share/ShareRouterSelector";
import { SubdomainGeneratorButton } from "@/components/SubdomainGeneratorButton";
import {
  checkSetupRouter,
  checkSetupSubdomain,
  completeServerSetup,
  loginWithApiToken,
  requestEmailLoginCode,
  suggestSetupSubdomain,
  verifyEmailLoginCode,
} from "@/lib/server-legacy-api";
import { DEFAULT_SHARE_ROUTER_DOMAIN } from "@/config/shareRegions";
import { useI18n } from "@/lib/i18n";
import { loginWithPassword, readCachedPassword, WebRuntimeContext, writeToken } from "@/lib/runtime";
import {
  normalizeShareRouterDomain,
  shareRouterUrlFromDomain,
} from "@/utils/shareRouter";

type LoginMethod = "password" | "email" | "apiToken";

function normalizeMethods(context: WebRuntimeContext): LoginMethod[] {
  // LoginPanel is only rendered for direct server access (IP/loopback:15721).
  // Router-backed API Token login stays on ClientWebLoginPage (tunnel URL).
  if (context.status === "setup-required" || context.auth?.setupRequired) {
    return ["password"];
  }
  const methods: LoginMethod[] = ["password"];
  if (context.auth?.ownerEmail?.trim()) {
    methods.push("email");
  }
  return methods;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isSetupAlreadyCompleteError(error: unknown): boolean {
  return errorMessage(error).toLowerCase().includes("already complete");
}

function formatSetupError(error: unknown): string {
  const message = errorMessage(error);
  if (message.includes("client_tunnel_subdomain_conflict")) {
    return "该 Client 子域名已被其他安装占用，请更换或使用随机生成。";
  }
  return message;
}

async function saveSetupConfig(input: {
  password: string;
  ownerEmail: string;
  routerDomain: string;
  clientTunnelSubdomain?: string;
}) {
  const routerUrl = shareRouterUrlFromDomain(input.routerDomain);
  try {
    await completeServerSetup({
      password: input.password,
      ownerEmail: input.ownerEmail,
      routerUrl,
      clientTunnelSubdomain: input.clientTunnelSubdomain,
    });
  } catch (error) {
    if (!isSetupAlreadyCompleteError(error)) {
      throw error;
    }
  }
}

export function LoginPanel({
  context,
  onAuthenticated,
}: {
  context: WebRuntimeContext;
  onAuthenticated: () => Promise<WebRuntimeContext>;
}) {
  const { t } = useI18n();
  const setupRequired =
    context.status === "setup-required" || context.auth?.setupRequired;
  const availableMethods = useMemo(() => normalizeMethods(context), [context]);
  const [activeMethod, setActiveMethod] = useState<LoginMethod>(
    availableMethods[0] ?? "password",
  );
  const [password, setPassword] = useState(() => readCachedPassword() ?? "");
  const [ownerEmail, setOwnerEmail] = useState(context.auth?.ownerEmail ?? "");
  const [routerDomain, setRouterDomain] = useState(DEFAULT_SHARE_ROUTER_DOMAIN);
  const [routerDomainError, setRouterDomainError] = useState<string | null>(null);
  const [clientTunnelSubdomain, setClientTunnelSubdomain] = useState("");
  const [subdomainStatus, setSubdomainStatus] = useState<
    "idle" | "checking" | "available" | "unavailable"
  >("idle");
  const [subdomainHint, setSubdomainHint] = useState<string | null>(null);
  const [routerReachable, setRouterReachable] = useState<boolean | null>(null);
  const [verificationCode, setVerificationCode] = useState("");
  const [apiToken, setApiToken] = useState("");
  const [codeHint, setCodeHint] = useState<string | null>(null);
  const [resendCooldown, setResendCooldown] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const loginOwnerEmail = context.auth?.ownerEmail?.trim() ?? "";

  useEffect(() => {
    if (!setupRequired) return;
    const trimmed = clientTunnelSubdomain.trim();
    if (!trimmed) {
      setSubdomainStatus("idle");
      setSubdomainHint(
        t("server.auth.clientSubdomainAuto", {
          defaultValue: "留空将自动生成随机单词子域名",
        }),
      );
      return;
    }
    let active = true;
    setSubdomainStatus("checking");
    setSubdomainHint(null);
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const normalizedRouterDomain = normalizeShareRouterDomain(routerDomain);
          const response = await checkSetupSubdomain({
            routerUrl: shareRouterUrlFromDomain(normalizedRouterDomain),
            subdomain: trimmed,
          });
          if (!active) return;
          if (response.available) {
            setSubdomainStatus("available");
            setSubdomainHint(
              t("server.auth.clientSubdomainAvailable", {
                defaultValue: "子域名可用",
              }),
            );
          } else {
            setSubdomainStatus("unavailable");
            setSubdomainHint(
              t("server.auth.clientSubdomainTaken", {
                defaultValue: "子域名已被占用，请更换或使用随机生成",
              }),
            );
          }
        } catch (reason) {
          if (!active) return;
          setSubdomainStatus("idle");
          setSubdomainHint(formatSetupError(reason));
        }
      })();
    }, 400);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [clientTunnelSubdomain, routerDomain, setupRequired, t]);

  useEffect(() => {
    if (!setupRequired) return;
    let active = true;
    setRouterReachable(null);
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const normalizedRouterDomain = normalizeShareRouterDomain(routerDomain);
          const response = await checkSetupRouter({
            routerUrl: shareRouterUrlFromDomain(normalizedRouterDomain),
          });
          if (!active) return;
          setRouterReachable(response.reachable);
        } catch {
          if (!active) return;
          setRouterReachable(false);
        }
      })();
    }, 400);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [routerDomain, setupRequired]);

  useEffect(() => {
    if (!availableMethods.includes(activeMethod)) {
      setActiveMethod(availableMethods[0] ?? "password");
    }
  }, [activeMethod, availableMethods]);

  useEffect(() => {
    if (context.auth?.ownerEmail) {
      setOwnerEmail(context.auth.ownerEmail);
    }
  }, [context.auth?.ownerEmail]);

  useEffect(() => {
    if (resendCooldown <= 0) return;
    const timer = window.setTimeout(() => {
      setResendCooldown((value) => Math.max(0, value - 1));
    }, 1000);
    return () => window.clearTimeout(timer);
  }, [resendCooldown]);

  async function completeLogin(token: string) {
    writeToken(token);
    await onAuthenticated();
  }

  async function submitPassword(event?: FormEvent) {
    event?.preventDefault();
    setError(null);
    setRouterDomainError(null);
    if (setupRequired && subdomainStatus === "unavailable") {
      setError(
        t("server.auth.clientSubdomainTaken", {
          defaultValue: "子域名已被占用，请更换或使用随机生成",
        }),
      );
      return;
    }
    setBusy("password");
    try {
      if (setupRequired) {
        let normalizedRouterDomain: string;
        try {
          normalizedRouterDomain = normalizeShareRouterDomain(routerDomain);
        } catch (reason) {
          const key = errorMessage(reason);
          setRouterDomainError(
            t(key, { defaultValue: "Router 域名无效" }),
          );
          return;
        }
        await saveSetupConfig({
          password,
          ownerEmail,
          routerDomain: normalizedRouterDomain,
          clientTunnelSubdomain: clientTunnelSubdomain.trim() || undefined,
        });
        await loginWithPassword(password);
        await onAuthenticated();
        return;
      }
      await loginWithPassword(password);
      await onAuthenticated();
    } catch (reason) {
      setError(
        setupRequired ? formatSetupError(reason) : errorMessage(reason),
      );
    } finally {
      setBusy(null);
    }
  }

  async function submitApiToken(event?: FormEvent) {
    event?.preventDefault();
    setError(null);
    setBusy("apiToken");
    try {
      const login = await loginWithApiToken(apiToken.trim());
      await completeLogin(login.token);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function requestCode() {
    if (!loginOwnerEmail) return;
    setError(null);
    setBusy("requestCode");
    try {
      const response = await requestEmailLoginCode(loginOwnerEmail);
      setCodeHint(
        t("server.auth.codeSentTo", {
          defaultValue: "验证码已发送至 {{destination}}",
          destination: response.maskedDestination,
        }),
      );
      setResendCooldown(Math.max(response.cooldownSecs ?? 60, 1));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function submitEmail(event?: FormEvent) {
    event?.preventDefault();
    const normalizedCode = verificationCode.trim();
    if (!loginOwnerEmail || !normalizedCode) return;
    setError(null);
    setBusy("email");
    try {
      const login = await verifyEmailLoginCode({
        email: loginOwnerEmail,
        code: normalizedCode,
      });
      await completeLogin(login.token);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault();
    if (setupRequired || activeMethod === "password") {
      void submitPassword();
      return;
    }
    if (activeMethod === "email") {
      void submitEmail();
      return;
    }
    if (activeMethod === "apiToken") {
      void submitApiToken();
    }
  };

  const showMethodTabs = !setupRequired && availableMethods.length > 1;

  return (
    <div className="auth-shell">
      <AuthLanguageSwitcher />
      <div className="auth-shell-card">
        <form className="auth-panel" onSubmit={handleSubmit}>
          <div className="auth-panel-header">
            <div className="auth-panel-brand">
              <img
                src="./favicon.png"
                alt=""
                className="auth-panel-logo"
                width={40}
                height={40}
              />
              <div>
                <strong>{t("server.common.server")}</strong>
                <span>
                  {setupRequired
                    ? t("server.auth.setupTitle", {
                        defaultValue: "初始化 Server",
                      })
                    : t("server.auth.loginTitle", {
                        defaultValue: "登录管理控制台",
                      })}
                </span>
              </div>
            </div>
            <p className="auth-panel-subtitle">
              {setupRequired
                ? t("server.auth.setupSubtitle", {
                    defaultValue: "设置 Owner 邮箱、Router 与管理员密码。",
                  })
                : t("server.auth.loginSubtitlePasswordOnly")}
            </p>
          </div>

          {showMethodTabs ? (
            <div
              className="segmented auth-method-tabs"
              style={{
                gridTemplateColumns: `repeat(${availableMethods.length}, minmax(0, 1fr))`,
              }}
            >
              {availableMethods.includes("password") ? (
                <button
                  type="button"
                  className={activeMethod === "password" ? "active" : ""}
                  onClick={() => {
                    setActiveMethod("password");
                    setError(null);
                  }}
                >
                  {t("server.auth.methodPassword", { defaultValue: "密码" })}
                </button>
              ) : null}
              {availableMethods.includes("email") ? (
                <button
                  type="button"
                  className={activeMethod === "email" ? "active" : ""}
                  onClick={() => {
                    setActiveMethod("email");
                    setError(null);
                  }}
                >
                  {t("server.auth.methodEmail", { defaultValue: "邮箱验证码" })}
                </button>
              ) : null}
              {availableMethods.includes("apiToken") ? (
                <button
                  type="button"
                  className={activeMethod === "apiToken" ? "active" : ""}
                  onClick={() => {
                    setActiveMethod("apiToken");
                    setError(null);
                  }}
                >
                  {t("server.auth.methodApiToken", { defaultValue: "API Token" })}
                </button>
              ) : null}
            </div>
          ) : null}

          {setupRequired ? (
            <div className="auth-grid">
              <label>
                <span>{t("server.auth.ownerEmail")}</span>
                <input
                  value={ownerEmail}
                  onChange={(event) => setOwnerEmail(event.target.value)}
                  autoComplete="email"
                />
              </label>
              <label>
                <span>
                  {t("share.tunnel.region", { defaultValue: "路由节点" })}
                </span>
                <ShareRouterSelector
                  value={routerDomain}
                  onChange={(value) => {
                    setRouterDomain(value);
                    setRouterDomainError(null);
                  }}
                  selectId="setup-share-router"
                  customInputId="setup-share-router-custom"
                  disabled={busy !== null}
                  error={routerDomainError}
                />
              </label>
              <label>
                <span>{t("server.auth.clientSubdomain")}</span>
                <div className="flex items-center gap-2">
                  <input
                    className="min-w-0 flex-1"
                    value={clientTunnelSubdomain}
                    autoComplete="off"
                    disabled={busy !== null}
                    onChange={(event) =>
                      setClientTunnelSubdomain(event.target.value)
                    }
                  />
                  <SubdomainGeneratorButton
                    disabled={
                      busy !== null ||
                      routerReachable !== true ||
                      !routerDomain.trim()
                    }
                    onGenerated={setClientTunnelSubdomain}
                    onError={setError}
                    suggest={async () => {
                      const normalizedRouterDomain =
                        normalizeShareRouterDomain(routerDomain);
                      return suggestSetupSubdomain({
                        routerUrl: shareRouterUrlFromDomain(
                          normalizedRouterDomain,
                        ),
                      });
                    }}
                  />
                </div>
                {routerReachable === false ? (
                  <span className="text-xs text-muted-foreground">
                    {t("server.auth.routerUnreachableForSubdomain", {
                      defaultValue: "Router 不可达，无法随机生成子域名",
                    })}
                  </span>
                ) : null}
                {subdomainHint ? (
                  <span
                    className={
                      subdomainStatus === "unavailable"
                        ? "text-xs text-red-600 dark:text-red-400"
                        : "text-xs text-muted-foreground"
                    }
                  >
                    {subdomainHint}
                  </span>
                ) : null}
              </label>
              <AuthPasswordInput
                label={t("server.common.password")}
                autoComplete="new-password"
                value={password}
                onChange={setPassword}
              />
            </div>
          ) : null}

          {!setupRequired && activeMethod === "password" ? (
            <AuthPasswordInput
              label={t("server.common.password")}
              autoComplete="current-password"
              value={password}
              onChange={setPassword}
            />
          ) : null}

          {!setupRequired && activeMethod === "email" ? (
            <div className="auth-grid">
              <label className="auth-grid-span-2">
                <span>{t("server.auth.ownerEmail")}</span>
                <input
                  readOnly
                  value={loginOwnerEmail}
                  className="auth-readonly-input"
                />
              </label>
              <label>
                <span>{t("server.settings.verificationCode")}</span>
                <input
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  value={verificationCode}
                  onChange={(event) => setVerificationCode(event.target.value)}
                  placeholder="123456"
                />
              </label>
              <div className="auth-inline-actions">
                <button
                  className="secondary-button"
                  type="button"
                  disabled={
                    !loginOwnerEmail || busy !== null || resendCooldown > 0
                  }
                  onClick={() => void requestCode()}
                >
                  {busy === "requestCode" ? (
                    <Loader2 size={15} className="spin" />
                  ) : (
                    <Mail size={15} />
                  )}
                  <span>
                    {resendCooldown > 0
                      ? t("server.auth.resendIn", {
                          defaultValue: "{{seconds}} 秒后可重发",
                          seconds: resendCooldown,
                        })
                      : t("server.settings.requestCode")}
                  </span>
                </button>
              </div>
              {codeHint ? (
                <p className="auth-hint auth-grid-span-2">{codeHint}</p>
              ) : null}
              <p className="auth-hint auth-grid-span-2">
                {t("server.auth.emailLoginHint", {
                  defaultValue:
                    "验证码由 Router 发送到已配置的 Owner 邮箱。",
                })}
              </p>
            </div>
          ) : null}

          {!setupRequired && activeMethod === "apiToken" ? (
            <div className="auth-grid">
              <AuthPasswordInput
                className="auth-grid-span-2"
                label={t("server.auth.apiToken", { defaultValue: "API Token" })}
                autoComplete="off"
                placeholder="ccs_..."
                value={apiToken}
                onChange={setApiToken}
              />
              <p className="auth-hint auth-grid-span-2">
                {t("server.auth.apiTokenHint", {
                  defaultValue:
                    "使用初始化或设置页轮换得到的 API Token 登录。",
                })}
              </p>
            </div>
          ) : null}

          {error ? <div className="form-error">{error}</div> : null}

          {setupRequired || activeMethod === "password" ? (
            <button
              className="primary-button"
              type="submit"
              disabled={
                busy !== null ||
                !password ||
                (setupRequired &&
                  (!ownerEmail.trim() || !routerDomain.trim()))
              }
            >
              {busy === "password" ? (
                <Loader2 size={16} className="spin" />
              ) : (
                <KeyRound size={16} />
              )}
              <span>
                {setupRequired
                  ? t("server.common.setup")
                  : t("server.common.login")}
              </span>
            </button>
          ) : null}

          {!setupRequired && activeMethod === "email" ? (
            <button
              className="primary-button"
              type="submit"
              disabled={
                busy !== null || !loginOwnerEmail || !verificationCode.trim()
              }
            >
              {busy === "email" ? (
                <Loader2 size={16} className="spin" />
              ) : (
                <Shield size={16} />
              )}
              <span>{t("server.settings.verify")}</span>
            </button>
          ) : null}

          {!setupRequired && activeMethod === "apiToken" ? (
            <button
              className="primary-button"
              type="submit"
              disabled={busy !== null || !apiToken.trim()}
            >
              {busy === "apiToken" ? (
                <Loader2 size={16} className="spin" />
              ) : (
                <KeyRound size={16} />
              )}
              <span>{t("server.common.login")}</span>
            </button>
          ) : null}
        </form>
      </div>
    </div>
  );
}
