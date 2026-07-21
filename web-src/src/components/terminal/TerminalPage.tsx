import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Loader2, Square } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "@fontsource/source-code-pro/latin-400.css";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { PAGE_SHELL_PADDING_X } from "@/lib/layout";
import { jsonFetch, readWebSessionToken } from "@/lib/runtime";

export interface TerminalPageProps {
  onBackHome: () => void;
}

type ConnState = "connecting" | "replaying" | "live" | "closed" | "error";

const MIN_FONT = 12;
const MAX_FONT = 18;
const TARGET_COLS = 100;

function encodeBytes(data: string | Uint8Array): string {
  const bytes =
    typeof data === "string" ? new TextEncoder().encode(data) : data;
  let binary = "";
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return btoa(binary);
}

function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}

function buildWsUrl(token: string): string {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  const url = new URL(`${protocol}//${window.location.host}/web-api/terminal/ws`);
  url.searchParams.set("token", token);
  return url.toString();
}

function computeFontSize(width: number): number {
  if (width <= 0) return 14;
  const estimated = Math.floor(width / (TARGET_COLS * 0.6));
  return Math.min(MAX_FONT, Math.max(MIN_FONT, estimated || 14));
}

export default function TerminalPage({ onBackHome }: TerminalPageProps) {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const disposedRef = useRef(false);
  const [connState, setConnState] = useState<ConnState>("connecting");
  const [error, setError] = useState<string | null>(null);
  const [ending, setEnding] = useState(false);

  const fitAndResize = useCallback(() => {
    const term = termRef.current;
    const fit = fitRef.current;
    const host = hostRef.current;
    const ws = wsRef.current;
    if (!term || !fit || !host) return;
    const nextFont = computeFontSize(host.clientWidth);
    if (term.options.fontSize !== nextFont) {
      term.options.fontSize = nextFont;
    }
    try {
      fit.fit();
    } catch {
      // ignore fit races during unmount
    }
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ t: "rs", c: term.cols, r: term.rows }));
    }
  }, []);

  useEffect(() => {
    disposedRef.current = false;
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      convertEol: true,
      cursorBlink: true,
      scrollback: 1000,
      fontFamily: '"Source Code Pro", ui-monospace, SFMono-Regular, Menlo, monospace',
      fontSize: computeFontSize(host.clientWidth),
      theme: {
        background: "#f6f8fa",
        foreground: "#1f2328",
        cursor: "#1f2328",
        selectionBackground: "#0969da55",
        black: "#24292f",
        red: "#cf222e",
        green: "#116329",
        yellow: "#4d2d00",
        blue: "#0969da",
        magenta: "#8250df",
        cyan: "#1b7c83",
        white: "#6e7781",
        brightBlack: "#57606a",
        brightRed: "#a40e26",
        brightGreen: "#1a7f37",
        brightYellow: "#633c01",
        brightBlue: "#218bff",
        brightMagenta: "#a475f9",
        brightCyan: "#3192aa",
        brightWhite: "#8c959f",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    termRef.current = term;
    fitRef.current = fit;

    const token = readWebSessionToken();
    if (!token) {
      setConnState("error");
      setError(t("terminal.authRequired"));
      return () => {
        disposedRef.current = true;
        term.dispose();
        termRef.current = null;
        fitRef.current = null;
      };
    }

    const ws = new WebSocket(buildWsUrl(token));
    wsRef.current = ws;
    setConnState("connecting");

    ws.onopen = () => {
      if (disposedRef.current) return;
      setConnState("replaying");
      fitAndResize();
    };

    ws.onmessage = (event) => {
      if (disposedRef.current || typeof event.data !== "string") return;
      let message: { t?: string; d?: string; m?: string };
      try {
        message = JSON.parse(event.data) as { t?: string; d?: string; m?: string };
      } catch {
        return;
      }
      switch (message.t) {
        case "rb":
          setConnState("replaying");
          break;
        case "re":
          setConnState("live");
          fitAndResize();
          break;
        case "out":
          if (message.d) {
            term.write(decodeBase64(message.d));
          }
          break;
        case "pong":
          break;
        case "exit":
          setConnState("closed");
          term.writeln("");
          term.writeln(t("terminal.sessionEnded"));
          break;
        case "err":
          setConnState("error");
          setError(message.m || t("terminal.unknownError"));
          break;
        default:
          break;
      }
    };

    ws.onerror = () => {
      if (disposedRef.current) return;
      setConnState("error");
      setError(t("terminal.connectionFailed"));
    };

    ws.onclose = () => {
      if (disposedRef.current) return;
      setConnState((prev) => (prev === "error" ? prev : "closed"));
      wsRef.current = null;
    };

    const dataSub = term.onData((data) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ t: "in", d: encodeBytes(data) }));
      }
    });

    const ro = new ResizeObserver(() => {
      window.clearTimeout((ro as unknown as { _timer?: number })._timer);
      (ro as unknown as { _timer?: number })._timer = window.setTimeout(() => {
        fitAndResize();
      }, 100);
    });
    ro.observe(host);
    window.addEventListener("resize", fitAndResize);

    return () => {
      disposedRef.current = true;
      window.removeEventListener("resize", fitAndResize);
      ro.disconnect();
      dataSub.dispose();
      // Detach only: close WS, keep server PTY alive.
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [fitAndResize, t]);

  const endSession = useCallback(async () => {
    setEnding(true);
    try {
      await jsonFetch("/web-api/terminal/session/end", { method: "POST" });
      wsRef.current?.close();
      setConnState("closed");
      termRef.current?.writeln("");
      termRef.current?.writeln(t("terminal.sessionEnded"));
    } catch (err) {
      setError(err instanceof Error ? err.message : t("terminal.unknownError"));
    } finally {
      setEnding(false);
    }
  }, [t]);

  const statusLabel = (() => {
    switch (connState) {
      case "connecting":
        return t("terminal.statusConnecting");
      case "replaying":
        return t("terminal.statusReplaying");
      case "live":
        return t("terminal.statusLive");
      case "closed":
        return t("terminal.statusClosed");
      case "error":
        return t("terminal.statusError");
      default:
        return "";
    }
  })();

  return (
    <div
      className={cn(
        PAGE_SHELL_PADDING_X,
        "flex h-full min-h-0 flex-col gap-3 pb-4",
      )}
    >
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          onClick={onBackHome}
          className="gap-1.5 rounded-lg"
        >
          <ArrowLeft className="h-4 w-4" />
          {t("terminal.backHome")}
        </Button>
        <div className="min-w-0 flex-1 text-sm text-muted-foreground truncate">
          {t("terminal.subtitle")} · {statusLabel}
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => void endSession()}
          disabled={ending || connState === "connecting"}
          className="gap-1.5 rounded-lg"
          title={t("terminal.endSession")}
        >
          {ending ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Square className="h-3.5 w-3.5" />
          )}
          {t("terminal.endSession")}
        </Button>
      </div>

      {error && (
        <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
          {error}
        </div>
      )}

      <div
        ref={hostRef}
        className={cn(
          "min-h-0 flex-1 overflow-hidden rounded-lg border border-black/10",
          "bg-[#f6f8fa]",
        )}
      />
    </div>
  );
}
