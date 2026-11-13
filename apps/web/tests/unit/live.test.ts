import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import { RefreshBatch, staleFor } from "@/modules/live";
import { configureAuth } from "@/services/api";
import { backoff, SseParser, subscribe, type Hint, type LiveStatus } from "@/services/realtime";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("SSE parsing", () => {
  it("handles events split across chunks, comments and CRLF", () => {
    const parser = new SseParser();
    expect(parser.push(": keep-alive\n\nevent: ready\nid: 1-0\nda")).toEqual([]);
    expect(parser.push('ta: {}\n\nevent: change\r\nid: 2-0\r\ndata: {"a":1}\r\n\r\n')).toEqual([
      { event: "ready", id: "1-0", data: "{}" },
      { event: "change", id: "2-0", data: '{"a":1}' },
    ]);
    expect(parser.push("data: one\ndata: two\n\n")).toEqual([{ event: "message", data: "one\ntwo" }]);
  });

  it("backs off exponentially with jitter, capped at 30 seconds", () => {
    expect(backoff(1, () => 1)).toBe(1000);
    expect(backoff(3, () => 1)).toBe(4000);
    expect(backoff(3, () => 0)).toBe(2000);
    expect(backoff(20, () => 1)).toBe(30_000);
  });
});

const hint = (type: string, key?: string): Hint => ({ type, id: "a", version: 1, key, at: "2026-11-12T00:00:00Z" });

describe("what a hint refreshes", () => {
  const keys = { lists: ["lists"], issue: (k: string) => ["issue", k], project: ["project"] };

  it("maps event types to the screens they affect", () => {
    expect(staleFor(hint("issue.updated", "PAY-1"), keys)).toEqual([["lists"], ["issue", "PAY-1"]]);
    expect(staleFor(hint("comment.created", "PAY-1"), keys)).toEqual([["lists"], ["issue", "PAY-1"]]);
    expect(staleFor(hint("sprint.started"), keys)).toEqual([["lists"]]);
    expect(staleFor(hint("issue.ranked"), keys)).toEqual([["lists"]]);
    expect(staleFor(hint("project.updated"), keys)).toEqual([["project"], ["lists"]]);
    expect(staleFor(hint("workflow.published"), keys)).toEqual([]);
  });

  it("batches refreshes and waits for the person's own change to finish", () => {
    vi.useFakeTimers();
    const qc = new QueryClient();
    const invalidate = vi.spyOn(qc, "invalidateQueries").mockResolvedValue();
    const mutating = vi.spyOn(qc, "isMutating").mockReturnValue(1);
    const batch = new RefreshBatch(qc);
    batch.add(["a"], ["b"]);
    batch.add(["a"]);
    vi.advanceTimersByTime(300);
    expect(invalidate).not.toHaveBeenCalled();
    mutating.mockReturnValue(0);
    vi.advanceTimersByTime(300);
    expect(invalidate.mock.calls.map((c) => c[0]?.queryKey)).toEqual([["a"], ["b"]]);
  });
});

/** A fetch that answers each stream request from a script, then holds the stream open. */
function streams(script: (call: number, request: { headers: Record<string, string> }) => { status: number; body?: string; end?: boolean }) {
  const calls: Record<string, string>[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_url: string, init: RequestInit) => {
      const headers = (init.headers ?? {}) as Record<string, string>;
      calls.push(headers);
      const { status, body, end } = script(calls.length, { headers });
      if (status !== 200) return new Response(null, { status });
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          if (body) controller.enqueue(new TextEncoder().encode(body));
          if (end) controller.close();
          init.signal?.addEventListener("abort", () => controller.error(new DOMException("aborted", "AbortError")));
        },
      });
      return new Response(stream, { status: 200, headers: { "content-type": "text/event-stream" } });
    }),
  );
  return calls;
}

const waitFor = async (check: () => boolean) => {
  for (let i = 0; i < 100 && !check(); i++) await new Promise((r) => setTimeout(r, 10));
  expect(check()).toBe(true);
};

describe("live subscription", () => {
  it("delivers hints, then reconnects from its cursor when the token expires", async () => {
    let token = "first";
    configureAuth({ getToken: () => token, refresh: async () => ((token = "second"), true) });
    const calls = streams((n) =>
      n === 1
        ? { status: 200, body: 'event: ready\nid: 5-0\ndata: {}\n\nevent: change\nid: 6-0\ndata: {"type":"issue.updated","id":"x","version":2,"key":"PAY-1","at":"t"}\n\nevent: expired\ndata: {}\n\n', end: true }
        : n === 2
          ? { status: 401 }
          : { status: 200, body: "event: ready\nid: 6-0\ndata: {}\n\n" },
    );
    const hints: Hint[] = [];
    const statuses: LiveStatus[] = [];
    const stop = subscribe("project:p", { onHint: (h) => hints.push(h), onResync: () => {}, onStatus: (s) => statuses.push(s) });
    await waitFor(() => calls.length === 3);
    stop();

    expect(hints.map((h) => h.key)).toEqual(["PAY-1"]);
    expect(calls[0]!.Authorization).toBe("Bearer first");
    expect(calls[0]!["Last-Event-ID"]).toBeUndefined();
    // Reconnected at once, refreshed the token on 401, and resumed after the last event seen.
    expect(calls[1]!["Last-Event-ID"]).toBe("6-0");
    expect(calls[2]!.Authorization).toBe("Bearer second");
    expect(calls[2]!["Last-Event-ID"]).toBe("6-0");
    expect(statuses).toContain("live");
  });

  it("asks for a refetch on resync and stops for good when access is denied", async () => {
    configureAuth({ getToken: () => "t", refresh: async () => false });
    const calls = streams(() => ({ status: 200, body: "event: resync\nid: 9-0\ndata: {}\n\nevent: denied\ndata: {}\n\n", end: true }));
    let resyncs = 0;
    const statuses: LiveStatus[] = [];
    subscribe("project:p", { onHint: () => {}, onResync: () => (resyncs += 1), onStatus: (s) => statuses.push(s) });
    await waitFor(() => statuses.at(-1) === "stopped");
    expect(resyncs).toBe(1);
    await new Promise((r) => setTimeout(r, 50));
    expect(calls).toHaveLength(1);
  });

  it("does not retry a channel it may not open", async () => {
    configureAuth({ getToken: () => "t", refresh: async () => false });
    const calls = streams(() => ({ status: 404 }));
    const statuses: LiveStatus[] = [];
    subscribe("project:secret", { onHint: () => {}, onResync: () => {}, onStatus: (s) => statuses.push(s) });
    await waitFor(() => statuses.at(-1) === "stopped");
    expect(calls).toHaveLength(1);
  });
});
