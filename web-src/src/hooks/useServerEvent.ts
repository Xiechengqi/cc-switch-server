import { useEffect, useRef } from "react";

import { readWebSessionToken } from "@/lib/runtime";
import {
  abortableDelay,
  consumeAuthenticatedSse,
  isAbortError,
} from "@/lib/sse";

type ServerEventPayload = {
  eventType?: string;
  [key: string]: unknown;
};

export function useServerEvent<P = ServerEventPayload>(
  eventName: string,
  handler: (payload: P) => void | Promise<void>,
): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    const token = readWebSessionToken();
    if (!token) {
      return;
    }

    const controller = new AbortController();
    void (async () => {
      let attempt = 0;
      while (!controller.signal.aborted) {
        try {
          await consumeAuthenticatedSse("/web-api/events", {
            signal: controller.signal,
            onEvent: async (event) => {
              if (event.event !== eventName) return;
              try {
                await handlerRef.current(JSON.parse(event.data) as P);
              } catch (error) {
                console.error(`Failed to parse server event ${eventName}`, error);
              }
            },
          });
          attempt = 0;
        } catch (error) {
          if (controller.signal.aborted || isAbortError(error)) return;
          attempt += 1;
        }
        await abortableDelay(
          Math.min(30_000, 500 * 2 ** Math.min(attempt, 6)),
          controller.signal,
        ).catch(() => undefined);
      }
    })();

    return () => {
      controller.abort();
    };
  }, [eventName]);
}
