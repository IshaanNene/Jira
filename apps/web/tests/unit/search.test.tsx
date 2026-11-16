import { screen, waitFor, within } from "@testing-library/react";
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
          ? { items: [hit({ id: "i2", key: "PAY-2", summary: "Second page", summary_parts: null, snippet: null, status_name: "Code review" })], total: 2, total_is_lower_bound: false, next_cursor: null }
          : { items: [hit()], total: 2, total_is_lower_bound: false, next_cursor: "c1" },
      }),
    });
    renderAt(<SearchPage />, "/search", "/search?q=refunds");
    await userEvent.click(await screen.findByRole("button", { name: "Show more" }));
    expect(await screen.findByText("Second page")).toBeInTheDocument();
    expect(screen.getByText("Code review")).toBeInTheDocument(); // the workflow's name for the status
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
    expect(labels("type = bug ORDER BY ")).toEqual(["created", "updated", "resolved", "due", "priority", "key"]);
    expect(labels("fix")).toEqual(["fixVersion"]);
    expect(labels("component ")).toEqual(["=", "!=", "IN", "NOT IN", "IS EMPTY", "IS NOT EMPTY"]);
    expect(labels("points ")).toEqual(["=", "!=", ">=", "<=", ">", "<", "IS EMPTY", "IS NOT EMPTY"]);
    expect(labels("due < ")).toContain("now()");
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

  it("opens a shared filter, saves a copy, saves changes to it and deletes it", async () => {
    signIn();
    const BOB = "00000000-0000-7000-8000-0000000000b0";
    const at = "2026-11-15T00:00:00Z";
    type F = { id: string; name: string; query: string; owner_id: string; shared: boolean; editable: boolean; updated_at: string };
    let filters: F[] = [{ id: "f-bob", name: "Release blockers", query: "priority = highest", owner_id: BOB, shared: true, editable: false, updated_at: at }];
    const api = fakeApi({
      "GET /api/v1/projects": { items: [project({ id: "p1", key: "PAY", name: "Payments" })], next_cursor: null },
      "GET /api/v1/organizations/current/members": MEMBERS,
      "GET /api/v1/search/issues/query": { items: [hit()], total: 1, total_is_lower_bound: false, next_cursor: null },
      "GET /api/v1/search/filters": () => ({ body: { items: filters } }),
      "GET /api/v1/search/filters/f-bob": () => ({ body: filters.find((f) => f.id === "f-bob") }),
      "GET /api/v1/search/filters/f-mine": () => {
        const f = filters.find((x) => x.id === "f-mine");
        return f ? { body: f } : { status: 404, body: { code: "not_found", detail: "filter not found" } };
      },
      "POST /api/v1/search/filters": (call: Call) => {
        const body = call.body as { name: string; query: string; shared: boolean };
        if (body.name === "Release blockers") {
          return { status: 422, body: { code: "validation_failed", detail: "invalid", errors: [{ field: "name", code: "taken", message: 'You already have a filter called "Release blockers".' }] } };
        }
        const f = { id: "f-mine", ...body, owner_id: "00000000-0000-7000-8000-000000000001", editable: true, updated_at: at };
        filters = [f, ...filters];
        return { status: 201, body: f };
      },
      "PATCH /api/v1/search/filters/f-mine": (call: Call) => {
        filters = filters.map((f) => (f.id === "f-mine" ? { ...f, ...(call.body as Partial<F>) } : f));
        return { body: filters[0] };
      },
      "DELETE /api/v1/search/filters/f-mine": () => {
        filters = filters.filter((f) => f.id !== "f-mine");
        return { status: 204 };
      },
    });
    renderAt(<SearchPage />, "/search", "/search?mode=mql&mql=status%20%3D%20done");

    // Someone else's shared filter runs but cannot be changed.
    await userEvent.click(await screen.findByRole("button", { name: "Filters" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Release blockers" }));
    expect(await screen.findByRole("heading", { name: "Release blockers" })).toBeInTheDocument();
    await waitFor(() => expect(api.calls.at(-1)?.query.get("mql")).toBe("priority = highest"));
    const editor = screen.getByRole("combobox", { name: "MQL query" });
    await userEvent.clear(editor);
    await userEvent.type(editor, "priority = high{Enter}");
    expect(await screen.findByText("Edited")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save changes" })).toBeNull();

    // Save a copy of the edited query as one's own.
    await userEvent.click(screen.getByRole("button", { name: "Filter actions" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Save as new filter" }));
    let dialog = await screen.findByRole("dialog", { name: "Save filter" });
    expect(within(dialog).getByText("priority = high")).toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText("Name"), "Release blockers");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByText('You already have a filter called "Release blockers".')).toBeInTheDocument();
    await userEvent.clear(within(dialog).getByLabelText("Name"));
    await userEvent.type(within(dialog).getByLabelText("Name"), "High priority");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("heading", { name: "High priority" })).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/search/filters").at(-1)!.body).toEqual({ name: "High priority", query: "priority = high", shared: false });

    // Change and save one's own filter.
    await userEvent.clear(screen.getByRole("combobox", { name: "MQL query" }));
    await userEvent.type(screen.getByRole("combobox", { name: "MQL query" }), "priority >= high{Enter}");
    await userEvent.click(await screen.findByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(screen.queryByText("Edited")).toBeNull());
    expect(api.sent("PATCH", "/api/v1/search/filters/f-mine").at(-1)!.body).toEqual({ query: "priority >= high" });

    // Both groups are listed.
    await userEvent.click(screen.getByRole("button", { name: "Filters" }));
    expect(await screen.findByText("My filters")).toBeInTheDocument();
    expect(screen.getByText("Shared with the workspace")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");

    await userEvent.click(screen.getByRole("button", { name: "Filter actions" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    dialog = await screen.findByRole("dialog", { name: 'Delete "High priority"?' });
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await screen.findByRole("button", { name: "Save filter" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "High priority" })).toBeNull();
  });

  it("subscribes to a filter by email, changes the schedule and unsubscribes", async () => {
    signIn();
    const at = "2026-11-15T00:00:00Z";
    const filter = { id: "f1", name: "My bugs", query: "type = bug", owner_id: "00000000-0000-7000-8000-000000000001", shared: false, editable: true, updated_at: at };
    let sub: Record<string, unknown> | null = null;
    const api = fakeApi({
      "GET /api/v1/projects": { items: [], next_cursor: null },
      "GET /api/v1/organizations/current/members": MEMBERS,
      "GET /api/v1/search/issues/query": { items: [hit()], total: 1, total_is_lower_bound: false, next_cursor: null },
      "GET /api/v1/search/filters": { items: [filter] },
      "GET /api/v1/search/filters/f1": filter,
      "GET /api/v1/search/filters/f1/subscription": () => (sub ? { body: sub } : { status: 404, body: { code: "not_found", detail: "subscription not found" } }),
      "PUT /api/v1/search/filters/f1/subscription": (call: Call) => {
        sub = { filter_id: "f1", ...(call.body as object), next_run_at: "2026-11-16T09:00:00Z", last_sent_at: null };
        return { body: sub };
      },
      "DELETE /api/v1/search/filters/f1/subscription": () => {
        sub = null;
        return { status: 204 };
      },
    });
    renderAt(<SearchPage />, "/search", "/search?mode=mql&filter=f1");
    expect(await screen.findByRole("heading", { name: "My bugs" })).toBeInTheDocument();

    const open = async () => {
      await userEvent.click(screen.getByRole("button", { name: "Filter actions" }));
      await userEvent.click(await screen.findByRole("menuitem", { name: "Email subscription…" }));
      return screen.findByRole("dialog", { name: "Email subscription" });
    };
    let dialog = await open();
    expect(within(dialog).queryByRole("button", { name: "Unsubscribe" })).toBeNull();
    await userEvent.click(await within(dialog).findByRole("button", { name: "Subscribe" }));
    await waitFor(() => expect(api.sent("PUT", "/api/v1/search/filters/f1/subscription")).toHaveLength(1));
    expect(api.sent("PUT", "/api/v1/search/filters/f1/subscription")[0]!.body).toEqual({ frequency: "daily", hour: 9, weekday: null });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // Weekly on a chosen day.
    dialog = await open();
    expect(await within(dialog).findByText(/Next email/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("combobox", { name: "How often" }));
    await userEvent.click(await screen.findByRole("option", { name: "Every week" }));
    await userEvent.click(within(dialog).getByRole("combobox", { name: "Day" }));
    await userEvent.click(await screen.findByRole("option", { name: "Friday" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.sent("PUT", "/api/v1/search/filters/f1/subscription")).toHaveLength(2));
    expect(api.sent("PUT", "/api/v1/search/filters/f1/subscription")[1]!.body).toEqual({ frequency: "weekly", hour: 9, weekday: 5 });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    dialog = await open();
    await userEvent.click(await within(dialog).findByRole("button", { name: "Unsubscribe" }));
    await waitFor(() => expect(api.sent("DELETE", "/api/v1/search/filters/f1/subscription")).toHaveLength(1));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
