import { ArrowRight, Check } from "lucide-react";
import { useEffect } from "react";
import { Link } from "react-router-dom";

import { Button } from "@/components/ui";
import { PRODUCT_NAME } from "@/utils/config";

import { MarketingLayout, Section } from "../components/MarketingLayout";
import { ProductPreview } from "../components/ProductPreview";
import { FEATURE_AREAS } from "../content";

const STEPS = [
  { title: "Create a workspace", body: "Sign up, name your workspace and invite your team by email. Guests can be invited to just the projects they need." },
  { title: "Shape your process", body: "Start from a project template, then draw your own workflow, add custom fields and set up the board's columns." },
  { title: "Plan, ship, learn", body: "Run sprints from the backlog, track them on the board and see velocity, cycle time and flow on your dashboards." },
];

const PROOF = [
  { value: "Row-level", label: "tenant isolation in the database" },
  { value: "Live", label: "board and issue updates" },
  { value: "7", label: "built-in chart kinds" },
  { value: "0", label: "lines of user code run by workflows" },
];

export default function LandingPage() {
  useEffect(() => {
    document.title = `${PRODUCT_NAME} · Plan, track and ship work`;
  }, []);
  return (
    <MarketingLayout>
      <div className="border-b border-line bg-gradient-to-b from-accent-soft/60 to-canvas">
        <div className="mx-auto grid max-w-6xl items-center gap-12 px-4 py-16 lg:grid-cols-[1fr_1.15fr] lg:py-24">
          <div>
            <p className="inline-flex items-center gap-2 rounded-full border border-line bg-surface px-3 py-1 text-xs font-medium text-ink-muted">
              <span className="h-1.5 w-1.5 rounded-full bg-success" aria-hidden /> Now in preview
            </p>
            <h1 className="mt-5 text-4xl font-semibold leading-tight tracking-tight sm:text-5xl">
              Plan, track and ship work with your whole team.
            </h1>
            <p className="mt-5 max-w-xl text-lg text-ink-muted">
              {PRODUCT_NAME} brings issues, boards, sprints, workflows and dashboards together in one place, with the
              security an organisation needs from day one.
            </p>
            <div className="mt-8 flex flex-wrap gap-3">
              <Button asChild variant="primary" size="lg">
                <Link to="/register">Get started free <ArrowRight className="h-4 w-4" aria-hidden /></Link>
              </Button>
              <Button asChild size="lg"><Link to="/features">See the features</Link></Button>
            </div>
            <ul className="mt-6 flex flex-wrap gap-x-5 gap-y-2 text-sm text-ink-muted">
              {["Free for up to 10 people", "No credit card", "Invite your team in minutes"].map((t) => (
                <li key={t} className="flex items-center gap-1.5"><Check className="h-4 w-4 text-success" aria-hidden /> {t}</li>
              ))}
            </ul>
          </div>
          <ProductPreview />
        </div>
      </div>

      <section aria-label="At a glance" className="border-b border-line">
        <dl className="mx-auto grid max-w-6xl grid-cols-2 gap-6 px-4 py-10 md:grid-cols-4">
          {PROOF.map((p) => (
            <div key={p.label}>
              <dt className="sr-only">{p.label}</dt>
              <dd className="text-2xl font-semibold tracking-tight">{p.value}</dd>
              <dd className="mt-1 text-sm text-ink-muted">{p.label}</dd>
            </div>
          ))}
        </dl>
      </section>

      <Section id="everything" eyebrow="One product" title="Everything a team needs to get work done" intro="From the first idea in the backlog to the report on how the sprint went.">
        <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {FEATURE_AREAS.map((f) => (
            <li key={f.id}>
              <Link to={`/features#${f.id}`} className="group block h-full rounded-lg border border-line bg-surface p-5 transition-colors hover:border-accent">
                <f.icon className="h-6 w-6 text-accent" aria-hidden />
                <h3 className="mt-3 font-semibold">{f.title}</h3>
                <p className="mt-1.5 text-sm text-ink-muted">{f.summary}</p>
                <span className="mt-3 inline-flex items-center gap-1 text-sm font-medium text-accent">
                  Learn more <ArrowRight className="h-3.5 w-3.5 transition-transform group-hover:translate-x-0.5" aria-hidden />
                </span>
              </Link>
            </li>
          ))}
        </ul>
      </Section>

      <div className="border-y border-line bg-sunken">
        <Section id="how" eyebrow="How it works" title="Up and running in an afternoon">
          <ol className="grid gap-6 md:grid-cols-3">
            {STEPS.map((s, i) => (
              <li key={s.title} className="rounded-lg border border-line bg-surface p-5">
                <span className="flex h-8 w-8 items-center justify-center rounded-full bg-accent text-sm font-semibold text-accent-ink" aria-hidden>{i + 1}</span>
                <h3 className="mt-4 font-semibold">{s.title}</h3>
                <p className="mt-1.5 text-sm text-ink-muted">{s.body}</p>
              </li>
            ))}
          </ol>
        </Section>
      </div>

      <Section id="trust" eyebrow="Security" title="Your data stays yours" intro="Each workspace is isolated by the database itself, not just by application code. Every request is authorised against the caller's own permissions.">
        <div className="flex flex-wrap gap-3">
          <Button asChild><Link to="/security">How we keep data safe</Link></Button>
          <Button asChild variant="ghost"><Link to="/pricing">Compare plans</Link></Button>
        </div>
      </Section>

      <section className="mx-auto max-w-6xl px-4 pb-20">
        <div className="rounded-xl bg-accent px-6 py-12 text-center text-accent-ink sm:px-12">
          <h2 className="text-2xl font-semibold tracking-tight sm:text-3xl">Bring your team&apos;s work together</h2>
          <p className="mx-auto mt-3 max-w-xl text-lg opacity-90">Create a workspace in under a minute. Free for teams of up to 10.</p>
          <Button asChild size="lg" className="mt-8 bg-surface text-ink hover:bg-sunken">
            <Link to="/register">Create your workspace</Link>
          </Button>
        </div>
      </section>
    </MarketingLayout>
  );
}
