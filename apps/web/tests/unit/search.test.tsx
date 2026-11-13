import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SearchPage } from "@/modules/search";
import type { SearchHit } from "@/services/api";

import { fakeApi, project, renderAt, signIn, type Call } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const hit = (overrides: Partial<SearchHit> = {}): SearchHit => ({
  id: "i1", key: "PAY-1", project_id: "p1", project_key: "PAY", type: "bug", status: "in_review", status_category: "in_progress",
  priority: "high", summary: "Refunds <b>fail</b>", assignee_id: null, updated_at: new Date().toISOString(),
  summary_parts: [{ text: "Refunds", matched: true }, { text: " <b>fail</b>", matched: false }],
  snippet: { field: "comments", parts: [{ text: "seen on ", matched: false }, { text: "refund", matched: true }] },
  ...overrides,
});

describe("issue search", () => {
  it("searches as you type, highlights matches as text, and narrows by filters", async () => {
    signIn();
    const api = fakeApi({
      "GET /api/v1/projects": { items: [project({ id: "p1", key: "PAY", name: "Payments" })], next_cursor: null },
      "GET /api/v1/search/issues": (call: Call) => ({
        body: call.query.get("q") === "refunds" && !call.query.get("assignee")
          ? { items: [hit()], total: 1, total_is_lower_bound: false, next_cursor: null }
          : { items: [], total: 0, total_is_lower_bound: false, next_cursor: null },
      }),
    });
    renderAt(<SearchPage />, "/search", "/search");
    expect(await screen.findByText("No issues yet")).toBeInTheDocument();

    await userEvent.type(screen.getByRole("textbox", { name: "Search issues" }), "refunds");
    expect(await screen.findByText("1 results")).toBeInTheDocument();
    const mark = screen.getByText("Refunds");
    expect(mark.tagName).toBe("MARK");
    // Document text is never interpreted as markup.
    expect(screen.getByText("<b>fail</b>", { exact: false })).toBeInTheDocument();
    expect(document.querySelector("li b")).toBeNull();
    expect(screen.getByText("In comments:", { exact: false })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /PAY-1/ })).toHaveAttribute("href", "/projects/PAY/issues/PAY-1");
    expect(screen.getByText("In review")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Assigned to me" }));
    await waitFor(() => expect(api.calls.some((c) => c.path === "/api/v1/search/issues" && c.query.get("assignee") === "me" && c.query.get("q") === "refunds")).toBe(true));
    expect(await screen.findByText("No issues match your search")).toBeInTheDocument();
  });

  it("pages through results with the cursor", async () => {
    signIn();
    const api = fakeApi({
      "GET /api/v1/projects": { items: [], next_cursor: null },
      "GET /api/v1/search/issues": (call: Call) => ({
        body: call.query.get("cursor")
          ? { items: [hit({ id: "i2", key: "PAY-2", summary: "Second page", summary_parts: null, snippet: null })], total: 2, total_is_lower_bound: false, next_cursor: null }
          : { items: [hit()], total: 2, total_is_lower_bound: false, next_cursor: "c1" },
      }),
    });
    renderAt(<SearchPage />, "/search", "/search?q=refunds");
    await userEvent.click(await screen.findByRole("button", { name: "Show more" }));
    expect(await screen.findByText("Second page")).toBeInTheDocument();
    expect(api.calls.filter((c) => c.path === "/api/v1/search/issues").at(-1)!.query.get("cursor")).toBe("c1");
    expect(screen.queryByRole("button", { name: "Show more" })).toBeNull();
  });
});
