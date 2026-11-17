import { fireEvent, render, screen, within } from "@testing-library/react";
import { Suspense } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";

import { RequireAuth } from "@/app/guards";
import FeaturesPage from "@/modules/marketing/pages/FeaturesPage";
import LandingPage from "@/modules/marketing/pages/LandingPage";
import { quote } from "@/modules/marketing/landing/data";
import { runQuery, QueryError, type SampleIssue } from "@/modules/marketing/landing/query";
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
    expect(await screen.findByRole("heading", { level: 1, name: /plan it\. ship it\. prove it\./i })).toBeInTheDocument();
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
    expect(within(enterprise).getByText("Single sign-on (OpenID Connect or SAML)")).toBeInTheDocument();
    expect(within(enterprise).getByText("SCIM provisioning, with roles from your groups")).toBeInTheDocument();
    expect(within(enterprise).getByText(/Single logout/)).toBeInTheDocument();
    expect(within(enterprise).getAllByText("(coming soon)")).toHaveLength(2);
  });
});

describe("landing page playground and pricing", () => {
  const issue = (key: string, overrides: Partial<SampleIssue> = {}): SampleIssue => ({
    key, summary: `Issue ${key}`, type: "story", status: "To do", statusCategory: "todo", assignee: null,
    priority: "medium", points: 3, labels: [], epic: null, ...overrides,
  });
  const issues = [
    issue("A-1", { assignee: "you", type: "bug", priority: "highest", labels: ["fraud"] }),
    issue("A-2", { assignee: "maya", statusCategory: "done", status: "Done", points: null }),
    issue("A-3", { assignee: "you", statusCategory: "in_progress", status: "In progress", epic: "Checkout", summary: "Payout email" }),
  ];
  const keys = (q: string) => runQuery(q, issues).map((i) => i.key);

  it("filters example issues with the query language's fields and operators", () => {
    expect(keys("assignee = currentUser() AND statusCategory != done")).toEqual(["A-1", "A-3"]);
    expect(keys("type = bug OR epic = Checkout")).toEqual(["A-1", "A-3"]);
    expect(keys("priority >= high")).toEqual(["A-1"]);
    expect(keys("labels IN (fraud, payouts)")).toEqual(["A-1"]);
    expect(keys("points IS EMPTY")).toEqual(["A-2"]);
    expect(keys('summary ~ "payout" AND NOT (assignee = maya)')).toEqual(["A-3"]);
    expect(keys("  ")).toEqual(["A-1", "A-2", "A-3"]);
  });

  it("explains a query that can't run", () => {
    expect(() => keys("colour = red")).toThrow(QueryError);
    expect(() => keys("colour = red")).toThrow(/no field called "colour"/);
    expect(() => keys("type = bug type = story")).toThrow(/Join conditions with AND or OR/);
    expect(() => keys('summary ~ "open')).toThrow(/never closed/);
    expect(() => keys("priority > urgent")).toThrow(/isn't a priority/);
  });

  it("prices a team: free up to 10 people, then per person", () => {
    expect(quote(10)).toEqual({ plan: "Free", monthly: 0, yearly: 0 });
    expect(quote(11)).toEqual({ plan: "Team", monthly: 88, yearly: 1056 });
  });

  it("marks the plan that fits as the team size changes", async () => {
    signOut();
    render(<MemoryRouter><LandingPage /></MemoryRouter>);
    expect(screen.getByRole("article", { name: "Team plan" })).toHaveTextContent("FITS YOUR TEAM");
    fireEvent.change(screen.getByLabelText("Your team size"), { target: { value: "4" } });
    expect(screen.getByRole("article", { name: "Free plan" })).toHaveTextContent("FITS YOUR TEAM");
    expect(screen.getByRole("article", { name: "Team plan" })).not.toHaveTextContent("FITS YOUR TEAM");
  });
});
