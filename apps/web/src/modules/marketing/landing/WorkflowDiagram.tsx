const DISPLAY = "Bricolage Grotesque, Arial Black, sans-serif";
const MONO = "JetBrains Mono, monospace";

const STATUSES = [
  { x: 18, name: "To do", category: "TO DO", fill: "var(--paper)", ink: "var(--text)" },
  { x: 306, name: "In progress", category: "IN PROGRESS", fill: "var(--blue)", ink: "#121016" },
  { x: 596, name: "In review", category: "IN PROGRESS", fill: "var(--blue)", ink: "#121016" },
  { x: 886, name: "Done", category: "DONE", fill: "var(--mint)", ink: "#121016" },
];

const FORWARD = [
  { x: 214, label: "Start" },
  { x: 504, label: "Submit" },
  { x: 794, label: "Approve" },
];

/** An example workflow drawn the way the designer shows one. */
export function WorkflowDiagram() {
  return (
    <div className="flow-wrap">
      <svg viewBox="0 0 1100 300" role="img" aria-label="Example workflow: To do, In progress, In review and Done, with transitions between them">
        <defs>
          <marker id="landing-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0 0L10 5L0 10z" fill="var(--ink)" />
          </marker>
        </defs>
        <rect width="1100" height="300" fill="var(--paper)" />
        <g stroke="var(--ink)" strokeWidth="3" fill="none" markerEnd="url(#landing-arrow)">
          <path d="M210 120 H 300" />
          <path d="M500 120 H 590" />
          <path d="M790 120 H 880" />
          <path d="M690 165 C 690 250, 400 250, 400 172" />
          <path d="M980 165 C 980 270, 110 270, 110 172" />
        </g>
        <g fontFamily={MONO} fontSize="13" fontWeight="700">
          {FORWARD.map((t) => (
            <g key={t.label}>
              <rect x={t.x} y="78" width="80" height="26" fill="var(--yellow)" stroke="var(--ink)" strokeWidth="2" />
              <text x={t.x + 40} y="96" textAnchor="middle" fill="#121016">{t.label}</text>
            </g>
          ))}
          <rect x="490" y="214" width="120" height="26" fill="var(--paper)" stroke="var(--ink)" strokeWidth="2" />
          <text x="550" y="232" textAnchor="middle" fill="var(--text)">Request changes</text>
          <rect x="505" y="252" width="90" height="26" fill="var(--paper)" stroke="var(--ink)" strokeWidth="2" />
          <text x="550" y="270" textAnchor="middle" fill="var(--text)">Reopen</text>
        </g>
        {STATUSES.map((s) => (
          <g key={s.name}>
            <rect x={s.x} y="80" width="194" height="88" fill={s.fill} stroke="var(--ink)" strokeWidth="3" />
            <text x={s.x + 97} y="124" textAnchor="middle" fontSize="24" fontWeight="800" fill={s.ink} fontFamily={DISPLAY}>{s.name}</text>
            <text x={s.x + 97} y="150" textAnchor="middle" fontSize="12" fill={s.ink} fontFamily={MONO}>{s.category}</text>
          </g>
        ))}
        <g fontFamily={MONO} fontSize="12" fontWeight="700">
          <g transform="rotate(-3 695 42)">
            <rect x="590" y="28" width="210" height="28" fill="var(--pink)" stroke="var(--ink)" strokeWidth="2" />
            <text x="695" y="47" textAnchor="middle" fill="#121016">needs: assignee + estimate</text>
          </g>
          <g transform="rotate(2 983 42)">
            <rect x="868" y="28" width="230" height="28" fill="var(--tomato)" stroke="var(--ink)" strokeWidth="2" />
            <text x="983" y="47" textAnchor="middle" fill="#121016">then: set resolution, notify</text>
          </g>
        </g>
      </svg>
    </div>
  );
}
