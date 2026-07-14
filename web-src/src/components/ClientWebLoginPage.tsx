import { FormEvent, useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import { AuthLanguageSwitcher } from "@/components/AuthLanguageSwitcher";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SecretInput } from "@/components/ui/secret-input";
import { useI18n } from "@/lib/i18n";
import {
  clearRouterSessionTokens,
  getWebAuthMethods,
  loginWithWebPassword,
  requestRouterEmailCodeWithIdentityRetry,
  setRouterApiToken,
  setupWebPassword,
  verifyRouterEmailCode,
  type WebAuthMethods,
} from "@/lib/routerAuth";
import { writeCachedPassword, type WebRuntimeContext } from "@/lib/runtime";
import { extractErrorMessage } from "@/utils/errorUtils";

type LoginMode = "email" | "token" | "password" | "setup";

const AUTH_SECRET_PLACEHOLDER_CLASS =
  "placeholder:text-slate-400 dark:placeholder:text-slate-500";

export function ClientWebLoginPage({
  onAuthenticated,
}: {
  onAuthenticated: () => Promise<WebRuntimeContext>;
}) {
  const { t } = useI18n();
  const [authMethods, setAuthMethods] = useState<WebAuthMethods | null>(null);
  const [mode, setMode] = useState<LoginMode>("password");
  const [code, setCode] = useState("");
  const [apiToken, setApiToken] = useState("");
  const [password, setPassword] = useState("");
  const [setupPasswordValue, setSetupPasswordValue] = useState("");
  const [codeSent, setCodeSent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    void getWebAuthMethods()
      .then((methods) => {
        if (!active) return;
        setAuthMethods(methods);
        if (methods.methods.includes("passwordSetup")) {
          setMode("setup");
        } else if (methods.methods.includes("password")) {
          setMode("password");
        } else if (methods.methods.includes("email")) {
          setMode("email");
        } else if (methods.methods.includes("apiToken")) {
          setMode("token");
        } else {
          setMode("password");
        }
      })
      .catch((err) => {
        if (active) setError(extractErrorMessage(err));
      });
    return () => {
      active = false;
    };
  }, []);

  const finishAuth = useCallback(async () => {
    const context = await onAuthenticated();
    if (context.mode === "client-login") {
      throw new Error(t("server.auth.clientWeb.unauthorizedCredential"));
    }
  }, [onAuthenticated, t]);

  const ownerEmail = authMethods?.ownerEmail?.trim() ?? "";

  const sendCode = useCallback(async () => {
    if (!ownerEmail || busy) return;
    setBusy(true);
    setError("");
    try {
      await requestRouterEmailCodeWithIdentityRetry(ownerEmail, {
        clientWeb: true,
      });
      setCode("");
      setCodeSent(true);
      toast.success(t("server.auth.clientWeb.codeSent"));
    } catch (err) {
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [busy, ownerEmail, t]);

  const verifyCode = useCallback(async () => {
    if (!ownerEmail || code.trim().length < 6 || busy) return;
    setBusy(true);
    setError("");
    try {
      await verifyRouterEmailCode(ownerEmail, code.trim(), { clientWeb: true });
      await finishAuth();
      toast.success(t("server.auth.clientWeb.loggedIn"));
    } catch (err) {
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [busy, code, finishAuth, ownerEmail, t]);

  const loginWithToken = useCallback(async () => {
    const token = apiToken.trim();
    if (!token || busy) return;
    setBusy(true);
    setError("");
    try {
      setRouterApiToken(token);
      await finishAuth();
      toast.success(t("server.auth.clientWeb.loggedInWithToken"));
    } catch (err) {
      clearRouterSessionTokens();
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [apiToken, busy, finishAuth, t]);

  const loginWithPassword = useCallback(async () => {
    if (!password || busy) return;
    setBusy(true);
    setError("");
    try {
      await loginWithWebPassword(password);
      writeCachedPassword(password);
      await finishAuth();
      toast.success(t("server.auth.clientWeb.loggedInWithPassword"));
    } catch (err) {
      clearRouterSessionTokens();
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [busy, finishAuth, password, t]);

  const setupPasswordOnly = useCallback(async () => {
    if (!setupPasswordValue || busy) return;
    setBusy(true);
    setError("");
    try {
      await setupWebPassword(setupPasswordValue);
      writeCachedPassword(setupPasswordValue);
      await finishAuth();
      toast.success(t("server.auth.clientWeb.passwordSet"));
    } catch (err) {
      clearRouterSessionTokens();
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [busy, finishAuth, setupPasswordValue, t]);

  const canUseEmail = authMethods?.methods.includes("email") ?? false;
  const canUseToken = authMethods?.methods.includes("apiToken") ?? false;
  const canUsePassword = authMethods?.methods.includes("password") ?? false;
  const needsPasswordSetup =
    authMethods?.methods.includes("passwordSetup") ?? false;
  const tabCount = [canUsePassword, canUseEmail, canUseToken].filter(
    Boolean,
  ).length;

  const handleSetupSubmit = (event: FormEvent) => {
    event.preventDefault();
    void setupPasswordOnly();
  };

  const handleEmailSubmit = (event: FormEvent) => {
    event.preventDefault();
    void (codeSent ? verifyCode() : sendCode());
  };

  const handleTokenSubmit = (event: FormEvent) => {
    event.preventDefault();
    void loginWithToken();
  };

  const handlePasswordSubmit = (event: FormEvent) => {
    event.preventDefault();
    void loginWithPassword();
  };

  return (
    <div className="auth-shell density-auth-page">
      <div className="auth-shell-card auth-shell-card--compact">
        <AuthLanguageSwitcher />
        <div className="auth-client-card">
        <div className="mb-5 text-center auth-client-card-header">
          <img
            src="./favicon.png"
            alt="cc-switch"
            className="mx-auto mb-3 h-12 w-12"
          />
          <div className="text-lg font-semibold">
            {needsPasswordSetup
              ? t("server.auth.clientWeb.setupTitle")
              : t("server.auth.clientWeb.loginTitle")}
          </div>
          {needsPasswordSetup ? (
            <div className="mt-1 text-sm text-muted-foreground">
              {t("server.auth.clientWeb.setupSubtitle")}
            </div>
          ) : null}
        </div>
        {needsPasswordSetup ? null : tabCount > 1 ? (
          <div
            className="mb-4 grid gap-2"
            style={{
              gridTemplateColumns: `repeat(${tabCount}, minmax(0, 1fr))`,
            }}
          >
            {canUsePassword ? (
              <Button
                type="button"
                variant={mode === "password" ? "default" : "outline"}
                onClick={() => setMode("password")}
              >
                {t("server.auth.clientWeb.webPassword")}
              </Button>
            ) : null}
            {canUseEmail ? (
              <Button
                type="button"
                variant={mode === "email" ? "default" : "outline"}
                onClick={() => setMode("email")}
              >
                {t("server.auth.methodEmail")}
              </Button>
            ) : null}
            {canUseToken ? (
              <Button
                type="button"
                variant={mode === "token" ? "default" : "outline"}
                onClick={() => setMode("token")}
              >
                {t("server.auth.methodApiToken")}
              </Button>
            ) : null}
          </div>
        ) : null}
        {needsPasswordSetup ? (
          <form className="grid gap-3" onSubmit={handleSetupSubmit}>
            <SecretInput
              value={setupPasswordValue}
              placeholder={t("server.auth.clientWeb.setupPasswordPlaceholder")}
              autoComplete="new-password"
              disabled={busy}
              className={AUTH_SECRET_PLACEHOLDER_CLASS}
              onChange={(event) =>
                setSetupPasswordValue(event.currentTarget.value)
              }
            />
            <div className="auth-panel-footer">
              <Button
                type="submit"
                disabled={busy || setupPasswordValue.length < 8}
              >
                {t("server.auth.clientWeb.setupAndLogin")}
              </Button>
            </div>
          </form>
        ) : mode === "email" && canUseEmail ? (
          <form className="grid gap-3" onSubmit={handleEmailSubmit}>
            <label className="grid gap-2">
              <span className="text-sm font-medium">{t("server.auth.ownerEmail")}</span>
              <Input
                readOnly
                value={ownerEmail}
                className="bg-muted text-muted-foreground"
              />
            </label>
            {codeSent ? (
              <Input
                value={code}
                placeholder={t("server.auth.clientWeb.verificationCode")}
                inputMode="numeric"
                autoComplete="one-time-code"
                disabled={busy}
                onChange={(event) => setCode(event.currentTarget.value)}
              />
            ) : null}
            <div className="auth-panel-footer">
              <Button type="submit" disabled={busy || !ownerEmail}>
                {codeSent
                  ? t("server.auth.clientWeb.verifyAndLogin")
                  : t("server.auth.clientWeb.sendCode")}
              </Button>
            </div>
          </form>
        ) : mode === "token" && canUseToken ? (
          <form className="grid gap-3" onSubmit={handleTokenSubmit}>
            <SecretInput
              value={apiToken}
              placeholder="ccrt_..."
              autoComplete="off"
              disabled={busy}
              className={AUTH_SECRET_PLACEHOLDER_CLASS}
              onChange={(event) => setApiToken(event.currentTarget.value)}
            />
            <div className="auth-panel-footer">
              <Button type="submit" disabled={busy || !apiToken.trim()}>
                {t("server.auth.clientWeb.login")}
              </Button>
            </div>
          </form>
        ) : (
          <form className="grid gap-3" onSubmit={handlePasswordSubmit}>
            <SecretInput
              value={password}
              placeholder={t("server.auth.clientWeb.webPassword")}
              autoComplete="current-password"
              disabled={busy}
              className={AUTH_SECRET_PLACEHOLDER_CLASS}
              onChange={(event) => setPassword(event.currentTarget.value)}
            />
            <div className="auth-panel-footer">
              <Button type="submit" disabled={busy || !password}>
                {t("server.auth.clientWeb.login")}
              </Button>
            </div>
          </form>
        )}
        {error ? (
          <div className="mt-4 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        ) : null}
        </div>
      </div>
    </div>
  );
}
