import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AutomationSettings, describeRule } from "@/modules/automation";
import type { AutomationRule } from "@/services/api";

import { fakeApi, MEMBERS, META, renderAt, signIn, STATUSES, USER_ID } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const names = { status: (k: string) => (k === "in_review" ? "In Review" : k), type: (k: string) => (k === "bug" ? "Bug" : k), person: () => "Ada Lovelace" };

const rule = (overrides: Partial<AutomationRule> = {}): AutomationRule => ({
  id: "r1",
  project_id: "p1",
  name: "Triage bugs",
  enabled: true,
  version: 3,
  created_by: USER_ID,
  actor_id: USER_ID,
  next_run_at: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  definition: {
    trigger: { type: "issue_created" },
    conditions: { issue_types: ["bug"], priorities: [], labels_any: [] },
    actions: [{ type: "assign", assignee: { kind: "reporter" } }, { type: "set_priority", priority: "high" }],
  },
  ...overrides,
});

const routes = (rules: AutomationRule[], extra: Record<string, unknown> = {}) => ({
  "GET /api/v1/automation-rules": { items: rules },
  "GET /api/v1/workflow-statuses": STATUSES,
  "GET /api/v1/issues/meta": META,
  "GET /api/v1/organizations/current/members": MEMBERS,
  ...extra,
});

const show = () => renderAt(<AutomationSettings projectKey="PAY" projectId="p1" />, "/settings", "/settings");

describe("automation rules", () => {
  it("describes a rule in one sentence", () => {
    expect(describeRule(rule().definition, names)).toBe("When an issue is created, if type is Bug: assign to the reporter, set priority to High.");
    expect(
      describeRule(
        { trigger: { type: "issue_transitioned", to_status: "in_review" }, actions: [{ type: "add_labels", labels: ["qa", "web"] }, { type: "assign", assignee: { kind: "user", user_id: USER_ID } }] },
        names,
      ),
    ).toBe("When an issue moves to In Review: add labels qa, web, assign to Ada Lovelace.");
    expect(
      describeRule(
        {
          trigger: { type: "scheduled", frequency: "weekly", hour: 9, weekday: 1 },
          conditions: { statuses: ["in_review"], untouched_days: 7 },
          actions: [{ type: "transition", to_status: "in_review" }, { type: "add_comment", body: "Is this still needed?\nPlease update it or close it." }],
        },
        names,
      ),
    ).toBe("Every Monday at 09:00 UTC, for issues where status is In Review and unchanged for 7 days: move to In Review, comment “Is this still needed? Please update it…”.");
    expect(describeRule({ trigger: { type: "scheduled", frequency: "daily", hour: 17 }, actions: [{ type: "add_labels", labels: ["x"] }] }, names)).toBe(
      "Every day at 17:00 UTC, for every issue: add label x.",
    );
  });

  it("shows who a scheduled rule runs as and edits its comment and staleness", async () => {
    signIn();
    const scheduled = rule({
      id: "r3",
      name: "Nudge",
      next_run_at: "2026-11-17T09:00:00Z",
      definition: {
        trigger: { type: "scheduled", frequency: "daily", hour: 9 },
        conditions: { statuses: ["todo"], untouched_days: 7 },
        actions: [{ type: "add_comment", body: "Still needed?" }],
      },
    });
    const api = fakeApi(routes([scheduled], { "PUT /api/v1/automation-rules/r3": rule({ id: "r3", version: 4 }) }));
    show();
    const list = await screen.findByRole("list", { name: "Automation rules" });
    expect(within(list).getByText(/Runs as .* next run/)).toBeInTheDocument();
    await userEvent.click(within(list).getByRole("button", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit rule" });
    const comment = within(dialog).getByLabelText("Action 1 comment");
    await userEvent.clear(comment);
    await userEvent.type(comment, "Please update or close this.");
    const days = within(dialog).getByLabelText("Unchanged for at least (days)");
    await userEvent.clear(days);
    await userEvent.type(days, "14");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await vi.waitFor(() => expect(api.sent("PUT", "/api/v1/automation-rules/r3")).toHaveLength(1));
    expect(api.sent("PUT", "/api/v1/automation-rules/r3")[0]!.body).toMatchObject({
      definition: {
        trigger: { type: "scheduled", frequency: "daily", hour: 9 },
        conditions: { statuses: ["todo"], untouched_days: 14 },
        actions: [{ type: "add_comment", body: "Please update or close this." }],
      },
    });
  });

  it("lists the project's rules and turns one off with the version it was based on", async () => {
    signIn();
    const api = fakeApi(routes([rule()], { "PUT /api/v1/automation-rules/r1": rule({ enabled: false, version: 4 }) }));
    show();
    const list = await screen.findByRole("list", { name: "Automation rules" });
    expect(within(list).getByText("Triage bugs")).toBeInTheDocument();
    expect(api.sent("GET", "/api/v1/automation-rules")[0]!.query.get("project")).toBe("PAY");
    await userEvent.click(screen.getByRole("switch", { name: "Triage bugs enabled" }));
    const put = api.sent("PUT", "/api/v1/automation-rules/r1")[0]!;
    expect(put.body).toMatchObject({ name: "Triage bugs", enabled: false });
    expect(put.headers["If-Match"]).toBe('"3"');
  });

  it("creates a rule from the dialog", async () => {
    signIn();
    const api = fakeApi(routes([], { "POST /api/v1/automation-rules": () => ({ status: 201, body: rule({ id: "r2" }) }) }));
    show();
    expect(await screen.findByText("No rules yet")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /new rule/i }));
    const dialog = await screen.findByRole("dialog", { name: "New rule" });
    await userEvent.type(within(dialog).getByLabelText("Name"), "Escalate");
    await userEvent.type(within(dialog).getByLabelText("Has any of these labels"), "Customer, outage");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add action" }));
    await userEvent.type(within(dialog).getByLabelText("Action 2 labels"), "escalated");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create rule" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(api.sent("POST", "/api/v1/automation-rules")[0]!.body).toEqual({
      project_key: "PAY",
      name: "Escalate",
      enabled: true,
      definition: {
        trigger: { type: "issue_created" },
        conditions: { issue_types: [], priorities: [], labels_any: ["Customer", "outage"], statuses: [] },
        actions: [{ type: "set_priority", priority: "high" }, { type: "add_labels", labels: ["escalated"] }],
      },
    });
  });

  it("shows the server's reason when a rule is refused", async () => {
    signIn();
    fakeApi(routes([], {
      "POST /api/v1/automation-rules": () => ({
        status: 422,
        body: { type: "about:blank", title: "Validation failed", status: 422, code: "validation_failed", detail: "The person to assign must be a member of this workspace.", errors: [] },
      }),
    }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: /new rule/i }));
    const dialog = await screen.findByRole("dialog", { name: "New rule" });
    await userEvent.type(within(dialog).getByLabelText("Name"), "x");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create rule" }));
    expect(await within(dialog).findByRole("alert")).toBeInTheDocument();
  });

  it("shows what a rule did recently, including why it was skipped", async () => {
    signIn();
    fakeApi(routes([rule()], {
      "GET /api/v1/automation-rules/r1/runs": {
        items: [
          { id: "x1", issue_id: "i1", issue_key: "PAY-7", actor_id: USER_ID, outcome: "failed", detail: "The person to assign is no longer a member of this workspace.", created_at: "2026-01-01T00:00:00Z" },
          { id: "x2", issue_id: "i2", issue_key: "PAY-6", actor_id: USER_ID, outcome: "applied", detail: null, created_at: "2026-01-01T00:00:00Z" },
          { id: "x3", issue_id: null, issue_key: null, actor_id: USER_ID, outcome: "failed", detail: "The person this rule runs as no longer administers this project.", created_at: "2026-01-01T00:00:00Z" },
        ],
      },
    }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Recent runs of Triage bugs" }));
    const runs = await screen.findByRole("list", { name: "Runs" });
    expect(within(runs).getByRole("link", { name: "PAY-7" })).toHaveAttribute("href", "/projects/PAY/issues/PAY-7");
    expect(within(runs).getAllByText("Skipped")).toHaveLength(2);
    expect(within(runs).getByText(/no longer a member/)).toBeInTheDocument();
    expect(within(runs).getByText("Applied")).toBeInTheDocument();
    expect(within(runs).getByText("Didn't start")).toBeInTheDocument();
  });
});
