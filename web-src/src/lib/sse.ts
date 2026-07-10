import { apiFetch } from "@/lib/runtime";

export interface ServerSentEvent {
  event: string;
  data: string;
  id?: string;
}

export interface ConsumeSseOptions {
  signal: AbortSignal;
  onEvent: (event: ServerSentEvent) => void | Promise<void>;
}

function parseEventBlock(block: string): ServerSentEvent | null {
  let event = "message";
  let id: string | undefined;
  const data: string[] = [];

  for (const line of block.split("\n")) {
    if (!line || line.startsWith(":")) continue;
    const separator = line.indexOf(":");
    const field = separator >= 0 ? line.slice(0, separator) : line;
    let value = separator >= 0 ? line.slice(separator + 1) : "";
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") event = value || "message";
    if (field === "data") data.push(value);
    if (field === "id") id = value;
  }

  if (data.length === 0) return null;
  return { event, data: data.join("\n"), id };
}

export async function consumeAuthenticatedSse(
  url: string,
  options: ConsumeSseOptions,
): Promise<void> {
  const response = await apiFetch(url, {
    headers: { accept: "text/event-stream" },
    cache: "no-store",
    signal: options.signal,
  });
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    throw new Error(body.trim() || `HTTP ${response.status}`);
  }
  if (!response.body) {
    throw new Error("SSE response body is unavailable");
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    while (!options.signal.aborted) {
      const { value, done } = await reader.read();
      buffer = (buffer + decoder.decode(value, { stream: !done })).replace(
        /\r\n/g,
        "\n",
      );
      let boundary = buffer.indexOf("\n\n");
      while (boundary >= 0) {
        const event = parseEventBlock(buffer.slice(0, boundary));
        buffer = buffer.slice(boundary + 2);
        if (event) await options.onEvent(event);
        boundary = buffer.indexOf("\n\n");
      }
      if (done) break;
    }
  } finally {
    reader.releaseLock();
  }
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === "AbortError"
    : error instanceof Error && error.name === "AbortError";
}

export function abortableDelay(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(new DOMException("Aborted", "AbortError"));
      return;
    }
    const onAbort = () => {
      window.clearTimeout(timeout);
      reject(new DOMException("Aborted", "AbortError"));
    };
    const timeout = window.setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal.addEventListener("abort", onAbort, { once: true });
  });
}
