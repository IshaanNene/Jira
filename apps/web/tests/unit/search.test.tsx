import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SearchPage } from "@/modules/search";
import { applySuggestion, basicToMql, suggest, tokenize } from "@/modules/search/mql";
import type { SearchHit } from "@/services/api";

import { fakeApi, MEMBERS, project, renderAt, signIn, type Call } from "./harness";

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

describe("MQL suggestions", () => {
  const data = { projectKeys: ["PAY", "HR"], people: [{ name: "Maya Chen", email: "maya@example.com" }] };
  const labels = (text: string, caret = text.length) => suggest(text, caret, data).items.map((i) => i.label);

  it("offers fields, then operators, then values, then connectives", () => {
    expect(labels("")).toContain("ORDER BY");
    expect(labels("sta")).toEqual(["status", "statusCategory"]);
    expect(labels("status ")).toEqual(["=", "!=", "IN", "NOT IN"]);
    expect(labels("text ")).toEqual(["~", "!~"]);
    expect(labels("status = In")).toEqual(["In Progress", "In Review"]);
    expect(labels("project = ")).toEqual(["PAY", "HR"]);
    expect(labels("project in (PAY, ")).toEqual(["PAY", "HR"]);
    expect(labels("project in (PAY) ")).toEqual(["AND", "OR", "ORDER BY"]);
    expect(labels("assignee = ")).toEqual(["currentUser()", "Maya Chen"]);
    expect(labels("assignee is ")).toEqual(["EMPTY", "NOT EMPTY"]);
    expect(labels("created >= ")).toContain("startOfWeek()");
    expect(labels("status = done an")).toEqual(["AND"]);
    expect(labels("status = done AND pri")).toEqual(["priority"]);
    expect(labels("ORDER BY created ")).toEqual(["DESC", "ASC"]);
    expect(labels("type = bug ORDER BY ")).toEqual(["created", "updated", "resolved", "priority", "key"]);
    expect(suggest("assignee = ma", 13, data).valuesFor).toBe("assignee");
  });

  it("inserts the chosen suggestion, quoting where needed", () => {
    const s = suggest("assignee = ", 11, data);
    const maya = s.items.find((i) => i.label === "Maya Chen")!;
    expect(maya.insert).toBe("maya@example.com");
    expect(applySuggestion("assignee = ", s, maya)).toEqual({ text: "assignee = maya@example.com ", caret: 28 });
    const status = suggest('status = "in r', 14, data);
    expect(status.items.map((i) => i.label)).toEqual(["In Review"]);
    expect(applySuggestion('status = "in r', status, status.items[0]!).text).toBe('status = "In Review" ');
    // Replacing a word the caret is inside keeps what follows.
    const mid = suggest("sta = done", 3, data);
    expect(applySuggestion("sta = done", mid, mid.items[0]!).text).toBe("status = done");
  });

  it("tokenizes like the server and turns basic filters into a query", () => {
    expect(tokenize('summary ~ "a (b)" AND x!=y').map((t) => t.text)).toEqual(["summary", "~", '"a (b)"', "AND", "x", "!=", "y"]);
    expect(basicToMql({ q: 'refund "fast"', projectKey: "PAY", statusCategory: "in_progress", mine: true })).toBe(
      'project = PAY AND statusCategory = "In Progress" AND assignee = currentUser() AND text ~ "refund \\"fast\\"" ORDER BY updated DESC',
    );
    expect(basicToMql({})).toBe("ORDER BY updated DESC");
  });
});

describe("MQL search", () => {
  it("runs the query from the URL, shows where an invalid query goes wrong, and runs a fixed one", async () => {
    signIn();
    const api = fakeApi({
      "GET /api/v1/projects": { items: [project({ id: "p1", key: "PAY", name: "Payments" })], next_cursor: null },
      "GET /api/v1/organizations/current/members": MEMBERS,
      "GET /api/v1/search/issues/query": (call: Call) =>
        call.query.get("mql")?.startsWith("stauts")
          ? {
              status: 422,
              body: {
                code: "validation_failed",
                detail: "One or more fields are invalid.",
                errors: [{ field: "mql", code: "invalid_query", message: 'Unknown field "stauts".', span: [0, 6] }],
              },
            }
          : { body: { items: [hit()], total: 1, total_is_lower_bound: false, next_cursor: null } },
    });
    renderAt(<SearchPage />, "/search", "/search?mode=mql&mql=stauts%20%3D%20done");

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent('Unknown field "stauts".');
    expect(alert.querySelector("mark")).toHaveTextContent("stauts");
    const editor = screen.getByRole("combobox", { name: "MQL query" });
    expect(editor).toHaveAttribute("aria-invalid", "true");
    expect(api.calls.filter((c) => c.path === "/api/v1/search/issues/query")).toHaveLength(1);

    await userEvent.clear(editor);
    await userEvent.type(editor, "status = done{Enter}");
    await waitFor(() => expect(api.calls.at(-1)?.query.get("mql")).toBe("status = done"));
    expect(await screen.findByText("1 issue")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("switches from basic search carrying the filters over, and offers suggestions", async () => {
    signIn();
    fakeApi({
      "GET /api/v1/projects": { items: [project({ id: "p1", key: "PAY", name: "Payments" })], next_cursor: null },
      "GET /api/v1/organizations/current/members": MEMBERS,
      "GET /api/v1/search/issues": { items: [], total: 0, total_is_lower_bound: false, next_cursor: null },
      "GET /api/v1/search/issues/query": { items: [], total: 0, total_is_lower_bound: false, next_cursor: null },
    });
    renderAt(<SearchPage />, "/search", "/search?q=refund&project=p1&assignee=me");
    await screen.findByText("No issues match your search");
    await userEvent.click(screen.getByRole("button", { name: "MQL" }));
    const editor = await screen.findByRole("combobox", { name: "MQL query" });
    await waitFor(() => expect(editor).toHaveValue("project = PAY AND assignee = currentUser() AND text ~ refund ORDER BY updated DESC"));

    await userEvent.clear(editor);
    await userEvent.type(editor, "proj");
    const option = await screen.findByRole("option", { name: /project/ });
    await userEvent.keyboard("{Tab}");
    expect(editor).toHaveValue("project ");
    expect(option).not.toBeInTheDocument();
    expect(await screen.findByRole("option", { name: "=" })).toBeInTheDocument();
  });
});
