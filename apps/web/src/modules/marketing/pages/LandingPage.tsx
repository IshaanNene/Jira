import { ArrowRight } from "lucide-react";
import { useEffect, useState } from "react";
import { Link } from "react-router-dom";

import { useSession } from "@/state/session";
import { PRODUCT_NAME } from "@/utils/config";

import { FEATURE_AREAS, PLANS } from "../content";
import { FACTS, FAQ, quote, SECURITY_POINTS } from "../landing/data";
import { DemoBoard } from "../landing/DemoBoard";
import { QueryPlayground } from "../landing/QueryPlayground";
import { WorkflowDiagram } from "../landing/WorkflowDiagram";
import "../landing/landing.css";

const FONTS_ID = "landing-fonts";
const FONTS_URL =
  "https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,500;12..96,700;12..96,800&family=Instrument+Sans:wght@400;500;600;700&family=JetBrains+Mono:wght@400;600;700&display=swap";

const SECTIONS = [
  ["#features", "Features"],
  ["#query", "Query"],
  ["#workflows", "Workflows"],
  ["#security", "Security"],
  ["#pricing", "Pricing"],
  ["#faq", "FAQ"],
] as const;

const CARD_COLORS = ["c-yellow", "c-blue", "c-pink", "c-mint", "c-paper", "c-tomato", "c-yellow", "c-blue"];
const TILTS = [-0.6, 0.5, -0.3, 0.7, -0.5, 0.4, -0.7, 0.3];
const PLAN_LINES = 5;

function Mark({ size = 18 }: { size?: number }) {
  return (
    <span className="logo-mark" aria-hidden>
      <svg width={size} height={size} viewBox="0 0 18 18"><path d="M2 16V2l7 8 7-8v14" fill="none" stroke="#121016" strokeWidth="3" /></svg>
    </span>
  );
}

function SectionHead({ chip, title, intro }: { chip: string; title: React.ReactNode; intro?: string }) {
  return (
    <div className="sec-head">
      <div>
        <span className="chip">{chip}</span>
        <h2>{title}</h2>
      </div>
      {intro && <p>{intro}</p>}
    </div>
  );
}

function Pricing() {
  const [people, setPeople] = useState(12);
  const q = quote(people);
  const money = (n: number) => `$${n.toLocaleString("en-US")}`;
  return (
    <>
      <div className="calc">
        <div className="calc-l">
          <label htmlFor="landing-seats">Your team size</label>
          <div className="seats"><output htmlFor="landing-seats">{people}</output><span className="unit">{people === 1 ? "person" : "people"}</span></div>
          <input id="landing-seats" type="range" min={1} max={500} value={people} onChange={(e) => setPeople(Number(e.target.value))} />
        </div>
        <div className="calc-r" role="status">
          <span className="which">{q.plan} plan</span>
          <span className="total">{money(q.monthly)}</span>
          <span className="per">
            {q.plan === "Free" ? "forever, for up to 10 people" : `per month · ${money(q.yearly)} a year${people >= 100 ? ". Need SSO or SCIM? See Enterprise." : ""}`}
          </span>
        </div>
      </div>
      <ul className="plans">
        {PLANS.map((plan) => {
          const fits = plan.name === q.plan;
          return (
            <li key={plan.name}>
              <article className={["plan", plan.highlighted && "hot", fits && "fit"].filter(Boolean).join(" ")} aria-label={`${plan.name} plan`}>
                {fits && <span className="fits">FITS YOUR TEAM</span>}
                <h3>{plan.name}</h3>
                <div className="price">{plan.price} {plan.name === "Team" && <small>/ person / month</small>}</div>
                <p className="muted">{plan.name === "Team" ? plan.blurb : `${plan.cadence} · ${plan.blurb}`}</p>
                <ul>
                  {plan.features.slice(0, PLAN_LINES).map((f) => <li key={f}>{f}</li>)}
                </ul>
                <Link className={plan.name === "Enterprise" ? "btn dark" : "btn"} to="/register">{plan.cta}</Link>
              </article>
            </li>
          );
        })}
      </ul>
      <p className="fine">Prices in US dollars, before tax. <Link to="/pricing">Compare every plan in full →</Link></p>
    </>
  );
}

export default function LandingPage() {
  const signedIn = useSession((s) => s.status === "authenticated");
  useEffect(() => {
    document.title = `${PRODUCT_NAME} · Plan it. Ship it. Prove it.`;
    if (!document.getElementById(FONTS_ID)) {
      const link = document.createElement("link");
      link.id = FONTS_ID;
      link.rel = "stylesheet";
      link.href = FONTS_URL;
      document.head.appendChild(link);
    }
  }, []);

  return (
    <div className="nb">
      <a className="skip" href="#main">Skip to content</a>
      <header className="top">
        <div className="wrap">
          <Link className="logo" to="/" aria-label={`${PRODUCT_NAME} home`}><Mark />{PRODUCT_NAME}</Link>
          <nav className="links" aria-label="Sections">
            {SECTIONS.map(([href, label]) => <a key={href} href={href}>{label}</a>)}
          </nav>
          <div className="actions">
            {signedIn ? (
              <Link className="btn" to="/">Open your workspace</Link>
            ) : (
              <>
                <Link className="signin" to="/login">Sign in</Link>
                <Link className="btn" to="/register">Get started free</Link>
              </>
            )}
          </div>
        </div>
      </header>

      <main id="main">
        <section className="hero" aria-labelledby="landing-title">
          <div className="wrap hero-grid">
            <div>
              <span className="chip">Issue tracking · boards · sprints</span>
              <h1 id="landing-title">Plan it.<br /><span className="hl">Ship it.</span><br /><span className="under">Prove it.</span></h1>
              <p className="lede">
                {PRODUCT_NAME} is the project tracker for teams who want <b>issues, boards, workflows, search and reports</b> in one place,
                with every workspace locked down in the database itself.
              </p>
              <div className="ctas">
                <Link className="btn" to="/register">Get started free <ArrowRight className="h-5 w-5" strokeWidth={3} aria-hidden /></Link>
                <a className="btn alt" href="#query">Try the query language</a>
              </div>
            </div>
            <DemoBoard />
          </div>
        </section>

        <div className="ticker" aria-hidden>
          <div className="track">{[...FACTS, ...FACTS].map((f, i) => <span key={i}>{f}</span>)}</div>
        </div>

        <section className="block" id="features">
          <div className="wrap">
            <SectionHead chip="What's inside" title={<>Eight tools.<br />One tracker.</>} intro="Every item here is built and tested today. If something is still coming, we say so." />
            <ul className="features">
              {FEATURE_AREAS.map((f, i) => (
                <li key={f.id}>
                  <Link to={`/features#${f.id}`} className={`feat ${CARD_COLORS[i % CARD_COLORS.length]}`} style={{ "--tilt": `${TILTS[i % TILTS.length]}deg` } as React.CSSProperties}>
                    <span className="ico" aria-hidden><f.icon className="h-7 w-7" strokeWidth={2.6} /></span>
                    <h3>{f.title}</h3>
                    <p>{f.summary}</p>
                    <span className="more">Learn more →</span>
                  </Link>
                </li>
              ))}
            </ul>
          </div>
        </section>

        <section className="block" id="query">
          <div className="wrap">
            <SectionHead
              chip="Meridian Query Language"
              title="Ask your backlog anything."
              intro="Type a query and watch the example issues filter as you type. It uses the same fields and operators as the real search."
            />
            <QueryPlayground />
          </div>
        </section>

        <section className="block" id="workflows">
          <div className="wrap">
            <SectionHead
              chip="Workflow designer"
              title={<>Draw the process.<br />{PRODUCT_NAME} enforces it.</>}
              intro="Drag statuses onto a canvas and connect them. Transitions can require fields and run actions, and every published version is kept."
            />
            <WorkflowDiagram />
            <div className="flow-notes">
              <div className="note"><h3>Conditions and validators</h3>A transition can require a field, a role or a resolution before an issue is allowed through.</div>
              <div className="note"><h3>Drafts and versions</h3>Edit a draft, publish when it&apos;s ready, and restore any earlier version as a new draft.</div>
              <div className="note"><h3>Automation rules</h3>Assign, prioritise, label, move and comment, on a trigger or a schedule. Rules are settings, so no one&apos;s code ever runs.</div>
            </div>
          </div>
        </section>

        <section className="slab" id="security" aria-labelledby="landing-security">
          <div className="wrap">
            <span className="chip">Security</span>
            <h2 id="landing-security">Your data stays <em>yours</em>.<br />The database enforces it.</h2>
            <ul className="sec-grid">
              {SECURITY_POINTS.slice(0, 3).map((s) => (
                <li key={s.title} className="sec-item"><span className="k">{s.label}</span><h3>{s.title}</h3><p>{s.body}</p></li>
              ))}
            </ul>
            <div className="codeblock" role="figure" aria-label="The database policy that isolates workspaces">
              <pre>
                <span className="c">-- Applied to every workspace table. The app sets the tenant from the signed token.</span>{"\n"}
                <span className="k2">ALTER TABLE</span> issues <span className="k2">FORCE ROW LEVEL SECURITY</span>;{"\n"}
                <span className="k2">CREATE POLICY</span> tenant_isolation <span className="k2">ON</span> issues{"\n"}
                {"  "}<span className="k2">USING</span> (tenant_id = app_tenant_id()){"\n"}
                {"  "}<span className="k2">WITH CHECK</span> (tenant_id = app_tenant_id());
              </pre>
            </div>
            <div className="ctas"><Link className="btn" to="/security">Read the security overview</Link></div>
          </div>
        </section>

        <section className="block" id="pricing">
          <div className="wrap">
            <SectionHead chip="Pricing" title={<>Simple prices.<br />No surprises.</>} intro="Free for up to 10 people. After that, one price per person. Move the slider to see what your team would pay." />
            <Pricing />
          </div>
        </section>

        <section className="block" id="faq">
          <div className="wrap">
            <SectionHead chip="Questions" title="Before you ask." />
            <div className="faq">
              {FAQ.map((f, i) => (
                <details key={f.q} open={i === 0}>
                  <summary>{f.q}</summary>
                  <div className="a">{f.a}</div>
                </details>
              ))}
            </div>
          </div>
        </section>

        <section className="final" aria-labelledby="landing-final">
          <div className="wrap">
            <div className="final-box">
              <h2 id="landing-final">Your next sprint starts here.</h2>
              <p>Set up a workspace, invite your team and draw your first workflow. It&apos;s free for up to 10 people.</p>
              <div className="ctas">
                {signedIn ? <Link className="btn dark" to="/">Open your workspace</Link> : <Link className="btn dark" to="/register">Create your workspace</Link>}
                <Link className="btn alt" to="/features">See every feature</Link>
              </div>
              <svg className="big-star" viewBox="0 0 200 200" aria-hidden>
                <path d="M100 6l20 62 64-8-50 42 34 56-58-26-46 50 4-66L6 92l64-18z" fill="var(--yellow)" stroke="#121016" strokeWidth="6" strokeLinejoin="round" />
              </svg>
            </div>
          </div>
        </section>
      </main>

      <footer>
        <div className="wrap">
          <Link className="logo" to="/"><Mark size={14} />{PRODUCT_NAME}</Link>
          <nav aria-label="Site">
            <Link to="/features">Features</Link>
            <Link to="/pricing">Pricing</Link>
            <Link to="/security">Security</Link>
            {signedIn ? <Link to="/">Open your workspace</Link> : <Link to="/login">Sign in</Link>}
          </nav>
          <span>© {new Date().getFullYear()} {PRODUCT_NAME}. Preview release.</span>
        </div>
      </footer>
    </div>
  );
}
