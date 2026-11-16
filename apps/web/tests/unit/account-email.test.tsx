import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ChangeEmailDialog, VerifyEmailBanner } from "@/modules/auth";
import ConfirmEmailChangePage from "@/modules/auth/pages/ConfirmEmailChangePage";
import ForgotPasswordPage from "@/modules/auth/pages/ForgotPasswordPage";
import ResetPasswordPage from "@/modules/auth/pages/ResetPasswordPage";
import VerifyEmailPage from "@/modules/auth/pages/VerifyEmailPage";
import AcceptInvitationPage from "@/modules/organizations/pages/AcceptInvitationPage";
import { useSession } from "@/state/session";

import { fakeApi, renderAt, signIn, type Call } from "./harness";

afterEach(() => {
  vi.unstubAllGlobals();
  useSession.setState({ status: "anonymous", accessToken: null, account: null, organization: null });
});

const INVALID = { code: "validation_failed", detail: "invalid", errors: [{ field: "token", code: "invalid", message: "This link is invalid, was already used or has expired. Ask for a new one." }] };

function unverified() {
  signIn();
  const account = useSession.getState().account!;
  useSession.setState({ account: { ...account, email_verified: false } });
}

describe("forgotten password", () => {
  it("asks for the address and says the same thing whether or not it has an account", async () => {
    const api = fakeApi({ "POST /api/v1/auth/password/forgot": () => ({ status: 202 }) });
    renderAt(<ForgotPasswordPage />, "/forgot-password", "/forgot-password");
    await userEvent.click(screen.getByRole("button", { name: "Send reset link" }));
    expect(await screen.findByText("Enter your email address.")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Email"), "ada@example.com");
    await userEvent.click(screen.getByRole("button", { name: "Send reset link" }));
    expect(await screen.findByRole("heading", { name: "Check your email" })).toBeInTheDocument();
    expect(screen.getByText(/has an account, we've sent it a link/)).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/password/forgot")[0]!.body).toEqual({ email: "ada@example.com" });
  });

  it("sets a new password with the link, and explains a spent link", async () => {
    let spent = false;
    const api = fakeApi({
      "POST /api/v1/auth/password/reset": () => (spent ? { status: 422, body: INVALID } : ((spent = true), { status: 204 })),
    });
    renderAt(<ResetPasswordPage />, "/reset-password", "/reset-password?token=tok-1");
    await userEvent.type(screen.getByLabelText("New password"), "a-new-password-1");
    await userEvent.type(screen.getByLabelText("Confirm new password"), "a-new-password-2");
    await userEvent.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByText("The passwords don't match.")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/password/reset")).toHaveLength(0);

    await userEvent.clear(screen.getByLabelText("Confirm new password"));
    await userEvent.type(screen.getByLabelText("Confirm new password"), "a-new-password-1");
    await userEvent.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByRole("heading", { name: "Password changed" })).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/password/reset")[0]!.body).toEqual({ token: "tok-1", password: "a-new-password-1" });
    expect(screen.getByRole("link", { name: "Sign in" })).toHaveAttribute("href", "/login");
  });

  it("offers a new link when the reset link is spent", async () => {
    fakeApi({ "POST /api/v1/auth/password/reset": () => ({ status: 422, body: INVALID }) });
    renderAt(<ResetPasswordPage />, "/reset-password", "/reset-password?token=old");
    await userEvent.type(screen.getByLabelText("New password"), "a-new-password-1");
    await userEvent.type(screen.getByLabelText("Confirm new password"), "a-new-password-1");
    await userEvent.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByRole("heading", { name: "This link doesn't work" })).toBeInTheDocument();
    expect(screen.getByText(/already used or has expired/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Get a new link" })).toHaveAttribute("href", "/forgot-password");
  });
});

describe("email verification", () => {
  it("verifies once from the link", async () => {
    const api = fakeApi({ "POST /api/v1/auth/email/verify": () => ({ status: 204 }) });
    renderAt(<VerifyEmailPage />, "/verify-email", "/verify-email?token=tok-v");
    expect(await screen.findByRole("heading", { name: "Email verified" })).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/email/verify")).toHaveLength(1);
    expect(api.sent("POST", "/api/v1/auth/email/verify")[0]!.body).toEqual({ token: "tok-v" });
  });

  it("lets a signed-in person ask for a new link when theirs is spent", async () => {
    unverified();
    const api = fakeApi({
      "POST /api/v1/auth/email/verify": () => ({ status: 422, body: INVALID }),
      "POST /api/v1/auth/email/verification": () => ({ status: 202 }),
    });
    renderAt(<VerifyEmailPage />, "/verify-email", "/verify-email?token=old");
    expect(await screen.findByText(/already used or has expired/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Send a new link" }));
    expect(await screen.findByRole("button", { name: "Link sent" })).toBeDisabled();
    expect(api.sent("POST", "/api/v1/auth/email/verification")).toHaveLength(1);
  });

  it("reminds an unverified person in the app and resends, saying when it is too soon", async () => {
    unverified();
    let calls = 0;
    fakeApi({
      "POST /api/v1/auth/email/verification": () => (++calls === 1 ? { status: 202 } : { status: 429, body: { code: "rate_limited", detail: "slow down" } }),
    });
    renderAt(<VerifyEmailBanner />, "/", "/");
    expect(screen.getByText(/Verify your email address/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Send a new link" }));
    expect((await screen.findAllByText("Link sent")).length).toBeGreaterThan(0);
    await userEvent.click(screen.getByRole("button", { name: "Send another link" }));
    expect(await screen.findByText("One was sent a moment ago. Try again in a minute.")).toBeInTheDocument();

    useSession.setState({ account: { ...useSession.getState().account!, email_verified: true } });
    await waitFor(() => expect(screen.queryByText(/Verify your email address/)).toBeNull());
  });

  it("explains on an invitation that the address must be verified first", async () => {
    unverified();
    const api = fakeApi({
      "GET /api/v1/invitations/inv-token": {
        organization_name: "Acme", email: "ada@example.com", org_role: "member", invited_by_name: "Olivia", state: "pending", expires_at: "2026-12-01T00:00:00Z",
      },
      "POST /api/v1/invitations/inv-token/accept": () => ({
        status: 403, body: { code: "email_unverified", detail: "Verify your email address first: use the link we emailed you, then try again." },
      }),
      // Still not verified after a refresh.
      "POST /api/v1/auth/token/refresh": () => ({
        body: { access_token: "t2", token_type: "Bearer", expires_in: 600, organization: null, account: { ...useSession.getState().account!, email_verified: false } },
      }),
    });
    renderAt(<AcceptInvitationPage />, "/invite/:token", "/invite/inv-token");
    expect(await screen.findByText(/Verify ada@example.com first/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Accept invitation" }));
    await waitFor(() => expect(api.calls.some((c: Call) => c.path === "/api/v1/auth/token/refresh")).toBe(true));
    expect(await screen.findByText("Verify your email address first: use the link we emailed you, then try again.")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/invitations/inv-token/accept")).toHaveLength(2);
  });
});

describe("changing the sign-in address", () => {
  const WRONG = { code: "validation_failed", detail: "invalid", errors: [{ field: "password", code: "incorrect", message: "The password is not right." }] };

  it("needs the password and says a link went to the new address", async () => {
    signIn();
    let attempts = 0;
    const api = fakeApi({
      "POST /api/v1/auth/email/change": () => ((attempts += 1), attempts === 1 ? { status: 422, body: WRONG } : { status: 202 }),
    });
    renderAt(<ChangeEmailDialog current="ada@example.com" />, "/settings/profile", "/settings/profile");
    await userEvent.click(screen.getByRole("button", { name: "Change email" }));
    await userEvent.type(screen.getByLabelText("New email address"), "ADA@example.com");
    await userEvent.type(screen.getByLabelText("Current password"), "nope");
    await userEvent.click(screen.getByRole("button", { name: "Send link" }));
    expect(await screen.findByText("This is already your address.")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/email/change")).toHaveLength(0);

    await userEvent.clear(screen.getByLabelText("New email address"));
    await userEvent.type(screen.getByLabelText("New email address"), "Ada@New.example");
    await userEvent.click(screen.getByRole("button", { name: "Send link" }));
    expect(await screen.findByText("The password is not right.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Send link" }));
    expect(await screen.findByRole("heading", { name: "Check your inbox" })).toBeInTheDocument();
    expect(screen.getByText("ada@new.example")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/email/change")[1]!.body).toEqual({ new_email: "ada@new.example", password: "nope" });
  });

  it("confirms with the link, and explains a spent one", async () => {
    let spent = false;
    const api = fakeApi({
      "POST /api/v1/auth/email/change/confirm": () => (spent ? { status: 422, body: INVALID } : ((spent = true), { status: 204 })),
    });
    const first = renderAt(<ConfirmEmailChangePage />, "/confirm-email", "/confirm-email?token=tok-9");
    expect(await screen.findByRole("heading", { name: "Email changed" })).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/auth/email/change/confirm")[0]!.body).toEqual({ token: "tok-9" });
    expect(screen.getByRole("link", { name: "Sign in" })).toHaveAttribute("href", "/login");
    first.unmount();

    renderAt(<ConfirmEmailChangePage />, "/confirm-email", "/confirm-email?token=tok-9");
    expect(await screen.findByRole("heading", { name: "We couldn't change your email" })).toBeInTheDocument();
    expect(screen.getByText(/already used or has expired/)).toBeInTheDocument();
  });
});
