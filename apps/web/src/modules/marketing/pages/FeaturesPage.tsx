import { Check } from "lucide-react";
import { useEffect } from "react";
import { Link, useLocation } from "react-router-dom";

import { Button } from "@/components/ui";
import { cn } from "@/utils/cn";
import { PRODUCT_NAME } from "@/utils/config";

import { MarketingLayout } from "../components/MarketingLayout";
import { FEATURE_AREAS } from "../content";

export default function FeaturesPage() {
  const { hash } = useLocation();
  useEffect(() => {
    document.title = `Features · ${PRODUCT_NAME}`;
  }, []);
  // Follow links like /features#agile once the sections are on the page.
  useEffect(() => {
    if (hash) document.getElementById(hash.slice(1))?.scrollIntoView?.();
  }, [hash]);
  return (
    <MarketingLayout>
      <div className="border-b border-line bg-sunken">
        <div className="mx-auto max-w-6xl px-4 py-14">
          <h1 className="text-3xl font-semibold tracking-tight sm:text-4xl">Features</h1>
          <p className="mt-3 max-w-2xl text-lg text-ink-muted">Everything in {PRODUCT_NAME} today, area by area.</p>
          <nav aria-label="Feature areas" className="mt-6 flex flex-wrap gap-2">
            {FEATURE_AREAS.map((f) => (
              <a key={f.id} href={`#${f.id}`} className="rounded-full border border-line bg-surface px-3 py-1 text-sm text-ink-muted hover:border-accent hover:text-ink">
                {f.title.split(" ").slice(0, 3).join(" ")}
              </a>
            ))}
          </nav>
        </div>
      </div>
      <div className="mx-auto max-w-6xl divide-y divide-line px-4">
        {FEATURE_AREAS.map((f, i) => (
          <section key={f.id} id={f.id} aria-labelledby={`${f.id}-title`} className="grid scroll-mt-20 gap-8 py-14 md:grid-cols-2">
            <div className={cn(i % 2 === 1 && "md:order-2")}>
              <f.icon className="h-7 w-7 text-accent" aria-hidden />
              <h2 id={`${f.id}-title`} className="mt-3 text-2xl font-semibold tracking-tight">{f.title}</h2>
              <p className="mt-3 text-lg text-ink-muted">{f.summary}</p>
            </div>
            <ul className={cn("space-y-3 self-center rounded-lg border border-line bg-surface p-6", i % 2 === 1 && "md:order-1")}>
              {f.points.map((p) => (
                <li key={p} className="flex gap-3 text-base">
                  <Check className="mt-0.5 h-4 w-4 shrink-0 text-success" aria-hidden />
                  <span>{p}</span>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
      <div className="mx-auto max-w-6xl px-4 pb-20 pt-6 text-center">
        <h2 className="text-2xl font-semibold tracking-tight">See it with your own work</h2>
        <div className="mt-6 flex justify-center gap-3">
          <Button asChild variant="primary" size="lg"><Link to="/register">Get started free</Link></Button>
          <Button asChild size="lg"><Link to="/pricing">See pricing</Link></Button>
        </div>
      </div>
    </MarketingLayout>
  );
}
