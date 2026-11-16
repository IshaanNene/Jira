import { Check, Clock } from "lucide-react";
import { useEffect } from "react";
import { Link } from "react-router-dom";

import { Button } from "@/components/ui";
import { cn } from "@/utils/cn";
import { PRODUCT_NAME } from "@/utils/config";

import { MarketingLayout } from "../components/MarketingLayout";
import { PLANS } from "../content";

const FAQ = [
  {
    q: "Am I charged during the preview?",
    a: "No. Billing isn't connected yet, so every workspace has every built feature at no charge while the product is in preview. We'll give you notice before that changes.",
  },
  { q: "Can I invite people outside my company?", a: "Yes. Invite them as guests and they see only the projects you add them to." },
  { q: "Can we run it ourselves?", a: "Yes. It ships as container images with a Helm chart for Kubernetes and Terraform for AWS." },
  { q: "How do I get my data out?", a: "Everything is available through the same REST API the app uses, with personal API tokens for scripts." },
];

export default function PricingPage() {
  useEffect(() => {
    document.title = `Pricing · ${PRODUCT_NAME}`;
  }, []);
  return (
    <MarketingLayout>
      <div className="mx-auto max-w-6xl px-4 py-16">
        <div className="text-center">
          <h1 className="text-3xl font-semibold tracking-tight sm:text-4xl">Simple pricing that grows with you</h1>
          <p className="mx-auto mt-3 max-w-xl text-lg text-ink-muted">Start free. Upgrade when your team needs reporting and controls.</p>
          <p className="mx-auto mt-4 inline-block rounded-full bg-accent-soft px-4 py-1.5 text-sm text-accent" role="note">
            Preview: billing isn&apos;t connected yet, so every workspace gets every built feature free.
          </p>
        </div>
        <ul className="mt-12 grid gap-6 lg:grid-cols-3">
          {PLANS.map((plan) => (
            <li
              key={plan.name}
              aria-labelledby={`plan-${plan.name}`}
              className={cn("flex flex-col rounded-xl border bg-surface p-6", plan.highlighted ? "border-accent ring-2 ring-accent/25" : "border-line")}
            >
              <div className="flex items-center justify-between">
                <h2 id={`plan-${plan.name}`} className="text-lg font-semibold">{plan.name}</h2>
                {plan.highlighted && <span className="rounded-full bg-accent px-2.5 py-0.5 text-xs font-semibold text-accent-ink">Most popular</span>}
              </div>
              <p className="mt-1 text-sm text-ink-muted">{plan.blurb}</p>
              <p className="mt-6">
                <span className="text-3xl font-semibold tracking-tight">{plan.price}</span>
                <span className="ml-2 text-sm text-ink-muted">{plan.cadence}</span>
              </p>
              <Button asChild variant={plan.highlighted ? "primary" : "secondary"} size="lg" className="mt-6">
                <Link to="/register">{plan.cta}</Link>
              </Button>
              <ul className="mt-6 space-y-2.5 text-sm">
                {plan.features.map((f) => (
                  <li key={f} className="flex gap-2.5"><Check className="mt-0.5 h-4 w-4 shrink-0 text-success" aria-hidden /> {f}</li>
                ))}
                {plan.soon?.map((f) => (
                  <li key={f} className="flex gap-2.5 text-ink-muted">
                    <Clock className="mt-0.5 h-4 w-4 shrink-0" aria-hidden /> {f} <span className="text-ink-subtle">(coming soon)</span>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
        <section aria-labelledby="faq-title" className="mx-auto mt-20 max-w-3xl">
          <h2 id="faq-title" className="text-2xl font-semibold tracking-tight">Questions</h2>
          <dl className="mt-6 divide-y divide-line border-y border-line">
            {FAQ.map((f) => (
              <div key={f.q} className="py-5">
                <dt className="font-semibold">{f.q}</dt>
                <dd className="mt-1.5 text-ink-muted">{f.a}</dd>
              </div>
            ))}
          </dl>
        </section>
      </div>
    </MarketingLayout>
  );
}
