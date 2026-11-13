import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AuditLogPage } from "@/modules/organizations";
import { describe as sentence } from "@/modules/organizations/audit";
import type { AuditEntry } from "@/services/api";

import { fakeApi, MEMBERS, renderAt, signIn, USER_ID, type Call } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const MAYA = "00000000-0000-7000-8000-000000000002";
const people = { [USER_ID]: "Ada Lovelace", [MAYA]: "Maya Chen" };

const entry = (overrides: Partial<AuditEntry> = {}): AuditEntry => ({
  seq: 1,
  id: `00000000-0000-7000-8000-1000000000${String(overrides.seq ?? 1).padStart(2, "0")}`,
  occurred_at: new Date().toISOString(),
  actor: { type: "user", id: USER_ID },
  action: "member.role_changed",
  resource_type: "user",
  resource_id: MAYA,
  resource_label: "",
  details: { role: "admin" },
  request_id: null,
  ...overrides,
});

describe("audit sentences", () => {
  it("reads each kind of entry as plain words", () => {
    expect(sentence(entry(), people)).toBe("changed Maya Chen's role to Admin");
    expect(sentence(entry({ action: "member.joined", actor: { type: "user", id: MAYA }, details: { role: "guest" } }), people)).toBe("joined as Guest");
    expect(sentence(entry({ action: "member.removed" }), people)).toBe("removed Maya Chen from the workspace");
    expect(sentence(entry({ action: "member.removed", resource_id: "gone" }), people)).toBe("removed a former member from the workspace");
    const project = { resource_type: "project", resource_id: "p1", resource_label: "PAY" };
    expect(sentence(entry({ ...project, action: "project.access_granted", details: { user_id: MAYA, role: "viewer" } }), people)).toBe(
      "gave Maya Chen Viewer access to PAY",
    );
    expect(sentence(entry({ ...project, action: "project.access_removed", details: { user_id: MAYA } }), people)).toBe("removed Maya Chen's access to PAY");
    expect(sentence(entry({ ...project, action: "project.default_access_changed", details: { default_role: "none" } }), people)).toBe("made PAY private");
    expect(sentence(entry({ ...project, action: "project.default_access_changed", details: { default_role: "member" } }), people)).toBe(
      "let every member into PAY as Member",
    );
    expect(sentence(entry({ action: "board.columns_changed", resource_type: "board", resource_label: "" }), people)).toBe("changed a board's columns");
    // Actions the page doesn't know yet still read sensibly.
    expect(sentence(entry({ action: "widget.spun_up", resource_type: "widget", resource_label: "W1", details: {} }), people)).toBe("widget spun up W1");
  });
});

describe("audit log page", () => {
  it("is for admins only", async () => {
    signIn("member");
    const api = fakeApi({});
    renderAt(<AuditLogPage />, "/settings/audit-log", "/settings/audit-log");
    expect(await screen.findByText("Admins only")).toBeInTheDocument();
    expect(api.calls.some((c) => c.path.includes("/audit"))).toBe(false);
  });

  it("lists entries newest first, passes URL filters on, pages, and checks integrity", async () => {
    signIn("admin");
    const api = fakeApi({
      "GET /api/v1/organizations/current/members": MEMBERS,
      "GET /api/v1/organizations/current/audit": (call: Call) => ({
        body: call.query.get("cursor")
          ? { items: [entry({ seq: 1, action: "member.invited", resource_type: "invitation", resource_label: "new@example.com", details: { role: "member" } })], people, next_cursor: null }
          : {
              items: [
                entry({ seq: 3, action: "project.access_granted", resource_type: "project", resource_id: "p1", resource_label: "PAY", details: { user_id: MAYA, role: "viewer" }, request_id: "req-9" }),
                entry({ seq: 2 }),
              ],
              people,
              next_cursor: "c1",
            },
      }),
      "GET /api/v1/organizations/current/audit/verify": { entries: 2, intact: false, broken_at: 3, reason: "its contents were changed after it was recorded" },
    });
    renderAt(<AuditLogPage />, "/settings/audit-log", "/settings/audit-log?category=project&from=2026-11-01");

    expect(await screen.findByText("gave Maya Chen Viewer access to PAY")).toBeInTheDocument();
    expect(screen.getByText("changed Maya Chen's role to Admin")).toBeInTheDocument();
    const first = api.sent("GET", "/api/v1/organizations/current/audit")[0]!;
    expect(first.query.get("category")).toBe("project");
    expect(first.query.get("from")).toBe("2026-11-01");
    expect(first.query.has("actor")).toBe(false);

    // Details expand in place.
    await userEvent.click(screen.getAllByRole("button", { name: "Show details" })[0]!);
    expect(screen.getByText("req-9")).toBeInTheDocument();
    expect(screen.getByText(`Maya Chen (${MAYA})`)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Show older entries" }));
    expect(await screen.findByText("invited new@example.com as Member")).toBeInTheDocument();
    expect(api.sent("GET", "/api/v1/organizations/current/audit").at(-1)!.query.get("cursor")).toBe("c1");

    await userEvent.click(screen.getByRole("button", { name: "Check integrity" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Entry #3 failed the check: its contents were changed after it was recorded. The 2 entries before it are intact.",
    );

    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(api.sent("GET", "/api/v1/organizations/current/audit").at(-1)!.query.has("category")).toBe(false));
  });

  it("says when the log is intact", async () => {
    signIn("owner");
    fakeApi({
      "GET /api/v1/organizations/current/members": MEMBERS,
      "GET /api/v1/organizations/current/audit": { items: [], people: {}, next_cursor: null },
      "GET /api/v1/organizations/current/audit/verify": { entries: 0, intact: true, broken_at: null, reason: null },
    });
    renderAt(<AuditLogPage />, "/settings/audit-log", "/settings/audit-log");
    expect(await screen.findByText("Nothing recorded yet")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Check integrity" }));
    expect(await screen.findByText("All 0 entries are intact.")).toBeInTheDocument();
  });
});
