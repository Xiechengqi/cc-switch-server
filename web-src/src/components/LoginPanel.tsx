import { FormEvent, useEffect, useMemo, useState } from "react";
import { KeyRound, Loader2, Mail, Shield } from "lucide-react";

import {
  requestEmailLoginCode,
  verifyEmailLoginCode,
} from "@/lib/server-legacy-api";
import { DEFAULT_SHARE_ROUTER_DOMAIN } from "@/config/shareRegions";
import { useI18n } from "@/lib/i18n";
import { jsonFetch, loginWithPassword, readCachedPassword, WebRuntimeContext, writeToken } from "@/lib/runtime";

type LoginMethod = "password" | "email" | "apiToken";

function normalizeMethods(context: WebRuntimeContext): LoginMethod[] {
  const raw = context.auth?.methods ?? ["password"];
  if (raw.includes("passwordSetup")) {
    return ["password"];
  }
  const methods: LoginMethod[] = [];
  if (raw.includes("password")) methods.push("password");
  if (raw.includes("email")) methods.push("email");
  if (raw.includes("apiToken")) methods.push("apiToken");
  return methods.length > 0 ? methods : ["password"];
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
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
  const [routerUrl, setRouterUrl] = useState(
    () => `https://${DEFAULT_SHARE_ROUTER_DOMAIN}`,
  );
  const [clientTunnelSubdomain, setClientTunnelSubdomain] = useState("");
  const [email, setEmail] = useState(context.auth?.ownerEmail ?? "");
  const [verificationCode, setVerificationCode] = useState("");
  const [apiToken, setApiToken] = useState("");
  const [codeHint, setCodeHint] = useState<string | null>(null);
  const [resendCooldown, setResendCooldown] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!availableMethods.includes(activeMethod)) {
      setActiveMethod(availableMethods[0] ?? "password");
    }
  }, [activeMethod, availableMethods]);

  useEffect(() => {
    if (context.auth?.ownerEmail) {
      setOwnerEmail(context.auth.ownerEmail);
      setEmail(context.auth.ownerEmail);
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
    setBusy("password");
    try {
      if (setupRequired) {
        await jsonFetch("/api/setup", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            password,
            ownerEmail,
            routerUrl,
            clientTunnelSubdomain,
          }),
        });
      }
      await loginWithPassword(password);
      await onAuthenticated();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function submitApiToken(event?: FormEvent) {
    event?.preventDefault();
    setError(null);
    setBusy("apiToken");
    try {
      const login = await jsonFetch<{ token: string }>("/api/auth/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ method: "api_token", apiToken: apiToken.trim() }),
      });
      await completeLogin(login.token);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function requestCode() {
    const normalizedEmail = email.trim();
    if (!normalizedEmail) return;
    setError(null);
    setBusy("requestCode");
    try {
      const response = await requestEmailLoginCode(normalizedEmail);
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
    const normalizedEmail = email.trim();
    const normalizedCode = verificationCode.trim();
    if (!normalizedEmail || !normalizedCode) return;
    setError(null);
    setBusy("email");
    try {
      const login = await verifyEmailLoginCode({
        email: normalizedEmail,
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
                : t("server.auth.loginSubtitle", {
                    defaultValue: "使用密码、邮箱验证码或 API Token 登录。",
                  })}
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
                <span>{t("server.auth.routerUrl")}</span>
                <input
                  value={routerUrl}
                  onChange={(event) => setRouterUrl(event.target.value)}
                />
              </label>
              <label className="auth-grid-span-2">
                <span>{t("server.auth.clientSubdomain")}</span>
                <input
                  value={clientTunnelSubdomain}
                  onChange={(event) =>
                    setClientTunnelSubdomain(event.target.value)
                  }
                />
              </label>
              <label className="auth-grid-span-2">
                <span>{t("server.common.password")}</span>
                <input
                  type="password"
                  autoComplete="new-password"
                  value={password}
                  onChange={(event) => setPassword(event.target.value)}
                />
              </label>
            </div>
          ) : null}

          {!setupRequired && activeMethod === "password" ? (
            <label>
              <span>{t("server.common.password")}</span>
              <input
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
            </label>
          ) : null}

          {!setupRequired && activeMethod === "email" ? (
            <div className="auth-grid">
              <label className="auth-grid-span-2">
                <span>{t("server.auth.ownerEmail")}</span>
                <input
                  type="email"
                  autoComplete="email"
                  value={email}
                  onChange={(event) => setEmail(event.target.value)}
                  placeholder="owner@example.com"
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
                    !email.trim() || busy !== null || resendCooldown > 0
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
              <label className="auth-grid-span-2">
                <span>{t("server.auth.apiToken", { defaultValue: "API Token" })}</span>
                <input
                  type="password"
                  autoComplete="off"
                  value={apiToken}
                  onChange={(event) => setApiToken(event.target.value)}
                  placeholder="ccs_..."
                />
              </label>
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
              disabled={busy !== null || !password}
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
                busy !== null || !email.trim() || !verificationCode.trim()
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
