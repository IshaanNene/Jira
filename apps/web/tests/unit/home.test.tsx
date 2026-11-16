import { screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import HomePage, { ASSIGNED_TO_ME } from "@/modules/workspace/pages/HomePage";

import { fakeApi, project, renderAt, signIn, USER_ID } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const hit = (key: string, project_key: string, overrides: Record<string, unknown> = {}) => ({
  id: key, key, project_key, project_id: `p-${project_key}`, summary: `Work on ${key}`, type: "task", priority: "medium",
  status: "in_review", status_category: "in_progress", assignee_id: USER_ID, updated_at: "2026-11-10T00:00:00Z", ...overrides,
});

describe("home page", () => {
  it("lists open work assigned to me across every project with one search", async () => {
    signIn();
    const projects = Array.from({ length: 10 }, (_, i) => project({ id: `p${i}`, key: `P${i}`, name: `Project ${i}` }));
    const api = fakeApi({
      "GET /api/v1/projects": { items: projects, next_cursor: null },
      "GET /api/v1/search/issues/query": { items: [hit("P9-4", "P9"), hit("P0-1", "P0")], next_cursor: "c", total: 31, total_is_lower_bound: false },
    });
    renderAt(<HomePage />, "/", "/");
    const link = await screen.findByRole("link", { name: /Work on P9-4/ });
    expect(link).toHaveAttribute("href", "/projects/P9/issues/P9-4");
    expect(screen.getAllByText(/^in review$/i)).toHaveLength(2);
    expect(api.sent("GET", "/api/v1/search/issues/query")).toHaveLength(1);
    expect(api.sent("GET", "/api/v1/search/issues/query")[0]!.query.get("mql")).toBe(ASSIGNED_TO_ME);
    expect(api.sent("GET", "/api/v1/issues")).toHaveLength(0);
    expect(screen.getByRole("link", { name: "View all 31 in search" })).toHaveAttribute("href", `/search?mode=mql&mql=${encodeURIComponent(ASSIGNED_TO_ME)}`);
  });

  it("says when nothing is assigned, and offers a retry when search is down", async () => {
    signIn();
    let down = true;
    fakeApi({
      "GET /api/v1/projects": { items: [project()], next_cursor: null },
      "GET /api/v1/search/issues/query": () => (down ? { status: 503, body: { code: "dependency_unavailable", detail: "Search is unavailable." } } : { body: { items: [], next_cursor: null, total: 0, total_is_lower_bound: false } }),
    });
    renderAt(<HomePage />, "/", "/");
    const retry = await screen.findByRole("button", { name: /Try again|Retry/ }, { timeout: 4000 });
    down = false;
    retry.click();
    expect(await screen.findByText("Nothing assigned to you")).toBeInTheDocument();
  });
});
