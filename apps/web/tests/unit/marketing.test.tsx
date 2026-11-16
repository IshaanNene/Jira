import { render, screen, within } from "@testing-library/react";
import { Suspense } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";

import { RequireAuth } from "@/app/guards";
import FeaturesPage from "@/modules/marketing/pages/FeaturesPage";
import LandingPage from "@/modules/marketing/pages/LandingPage";
import PricingPage from "@/modules/marketing/pages/PricingPage";
import { useSession } from "@/state/session";

import { signIn } from "./harness";

afterEach(() => vi.unstubAllGlobals());

function signOut() {
  useSession.setState({ status: "anonymous", accessToken: null, account: null, organization: null });
}

function show(url: string, element: React.ReactElement) {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <Suspense fallback={<p>loading</p>}>
        <Routes>
          <Route element={element}>
            <Route path="/" element={<p>workspace home</p>} />
            <Route path="/people" element={<p>people</p>} />
          </Route>
          <Route path="/login" element={<p>sign-in page</p>} />
        </Routes>
      </Suspense>
    </MemoryRouter>,
  );
}

describe("public site", () => {
  it("shows signed-out visitors the landing page at the root, and sign-in elsewhere", async () => {
    signOut();
    show("/", <RequireAuth />);
    expect(await screen.findByRole("heading", { level: 1, name: /plan, track and ship work/i })).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: /get started free/i })[0]).toHaveAttribute("href", "/register");
    expect(within(screen.getByRole("banner")).getByRole("link", { name: "Sign in" })).toHaveAttribute("href", "/login");
  });

  it("still sends signed-out visitors of app pages to sign in", async () => {
    signOut();
    show("/people", <RequireAuth />);
    expect(await screen.findByText("sign-in page")).toBeInTheDocument();
  });

  it("takes signed-in people straight to their workspace", async () => {
    signIn();
    show("/", <RequireAuth />);
    expect(await screen.findByText("workspace home")).toBeInTheDocument();
  });

  it("offers signed-in visitors their workspace instead of sign-in", () => {
    signIn();
    render(<MemoryRouter><LandingPage /></MemoryRouter>);
    expect(within(screen.getByRole("banner")).getByRole("link", { name: "Open your workspace" })).toHaveAttribute("href", "/");
    expect(screen.queryByRole("link", { name: "Sign in" })).not.toBeInTheDocument();
  });

  it("links each feature area on the landing page to its section", () => {
    signOut();
    render(<MemoryRouter><LandingPage /></MemoryRouter>);
    expect(screen.getByRole("link", { name: /boards, backlogs and sprints/i })).toHaveAttribute("href", "/features#agile");
  });

  it("lists every feature area with its own heading", () => {
    signOut();
    render(<MemoryRouter initialEntries={["/features"]}><FeaturesPage /></MemoryRouter>);
    expect(screen.getByRole("heading", { level: 1, name: "Features" })).toBeInTheDocument();
    // Eight areas and the closing call to action.
    expect(within(screen.getByRole("main")).getAllByRole("heading", { level: 2 })).toHaveLength(9);
    expect(document.getElementById("workflows")).not.toBeNull();
  });

  it("is honest about billing and what is not built yet", () => {
    signOut();
    render(<MemoryRouter><PricingPage /></MemoryRouter>);
    expect(screen.getByRole("note")).toHaveTextContent(/billing isn.t connected yet/i);
    const enterprise = screen.getByRole("listitem", { name: "Enterprise" });
    expect(within(enterprise).getByText(/SAML and OIDC single sign-on/)).toBeInTheDocument();
    expect(within(enterprise).getAllByText("(coming soon)")).toHaveLength(2);
  });
});
