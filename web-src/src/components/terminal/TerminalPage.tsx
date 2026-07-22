import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Square } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "@fontsource/source-code-pro/latin-400.css";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { PAGE_SHELL_PADDING_X } from "@/lib/layout";
import { jsonFetch } from "@/lib/runtime";
import {
  consumeAuthenticatedSse,
  isAbortError,
  type ServerSentEvent,
} from "@/lib/sse";

type ConnState = "connecting" | "replaying" | "live" | "closed" | "error";

type TerminalMessage = {
  t?: string;
  d?: string;
  m?: string;
};

const MIN_FONT = 12;
const MAX_FONT = 18;
const TARGET_COLS = 100;
const INPUT_BATCH_MS = 12;
const RESIZE_DEBOUNCE_MS = 100;

const STATUS_CLASS_NAME: Record<ConnState, string> = {
  connecting:
    "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  replaying:
    "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  live: "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  closed: "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-300",
  error: "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-300",
};

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

function computeFontSize(width: number): number {
  if (width <= 0) return 14;
  const estimated = Math.floor(width / (TARGET_COLS * 0.6));
  return Math.min(MAX_FONT, Math.max(MIN_FONT, estimated || 14));
}

function parseTerminalMessage(event: ServerSentEvent): TerminalMessage | null {
  if (event.event !== "terminal") return null;
  try {
    return JSON.parse(event.data) as TerminalMessage;
  } catch {
    return null;
  }
}

function readableError(error: unknown, fallback: string): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (!(error instanceof Error) || !error.message.trim()) return fallback;
  const message = error.message.trim();
  if (message.startsWith("{")) {
    try {
      const payload = JSON.parse(message) as {
        error?: string;
        message?: string;
      };
      return payload.error || payload.message || fallback;
    } catch {
      return message;
    }
  }
  return message;
}

export default function TerminalPage() {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const streamAbortRef = useRef<AbortController | null>(null);
  const streamReadyRef = useRef(false);
  const requestResizeRef = useRef<(() => void) | null>(null);
  const disposedRef = useRef(false);
  const [connState, setConnState] = useState<ConnState>("connecting");
  const [error, setError] = useState<string | null>(null);
  const [ending, setEnding] = useState(false);

  const fitAndResize = useCallback(() => {
    const term = termRef.current;
    const fit = fitRef.current;
    const host = hostRef.current;
    if (!term || !fit || !host) return;
    const nextFont = computeFontSize(host.clientWidth);
    if (term.options.fontSize !== nextFont) {
      term.options.fontSize = nextFont;
    }
    try {
      fit.fit();
    } catch {
      return;
    }
    requestResizeRef.current?.();
  }, []);

  useEffect(() => {
    disposedRef.current = false;
    streamReadyRef.current = false;
    setError(null);
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      convertEol: true,
      cursorBlink: true,
      scrollback: 1000,
      fontFamily:
        '"Source Code Pro", ui-monospace, SFMono-Regular, Menlo, monospace',
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

    const controller = new AbortController();
    streamAbortRef.current = controller;
    let inputBuffer = "";
    let inputTimer: number | null = null;
    let inputSending = false;
    let resizeTimer: number | null = null;

    const failConnection = (cause: unknown) => {
      if (disposedRef.current || controller.signal.aborted) return;
      streamReadyRef.current = false;
      setConnState("error");
      setError(readableError(cause, t("terminal.connectionFailed")));
      controller.abort();
    };

    const flushInput = async () => {
      if (inputSending || !streamReadyRef.current || !inputBuffer) return;
      inputSending = true;
      try {
        while (
          inputBuffer &&
          streamReadyRef.current &&
          !controller.signal.aborted
        ) {
          const data = inputBuffer;
          inputBuffer = "";
          await jsonFetch("/web-api/terminal/input", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ d: encodeBytes(data) }),
            signal: controller.signal,
          });
        }
      } catch (cause) {
        if (!isAbortError(cause)) failConnection(cause);
      } finally {
        inputSending = false;
      }
    };

    const scheduleInput = (data: string) => {
      if (!streamReadyRef.current || controller.signal.aborted) return;
      inputBuffer += data;
      if (inputSending || inputTimer !== null) return;
      inputTimer = window.setTimeout(() => {
        inputTimer = null;
        void flushInput();
      }, INPUT_BATCH_MS);
    };

    requestResizeRef.current = () => {
      if (!streamReadyRef.current || controller.signal.aborted) return;
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        resizeTimer = null;
        void jsonFetch("/web-api/terminal/resize", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ c: term.cols, r: term.rows }),
          signal: controller.signal,
        }).catch((cause) => {
          if (!isAbortError(cause)) failConnection(cause);
        });
      }, RESIZE_DEBOUNCE_MS);
    };

    const handleEvent = (event: ServerSentEvent) => {
      if (disposedRef.current) return;
      const message = parseTerminalMessage(event);
      if (!message) return;
      switch (message.t) {
        case "rb":
          setConnState("replaying");
          break;
        case "re":
          streamReadyRef.current = true;
          setConnState("live");
          fitAndResize();
          break;
        case "out":
          if (message.d) term.write(decodeBase64(message.d));
          break;
        case "exit":
          streamReadyRef.current = false;
          setConnState("closed");
          term.writeln("");
          term.writeln(t("terminal.sessionEnded"));
          break;
        case "err":
          failConnection(message.m || t("terminal.unknownError"));
          break;
        default:
          break;
      }
    };

    setConnState("connecting");
    void consumeAuthenticatedSse("/web-api/terminal/stream", {
      signal: controller.signal,
      onEvent: handleEvent,
    })
      .then(() => {
        if (disposedRef.current || controller.signal.aborted) return;
        streamReadyRef.current = false;
        setConnState("closed");
      })
      .catch((cause) => {
        if (!isAbortError(cause)) failConnection(cause);
      });

    const dataSub = term.onData(scheduleInput);
    const ro = new ResizeObserver(() => {
      window.clearTimeout((ro as unknown as { _timer?: number })._timer);
      (ro as unknown as { _timer?: number })._timer = window.setTimeout(() => {
        fitAndResize();
      }, RESIZE_DEBOUNCE_MS);
    });
    ro.observe(host);
    window.addEventListener("resize", fitAndResize);

    return () => {
      disposedRef.current = true;
      streamReadyRef.current = false;
      controller.abort();
      window.removeEventListener("resize", fitAndResize);
      ro.disconnect();
      dataSub.dispose();
      if (inputTimer !== null) window.clearTimeout(inputTimer);
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      requestResizeRef.current = null;
      streamAbortRef.current = null;
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [fitAndResize, t]);

  const endSession = useCallback(async () => {
    setEnding(true);
    try {
      await jsonFetch("/web-api/terminal/session/end", { method: "POST" });
      streamReadyRef.current = false;
      streamAbortRef.current?.abort();
      setConnState("closed");
      termRef.current?.writeln("");
      termRef.current?.writeln(t("terminal.sessionEnded"));
    } catch (cause) {
      setError(readableError(cause, t("terminal.unknownError")));
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
        <span
          className={cn(
            "inline-flex h-7 items-center rounded-md border px-2.5 text-xs font-medium",
            STATUS_CLASS_NAME[connState],
          )}
        >
          {statusLabel}
        </span>
        <div className="min-w-0 flex-1" />
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
