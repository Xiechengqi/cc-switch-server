import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { shareApi, type ShareUserPolicy } from "@/lib/api/share";
import { readToken } from "@/lib/runtime";
import { readRouterAccessToken, SERVER_AUTH_EXPIRED_EVENT } from "@/lib/routerAuth";
import {
  isValuePreview, mergeValueRows, splitValueRows, valueCapabilityReady, valueRequestError, ValueQuotaResponseError,
  type ValueDiagnostics, type ValuePreview, type ValueRequestError, type ValueStatus,
} from "@/lib/share-value-quota";

/** Admin Web session plus the Router SSO token, if any. */
const currentLogin = () => `${readToken() ?? ""}:${readRouterAccessToken() ?? ""}`;

/**
 * The Server admin login these answers belong to. Signing out, an expired
 * session or a Router SSO change in another tab all change it, and every hook
 * below then discards what it fetched for the previous login.
 */
function useCurrentLogin() {
  const [login, setLogin] = useState(currentLogin);
  useEffect(() => {
    const update = () => setLogin(currentLogin());
    const events = ["router-auth-changed", "storage", SERVER_AUTH_EXPIRED_EVENT];
    events.forEach((name) => window.addEventListener(name, update));
    return () => events.forEach((name) => window.removeEventListener(name, update));
  }, []);
  return login;
}

const CAPABILITY_RECHECK_MS = 30_000;
type CapabilityCheck = { key: string; preview?: ValuePreview; error?: ValueRequestError; loading: boolean; preparedRecords?: number };

/**
 * Whether this Share can run a USD-equivalent quota at all (protocol, SQLite
 * authority, metering storage, acknowledged tariff, verified text surface).
 * It asks the Server with an empty policy set, so no user is evaluated.
 *
 * `enabled` asks once — the quota dialog opened and needs to know whether `$`
 * may be offered. `poll` re-asks every 30 s, and only while a USD amount is
 * being entered or some user already has one.
 */
export function useShareValueReadiness(shareId?: string, revision?: number, { enabled = true, poll = true }: { enabled?: boolean; poll?: boolean } = {}) {
  const login = useCurrentLogin();
  const [manualRetry, retry] = useReducer((n: number) => n + 1, 0);
  const key = `${login}:${shareId}:${revision}`;
  const [check, setCheck] = useState<CapabilityCheck>();
  const answeredAt = useRef<{ key: string; manualRetry: number; at: number }>();
  useEffect(() => {
    if (!enabled || !shareId || revision == null) return;
    const controller = new AbortController();
    let inFlight = false;
    const ask = async (force: boolean) => {
      if (inFlight || document.visibilityState === "hidden" || controller.signal.aborted || login !== currentLogin()) return;
      const last = answeredAt.current;
      // Switching between "ask once" and "poll" must not refetch a fresh answer.
      if (!force && last?.key === key && last.manualRetry === manualRetry && Date.now() - last.at < CAPABILITY_RECHECK_MS) return;
      inFlight = true;
      setCheck((previous) => previous?.key === key ? { ...previous, error: undefined, loading: true } : { key, loading: true });
      try {
        const preview = await shareApi.previewValueQuota(shareId, revision, {}, controller.signal);
        if (!isValuePreview(preview, revision)) throw new ValueQuotaResponseError();
        if (!controller.signal.aborted && login === currentLogin()) {
          answeredAt.current = { key, manualRetry, at: Date.now() };
          // A Server without the checklist predates USD quotas.
          setCheck({ key, loading: false, preview, error: !Array.isArray(preview.checks) ? "upgrade" : undefined });
        }
      } catch (error) {
        if (!controller.signal.aborted && login === currentLogin()) {
          answeredAt.current = { key, manualRetry, at: Date.now() };
          // Keep the last checklist on screen next to the failure.
          setCheck((previous) => ({ key, loading: false, error: valueRequestError(error), preview: previous?.key === key ? previous.preview : undefined }));
        }
      } finally { inFlight = false; }
    };
    void ask(false);
    if (!poll) return () => controller.abort();
    const onVisible = () => { if (document.visibilityState === "visible") void ask(false); };
    const timer = window.setInterval(() => void ask(true), CAPABILITY_RECHECK_MS);
    document.addEventListener("visibilitychange", onVisible);
    return () => { controller.abort(); window.clearInterval(timer); document.removeEventListener("visibilitychange", onVisible); };
  }, [key, shareId, revision, login, manualRetry, enabled, poll]);
  const current = check?.key === key && login === currentLogin() ? check : undefined;
  const ready = !current?.error && valueCapabilityReady(current?.preview);
  return { key, preview: current?.preview, error: current?.error, loading: enabled && (current?.loading ?? !!shareId), ready, retry,
    /** A finished check said no. Not having asked yet is not a no. */
    unavailable: !!current && !ready && (!current.loading || !!current.preview) };
}

/**
 * The Server's own USD usage per user for one Share, refreshed every 15 s. It
 * does not depend on the quota draft, so typing an amount neither refetches
 * nor restarts the timer. A failed refresh keeps the last rows, marked stale;
 * refusals mixed into a response never replace a user's last good row.
 */
export function useShareValueStatus(shareId?: string, enabled = true, revision?: number) {
  const login = useCurrentLogin();
  const [snapshot, setSnapshot] = useState<{ scope: string; items: ValueStatus[]; diagnostics?: ValueDiagnostics; stale: boolean }>();
  const scope = `${login}:${shareId ?? ""}`;
  useEffect(() => {
    if (!shareId || !enabled) return;
    let live = true;
    const refresh = () => void shareApi.valueQuotaStatus(shareId).then((data) => {
      if (live) setSnapshot({ scope, items: splitValueRows(data.items).rows, diagnostics: data.diagnostics, stale: false });
    }).catch(() => {
      if (live) setSnapshot((previous) => previous?.scope === scope ? { ...previous, stale: true } : previous);
    });
    refresh();
    const timer = window.setInterval(refresh, 15_000);
    return () => { live = false; window.clearInterval(timer); };
  }, [shareId, enabled, scope, revision]);
  const visible = snapshot?.scope === scope ? snapshot : undefined;
  /** Fold in rows returned by a correction; other users keep theirs. */
  const merge = useCallback((rows: ValueStatus[]) => setSnapshot((previous) => ({
    scope, items: mergeValueRows(previous?.scope === scope ? previous.items : [], rows),
    diagnostics: previous?.diagnostics, stale: previous?.scope === scope ? previous.stale : false,
  })), [scope]);
  return { items: visible?.items ?? [], diagnostics: visible?.diagnostics, stale: visible?.stale ?? false, merge };
}

/** Identity of one preview question: who asks, about which Share revision, for which policies. */
export function valuePreviewIntent(scope: string, policies?: Record<string, ShareUserPolicy>) {
  return policies ? JSON.stringify([scope, Object.keys(policies).sort().map((email) => [email, policies[email]])]) : "";
}
export type ValuePreviewState = { intent: string; preview?: ValuePreview; error?: unknown; loading: boolean; preparedRecords?: number };

/**
 * When the given USD policies would start and what usage they would carry,
 * asked 400 ms after the last edit. Changing the user or amount, or closing
 * the dialog, forgets the previous answer at once so it cannot be confirmed
 * for a different policy. `fetchNow` lets Apply ask without waiting.
 */
export function useShareValuePreview(shareId: string | undefined, revision: number | undefined, scope: string,
  policies: Record<string, ShareUserPolicy> | undefined, delayMs = 400) {
  const intent = shareId && revision != null ? valuePreviewIntent(`${scope}:${shareId}:${revision}`, policies) : "";
  const [state, setState] = useState<ValuePreviewState>({ intent: "", loading: false });
  const pending = useRef<{ intent: string; controller: AbortController; promise: Promise<ValuePreview | undefined> }>();
  const wanted = useRef({ intent, shareId, revision, policies });
  wanted.current = { intent, shareId, revision, policies };
  const ask = useCallback((): Promise<ValuePreview | undefined> => {
    const { intent: question, shareId: id, revision: rev, policies: body } = wanted.current;
    if (!question || !id || rev == null || !body) return Promise.resolve(undefined);
    if (pending.current?.intent === question) return pending.current.promise;
    pending.current?.controller.abort();
    const controller = new AbortController();
    setState({ intent: question, loading: true });
    const promise = (async () => {
      let preview = await shareApi.previewValueQuota(id, rev, body, controller.signal);
      let users = preview.rows.filter(row => row.conversionRequired && row.targetUnit === "usdEquivalent"
        && ["history_incomplete", "history_preparing"].includes(row.reasonCode ?? "")).map(row => row.email);
      const prepared=new Map<string,number>();
      while (users.length && !controller.signal.aborted && wanted.current.intent === question) {
        const page = await shareApi.prepareQuotaHistory(id, rev, users.slice(0, 10), controller.signal);
        page.items.forEach(item=>prepared.set(item.email,item.preparation.processed));
        const processed=Array.from(prepared.values()).reduce((sum,n)=>sum+n,0);
        setState({intent: question, loading: true, preparedRecords: processed});
        const complete = new Set(page.items.filter(item => item.preparation.state === "complete").map(item => item.email));
        users = users.filter(email => !complete.has(email));
        if (!page.items.length) throw new Error("quota_history_invalid_response");
      }
      if (!controller.signal.aborted) preview = await shareApi.previewValueQuota(id, rev, body, controller.signal);
      return preview;
    })().then((preview) => {
      if (!isValuePreview(preview, rev)) throw new ValueQuotaResponseError();
      if (controller.signal.aborted || wanted.current.intent !== question) return undefined;
      setState({ intent: question, preview, loading: false });
      return preview;
    }).catch((error: unknown) => {
      if (controller.signal.aborted || wanted.current.intent !== question) return undefined;
      setState({ intent: question, error, loading: false });
      return undefined;
    }).finally(() => { if (pending.current?.controller === controller) pending.current = undefined; });
    pending.current = { intent: question, controller, promise };
    return promise;
  }, []);
  useEffect(() => {
    pending.current?.controller.abort(); pending.current = undefined;
    setState({ intent, loading: !!intent });
    if (!intent) return;
    const timer = window.setTimeout(() => void ask(), delayMs);
    return () => window.clearTimeout(timer);
  }, [intent, delayMs, ask]);
  useEffect(() => () => pending.current?.controller.abort(), []);
  const current: ValuePreviewState = state.intent === intent ? state : { intent, loading: !!intent };
  return { ...current, fetchNow: (): Promise<ValuePreview | undefined> => current.preview && current.preview.rows.every(row => !row.credentialExpiresAtMs || row.credentialExpiresAtMs > Date.now() + 5_000) ? Promise.resolve(current.preview) : ask() };
}
