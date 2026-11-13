import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import LoginPage from "@/modules/auth/pages/LoginPage";
import { ApiError } from "@/services/api";

const signIn = vi.fn();
const completeSignIn = vi.fn();
vi.mock("@/modules/auth/session-service", () => ({
  signIn: (...args: unknown[]) => signIn(...args),
  completeSignIn: (...args: unknown[]) => completeSignIn(...args),
}));

function renderAt(url: string) {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/people" element={<p>People page</p>} />
        <Route path="/" element={<p>Home page</p>} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("LoginPage", () => {
  // Braces matter: a function returned from beforeEach is run by vitest as a teardown hook.
  beforeEach(() => {
    signIn.mockReset();
    completeSignIn.mockReset();
  });

  it("validates before calling the API", async () => {
    renderAt("/login");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("Enter your email address.")).toBeInTheDocument();
    expect(screen.getByText("Enter your password.")).toBeInTheDocument();
    expect(screen.getByLabelText("Email")).toHaveAttribute("aria-invalid", "true");
    expect(signIn).not.toHaveBeenCalled();
  });

  it("signs in and returns to the page the person came from", async () => {
    signIn.mockResolvedValue(undefined);
    renderAt("/login?next=%2Fpeople");
    await userEvent.type(screen.getByLabelText("Email"), "ada@example.com");
    await userEvent.type(screen.getByLabelText("Password"), "a-long-enough-password");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("People page")).toBeInTheDocument();
    expect(signIn).toHaveBeenCalledWith("ada@example.com", "a-long-enough-password");
  });

  it("ignores an off-site next parameter", async () => {
    signIn.mockResolvedValue(undefined);
    renderAt("/login?next=https%3A%2F%2Fevil.example");
    await userEvent.type(screen.getByLabelText("Email"), "ada@example.com");
    await userEvent.type(screen.getByLabelText("Password"), "x");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("Home page")).toBeInTheDocument();
  });

  it("shows the server's message when credentials are wrong", async () => {
    signIn.mockRejectedValue(new ApiError(401, { code: "unauthenticated", detail: "Incorrect email or password." }));
    renderAt("/login");
    await userEvent.type(screen.getByLabelText("Email"), "ada@example.com");
    await userEvent.type(screen.getByLabelText("Password"), "wrong");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Incorrect email or password.");
  });

  describe("with two-step verification", () => {
    const challenge = { mfa_token: "challenge-1", expires_in: 300, methods: ["totp", "recovery_code"] };
    const passPassword = async () => {
      await userEvent.type(screen.getByLabelText("Email"), "ada@example.com");
      await userEvent.type(screen.getByLabelText("Password"), "a-long-enough-password");
      await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
    };

    it("asks for a code after the password, and accepts a recovery code instead", async () => {
      signIn.mockResolvedValue(challenge);
      completeSignIn
        .mockRejectedValueOnce(new ApiError(422, { code: "validation_failed", detail: "validation failed", errors: [{ field: "code", code: "incorrect", message: "That code didn't work. Check the app and try again." }] }))
        .mockResolvedValueOnce(undefined);
      renderAt("/login?next=%2Fpeople");
      await passPassword();

      const code = await screen.findByLabelText("Authentication code");
      expect(code).toHaveAttribute("autocomplete", "one-time-code");
      await userEvent.click(screen.getByRole("button", { name: "Verify" }));
      expect(await screen.findByText("Enter the code from your authenticator app.")).toBeInTheDocument();
      expect(completeSignIn).not.toHaveBeenCalled();

      await userEvent.type(code, "123456");
      await userEvent.click(screen.getByRole("button", { name: "Verify" }));
      expect(await screen.findByText("That code didn't work. Check the app and try again.")).toBeInTheDocument();
      expect(completeSignIn).toHaveBeenCalledWith("challenge-1", "123456");

      await userEvent.click(screen.getByRole("button", { name: "Can't use your app? Use a recovery code" }));
      await userEvent.type(screen.getByLabelText("Recovery code"), "ABCD-EFGH-JKMN");
      await userEvent.click(screen.getByRole("button", { name: "Verify" }));
      expect(await screen.findByText("People page")).toBeInTheDocument();
      expect(completeSignIn).toHaveBeenLastCalledWith("challenge-1", "ABCD-EFGH-JKMN");
    });

    it("goes back to the password when the attempt has expired", async () => {
      signIn.mockResolvedValue(challenge);
      completeSignIn.mockRejectedValue(new ApiError(401, { code: "unauthenticated", detail: "This sign-in attempt has expired. Sign in again." }));
      renderAt("/login");
      await passPassword();
      await userEvent.type(await screen.findByLabelText("Authentication code"), "123456");
      await userEvent.click(screen.getByRole("button", { name: "Verify" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("This sign-in attempt has expired. Sign in again.");
      expect(screen.getByLabelText("Password")).toBeInTheDocument();
    });
  });
});
