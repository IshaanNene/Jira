import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import type { ReactElement } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { vi } from "vitest";

import { Toaster, TooltipProvider } from "@/components/ui";
import { configureAuth, type OrgRole } from "@/services/api";
import { useSession } from "@/state/session";

export interface Call {
  method: string;
  path: string;
  query: URLSearchParams;
  headers: Record<string, string>;
  body: unknown;
}

type Handler = (call: Call) => { status?: number; body?: unknown; headers?: Record<string, string> };

/**
 * A fake API at the network boundary: components, hooks and the real HTTP client all
 * run for real; only `fetch` is replaced. Routes are "METHOD /path".
 */
export function fakeApi(routes: Record<string, Handler | unknown>) {
  const calls: Call[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), "http://localhost");
    const method = init?.method ?? "GET";
    const raw = init?.body;
    const call: Call = {
      method,
      path: url.pathname,
      query: url.searchParams,
      headers: (init?.headers ?? {}) as Record<string, string>,
      body: typeof raw === "string" ? JSON.parse(raw) : raw,
    };
    calls.push(call);
    const route = routes[`${method} ${url.pathname}`];
    if (route === undefined) {
      return new Response(JSON.stringify({ code: "not_found", detail: `no fake route for ${method} ${url.pathname}` }), { status: 404 });
    }
    const result = typeof route === "function" ? (route as Handler)(call) : { body: route };
    const status = result.status ?? 200;
    return new Response(status === 204 ? null : JSON.stringify(result.body ?? null), { status, headers: { "content-type": "application/json", ...result.headers } });
  });
  vi.stubGlobal("fetch", fetchMock);
  return { calls, sent: (method: string, path: string) => calls.filter((c) => c.method === method && c.path === path) };
}

export const USER_ID = "00000000-0000-7000-8000-000000000001";
export const ORG_ID = "00000000-0000-7000-8000-0000000000aa";

export function signIn(role: OrgRole = "member") {
  configureAuth({ getToken: () => "test-token", refresh: async () => false });
  useSession.setState({
    status: "authenticated",
    accessToken: "test-token",
    account: { id: USER_ID, email: "ada@example.com", display_name: "Ada Lovelace", email_verified: true, created_at: "2026-01-01T00:00:00Z" },
    organization: { id: ORG_ID, role },
  });
}

/** Render `element` at `path` (a route pattern) with the app's providers. */
export function renderAt(element: ReactElement, path: string, url: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[url]}>
          <Routes>
            <Route path={path} element={element} />
            <Route path="*" element={<p>other page</p>} />
          </Routes>
        </MemoryRouter>
        <Toaster />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

export const META = {
  types: [
    { key: "epic", name: "Epic", level: 1 },
    { key: "task", name: "Task", level: 0 },
    { key: "bug", name: "Bug", level: 0 },
    { key: "subtask", name: "Subtask", level: -1 },
  ],
  priorities: ["highest", "high", "medium", "low", "lowest"],
  link_types: [{ key: "blocks", outward: "blocks", inward: "is blocked by" }],
};

export const MEMBERS = {
  items: [{ user_id: USER_ID, display_name: "Ada Lovelace", email: "ada@example.com", org_role: "member", status: "active", joined_at: "2026-01-01T00:00:00Z" }],
  next_cursor: null,
};

export function project(overrides: Record<string, unknown> = {}) {
  return {
    id: "00000000-0000-7000-8000-0000000000b1", key: "PAY", name: "Payments", description: "", project_type: "scrum", lead_user_id: null, status: "active",
    default_role: "member", my_role: "member", version: 1, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

export function issue(overrides: Record<string, unknown> = {}) {
  return {
    id: "i1", key: "PAY-1", project_key: "PAY", type: "bug", status: "todo", status_name: "To Do", status_category: "todo", priority: "high",
    summary: "Checkout fails", description: null, assignee_id: null, reporter_id: USER_ID, parent: null, labels: ["api"],
    component_ids: [], fix_version_ids: [], custom_fields: {}, story_points: null, start_date: null, due_date: null,
    resolution: null, resolved_at: null, version: 3, created_at: "2026-11-01T10:00:00Z", updated_at: "2026-11-02T10:00:00Z",
    ...overrides,
  };
}

export const STATUSES = {
  items: [
    { key: "todo", name: "To Do", category: "todo" },
    { key: "in_progress", name: "In Progress", category: "in_progress" },
    { key: "done", name: "Done", category: "done" },
  ],
};
