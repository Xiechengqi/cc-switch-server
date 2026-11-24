import { useEffect, useState } from "react";
import { jsonFetch } from "@/lib/runtime";

type App = "claude" | "codex" | "gemini";
type Share = { id: string; name: string; apps: App[] };
type Turn = {
  shareId: string;
  app: App;
  model: string;
  prompt: string;
  reply: string;
};
const field =
  "h-12 rounded-md border bg-background px-3 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";

export function WorkspaceChat({ zh }: { zh: boolean }) {
  const [shares, setShares] = useState<Share[]>([]);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [shareId, setShareId] = useState("");
  const [app, setApp] = useState<App>("codex");
  const [model, setModel] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  useEffect(() => {
    const controller = new AbortController();
    jsonFetch<{ turns: Turn[]; shares: Share[] }>("/web-api/market/chat", {
      signal: controller.signal,
    })
      .then((data) => {
        if (controller.signal.aborted) return;
        const options = data.shares.filter((s) => s.apps.length);
        setShares(options);
        setTurns(data.turns);
        setShareId(options[0]?.id || "");
        if (options[0]) setApp(options[0].apps[0]);
      })
      .catch((err) => {
        if (!controller.signal.aborted) setError(String(err));
      })
      .finally(() => {
        if (!controller.signal.aborted) setBusy(false);
      });
    return () => controller.abort();
  }, []);
  const selected = shares.find((s) => s.id === shareId);
  return (
    <form
      className="grid gap-2 py-2"
      onSubmit={async (e) => {
        e.preventDefault();
        if (busy || !selected || !prompt.trim() || !model.trim()) return;
        setBusy(true);
        setError("");
        try {
          const data = await jsonFetch<{ turns: Turn[] }>(
            "/web-api/market/chat",
            {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({
                shareId,
                app,
                model: model.trim(),
                prompt,
              }),
            },
          );
          setTurns(data.turns);
          setPrompt("");
        } catch (err) {
          setError(String(err));
        } finally {
          setBusy(false);
        }
      }}
    >
      {error && <p role="alert">{error}</p>}
      {!busy && !selected && (
        <p>
          {zh
            ? "请先在工作区创建并启用 Share。"
            : "Create and enable a workspace Share to chat."}
        </p>
      )}
      {selected && (
        <>
          <div className="flex flex-wrap gap-2">
            <label>
              Share{" "}
              <select
                className={field}
                value={shareId}
                disabled={busy}
                onChange={(e) => {
                  const next = shares.find((s) => s.id === e.target.value);
                  if (next) {
                    setShareId(next.id);
                    setApp(next.apps[0]);
                  }
                }}
              >
                {shares.map((s) => (
                  <option value={s.id} key={s.id}>
                    {s.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {zh ? "协议" : "Protocol"}{" "}
              <select
                className={field}
                value={app}
                disabled={busy}
                onChange={(e) => setApp(e.target.value as App)}
              >
                {selected.apps.map((a) => (
                  <option key={a}>{a}</option>
                ))}
              </select>
            </label>
            <label>
              {zh ? "模型" : "Model"}{" "}
              <input
                className={field}
                required
                value={model}
                maxLength={200}
                disabled={busy}
                onChange={(e) => setModel(e.target.value)}
              />
            </label>
          </div>
          <ol
            className="grid max-h-72 gap-2 overflow-y-auto"
            aria-live="polite"
          >
            {turns
              .filter(
                (t) =>
                  t.shareId === shareId &&
                  t.app === app &&
                  t.model === model.trim(),
              )
              .map((t, i) => (
                <li key={i}>
                  <p className="font-medium whitespace-pre-wrap">{t.prompt}</p>
                  <p className="whitespace-pre-wrap">{t.reply}</p>
                </li>
              ))}
          </ol>
          <label className="grid gap-1">
            {zh ? "消息" : "Message"}
            <textarea
              className={field}
              required
              maxLength={8192}
              value={prompt}
              disabled={busy}
              onChange={(e) => setPrompt(e.target.value)}
            />
          </label>
          <button
            className="min-h-11 justify-self-start rounded-md border px-4"
            disabled={busy || !model.trim() || !prompt.trim()}
          >
            {zh ? "发送" : "Send"}
          </button>
        </>
      )}
    </form>
  );
}
