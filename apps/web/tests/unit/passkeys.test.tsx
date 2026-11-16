import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import LoginPage from "@/modules/auth/pages/LoginPage";
import SecurityPage from "@/modules/users/pages/SecurityPage";
import { useSession } from "@/state/session";

import { fakeApi, renderAt, signIn, USER_ID, type Call } from "./harness";

const bytes = (text: string) => new TextEncoder().encode(text).buffer;
const b64 = (text: string) => btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

const credentials = { create: vi.fn(), get: vi.fn() };

beforeEach(() => {
  vi.stubGlobal("PublicKeyCredential", class {});
  Object.defineProperty(navigator, "credentials", { value: credentials, configurable: true });
  credentials.create.mockReset();
  credentials.get.mockReset();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  useSession.setState({ status: "anonymous", accessToken: null, account: null, organization: null });
});

const TOKEN = {
  access_token: "new-token", token_type: "Bearer", expires_in: 600, organization: null,
  account: { id: USER_ID, email: "ada@example.com", display_name: "Ada Lovelace", email_verified: true, created_at: "2026-01-01T00:00:00Z" },
};

describe("signing in with a passkey", () => {
  it("asks the browser for a passkey and signs in with its answer", async () => {
    const api = fakeApi({
      "POST /api/v1/auth/passkeys/sign-in/options": { public_key: { rpId: "localhost", challenge: b64("challenge"), allowCredentials: [], userVerification: "required" } },
      "POST /api/v1/auth/passkeys/sign-in": TOKEN,
    });
    credentials.get.mockResolvedValue({
      rawId: bytes("cred"),
      response: { clientDataJSON: bytes("client"), authenticatorData: bytes("data"), signature: bytes("sig"), userHandle: bytes("user") },
    });
    renderAt(<LoginPage />, "/login", "/login");
    await userEvent.click(screen.getByRole("button", { name: /sign in with a passkey/i }));
    expect(await screen.findByText("other page")).toBeInTheDocument();

    const request = credentials.get.mock.calls[0]![0] as { publicKey: { challenge: ArrayBuffer; userVerification: string } };
    expect(new TextDecoder().decode(request.publicKey.challenge)).toBe("challenge");
    expect(request.publicKey.userVerification).toBe("required");
    expect(api.sent("POST", "/api/v1/auth/passkeys/sign-in")[0]!.body).toEqual({
      credential_id: b64("cred"), client_data_json: b64("client"), authenticator_data: b64("data"), signature: b64("sig"), user_handle: b64("user"),
    });
    expect(useSession.getState().accessToken).toBe("new-token");
  });

  it("stays quiet when the prompt is cancelled, and shows the server's refusal", async () => {
    fakeApi({
      "POST /api/v1/auth/passkeys/sign-in/options": { public_key: { rpId: "localhost", challenge: b64("c") } },
      "POST /api/v1/auth/passkeys/sign-in": () => ({ status: 401, body: { code: "unauthenticated", detail: "That passkey didn't work." } }),
    });
    credentials.get.mockRejectedValueOnce(new DOMException("cancelled", "NotAllowedError"));
    renderAt(<LoginPage />, "/login", "/login");
    const button = screen.getByRole("button", { name: /sign in with a passkey/i });
    await userEvent.click(button);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    credentials.get.mockResolvedValueOnce({
      rawId: bytes("cred"),
      response: { clientDataJSON: bytes("client"), authenticatorData: bytes("data"), signature: bytes("sig"), userHandle: bytes("user") },
    });
    await userEvent.click(button);
    expect(await screen.findByText("That passkey didn't work.")).toBeInTheDocument();
  });
});

const PASSKEY = { id: "p1", name: "Mac", synced: true, created_at: "2026-11-01T00:00:00Z", last_used_at: null };

describe("passkey settings", () => {
  const routes = (passkeys: unknown[], extra: Record<string, unknown> = {}) => ({
    "GET /api/v1/auth/passkeys": () => ({ body: { items: passkeys } }),
    "GET /api/v1/auth/sessions": { items: [] },
    "GET /api/v1/auth/mfa": { totp_enabled: false, enabled_at: null, recovery_codes_remaining: 0 },
    ...extra,
  });

  it("adds a passkey: password, browser prompt, saved with its name", async () => {
    signIn("member");
    const passkeys: unknown[] = [];
    const api = fakeApi(routes(passkeys, {
      "POST /api/v1/auth/passkeys/options": {
        public_key: {
          rp: { id: "localhost", name: "Meridian" }, user: { id: b64("user-id"), name: "ada@example.com", displayName: "Ada" },
          challenge: b64("challenge"), pubKeyCredParams: [{ type: "public-key", alg: -7 }], excludeCredentials: [{ type: "public-key", id: b64("old") }],
          authenticatorSelection: { residentKey: "required", userVerification: "required" }, attestation: "none",
        },
      },
      "POST /api/v1/auth/passkeys": (call: Call) => {
        const created = { ...PASSKEY, name: (call.body as { name: string }).name };
        passkeys.push(created);
        return { status: 201, body: created };
      },
    }));
    credentials.create.mockResolvedValue({ rawId: bytes("cred"), response: { clientDataJSON: bytes("client"), attestationObject: bytes("att") } });
    renderAt(<SecurityPage />, "/settings/security", "/settings/security");

    expect(await screen.findByText("No passkeys yet")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Add a passkey" }));
    const dialog = await screen.findByRole("dialog", { name: "Add a passkey" });
    const name = within(dialog).getByLabelText("Name");
    await userEvent.clear(name);
    await userEvent.type(name, "Work laptop");
    await userEvent.type(within(dialog).getByLabelText("Password"), "right-password");
    await userEvent.click(within(dialog).getByRole("button", { name: "Continue" }));

    const list = await screen.findByRole("list", { name: "Passkeys" });
    expect(within(list).getByText("Work laptop")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/passkeys/options")[0]!.body).toEqual({ password: "right-password" });
    expect(api.sent("POST", "/api/v1/auth/passkeys")[0]!.body).toEqual({ name: "Work laptop", client_data_json: b64("client"), attestation_object: b64("att") });
    const options = credentials.create.mock.calls[0]![0] as { publicKey: { user: { id: ArrayBuffer }; excludeCredentials: { id: ArrayBuffer }[] } };
    expect(new TextDecoder().decode(options.publicKey.user.id)).toBe("user-id");
    expect(new TextDecoder().decode(options.publicKey.excludeCredentials[0]!.id)).toBe("old");
  });

  it("removes a passkey after the password", async () => {
    signIn("member");
    let passkeys: unknown[] = [PASSKEY];
    const api = fakeApi(routes([], {
      "GET /api/v1/auth/passkeys": () => ({ body: { items: passkeys } }),
      "POST /api/v1/auth/passkeys/p1/remove": () => {
        passkeys = [];
        return { status: 204 };
      },
    }));
    renderAt(<SecurityPage />, "/settings/security", "/settings/security");
    const list = await screen.findByRole("list", { name: "Passkeys" });
    expect(within(list).getByText("Synced")).toBeInTheDocument();
    await userEvent.click(within(list).getByRole("button", { name: "Remove Mac" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Password"), "right-password");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove passkey" }));
    expect(await screen.findByText("No passkeys yet")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/passkeys/p1/remove")[0]!.body).toEqual({ password: "right-password" });
  });
});
