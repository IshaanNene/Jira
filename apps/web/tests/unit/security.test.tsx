import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SecurityPage } from "@/modules/users";

import { fakeApi, renderAt, signIn, type Call } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const SESSIONS = {
  items: [{ id: "s1", current: true, created_at: "2026-11-01T00:00:00Z", last_seen_at: new Date().toISOString(), expires_at: "2026-12-01T00:00:00Z", ip: "10.0.0.1", user_agent: "Mozilla/5.0 (Macintosh) Chrome/131" }],
};
const CODES = ["AAAA-BBBB-CCCC", "DDDD-EEEE-FFFF"];
const invalid = (field: string, message: string) => ({
  status: 422,
  body: { code: "validation_failed", detail: "validation failed", errors: [{ field, code: "incorrect", message }] },
});

describe("security settings", () => {
  it("sets up two-step verification: password, scan, code, recovery codes", async () => {
    signIn("member");
    let enabled = false;
    const api = fakeApi({
      "GET /api/v1/auth/sessions": SESSIONS,
      "GET /api/v1/auth/mfa": () => ({ body: { totp_enabled: enabled, enabled_at: enabled ? "2026-11-13T00:00:00Z" : null, recovery_codes_remaining: enabled ? 10 : 0 } }),
      "POST /api/v1/auth/mfa/totp/setup": (call: Call) =>
        (call.body as { password: string }).password === "right-password"
          ? { body: { secret: "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP", otpauth_uri: "otpauth://totp/x", qr_svg: "<svg xmlns='http://www.w3.org/2000/svg'/>" } }
          : invalid("password", "That password isn't right."),
      "POST /api/v1/auth/mfa/totp/confirm": (call: Call) => {
        if ((call.body as { code: string }).code !== "654321") return invalid("code", "That code didn't work. Check the app and try again.");
        enabled = true;
        return { body: { codes: CODES } };
      },
    });
    renderAt(<SecurityPage />, "/settings/security", "/settings/security");

    await userEvent.click(await screen.findByRole("button", { name: "Set up" }));
    let dialog = await screen.findByRole("dialog", { name: "Set up two-step verification" });
    await userEvent.type(within(dialog).getByLabelText("Password"), "wrong");
    await userEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    expect(await within(dialog).findByText("That password isn't right.")).toBeInTheDocument();
    await userEvent.clear(within(dialog).getByLabelText("Password"));
    await userEvent.type(within(dialog).getByLabelText("Password"), "right-password");
    await userEvent.click(within(dialog).getByRole("button", { name: "Continue" }));

    dialog = await screen.findByRole("dialog", { name: "Scan with your authenticator app" });
    const qr = within(dialog).getByRole("img", { name: /QR code/ });
    expect(qr.getAttribute("src")).toMatch(/^data:image\/svg\+xml/);
    expect(within(dialog).getByLabelText("Setup key")).toHaveTextContent("JBSW Y3DP EHPK 3PXP JBSW Y3DP EHPK 3PXP");

    await userEvent.type(within(dialog).getByLabelText("Code from the app"), "111111");
    await userEvent.click(within(dialog).getByRole("button", { name: "Turn on" }));
    expect(await within(dialog).findByText("That code didn't work. Check the app and try again.")).toBeInTheDocument();
    await userEvent.clear(within(dialog).getByLabelText("Code from the app"));
    await userEvent.type(within(dialog).getByLabelText("Code from the app"), "654321");
    await userEvent.click(within(dialog).getByRole("button", { name: "Turn on" }));

    dialog = await screen.findByRole("dialog", { name: "Save your recovery codes" });
    expect(within(within(dialog).getByRole("list", { name: "Recovery codes" })).getAllByRole("listitem").map((li) => li.textContent)).toEqual(CODES);
    await userEvent.click(within(dialog).getByRole("button", { name: "I've saved them" }));
    expect(await screen.findByText("10 recovery codes left")).toBeInTheDocument();
    expect(screen.getByText("On")).toBeInTheDocument();
    // Turning it on ends the other sessions, so the list is fetched again.
    await waitFor(() => expect(api.sent("GET", "/api/v1/auth/sessions").length).toBeGreaterThan(1));
  });

  it("turns it off with the password and a code, and warns when recovery codes run low", async () => {
    signIn("member");
    let enabled = true;
    const api = fakeApi({
      "GET /api/v1/auth/sessions": SESSIONS,
      "GET /api/v1/auth/mfa": () => ({ body: { totp_enabled: enabled, enabled_at: enabled ? "2026-11-01T00:00:00Z" : null, recovery_codes_remaining: enabled ? 2 : 0 } }),
      "POST /api/v1/auth/mfa/totp/disable": () => {
        enabled = false;
        return { status: 204 };
      },
    });
    renderAt(<SecurityPage />, "/settings/security", "/settings/security");
    expect(await screen.findByText("2 recovery codes left")).toHaveClass("text-warning");

    await userEvent.click(screen.getByRole("button", { name: "Turn off" }));
    const dialog = await screen.findByRole("dialog", { name: "Turn off two-step verification?" });
    await userEvent.type(within(dialog).getByLabelText("Password"), "right-password");
    await userEvent.type(within(dialog).getByLabelText("Authentication or recovery code"), "AAAA-BBBB-CCCC");
    await userEvent.click(within(dialog).getByRole("button", { name: "Turn off" }));
    expect(await screen.findByRole("button", { name: "Set up" })).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/mfa/totp/disable")[0]!.body).toEqual({ password: "right-password", code: "AAAA-BBBB-CCCC" });
  });
});
