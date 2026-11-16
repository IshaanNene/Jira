import { Database, FileLock2, KeyRound, Lock, ScrollText, ServerCog } from "lucide-react";
import { useEffect } from "react";
import { Link } from "react-router-dom";

import { Button } from "@/components/ui";
import { PRODUCT_NAME } from "@/utils/config";

import { MarketingLayout, Section } from "../components/MarketingLayout";

const PRACTICES = [
  {
    icon: Database,
    title: "Isolation in the database",
    body: "Every table carries its workspace and is protected by forced row-level security. A query without a workspace sees nothing, even if application code has a bug.",
  },
  {
    icon: Lock,
    title: "The workspace comes from a signed token",
    body: "Which workspace a request belongs to is read only from a signed token, never from anything the browser sends. Permissions are checked on the server for every request.",
  },
  {
    icon: KeyRound,
    title: "Accounts and sign-in",
    body: "Passwords are hashed with Argon2. Passkeys, two-step verification with an authenticator app and recovery codes, and the option for admins to require it across the workspace. Single sign-on through your OpenID Connect or SAML provider, and SCIM provisioning so people leave the workspace when they leave your company and their roles follow your groups.",
  },
  {
    icon: FileLock2,
    title: "Safe content",
    body: "Descriptions and comments are structured documents checked against an allow-list, never stored as HTML. Attachments are typed from their bytes, executables are refused, and downloads are never shown inline.",
  },
  {
    icon: ScrollText,
    title: "A tamper-evident audit log",
    body: "Administrative changes are recorded in a per-workspace hash chain, so a removed or altered entry can be detected.",
  },
  {
    icon: ServerCog,
    title: "No user code, ever",
    body: "Workflows are declarative rules. Nothing anyone types is executed by the platform.",
  },
];

export default function SecurityPage() {
  useEffect(() => {
    document.title = `Security · ${PRODUCT_NAME}`;
  }, []);
  return (
    <MarketingLayout>
      <div className="border-b border-line bg-sunken">
        <div className="mx-auto max-w-6xl px-4 py-14">
          <h1 className="text-3xl font-semibold tracking-tight sm:text-4xl">Security</h1>
          <p className="mt-3 max-w-2xl text-lg text-ink-muted">How {PRODUCT_NAME} keeps each workspace&apos;s data separate, private and accounted for.</p>
        </div>
      </div>
      <Section title="How we protect your work">
        <ul className="grid gap-6 md:grid-cols-2 lg:grid-cols-3">
          {PRACTICES.map((p) => (
            <li key={p.title} className="rounded-lg border border-line bg-surface p-6">
              <p.icon className="h-6 w-6 text-accent" aria-hidden />
              <h3 className="mt-3 font-semibold">{p.title}</h3>
              <p className="mt-1.5 text-sm text-ink-muted">{p.body}</p>
            </li>
          ))}
        </ul>
      </Section>
      <Section title="On the roadmap" intro="Not built yet, and listed here so you know: signed SAML requests, encrypted SAML assertions, and single logout.">
        <Button asChild variant="primary"><Link to="/register">Get started free</Link></Button>
      </Section>
    </MarketingLayout>
  );
}
