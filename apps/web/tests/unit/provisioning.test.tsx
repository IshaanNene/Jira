import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import ProvisioningPage from "@/modules/admin/pages/ProvisioningPage";

import { fakeApi, renderAt, signIn, USER_ID, type Call } from "./harness";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const BASE = "/api/v1/organizations/current/provisioning";
const BASE_URL = "https://meridian.example.com/scim/v2";
const TOKEN = {
  id: "t1", name: "Okta", prefix: "mscim_AbCd", created_by: USER_ID, created_by_name: "Ada Lovelace",
  created_at: "2026-11-01T00:00:00Z", last_used_at: null,
};
const person = (id: string, name: string, state: string, active = true) => ({
  id, user_name: `${name.toLowerCase()}@example.com`, email: `${name.toLowerCase()}@example.com`, display_name: name,
  active, state, user_id: state === "member" ? USER_ID : null, group_role: name === "Linus" ? "admin" : null, updated_at: "2026-11-15T00:00:00Z",
});
const GROUP = { id: "g1", display_name: "Engineering", members: 12, org_role: null, updated_at: "2026-11-15T00:00:00Z" };

const routes = (extra: Record<string, unknown> = {}) => ({
  [`GET ${BASE}/tokens`]: { items: [TOKEN], base_url: BASE_URL },
  [`GET ${BASE}/users`]: {
    items: [person("1", "Linus", "member"), person("2", "Carol", "invited"), person("3", "Dave", "deactivated", false)],
    total: 3,
  },
  [`GET ${BASE}/groups`]: { items: [GROUP] },
  ...extra,
});

const show = () => renderAt(<ProvisioningPage />, "/settings/provisioning", "/settings/provisioning");

describe("provisioning settings", () => {
  it("is for admins only", async () => {
    signIn("member");
    const api = fakeApi(routes());
    show();
    expect(await screen.findByText("Admins only")).toBeInTheDocument();
    expect(api.sent("GET", `${BASE}/tokens`)).toHaveLength(0);
  });

  it("shows the base URL, the tokens and where each provisioned person stands", async () => {
    signIn("admin");
    fakeApi(routes());
    show();
    expect(await screen.findByLabelText("SCIM base URL")).toHaveTextContent(BASE_URL);
    const tokens = screen.getByRole("list", { name: "Provisioning tokens" });
    expect(within(tokens).getByText("Okta")).toBeInTheDocument();
    expect(within(tokens).getByText("mscim_AbCd…")).toBeInTheDocument();
    const people = await screen.findByRole("list", { name: "Provisioned people" });
    expect(within(people).getByText("Linus").closest("li")).toHaveTextContent("Member");
    expect(within(people).getByText("Linus").closest("li")).toHaveTextContent("Admin through groups");
    expect(within(people).getByText("Carol").closest("li")).toHaveTextContent("Invited");
    expect(within(people).getByText("Dave").closest("li")).toHaveTextContent("Deactivated");
  });

  it("creates a token and shows its secret once", async () => {
    signIn("admin");
    const api = fakeApi(
      routes({
        [`POST ${BASE}/tokens`]: (call: Call) =>
          (call.body as { name: string }).name === "Entra"
            ? { status: 201, body: { token: { ...TOKEN, id: "t2", name: "Entra" }, secret: "mscim_secret-value", base_url: BASE_URL } }
            : { status: 409, body: { code: "conflict", detail: "A workspace can have 5 provisioning tokens. Revoke one first.", errors: [] } },
      }),
    );
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Create token" }));
    await userEvent.type(screen.getByLabelText("Name"), "Entra");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByLabelText("New provisioning token")).toHaveTextContent("mscim_secret-value");
    expect(api.sent("POST", `${BASE}/tokens`)[0]!.body).toEqual({ name: "Entra" });
  });

  it("explains when a token can't be created", async () => {
    signIn("admin");
    fakeApi(routes({ [`POST ${BASE}/tokens`]: () => ({ status: 409, body: { code: "conflict", detail: "A workspace can have 5 provisioning tokens. Revoke one first.", errors: [] } }) }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Create token" }));
    await userEvent.type(screen.getByLabelText("Name"), "Sixth");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Revoke one first.");
  });

  it("revokes a token after confirming", async () => {
    signIn("admin");
    const api = fakeApi(routes({ [`DELETE ${BASE}/tokens/t1`]: () => ({ status: 204, body: null }) }));
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Revoke Okta" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Revoke" }));
    await vi.waitFor(() => expect(api.sent("DELETE", `${BASE}/tokens/t1`)).toHaveLength(1));
  });

  it("gives a provider group a role", async () => {
    signIn("admin");
    const api = fakeApi(
      routes({ [`PUT ${BASE}/groups/g1/role`]: (call: Call) => ({ body: { ...GROUP, ...(call.body as object) } }) }),
    );
    show();
    const groups = await screen.findByRole("list", { name: "Provisioned groups" });
    expect(within(groups).getByText("12 members")).toBeInTheDocument();
    await userEvent.selectOptions(within(groups).getByLabelText("Role for Engineering"), "admin");
    await vi.waitFor(() => expect(api.sent("PUT", `${BASE}/groups/g1/role`)).toHaveLength(1));
    expect(api.sent("PUT", `${BASE}/groups/g1/role`)[0]!.body).toEqual({ org_role: "admin" });
    await userEvent.selectOptions(within(groups).getByLabelText("Role for Engineering"), "");
    await vi.waitFor(() => expect(api.sent("PUT", `${BASE}/groups/g1/role`)).toHaveLength(2));
    expect(api.sent("PUT", `${BASE}/groups/g1/role`)[1]!.body).toEqual({ org_role: null });
  });

  it("says where groups come from before the provider sends any", async () => {
    signIn("owner");
    fakeApi(routes({ [`GET ${BASE}/groups`]: { items: [] } }));
    show();
    expect(await screen.findByText("No groups yet")).toBeInTheDocument();
  });
});
