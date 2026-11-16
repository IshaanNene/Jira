import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import AdminConsolePage from "@/modules/admin/pages/AdminConsolePage";

import { fakeApi, ORG_ID, project, renderAt, signIn, USER_ID } from "./harness";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const GRACE = "00000000-0000-7000-8000-000000000002";
const LEFT = "00000000-0000-7000-8000-000000000003";
const member = (user_id: string, display_name: string, org_role: string, status = "active") => ({
  user_id, display_name, email: `${display_name.split(" ")[0]!.toLowerCase()}@example.com`, org_role, status, joined_at: "2026-01-01T00:00:00Z",
});

const TOKEN = {
  id: "t1", owner_id: GRACE, name: "Deploy script", hint: "mrd_ab12", created_at: "2026-10-01T00:00:00Z",
  expires_at: "2027-01-01T00:00:00Z", last_used_at: null,
};

const routes = (extra: Record<string, unknown> = {}) => ({
  "GET /api/v1/organizations/current/members": {
    items: [member(USER_ID, "Ada Lovelace", "admin"), member(GRACE, "Grace Hopper", "member"), member(LEFT, "Old Timer", "member", "deactivated")],
    next_cursor: null,
  },
  "GET /api/v1/organizations/current": { id: ORG_ID, name: "Acme", slug: "acme", role: "admin", require_two_step: false, version: 1, created_at: "2026-01-01T00:00:00Z" },
  "GET /api/v1/projects": { items: [project()] },
  "GET /api/v1/webhooks": { items: [] },
  "POST /api/v1/auth/admin/two-step": { items: [{ user_id: USER_ID, enabled: true }, { user_id: GRACE, enabled: false }] },
  "GET /api/v1/auth/admin/api-tokens": { items: [TOKEN] },
  ...extra,
});

const show = () => renderAt(<AdminConsolePage />, "/settings/admin", "/settings/admin");

describe("admin console", () => {
  it("is for admins only", async () => {
    signIn("member");
    const api = fakeApi(routes());
    show();
    expect(await screen.findByText("Admins only")).toBeInTheDocument();
    expect(api.sent("GET", "/api/v1/auth/admin/api-tokens")).toHaveLength(0);
  });

  it("summarises the workspace and lists members without two-step verification", async () => {
    signIn("admin");
    const api = fakeApi(routes());
    show();
    const missing = await screen.findByRole("list", { name: "Members without two-step verification" });
    expect(within(missing).getByText("Grace Hopper")).toBeInTheDocument();
    expect(within(missing).queryByText("Ada Lovelace")).not.toBeInTheDocument();
    expect(screen.getByText("1 of 2 on")).toBeInTheDocument();
    expect(screen.getByText("1 admin · 1 member")).toBeInTheDocument();
    // Only active members are asked about.
    expect(api.sent("POST", "/api/v1/auth/admin/two-step")[0]!.body).toEqual({ user_ids: [USER_ID, GRACE].sort() });
  });

  it("revokes a workspace API token after confirmation", async () => {
    signIn("admin");
    let tokens = [TOKEN];
    const api = fakeApi(routes({
      "GET /api/v1/auth/admin/api-tokens": () => ({ body: { items: tokens } }),
      "DELETE /api/v1/auth/admin/api-tokens/t1": () => {
        tokens = [];
        return { status: 204 };
      },
    }));
    show();
    const list = await screen.findByRole("list", { name: "API tokens" });
    expect(within(list).getByText(/Grace Hopper · expires/)).toBeInTheDocument();
    await userEvent.click(within(list).getByRole("button", { name: "Revoke Deploy script" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Revoke token" }));
    expect(await screen.findByText("No API tokens")).toBeInTheDocument();
    expect(api.sent("DELETE", "/api/v1/auth/admin/api-tokens/t1")).toHaveLength(1);
  });

  it("says when everyone has two-step verification", async () => {
    signIn("owner");
    fakeApi(routes({ "POST /api/v1/auth/admin/two-step": { items: [{ user_id: USER_ID, enabled: true }, { user_id: GRACE, enabled: true }] } }));
    show();
    expect(await screen.findByText("Everyone is covered")).toBeInTheDocument();
  });
});
