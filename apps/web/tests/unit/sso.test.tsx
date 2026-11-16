import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import SingleSignOnPage from "@/modules/admin/pages/SingleSignOnPage";
import LoginPage from "@/modules/auth/pages/LoginPage";
import SsoSignInPage from "@/modules/auth/pages/SsoSignInPage";
import SecurityPage from "@/modules/users/pages/SecurityPage";
import type { SsoConnection } from "@/services/api";
import { useSession } from "@/state/session";

import { fakeApi, ORG_ID, renderAt, signIn, type Call } from "./harness";

const goTo = vi.fn();
vi.mock("@/utils/browser", () => ({ goTo: (url: string) => goTo(url) }));

beforeEach(() => goTo.mockReset());
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  useSession.setState({ status: "anonymous", accessToken: null, account: null, organization: null });
});

const IDP = "https://idp.example.com/authorize?state=abc";

describe("signing in with SSO", () => {
  it("finds the workspace's provider by its sign-in name and goes there", async () => {
    const api = fakeApi({
      "POST /api/v1/auth/sso/start": (call: Call) =>
        (call.body as { sign_in_name: string }).sign_in_name === "acme"
          ? { body: { authorization_url: IDP } }
          : { status: 422, body: { code: "validation_failed", detail: "x", errors: [{ field: "sign_in_name", code: "unknown", message: "No workspace uses single sign-on under that name." }] } },
    });
    renderAt(<SsoSignInPage />, "/sso", "/sso");
    const name = screen.getByLabelText("Workspace sign-in name");
    await userEvent.type(name, "nope");
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByText("No workspace uses single sign-on under that name.")).toBeInTheDocument();
    await userEvent.clear(name);
    await userEvent.type(name, "acme");
    await userEvent.click(screen.getByRole("button", { name: "Continue" }));
    await vi.waitFor(() => expect(goTo).toHaveBeenCalledWith(IDP));
    expect(api.sent("POST", "/api/v1/auth/sso/start")).toHaveLength(2);
  });

  it("explains why the provider's sign-in did not get the person in", () => {
    fakeApi({});
    renderAt(<LoginPage />, "/login", "/login?sso_error=not_linked");
    expect(screen.getByText(/isn't linked to this workspace's single sign-on yet/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Sign in with SSO" })).toHaveAttribute("href", "/sso");
  });
});

const CONNECTION: SsoConnection = {
  protocol: "oidc", saml: null,
  sign_in_name: "acme", issuer: "https://acme.okta.com", client_id: "abc", enabled: true, require_sso: false,
  redirect_uri: "https://meridian.example.com/api/v1/auth/sso/callback", linked_members: 3, updated_at: "2026-11-16T00:00:00Z",
};

describe("single sign-on settings", () => {
  it("is for admins only", async () => {
    signIn("member");
    fakeApi({});
    renderAt(<SingleSignOnPage />, "/settings/sso", "/settings/sso");
    expect(await screen.findByText("Admins only")).toBeInTheDocument();
  });

  it("connects a provider, then shows the redirect URI to register", async () => {
    signIn("owner");
    const api = fakeApi({
      "GET /api/v1/auth/sso/connection": () => ({ status: 404, body: { code: "not_found", detail: "SSO connection not found" } }),
      "PUT /api/v1/auth/sso/connection": (call: Call) => ({ body: { ...CONNECTION, ...(call.body as object), linked_members: 0 } }),
    });
    renderAt(<SingleSignOnPage />, "/settings/sso", "/settings/sso");
    await userEvent.type(await screen.findByLabelText("Sign-in name"), "acme");
    await userEvent.type(screen.getByLabelText("Issuer URL"), "https://acme.okta.com");
    await userEvent.type(screen.getByLabelText("Client ID"), "abc");
    await userEvent.type(screen.getByLabelText("Client secret"), "s3cret");
    await userEvent.click(screen.getByLabelText(/Require single sign-on/));
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByLabelText("Redirect URI")).toHaveTextContent(CONNECTION.redirect_uri);
    expect(api.sent("PUT", "/api/v1/auth/sso/connection")[0]!.body).toEqual({
      protocol: "oidc", sign_in_name: "acme", issuer: "https://acme.okta.com", client_id: "abc", client_secret: "s3cret", enabled: true, require_sso: true,
    });
  });

  it("keeps the stored secret when the field is left blank", async () => {
    signIn("admin");
    const api = fakeApi({
      "GET /api/v1/auth/sso/connection": CONNECTION,
      "PUT /api/v1/auth/sso/connection": (call: Call) => ({ body: { ...CONNECTION, ...(call.body as object), updated_at: "2026-11-17T00:00:00Z" } }),
    });
    renderAt(<SingleSignOnPage />, "/settings/sso", "/settings/sso");
    expect(await screen.findByText(/3 members have linked/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await vi.waitFor(() => expect(api.sent("PUT", "/api/v1/auth/sso/connection")).toHaveLength(1));
    expect((api.sent("PUT", "/api/v1/auth/sso/connection")[0]!.body as { client_secret: unknown }).client_secret).toBeNull();
  });
});

const SP = "https://meridian.example.com/api/v1/auth/sso/saml/" + ORG_ID;
const SAML: SsoConnection = {
  ...CONNECTION, protocol: "saml", issuer: "http://www.okta.com/exk1", client_id: null,
  saml: { entity_id: SP, acs_url: "https://meridian.example.com/api/v1/auth/sso/saml/acs", metadata_url: SP },
};

describe("single sign-on with SAML", () => {
  it("connects with pasted metadata, then shows what to enter at the provider", async () => {
    signIn("owner");
    const api = fakeApi({
      "GET /api/v1/auth/sso/connection": () => ({ status: 404, body: { code: "not_found", detail: "SSO connection not found" } }),
      "PUT /api/v1/auth/sso/connection": (call: Call) =>
        (call.body as { saml_metadata: string }).saml_metadata.includes("KeyDescriptor")
          ? { body: { ...SAML, linked_members: 0 } }
          : { status: 422, body: { code: "validation_failed", detail: "x", errors: [{ field: "saml_metadata", code: "invalid", message: "The metadata has no signing certificate." }] } },
    });
    renderAt(<SingleSignOnPage />, "/settings/sso", "/settings/sso");
    await userEvent.click(await screen.findByRole("radio", { name: "SAML 2.0" }));
    expect(screen.queryByLabelText("Client ID")).not.toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Sign-in name"), "acme");
    const metadata = screen.getByLabelText("Identity provider metadata");
    await userEvent.click(metadata);
    await userEvent.paste("<md:EntityDescriptor/>");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText("The metadata has no signing certificate.")).toBeInTheDocument();
    await userEvent.clear(metadata);
    await userEvent.paste("<md:EntityDescriptor><md:KeyDescriptor/></md:EntityDescriptor>");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByLabelText("Entity ID (audience)")).toHaveTextContent(SP);
    expect(screen.getByLabelText("ACS URL")).toHaveTextContent("/api/v1/auth/sso/saml/acs");
    expect(screen.getByLabelText("Metadata URL")).toHaveTextContent(SP);
    expect(screen.queryByLabelText("Redirect URI")).not.toBeInTheDocument();
    expect(api.sent("PUT", "/api/v1/auth/sso/connection")[1]!.body).toEqual({
      protocol: "saml", sign_in_name: "acme", enabled: true, require_sso: false,
      saml_metadata: "<md:EntityDescriptor><md:KeyDescriptor/></md:EntityDescriptor>",
    });
  });

  it("keeps the stored metadata when the field is left blank, and warns before switching protocol", async () => {
    signIn("admin");
    const api = fakeApi({
      "GET /api/v1/auth/sso/connection": SAML,
      "PUT /api/v1/auth/sso/connection": (call: Call) => ({ body: { ...SAML, ...(call.body as object), updated_at: "2026-11-17T00:00:00Z" } }),
    });
    renderAt(<SingleSignOnPage />, "/settings/sso", "/settings/sso");
    expect(await screen.findByRole("radio", { name: "SAML 2.0" })).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await vi.waitFor(() => expect(api.sent("PUT", "/api/v1/auth/sso/connection")).toHaveLength(1));
    expect((api.sent("PUT", "/api/v1/auth/sso/connection")[0]!.body as { saml_metadata: unknown }).saml_metadata).toBeNull();
    await userEvent.click(screen.getByRole("radio", { name: "OpenID Connect" }));
    expect(screen.getByText(/Switching protocol deletes every member's link/)).toBeInTheDocument();
  });
});

describe("linking single sign-on", () => {
  const routes = (extra: Record<string, unknown>) => ({
    "GET /api/v1/auth/passkeys": { items: [] },
    "GET /api/v1/auth/sessions": { items: [] },
    "GET /api/v1/auth/mfa": { totp_enabled: false, enabled_at: null, recovery_codes_remaining: 0 },
    "GET /api/v1/auth/api-tokens": { items: [] },
    ...extra,
  });

  it("offers to link the current workspace and sends the browser to its provider", async () => {
    signIn("member");
    const api = fakeApi(routes({
      "GET /api/v1/auth/sso/links": { items: [] },
      "GET /api/v1/auth/sso/workspace": { available: true, sign_in_name: "acme", require_sso: true, linked: false },
      "POST /api/v1/auth/sso/link": { authorization_url: IDP },
    }));
    renderAt(<SecurityPage />, "/settings/security", "/settings/security");
    await userEvent.click(await screen.findByRole("button", { name: /link this workspace/i }));
    await vi.waitFor(() => expect(goTo).toHaveBeenCalledWith(IDP));
    expect(api.sent("POST", "/api/v1/auth/sso/link")[0]!.body).toEqual({ organization_id: ORG_ID });
  });

  it("lists linked workspaces and unlinks one", async () => {
    signIn("member");
    let items = [{ organization_id: ORG_ID, sign_in_name: "acme", email: "ada@corp.example", created_at: "2026-11-01T00:00:00Z", last_used_at: null }];
    const api = fakeApi(routes({
      "GET /api/v1/auth/sso/links": () => ({ body: { items } }),
      "GET /api/v1/auth/sso/workspace": { available: true, sign_in_name: "acme", require_sso: false, linked: true },
      [`DELETE /api/v1/auth/sso/links/${ORG_ID}`]: () => {
        items = [];
        return { status: 204 };
      },
    }));
    renderAt(<SecurityPage />, "/settings/security", "/settings/security");
    const list = await screen.findByRole("list", { name: "Linked workspaces" });
    expect(within(list).getByText(/ada@corp.example/)).toBeInTheDocument();
    await userEvent.click(within(list).getByRole("button", { name: "Unlink acme" }));
    await vi.waitFor(() => expect(api.sent("DELETE", `/api/v1/auth/sso/links/${ORG_ID}`)).toHaveLength(1));
  });
});
