import { useEffect, useRef } from "react";

import { readWebSessionToken } from "@/lib/runtime";

type ServerEventPayload = {
  eventType?: string;
  [key: string]: unknown;
};

/**
 * Subscribe to server SSE (`/web-api/events`). EventSource cannot send Authorization
 * headers, so the session token is passed via query string (same as API contract).
 */
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

    const url = `/web-api/events?token=${encodeURIComponent(token)}`;
    const source = new EventSource(url);

    const onEvent = (event: MessageEvent<string>) => {
      try {
        const payload = JSON.parse(event.data) as P;
        void handlerRef.current(payload);
      } catch (error) {
        console.error(`Failed to parse server event ${eventName}`, error);
      }
    };

    source.addEventListener(eventName, onEvent as EventListener);

    return () => {
      source.removeEventListener(eventName, onEvent as EventListener);
      source.close();
    };
  }, [eventName]);
}
