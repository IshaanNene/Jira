// Example content for the landing page's interactive pieces. Everything here is a sample,
// labelled as such on the page; product claims live in ../content.ts.
import type { SampleIssue } from "./query";

export type DemoColumn = "todo" | "doing" | "done";

export interface DemoCard {
  key: string;
  summary: string;
  epic: "Checkout" | "Payouts" | "Fraud";
  points: number;
  column: DemoColumn;
}

export const DEMO_COLUMNS: { id: DemoColumn; name: string; limit?: number }[] = [
  { id: "todo", name: "To do" },
  { id: "doing", name: "In progress", limit: 2 },
  { id: "done", name: "Done" },
];

export const DEMO_CARDS: DemoCard[] = [
  { key: "PAY-142", summary: "Retry failed card payments", epic: "Checkout", points: 5, column: "todo" },
  { key: "PAY-151", summary: "Show payout schedule on the dashboard", epic: "Payouts", points: 3, column: "todo" },
  { key: "PAY-158", summary: "Flag cards used from three countries in a day", epic: "Fraud", points: 8, column: "todo" },
  { key: "PAY-137", summary: "Apple Pay on the checkout page", epic: "Checkout", points: 5, column: "doing" },
  { key: "PAY-129", summary: "Weekly payout email", epic: "Payouts", points: 2, column: "done" },
];

export const SAMPLE_ISSUES: SampleIssue[] = [
  { key: "PAY-101", summary: "Refunds over $500 need a second approver", type: "story", status: "In progress", statusCategory: "in_progress", assignee: "you", priority: "high", points: 5, labels: ["refunds"], epic: "Checkout" },
  { key: "PAY-104", summary: "Card form loses focus on Safari", type: "bug", status: "To do", statusCategory: "todo", assignee: "you", priority: "highest", points: 2, labels: ["frontend", "safari"], epic: "Checkout" },
  { key: "PAY-108", summary: "Export payouts to CSV", type: "story", status: "Done", statusCategory: "done", assignee: "you", priority: "medium", points: 3, labels: ["payouts"], epic: "Payouts" },
  { key: "PAY-112", summary: "Velocity block on the team dashboard", type: "task", status: "In review", statusCategory: "in_progress", assignee: "maya", priority: "low", points: 2, labels: ["reporting"], epic: null },
  { key: "PAY-115", summary: "Block cards on the sanctions list", type: "story", status: "To do", statusCategory: "todo", assignee: "olivia", priority: "highest", points: 8, labels: ["fraud", "compliance"], epic: "Fraud" },
  { key: "PAY-119", summary: "Duplicate charge when the network retries", type: "bug", status: "In progress", statusCategory: "in_progress", assignee: "maya", priority: "highest", points: 3, labels: ["payments"], epic: "Checkout" },
  { key: "PAY-121", summary: "Payout schedule in workspace settings", type: "task", status: "To do", statusCategory: "todo", assignee: null, priority: "medium", points: null, labels: [], epic: "Payouts" },
  { key: "PAY-124", summary: "Score transactions with velocity rules", type: "story", status: "In review", statusCategory: "in_progress", assignee: "you", priority: "high", points: 8, labels: ["fraud"], epic: "Fraud" },
  { key: "PAY-127", summary: "Receipt email shows the wrong currency", type: "bug", status: "Done", statusCategory: "done", assignee: "olivia", priority: "high", points: 1, labels: ["email"], epic: "Checkout" },
  { key: "PAY-130", summary: "Saved cards for returning customers", type: "story", status: "To do", statusCategory: "todo", assignee: "you", priority: "medium", points: 5, labels: ["frontend"], epic: "Checkout" },
  { key: "PAY-133", summary: "Alert when payouts fail twice", type: "task", status: "To do", statusCategory: "todo", assignee: "maya", priority: "high", points: 2, labels: ["payouts", "alerts"], epic: "Payouts" },
  { key: "PAY-136", summary: "Chargeback evidence upload", type: "story", status: "In progress", statusCategory: "in_progress", assignee: "olivia", priority: "medium", points: 5, labels: ["disputes"], epic: null },
];

export const EXAMPLE_QUERIES = [
  "assignee = currentUser() AND statusCategory != done",
  "type = bug AND priority >= high",
  "epic = Checkout AND NOT status = Done",
  "labels IN (fraud, payouts)",
  "assignee IS EMPTY OR points IS EMPTY",
  'summary ~ "payout" AND points < 5',
];

export const FACTS = [
  "Row-level security on every workspace table",
  "Tenant comes from a signed token, never the request",
  "Scrum and Kanban boards with WIP limits and swimlanes",
  "Visual workflow designer with version history",
  "Full-text search plus a real query language",
  "Dashboards on a ClickHouse pipeline",
  "SSO with OpenID Connect or SAML",
  "SCIM provisioning and passkeys",
  "Signed release images with SBOMs",
];

export const SECURITY_POINTS = [
  { label: "Isolation", title: "Row-level security", body: "Every table that holds workspace data is filtered by Postgres itself. A bug in an API can't read another team's issues." },
  { label: "Identity", title: "Tenant from a signed token", body: "Which workspace a request belongs to comes from a signed token. It never comes from anything the browser can change." },
  { label: "Sign-in", title: "SSO, SCIM, passkeys", body: "OpenID Connect or SAML, SCIM provisioning of people and groups, passkeys, and two-step verification that admins can require." },
  { label: "Record", title: "Tamper-evident audit log", body: "Admin changes are written to a chained log, so editing or deleting an entry afterwards shows up." },
  { label: "Delivery", title: "Signed images", body: "Release images are signed and carry a software bill of materials. The cluster can refuse anything unsigned." },
  { label: "Rules", title: "No user code runs", body: "Automation, workflows and webhooks are configuration. Nothing a customer types is ever executed." },
];

export const FAQ = [
  { q: "Can we move from another tracker?", a: "Meridian has the same building blocks as the big trackers: epics, stories, tasks, bugs and sub-tasks, custom fields, workflows, Scrum and Kanban boards, and a query language. Your team won't have to unlearn anything." },
  { q: "Where does our data live?", a: "On Meridian's hosted service, in Postgres, with every workspace isolated by row-level security. Enterprise customers can also run Meridian on their own Kubernetes cluster with the provided Helm chart and Terraform." },
  { q: "What happens when we pass 10 people on Free?", a: "Nothing breaks and nothing is deleted. You'll be asked to move to Team to add more people. Everything you've built stays where it is." },
  { q: "Do boards stay fast on big projects?", a: "Boards read from a local copy of each project's cards that issue events keep up to date, so a board loads from one place instead of asking the issue service every time. A board shows up to 5,000 cards." },
  { q: "Can we connect Slack, GitHub or our own tools?", a: "Yes. Outgoing webhooks are signed, so you can check they came from Meridian. A webhook can post to a Slack channel. Commits and pull requests that mention an issue key appear on that issue. Personal API tokens cover scripts." },
];

/** Monthly price of the Team plan per person, in US dollars. */
export const TEAM_PRICE = 8;
/** People the Free plan covers. */
export const FREE_SEATS = 10;

/** What a team of `people` pays: the plan that fits and its monthly and yearly cost. */
export function quote(people: number): { plan: "Free" | "Team"; monthly: number; yearly: number } {
  if (people <= FREE_SEATS) return { plan: "Free", monthly: 0, yearly: 0 };
  return { plan: "Team", monthly: people * TEAM_PRICE, yearly: people * TEAM_PRICE * 12 };
}
