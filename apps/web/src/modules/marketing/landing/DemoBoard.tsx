import { useRef, useState, type DragEvent } from "react";

import { DEMO_CARDS, DEMO_COLUMNS, type DemoCard, type DemoColumn } from "./data";

const EPIC_COLOR: Record<DemoCard["epic"], string> = { Checkout: "var(--pink)", Payouts: "var(--mint)", Fraud: "var(--yellow)" };
const ORDER = DEMO_COLUMNS.map((c) => c.id);

/** A sample sprint board visitors can play with: drag cards, or use the arrow buttons. */
export function DemoBoard() {
  const [cards, setCards] = useState(DEMO_CARDS);
  const [over, setOver] = useState<DemoColumn | null>(null);
  const [stamp, setStamp] = useState<{ id: number; left: number; top: number } | null>(null);
  const frame = useRef<HTMLDivElement>(null);
  const dragged = useRef<string | null>(null);

  const move = (key: string, column: DemoColumn, from?: HTMLElement | null) => {
    setCards((all) => all.map((c) => (c.key === key ? { ...c, column } : c)));
    if (column === "done" && frame.current && from && !window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) {
      const box = frame.current.getBoundingClientRect();
      const target = frame.current.querySelector<HTMLElement>(`[data-col="done"]`)?.getBoundingClientRect();
      if (target) setStamp({ id: Date.now(), left: target.left - box.left + 12, top: target.top - box.top + 40 });
    }
  };

  const total = cards.reduce((n, c) => n + c.points, 0);
  const done = cards.filter((c) => c.column === "done").reduce((n, c) => n + c.points, 0);
  const doing = cards.filter((c) => c.column === "doing").length;
  const limit = DEMO_COLUMNS.find((c) => c.id === "doing")?.limit ?? Infinity;

  const onDrop = (column: DemoColumn) => (event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setOver(null);
    if (dragged.current) move(dragged.current, column, event.currentTarget);
    dragged.current = null;
  };

  return (
    <div className="demo" ref={frame} role="group" aria-label="Example sprint board">
      <span className="stamp" aria-hidden>DRAG<br />ME</span>
      <div className="demo-bar">
        <div className="dots" aria-hidden><i /><i /><i /></div>
        <span className="demo-title">PAY · Sprint 14 (example)</span>
        <span className="demo-sprint">6 days left</span>
      </div>
      <div className="board">
        {DEMO_COLUMNS.map((column) => {
          const mine = cards.filter((c) => c.column === column.id);
          const overLimit = column.limit !== undefined && mine.length > column.limit;
          return (
            <section
              key={column.id}
              data-col={column.id}
              aria-label={column.name}
              className={["col", column.id, overLimit && "over", over === column.id && "drop"].filter(Boolean).join(" ")}
              onDragOver={(e) => {
                e.preventDefault();
                setOver(column.id);
              }}
              onDragLeave={(e) => {
                if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(null);
              }}
              onDrop={onDrop(column.id)}
            >
              <h3>
                {column.name}
                {column.limit !== undefined && <span className="limit">MAX {column.limit}</span>}
                <span className="count">{mine.length}</span>
              </h3>
              <div className="list">
                {mine.map((card) => {
                  const at = ORDER.indexOf(card.column);
                  return (
                    <article
                      key={card.key}
                      className="card"
                      draggable
                      aria-label={`${card.key}: ${card.summary}`}
                      onDragStart={(e) => {
                        dragged.current = card.key;
                        e.currentTarget.classList.add("dragging");
                        e.dataTransfer.effectAllowed = "move";
                      }}
                      onDragEnd={(e) => e.currentTarget.classList.remove("dragging")}
                    >
                      <div className="sum">{card.summary}</div>
                      <div className="row">
                        <span className="key">{card.key}</span>
                        <span className="epic" style={{ background: EPIC_COLOR[card.epic] }}>{card.epic}</span>
                        <span className="pts" title="Story points">{card.points}</span>
                      </div>
                      <div className="row">
                        <button type="button" className="mv" disabled={at === 0} aria-label={`Move ${card.key} left`} onClick={(e) => move(card.key, ORDER[at - 1]!, e.currentTarget)}>◀</button>
                        <button type="button" className="mv" disabled={at === ORDER.length - 1} aria-label={`Move ${card.key} right`} onClick={(e) => move(card.key, ORDER[at + 1]!, e.currentTarget)}>▶</button>
                      </div>
                    </article>
                  );
                })}
              </div>
            </section>
          );
        })}
      </div>
      <div className="demo-foot">
        <span>Sprint progress: <output>{done} of {total} pts done</output></span>
        <span role="status">{doing > limit ? `In progress is over its limit of ${limit}. Finish something first.` : done === total ? "Sprint cleared." : ""}</span>
      </div>
      {stamp && (
        <div key={stamp.id} className="shipped" style={{ left: stamp.left, top: stamp.top }} onAnimationEnd={() => setStamp(null)} aria-hidden>
          SHIPPED
        </div>
      )}
    </div>
  );
}
