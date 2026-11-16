import {
  BarChart3,
  Bell,
  GitBranch,
  KanbanSquare,
  ListChecks,
  Search,
  ShieldCheck,
  Workflow,
  type LucideIcon,
} from "lucide-react";

export interface FeatureArea {
  id: string;
  icon: LucideIcon;
  title: string;
  summary: string;
  points: string[];
}

/**
 * What the product does today. Every line here is built and tested; the roadmap
 * (docs/IMPLEMENTATION_ROADMAP.md) is the ledger, and this list must not run ahead of it.
 */
export const FEATURE_AREAS: FeatureArea[] = [
  {
    id: "issues",
    icon: ListChecks,
    title: "Issues that hold the whole story",
    summary: "Epics, stories, tasks, bugs and sub-tasks with rich descriptions, custom fields and a full change history.",
    points: [
      "Rich text with @mentions that notify the people named",
      "Custom fields scoped to projects and issue types",
      "Attachments checked by content, never served inline",
      "Links, cloning, watchers and votes",
      "Edits never overwrite someone else's change",
    ],
  },
  {
    id: "agile",
    icon: KanbanSquare,
    title: "Boards, backlogs and sprints",
    summary: "Plan in the backlog, run sprints on the board, and see how they went in sprint reports.",
    points: [
      "Drag cards across columns mapped to workflow statuses",
      "Rank the backlog and plan sprints with story points",
      "Start and complete sprints; unfinished work carries over",
      "Changes from teammates appear live, without reloading",
    ],
  },
  {
    id: "workflows",
    icon: Workflow,
    title: "Workflows you draw",
    summary: "A visual designer for statuses and transitions, with drafts, publishing and version history.",
    points: [
      "Statuses grouped into To do, In progress and Done",
      "Transitions that require fields before an issue moves",
      "Every published version kept; restore any of them as a draft",
      "Automation rules that assign, prioritise and label issues for you",
      "Rules are declarative: no user code ever runs",
    ],
  },
  {
    id: "search",
    icon: Search,
    title: "Search with a real query language",
    summary: "Full-text search plus a structured query language with saved filters and email subscriptions.",
    points: [
      'Queries like assignee = currentUser() AND statusCategory != done',
      "Save filters and get their results by email on a schedule",
      "Board and backlog filtering with the same language",
      "Results limited to what each person may see",
    ],
  },
  {
    id: "dashboards",
    icon: BarChart3,
    title: "Dashboards and reports",
    summary: "Charts built on an analytics pipeline, so reporting never slows down the work itself.",
    points: [
      "Seven chart kinds, from created vs. resolved to cumulative flow",
      "Time in status, velocity and sprint burndown",
      "Dashboards per person, shareable with the workspace",
    ],
  },
  {
    id: "notifications",
    icon: Bell,
    title: "Notifications people keep on",
    summary: "In-app and email notifications for what each person is part of, with per-event preferences.",
    points: [
      "Assignments, mentions, comments and status changes",
      "Watch any issue; reporters and assignees watch automatically",
      "One-click unsubscribe from every email",
    ],
  },
  {
    id: "security",
    icon: ShieldCheck,
    title: "Security built in, not bolted on",
    summary: "Every workspace's data is isolated in the database itself, and every action is checked and recorded.",
    points: [
      "Tenant isolation enforced by row-level security",
      "Passkeys and two-step verification, which admins can require for everyone",
      "Single sign-on with OpenID Connect or SAML, and SCIM provisioning from your identity provider",
      "Personal API tokens, signed outgoing webhooks and session management",
      "A tamper-evident audit log of administrative changes",
    ],
  },
  {
    id: "platform",
    icon: GitBranch,
    title: "Ready to run at scale",
    summary: "Independent services connected by an event pipeline, packaged for Kubernetes and observable end to end.",
    points: [
      "Services in Rust, events through Kafka, search in OpenSearch",
      "Helm chart, container images and Terraform for AWS",
      "Metrics, traces and alerts out of the box",
    ],
  },
];

export interface Plan {
  name: string;
  price: string;
  cadence: string;
  blurb: string;
  features: string[];
  /** Not built yet: shown so the plan is honest about what it will include. */
  soon?: string[];
  cta: string;
  highlighted?: boolean;
}

export const PLANS: Plan[] = [
  {
    name: "Free",
    price: "$0",
    cadence: "up to 10 people",
    blurb: "For small teams getting organised.",
    features: ["Unlimited projects and issues", "Boards, backlogs and sprints", "Workflow designer", "Search and saved filters"],
    cta: "Start free",
  },
  {
    name: "Team",
    price: "$8",
    cadence: "per person / month",
    blurb: "For teams that plan and report together.",
    features: ["Everything in Free", "Dashboards and reports", "Custom fields", "Automation rules", "Email subscriptions to filters", "Required two-step verification"],
    cta: "Start a free trial",
    highlighted: true,
  },
  {
    name: "Enterprise",
    price: "Talk to us",
    cadence: "annual agreement",
    blurb: "For organisations with compliance needs.",
    features: [
      "Everything in Team",
      "Single sign-on (OpenID Connect or SAML)",
      "SCIM provisioning",
      "Audit log",
      "API tokens",
      "Outgoing webhooks",
      "Self-hosting on your Kubernetes",
    ],
    soon: ["SCIM groups", "Signed SAML requests and encrypted assertions"],
    cta: "Contact sales",
  },
];
