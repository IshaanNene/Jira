// A reconnecting subscription to one realtime channel. The server sends hints, never
// data: callers refetch through the normal API, where every read is authorized.
import { openStream } from "@/services/api";

import { SseParser } from "./sse";

/** What the server says changed. */
export interface Hint {
  type: string;
  id: string;
  version: number;
  key?: string;
  actor?: string;
  at: string;
}

export type LiveStatus = "connecting" | "live" | "offline" | "stopped";

export interface LiveHandlers {
  onHint: (hint: Hint) => void;
  /** Something may have been missed: refetch what this channel covers. */
  onResync: () => void;
  onStatus?: (status: LiveStatus) => void;
}

const PATH = "/api/v1/rt/stream";

export function backoff(attempt: number, random: () => number = Math.random): number {
  const ceiling = Math.min(30_000, 1000 * 2 ** Math.max(0, attempt - 1));
  return Math.round(ceiling * (0.5 + random() / 2));
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener("abort", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

/** Subscribe until the returned function is called. */
export function subscribe(channel: string, handlers: LiveHandlers): () => void {
  const controller = new AbortController();
  const { signal } = controller;
  let cursor: string | undefined;
  let attempt = 0;
  // A status is reported only when it changes.
  let status: LiveStatus | undefined;
  const report = (next: LiveStatus) => {
    if (next !== status) {
      status = next;
      handlers.onStatus?.(next);
    }
  };

  const run = async () => {
    while (!signal.aborted) {
      report(attempt === 0 ? "connecting" : "offline");
      let next: "retry" | "now" | "stop" = "retry";
      try {
        const response = await openStream(PATH, { channel }, cursor, signal);
        // Not allowed, not a channel, or signed out: retrying cannot help.
        if ([401, 403, 404, 422].includes(response.status)) {
          report("stopped");
          return;
        }
        if (!response.ok || !response.body) throw new Error(`stream failed: ${response.status}`);
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        const parser = new SseParser();
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          for (const message of parser.push(decoder.decode(value, { stream: true }))) {
            if (message.id) cursor = message.id;
            if (message.event === "ready") {
              attempt = 0;
              report("live");
            } else if (message.event === "change") {
              try {
                handlers.onHint(JSON.parse(message.data) as Hint);
              } catch {
                // A malformed hint is ignored; the next one or a resync catches up.
              }
            } else if (message.event === "resync") {
              attempt = 0;
              report("live");
              handlers.onResync();
            } else if (message.event === "expired") {
              next = "now";
            } else if (message.event === "denied") {
              next = "stop";
            }
          }
        }
      } catch {
        if (signal.aborted) return;
      }
      if (next === "stop") {
        report("stopped");
        return;
      }
      if (next === "now") continue;
      attempt += 1;
      report("offline");
      await sleep(backoff(attempt), signal);
    }
  };
  void run();
  return () => controller.abort();
}
