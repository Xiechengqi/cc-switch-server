import { Copy, ExternalLink, Loader2, LogIn, Play, RefreshCw } from "lucide-react";
import { FormEvent, ReactNode, useEffect, useState } from "react";

import { JsonPreview } from "@/components/JsonPreview";
import { KeyValue } from "@/components/KeyValue";
import { ProviderIcon } from "@/components/ProviderIcon";
import { accountProviderIcon, formatTime, providerLabel } from "@/components/settings/accountDisplay";
import {
  AccountDeviceCodeResponse,
  AccountDevicePollResponse,
  AccountManagerCapability,
  finishAccountLogin,
  OAuthLoginFinish,
  OAuthLoginStart,
  pollCopilotDeviceLogin,
  pollKiroDeviceLogin,
  startAccountLogin,
  startCopilotDeviceLogin,
  startKiroDeviceLogin,
} from "@/lib/server-legacy-api";
import { useI18n } from "@/lib/i18n";

const oauthPreviewProviderTypes = [
  "codex_oauth",
  "claude_oauth",
  "gemini_cli",
  "cursor_oauth",
  "antigravity_oauth",
  "agy_oauth",
];

export function OAuthPreviewPanel({
  capabilities,
  onImported,
}: {
  capabilities: AccountManagerCapability[];
  onImported: () => void;
}) {
  const { tx } = useI18n();
  const available = oauthPreviewProviderTypes.filter((providerType) =>
    capabilities.some((item) => item.providerType === providerType),
  );
  const [providerType, setProviderType] = useState(available[0] || "codex_oauth");
  const [redirectUri, setRedirectUri] = useState("");
  const [login, setLogin] = useState<OAuthLoginStart | null>(null);
  const [finishResult, setFinishResult] = useState<OAuthLoginFinish | null>(null);
  const [accountResult, setAccountResult] = useState<ReactNode>(null);
  const [sessionId, setSessionId] = useState("");
  const [state, setState] = useState("");
  const [code, setCode] = useState("");
  const [executeTokenExchange, setExecuteTokenExchange] = useState(false);
  const [busy, setBusy] = useState<"start" | "finish" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (available.length && !available.includes(providerType)) {
      setProviderType(available[0]);
    }
  }, [available, providerType]);

  async function start(event: FormEvent) {
    event.preventDefault();
    setBusy("start");
    setError(null);
    try {
      const next = await startAccountLogin({
        providerType,
        redirectUri: redirectUri.trim() || undefined,
      });
      setLogin(next);
      setSessionId(next.sessionId);
      setState(next.state);
      setFinishResult(null);
      setAccountResult(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function finish(event: FormEvent) {
    event.preventDefault();
    setBusy("finish");
    setError(null);
    try {
      const next = await finishAccountLogin({
        sessionId: sessionId.trim() || undefined,
        state: state.trim() || undefined,
        code: code.trim() || undefined,
        executeTokenExchange,
      });
      setFinishResult(next.login);
      setAccountResult(
        next.account
          ? tx("{{account}} imported", { account: next.account.email || next.account.id })
          : tx("token request preview ready"),
      );
      if (next.account) onImported();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="account-tool-panel">
      <div className="section-heading">
        <h2>{tx("OAuth Preview")}</h2>
        <span>{tx("manual gate")}</span>
      </div>
      <form className="account-tool-form" onSubmit={start}>
        <label>
          <span>{tx("Provider")}</span>
          <select value={providerType} onChange={(event) => setProviderType(event.target.value)}>
            {available.map((item) => (
              <option key={item} value={item}>
                {providerLabel(item)}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{tx("Redirect URI")}</span>
          <input value={redirectUri} onChange={(event) => setRedirectUri(event.target.value)} />
        </label>
        <button className="secondary-button" type="submit" disabled={busy === "start"}>
          {busy === "start" ? <Loader2 size={15} /> : <LogIn size={15} />}
          <span>{tx("Start")}</span>
        </button>
      </form>
      {login && (
        <div className="oauth-result">
          <div className="provider-card-meta">
            <KeyValue label="session" value={login.sessionId} />
            <KeyValue label="state" value={login.state} />
            <KeyValue label="stage" value={login.serverNativeStage} />
            <KeyValue label="expires" value={formatTime(login.expiresAtMs)} />
          </div>
          <a href={login.authorizeUrl} target="_blank" rel="noreferrer" className="inline-link">
            <ExternalLink size={14} />
            <span>{tx("Open authorize URL")}</span>
          </a>
        </div>
      )}
      <form className="account-tool-form" onSubmit={finish}>
        <label>
          <span>{tx("Session ID")}</span>
          <input value={sessionId} onChange={(event) => setSessionId(event.target.value)} />
        </label>
        <label>
          <span>{tx("State")}</span>
          <input value={state} onChange={(event) => setState(event.target.value)} />
        </label>
        <label className="wide-field">
          <span>{tx("Authorization code")}</span>
          <input value={code} onChange={(event) => setCode(event.target.value)} />
        </label>
        <label className="toggle-row wide-field">
          <input
            type="checkbox"
            checked={executeTokenExchange}
            onChange={(event) => setExecuteTokenExchange(event.target.checked)}
          />
          <span>{tx("Execute token exchange")}</span>
        </label>
        <button className="secondary-button" type="submit" disabled={busy === "finish"}>
          {busy === "finish" ? <Loader2 size={15} /> : <Play size={15} />}
          <span>{tx(executeTokenExchange ? "Exchange" : "Preview")}</span>
        </button>
      </form>
      {accountResult && <div className="provider-card-result">{accountResult}</div>}
      {finishResult?.tokenRequest && (
        <details className="json-details">
          <summary>{tx("Token request")}</summary>
          <JsonPreview value={finishResult.tokenRequest} redact />
        </details>
      )}
      {error && <div className="form-error">{error}</div>}
    </section>
  );
}

export function DeviceFlowPanel({ onImported }: { onImported: () => void }) {
  const { tx } = useI18n();
  const [copilotDomain, setCopilotDomain] = useState("");
  const [copilotDevice, setCopilotDevice] = useState<AccountDeviceCodeResponse | null>(null);
  const [copilotPoll, setCopilotPoll] = useState<AccountDevicePollResponse | null>(null);
  const [kiroRegion, setKiroRegion] = useState("us-east-1");
  const [kiroStartUrl, setKiroStartUrl] = useState("https://view.awsapps.com/start");
  const [kiroDevice, setKiroDevice] = useState<AccountDeviceCodeResponse | null>(null);
  const [kiroPoll, setKiroPoll] = useState<AccountDevicePollResponse | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function startCopilot() {
    setBusy("copilot-start");
    setError(null);
    try {
      setCopilotDevice(await startCopilotDeviceLogin({ githubDomain: copilotDomain.trim() || undefined }));
      setCopilotPoll(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function pollCopilot() {
    if (!copilotDevice) return;
    setBusy("copilot-poll");
    setError(null);
    try {
      const next = await pollCopilotDeviceLogin({
        deviceCode: copilotDevice.deviceCode,
        githubDomain: copilotDevice.githubDomain || copilotDomain.trim() || undefined,
      });
      setCopilotPoll(next);
      if (next.account) onImported();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function startKiro() {
    setBusy("kiro-start");
    setError(null);
    try {
      setKiroDevice(await startKiroDeviceLogin({
        region: kiroRegion.trim() || undefined,
        startUrl: kiroStartUrl.trim() || undefined,
      }));
      setKiroPoll(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function pollKiro() {
    if (!kiroDevice) return;
    setBusy("kiro-poll");
    setError(null);
    try {
      const next = await pollKiroDeviceLogin({ deviceCode: kiroDevice.deviceCode });
      setKiroPoll(next);
      if (next.account) onImported();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="account-tool-panel">
      <div className="section-heading">
        <h2>{tx("Device Flow")}</h2>
        <span>{tx("Copilot / Kiro")}</span>
      </div>
      <div className="device-flow-grid">
        <DeviceFlowCard
          title="GitHub Copilot"
          icon={<AccountProviderIcon providerType="github_copilot" size={16} />}
          fields={
            <label>
              <span>{tx("GitHub domain")}</span>
              <input value={copilotDomain} onChange={(event) => setCopilotDomain(event.target.value)} />
            </label>
          }
          device={copilotDevice}
          poll={copilotPoll}
          startBusy={busy === "copilot-start"}
          pollBusy={busy === "copilot-poll"}
          onStart={() => void startCopilot()}
          onPoll={() => void pollCopilot()}
        />
        <DeviceFlowCard
          title="Kiro OAuth"
          icon={<AccountProviderIcon providerType="kiro_oauth" size={16} />}
          fields={
            <>
              <label>
                <span>{tx("Region")}</span>
                <input value={kiroRegion} onChange={(event) => setKiroRegion(event.target.value)} />
              </label>
              <label>
                <span>{tx("Start URL")}</span>
                <input value={kiroStartUrl} onChange={(event) => setKiroStartUrl(event.target.value)} />
              </label>
            </>
          }
          device={kiroDevice}
          poll={kiroPoll}
          startBusy={busy === "kiro-start"}
          pollBusy={busy === "kiro-poll"}
          onStart={() => void startKiro()}
          onPoll={() => void pollKiro()}
        />
      </div>
      {error && <div className="form-error">{error}</div>}
    </section>
  );
}

function DeviceFlowCard({
  title,
  icon,
  fields,
  device,
  poll,
  startBusy,
  pollBusy,
  onStart,
  onPoll,
}: {
  title: string;
  icon: ReactNode;
  fields: ReactNode;
  device: AccountDeviceCodeResponse | null;
  poll: AccountDevicePollResponse | null;
  startBusy: boolean;
  pollBusy: boolean;
  onStart: () => void;
  onPoll: () => void;
}) {
  const { tx } = useI18n();
  const [copyStatus, setCopyStatus] = useState<{ tone: "success" | "warning"; message: string } | null>(null);
  const verificationUrl = device?.verificationUriComplete || device?.verificationUri || "";

  async function copyDeviceText(value: string, successMessage: string) {
    if (!value) return;
    if (!navigator.clipboard?.writeText) {
      setCopyStatus({ tone: "warning", message: tx("Clipboard unavailable; copy the visible value manually.") });
      return;
    }
    try {
      await navigator.clipboard.writeText(value);
      setCopyStatus({ tone: "success", message: successMessage });
    } catch {
      setCopyStatus({ tone: "warning", message: tx("Copy failed; copy the visible value manually.") });
    }
  }

  return (
    <article className="device-flow-card">
      <header>
        <div className="section-title-row compact-title">
          {icon}
          <h3>{tx(title)}</h3>
        </div>
      </header>
      <div className="account-tool-form">
        {fields}
        <button className="secondary-button" type="button" onClick={onStart} disabled={startBusy}>
          {startBusy ? <Loader2 size={15} /> : <Play size={15} />}
          <span>{tx("Start")}</span>
        </button>
      </div>
      {device && (
        <div className="device-code-block">
          <KeyValue label="user code" value={device.userCode} />
          <KeyValue label="expires" value={`${device.expiresIn}s`} />
          <div className="modal-inline-footer compact-footer">
            <button className="secondary-button compact" type="button" onClick={() => void copyDeviceText(device.userCode, tx("Copied code"))}>
              <Copy size={13} />
              <span>{tx("Copy code")}</span>
            </button>
            <button className="secondary-button compact" type="button" onClick={() => void copyDeviceText(verificationUrl, tx("Copied URL"))}>
              <Copy size={13} />
              <span>{tx("Copy URL")}</span>
            </button>
          </div>
          {copyStatus && <div className={`connect-copy-status ${copyStatus.tone}`}>{copyStatus.message}</div>}
          <a href={verificationUrl} target="_blank" rel="noreferrer" className="inline-link">
            <ExternalLink size={14} />
            <span>{tx("Open verification")}</span>
          </a>
          <button className="secondary-button compact" type="button" onClick={onPoll} disabled={pollBusy}>
            {pollBusy ? <Loader2 size={13} /> : <RefreshCw size={13} />}
            <span>{tx("Poll")}</span>
          </button>
        </div>
      )}
      {poll && (
        <div className="provider-card-result">
          {poll.account ? tx("{{account}} imported", { account: poll.account.email || poll.account.id }) : poll.message}
        </div>
      )}
    </article>
  );
}

function AccountProviderIcon({ providerType, size = 20 }: { providerType: string; size?: number }) {
  const icon = accountProviderIcon(providerType);
  return <ProviderIcon icon={icon.icon} color={icon.color} name={providerLabel(providerType)} size={size} />;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
