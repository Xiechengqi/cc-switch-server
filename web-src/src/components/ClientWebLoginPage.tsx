import { FormEvent, useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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

export function ClientWebLoginPage({
  onAuthenticated,
}: {
  onAuthenticated: () => Promise<WebRuntimeContext>;
}) {
  const [authMethods, setAuthMethods] = useState<WebAuthMethods | null>(null);
  const [mode, setMode] = useState<LoginMode>("password");
  const [email, setEmail] = useState("");
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
      throw new Error("登录凭证无权访问当前 client");
    }
  }, [onAuthenticated]);

  const sendCode = useCallback(async () => {
    const normalized = email.trim().toLowerCase();
    if (!normalized || busy) return;
    setBusy(true);
    setError("");
    try {
      await requestRouterEmailCodeWithIdentityRetry(normalized, {
        clientWeb: true,
      });
      setEmail(normalized);
      setCode("");
      setCodeSent(true);
      toast.success("验证码已发送");
    } catch (err) {
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [busy, email]);

  const verifyCode = useCallback(async () => {
    const normalized = email.trim().toLowerCase();
    if (!normalized || code.trim().length < 6 || busy) return;
    setBusy(true);
    setError("");
    try {
      await verifyRouterEmailCode(normalized, code.trim(), { clientWeb: true });
      await finishAuth();
      toast.success("已登录 client");
    } catch (err) {
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [busy, code, email, finishAuth]);

  const loginWithToken = useCallback(async () => {
    const token = apiToken.trim();
    if (!token || busy) return;
    setBusy(true);
    setError("");
    try {
      setRouterApiToken(token);
      await finishAuth();
      toast.success("已使用 API token 登录 client");
    } catch (err) {
      clearRouterSessionTokens();
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [apiToken, busy, finishAuth]);

  const loginWithPassword = useCallback(async () => {
    if (!password || busy) return;
    setBusy(true);
    setError("");
    try {
      await loginWithWebPassword(password);
      writeCachedPassword(password);
      await finishAuth();
      toast.success("已使用 Web 密码登录");
    } catch (err) {
      clearRouterSessionTokens();
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [busy, finishAuth, password]);

  const setupPasswordOnly = useCallback(async () => {
    if (!setupPasswordValue || busy) return;
    setBusy(true);
    setError("");
    try {
      await setupWebPassword(setupPasswordValue);
      writeCachedPassword(setupPasswordValue);
      await finishAuth();
      toast.success("Web 密码已设置");
    } catch (err) {
      clearRouterSessionTokens();
      setError(extractErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }, [busy, finishAuth, setupPasswordValue]);

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
    <div className="flex min-h-screen items-center justify-center bg-background px-4 py-10 text-foreground">
      <div className="w-full max-w-sm rounded-lg border border-border bg-card p-5 shadow-sm">
        <div className="mb-5 text-center">
          <img
            src="./favicon.png"
            alt="cc-switch"
            className="mx-auto mb-3 h-12 w-12"
          />
          <div className="text-lg font-semibold">
            {needsPasswordSetup ? "设置 Web 密码" : "Client Web 登录"}
          </div>
          <div className="mt-1 text-sm text-muted-foreground">
            {needsPasswordSetup
              ? "首次访问需要设置 Web 管理密码。"
              : "使用可用的鉴权方式访问当前 client。"}
          </div>
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
                Web 密码
              </Button>
            ) : null}
            {canUseEmail ? (
              <Button
                type="button"
                variant={mode === "email" ? "default" : "outline"}
                onClick={() => setMode("email")}
              >
                邮箱验证码
              </Button>
            ) : null}
            {canUseToken ? (
              <Button
                type="button"
                variant={mode === "token" ? "default" : "outline"}
                onClick={() => setMode("token")}
              >
                API Token
              </Button>
            ) : null}
          </div>
        ) : null}
        {needsPasswordSetup ? (
          <form className="grid gap-3" onSubmit={handleSetupSubmit}>
            <Input
              value={setupPasswordValue}
              placeholder="设置 Web 密码"
              type="password"
              autoComplete="new-password"
              disabled={busy}
              onChange={(event) =>
                setSetupPasswordValue(event.currentTarget.value)
              }
            />
            <Button
              type="submit"
              disabled={busy || setupPasswordValue.length < 8}
            >
              设置并登录
            </Button>
          </form>
        ) : mode === "email" && canUseEmail ? (
          <form className="grid gap-3" onSubmit={handleEmailSubmit}>
            <Input
              type="email"
              value={email}
              placeholder="owner@example.com"
              disabled={busy || codeSent}
              onChange={(event) => setEmail(event.currentTarget.value)}
            />
            {codeSent ? (
              <Input
                value={code}
                placeholder="验证码"
                inputMode="numeric"
                autoComplete="one-time-code"
                disabled={busy}
                onChange={(event) => setCode(event.currentTarget.value)}
              />
            ) : null}
            <Button type="submit" disabled={busy || !email.trim()}>
              {codeSent ? "验证并登录" : "发送验证码"}
            </Button>
            {codeSent ? (
              <Button
                type="button"
                variant="ghost"
                disabled={busy}
                onClick={() => {
                  setCodeSent(false);
                  setCode("");
                }}
              >
                更换邮箱
              </Button>
            ) : null}
          </form>
        ) : mode === "token" && canUseToken ? (
          <form className="grid gap-3" onSubmit={handleTokenSubmit}>
            <Input
              value={apiToken}
              placeholder="ccrt_..."
              type="password"
              autoComplete="off"
              disabled={busy}
              onChange={(event) => setApiToken(event.currentTarget.value)}
            />
            <Button type="submit" disabled={busy || !apiToken.trim()}>
              登录
            </Button>
          </form>
        ) : (
          <form className="grid gap-3" onSubmit={handlePasswordSubmit}>
            <Input
              value={password}
              placeholder="Web 密码"
              type="password"
              autoComplete="current-password"
              disabled={busy}
              onChange={(event) => setPassword(event.currentTarget.value)}
            />
            <Button type="submit" disabled={busy || !password}>
              登录
            </Button>
          </form>
        )}
        {error ? (
          <div className="mt-4 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        ) : null}
      </div>
    </div>
  );
}
