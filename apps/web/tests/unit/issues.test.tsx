import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { applicableFields } from "@/modules/issues/api";
import { formatBytes } from "@/modules/issues/components/Attachments";
import IssueListPage from "@/modules/issues/pages/IssueListPage";
import IssuePage from "@/modules/issues/pages/IssuePage";
import type { CustomField } from "@/services/api";

import { fakeApi, issue, MEMBERS, META, project, renderAt, signIn, STATUSES } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const field = (overrides: Partial<CustomField>): CustomField => ({
  id: "f1", name: "Customer", field_type: "text", options: [], required: false, project_keys: [], issue_types: [], created_at: "2026-01-01T00:00:00Z",
  ...overrides,
});

function issueRoutes(detail: Record<string, unknown>, extra: Record<string, unknown> = {}) {
  return {
    "GET /api/v1/issues/PAY-1": detail,
    "GET /api/v1/issues/meta": META,
    "GET /api/v1/projects/PAY": project({ my_role: detail.my_role }),
    "GET /api/v1/projects/PAY/components": { items: [] },
    "GET /api/v1/projects/PAY/versions": { items: [] },
    "GET /api/v1/organizations/current/members": MEMBERS,
    "GET /api/v1/custom-fields": { items: [field({}), field({ id: "f2", name: "Severity", issue_types: ["task"] })] },
    "GET /api/v1/issues": { items: [], next_cursor: null },
    "GET /api/v1/issues/PAY-1/links": { items: [] },
    "GET /api/v1/issues/PAY-1/comments": { items: [] },
    "GET /api/v1/issues/PAY-1/attachments": { items: [] },
    "GET /api/v1/issues/PAY-1/transitions": {
      items: [
        { id: "start", name: "Start work", to: { key: "in_progress", name: "In Progress", category: "in_progress" } },
        { id: "close", name: "Close", to: { key: "done", name: "Done", category: "done" } },
      ],
    },
    ...extra,
  };
}
const detail = (overrides: Record<string, unknown> = {}) => ({ ...issue(), watching: true, watcher_count: 1, voted: false, vote_count: 0, my_role: "member", ...overrides });
const renderIssue = () => renderAt(<IssuePage />, "/projects/:projectKey/issues/:issueKey", "/projects/PAY/issues/PAY-1");

describe("IssueListPage", () => {
  const routes = (items: unknown[]) => ({
    "GET /api/v1/issues": { items, next_cursor: null },
    "GET /api/v1/issues/meta": META,
    "GET /api/v1/projects/PAY": project(),
    "GET /api/v1/organizations/current/members": MEMBERS,
    "GET /api/v1/workflow-statuses": STATUSES,
  });

  it("renders issues with key, status and links to the issue", async () => {
    signIn();
    fakeApi(routes([issue(), issue({ id: "i2", key: "PAY-2", summary: "Done thing", status: "done", status_name: "Shipped", status_category: "done" })]));
    renderAt(<IssueListPage />, "/projects/:projectKey/issues", "/projects/PAY/issues");
    expect(await screen.findByRole("link", { name: "PAY-1" })).toHaveAttribute("href", "/projects/PAY/issues/PAY-1");
    const row = screen.getByRole("link", { name: "Checkout fails" }).closest("tr")!;
    expect(within(row).getByText("To Do")).toBeInTheDocument();
    expect(within(row).getByText("api")).toBeInTheDocument();
    // Status names come from each issue's workflow, not a fixed list.
    expect(screen.getByText("Shipped")).toBeInTheDocument();
    expect(screen.getByText("2 issues")).toBeInTheDocument();
  });

  it("sends filters from the URL to the API and explains an empty filtered result", async () => {
    signIn();
    const api = fakeApi(routes([]));
    renderAt(<IssueListPage />, "/projects/:projectKey/issues", "/projects/PAY/issues?status=done&assignee=me");
    expect(await screen.findByText("No issues match your filters")).toBeInTheDocument();
    const query = api.sent("GET", "/api/v1/issues")[0]!.query;
    expect([query.get("project"), query.get("status"), query.get("assignee")]).toEqual(["PAY", "done", "me"]);
    expect(screen.getByRole("button", { name: "Clear filters" })).toBeInTheDocument();
  });

  it("hides Create for viewers", async () => {
    signIn();
    fakeApi({ ...routes([]), "GET /api/v1/projects/PAY": project({ my_role: "viewer" }) });
    renderAt(<IssueListPage />, "/projects/:projectKey/issues", "/projects/PAY/issues");
    expect(await screen.findByText("No issues yet")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create issue" })).not.toBeInTheDocument();
  });
});

describe("IssuePage", () => {
  it("shows the issue and only the custom fields that apply to it", async () => {
    signIn();
    fakeApi(issueRoutes(detail({ custom_fields: { f1: "Globex" } })));
    renderIssue();
    expect(await screen.findByLabelText("Summary")).toHaveValue("Checkout fails");
    expect(await screen.findByLabelText("Customer")).toHaveValue("Globex");
    // "Severity" is limited to tasks; this issue is a bug.
    expect(screen.queryByLabelText("Severity")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Status: To Do/ })).toBeInTheDocument();
  });

  it("saves an inline edit with the version it was based on", async () => {
    signIn();
    const api = fakeApi(
      issueRoutes(detail(), { "PATCH /api/v1/issues/PAY-1": (call: { body: unknown }) => ({ body: issue({ ...(call.body as object), version: 4 }), headers: { etag: '"4"' } }) }),
    );
    renderIssue();
    const summary = await screen.findByLabelText("Summary");
    await userEvent.clear(summary);
    await userEvent.type(summary, "Checkout fails for saved cards{Enter}");
    await waitFor(() => expect(api.sent("PATCH", "/api/v1/issues/PAY-1")).toHaveLength(1));
    const call = api.sent("PATCH", "/api/v1/issues/PAY-1")[0]!;
    expect(call.headers["If-Match"]).toBe('"3"');
    expect(call.body).toEqual({ summary: "Checkout fails for saved cards" });

    // The next edit is based on the version the server returned.
    fireEvent.change(await screen.findByLabelText("Labels"), { target: { value: "api urgent" } });
    fireEvent.blur(screen.getByLabelText("Labels"));
    await waitFor(() => expect(api.sent("PATCH", "/api/v1/issues/PAY-1")).toHaveLength(2));
    const second = api.sent("PATCH", "/api/v1/issues/PAY-1")[1]!;
    expect(second.headers["If-Match"]).toBe('"4"');
    expect(second.body).toEqual({ labels: ["api", "urgent"] });
  });

  it("does not overwrite someone else's change: a conflict shows their version and says so", async () => {
    signIn();
    fakeApi(
      issueRoutes(detail(), {
        "PATCH /api/v1/issues/PAY-1": () => ({ status: 409, body: { code: "version_conflict", detail: "conflict", current: issue({ summary: "Renamed by Grace", version: 9 }) } }),
      }),
    );
    renderIssue();
    const summary = await screen.findByLabelText("Summary");
    await userEvent.clear(summary);
    await userEvent.type(summary, "My rename{Enter}");
    expect(await screen.findByText("Someone else updated this issue")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Summary")).toHaveValue("Renamed by Grace"));
  });

  it("rolls back an optimistic edit when the server refuses it", async () => {
    signIn();
    fakeApi(issueRoutes(detail(), { "PATCH /api/v1/issues/PAY-1": () => ({ status: 403, body: { code: "forbidden", detail: "You do not have permission to perform this action." } }) }));
    renderIssue();
    const summary = await screen.findByLabelText("Summary");
    await userEvent.clear(summary);
    await userEvent.type(summary, "Not allowed{Enter}");
    expect(await screen.findByText("Couldn't save your change")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Summary")).toHaveValue("Checkout fails"));
  });

  it("offers the workflow's steps and moves the issue only when the server agrees", async () => {
    signIn();
    let attempt = 0;
    const api = fakeApi(
      issueRoutes(detail(), {
        "PATCH /api/v1/issues/PAY-1": (call: { body: unknown }) => {
          attempt += 1;
          // First try: the workflow refuses (a validator fails). Second try: allowed.
          return attempt === 1
            ? { status: 422, body: { code: "validation_failed", detail: "One or more fields are invalid.", errors: [{ field: "status", code: "transition_denied", message: "Due date is required to move this issue to In Progress." }] } }
            : { body: issue({ ...(call.body as object), status_name: "In Progress", status_category: "in_progress", version: 4 }) };
        },
      }),
    );
    renderIssue();
    const button = await screen.findByRole("button", { name: /Status: To Do/ });
    await userEvent.click(button);
    await userEvent.click(await screen.findByRole("menuitem", { name: /Start work/ }));
    // Refused: the reason from the workflow is shown and the status has not moved.
    expect(await screen.findByText("Due date is required to move this issue to In Progress.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Status: To Do/ })).toBeInTheDocument();
    expect(api.sent("PATCH", "/api/v1/issues/PAY-1")[0]!.body).toEqual({ status: "in_progress" });

    await userEvent.click(screen.getByRole("button", { name: /Status: To Do/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /Start work/ }));
    expect(await screen.findByRole("button", { name: /Status: In Progress/ })).toBeInTheDocument();
  });

  it("is read-only for viewers", async () => {
    signIn();
    fakeApi(issueRoutes(detail({ my_role: "viewer" })));
    renderIssue();
    expect(await screen.findByRole("heading", { name: "Checkout fails" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Summary")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Change status/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Attach/ })).not.toBeInTheDocument();
    expect(screen.queryByText("Add a comment…")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "More actions" })).not.toBeInTheDocument();
  });

  it("explains a missing or inaccessible issue instead of failing", async () => {
    signIn();
    fakeApi({ "GET /api/v1/issues/PAY-1": () => ({ status: 404, body: { code: "not_found", detail: "The requested issue does not exist." } }), "GET /api/v1/projects/PAY": project() });
    renderIssue();
    expect(await screen.findByText("Issue not found")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to issues" })).toHaveAttribute("href", "/projects/PAY/issues");
  });
});

describe("attachments", () => {
  it("uploads a chosen file as multipart and refuses oversized files without calling the API", async () => {
    signIn();
    const api = fakeApi(
      issueRoutes(detail(), {
        "POST /api/v1/issues/PAY-1/attachments": () => ({ status: 201, body: { id: "a1", filename: "log.txt", content_type: "text/plain", size_bytes: 5, uploader_id: "u", created_at: "2026-11-02T10:00:00Z" } }),
      }),
    );
    renderIssue();
    const input = await screen.findByLabelText("Attach files");

    const big = new File([new Uint8Array(1)], "huge.bin");
    Object.defineProperty(big, "size", { value: 11 * 1024 * 1024 });
    await userEvent.upload(input, big);
    expect(await screen.findByText("huge.bin is too large")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/issues/PAY-1/attachments")).toHaveLength(0);

    await userEvent.upload(input, new File(["hello"], "log.txt", { type: "text/plain" }));
    await waitFor(() => expect(api.sent("POST", "/api/v1/issues/PAY-1/attachments")).toHaveLength(1));
    const call = api.sent("POST", "/api/v1/issues/PAY-1/attachments")[0]!;
    expect(call.body).toBeInstanceOf(FormData);
    expect(((call.body as FormData).get("file") as File).name).toBe("log.txt");
    // The browser must set the multipart boundary itself.
    expect(call.headers["Content-Type"]).toBeUndefined();
  });

  it("formats sizes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });
});

describe("custom field applicability", () => {
  it("matches project and issue type contexts", () => {
    const fields = [field({ id: "all" }), field({ id: "pay", project_keys: ["PAY"] }), field({ id: "bugs", issue_types: ["bug"] }), field({ id: "ops-bugs", project_keys: ["OPS"], issue_types: ["bug"] })];
    expect(applicableFields(fields, "PAY", "bug").map((f) => f.id)).toEqual(["all", "pay", "bugs"]);
    expect(applicableFields(fields, "OPS", "task").map((f) => f.id)).toEqual(["all"]);
    expect(applicableFields(undefined, "PAY", "bug")).toEqual([]);
  });
});

describe("cloning", () => {
  it("lets members clone with an editable summary and opens the copy", async () => {
    signIn();
    const copy = detail({ id: "i2", key: "PAY-2", summary: "Checkout fails on iOS" });
    const api = fakeApi(issueRoutes(detail(), {
      "POST /api/v1/issues/PAY-1/clone": () => ({ status: 201, body: copy }),
      "GET /api/v1/issues/PAY-2": copy,
      "GET /api/v1/issues/PAY-2/links": { items: [] },
      "GET /api/v1/issues/PAY-2/comments": { items: [] },
      "GET /api/v1/issues/PAY-2/attachments": { items: [] },
      "GET /api/v1/issues/PAY-2/transitions": { items: [] },
    }));
    renderIssue();
    await userEvent.click(await screen.findByRole("button", { name: "More actions" }));
    expect(screen.queryByRole("menuitem", { name: "Delete issue" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("menuitem", { name: "Clone" }));
    const dialog = await screen.findByRole("dialog", { name: "Clone PAY-1" });
    const input = within(dialog).getByLabelText("Summary");
    expect(input).toHaveValue("CLONE - Checkout fails");
    await userEvent.clear(input);
    await userEvent.type(input, "Checkout fails on iOS");
    await userEvent.click(within(dialog).getByRole("button", { name: "Clone" }));
    expect(await screen.findByRole("heading", { name: "Checkout fails on iOS" })).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/issues/PAY-1/clone")[0]!.body).toEqual({ summary: "Checkout fails on iOS" });
  });

  it("hides cloning from viewers", async () => {
    signIn();
    fakeApi(issueRoutes(detail({ my_role: "viewer" })));
    renderIssue();
    expect(await screen.findByRole("heading", { name: "Checkout fails" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "More actions" })).not.toBeInTheDocument();
  });
});

describe("voting", () => {
  it("lets anyone who can see the issue vote and take the vote back", async () => {
    signIn();
    let current = detail({ my_role: "viewer" });
    const api = fakeApi(issueRoutes(current, {
      "GET /api/v1/issues/PAY-1": () => ({ status: 200, body: current }),
      "PUT /api/v1/issues/PAY-1/vote": () => {
        current = { ...current, voted: true, vote_count: 1 };
        return { status: 204, body: null };
      },
      "DELETE /api/v1/issues/PAY-1/vote": () => {
        current = { ...current, voted: false, vote_count: 0 };
        return { status: 204, body: null };
      },
    }));
    renderIssue();
    await userEvent.click(await screen.findByRole("button", { name: "Vote for this issue" }));
    const voted = await screen.findByRole("button", { name: "Remove your vote" });
    expect(voted).toHaveAttribute("aria-pressed", "true");
    expect(voted).toHaveTextContent("1");
    expect(api.sent("PUT", "/api/v1/issues/PAY-1/vote")).toHaveLength(1);
    await userEvent.click(voted);
    expect(await screen.findByRole("button", { name: "Vote for this issue" })).toHaveTextContent("0");
    expect(api.sent("DELETE", "/api/v1/issues/PAY-1/vote")).toHaveLength(1);
  });

  it("closes voting on resolved issues", async () => {
    signIn();
    fakeApi(issueRoutes(detail({ status: "done", status_category: "done", vote_count: 3 })));
    renderIssue();
    const closed = await screen.findByRole("button", { name: "Vote for this issue" });
    expect(closed).toBeDisabled();
    expect(closed).toHaveAttribute("title", "Voting is closed: this issue is resolved");
  });

  it("keeps a cast vote removable on a resolved issue", async () => {
    signIn();
    fakeApi(issueRoutes(detail({ status: "done", status_category: "done", voted: true, vote_count: 3 })));
    renderIssue();
    expect(await screen.findByRole("button", { name: "Remove your vote" })).toBeEnabled();
  });
});
