import { useMemo, useState } from "react";

import { EXAMPLE_QUERIES, SAMPLE_ISSUES } from "./data";
import { ME, QUERY_FIELDS, QueryError, runQuery } from "./query";

const person = (who: string | null) => (who === null ? null : who === ME ? "You" : who[0]!.toUpperCase() + who.slice(1));

/** Type a query and see the example issues filter as you type. */
export function QueryPlayground() {
  const [query, setQuery] = useState(EXAMPLE_QUERIES[0]!);
  const outcome = useMemo(() => {
    try {
      return { rows: runQuery(query, SAMPLE_ISSUES), error: null };
    } catch (error) {
      if (!(error instanceof QueryError)) throw error;
      return { rows: null, error: error.message + (error.at !== undefined ? ` (at character ${error.at + 1})` : "") };
    }
  }, [query]);
  // A query with a mistake keeps showing the last good result.
  const [shown, setShown] = useState(SAMPLE_ISSUES);
  const rows = outcome.rows ?? shown;
  if (outcome.rows && outcome.rows !== shown) setShown(outcome.rows);

  return (
    <div className="ql">
      <div className="ql-editor">
        <label htmlFor="landing-query">Query</label>
        <textarea id="landing-query" className="ql-input" spellCheck={false} autoComplete="off" value={query} onChange={(e) => setQuery(e.target.value)} />
        <div className={`ql-status ${outcome.error ? "err" : "ok"}`} role="status">
          {outcome.error ?? (query.trim() ? "Query OK" : "Showing every issue")}
        </div>
        <div className="ql-chips" role="group" aria-label="Example queries">
          {EXAMPLE_QUERIES.map((q) => (
            <button key={q} type="button" onClick={() => setQuery(q)}>{q}</button>
          ))}
        </div>
        <p className="ql-help">
          <b>Fields</b> {QUERY_FIELDS.join(", ")}<br />
          <b>Operators</b> = != &gt; &lt; &gt;= &lt;= ~ (contains) IN (…) IS EMPTY<br />
          <b>Join</b> AND, OR, NOT and (brackets)
        </p>
      </div>
      <div className="results">
        <div className="results-head"><span>Matching example issues</span><output aria-label="Matching issues">{rows.length}</output></div>
        <div className="results-scroll">
          <table>
            <thead><tr><th>Key</th><th>Summary</th><th>Status</th><th>Assignee</th><th>Pts</th></tr></thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.key}>
                  <td className="mono"><b>{r.key}</b></td>
                  <td className="s">{r.summary}</td>
                  <td><span className={`pill ${r.statusCategory}`}>{r.status}</span></td>
                  <td>{person(r.assignee) ?? <span style={{ color: "var(--muted)" }}>Unassigned</span>}</td>
                  <td className="mono">{r.points ?? "–"}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {rows.length === 0 && <div className="empty">No issues match. Loosen a condition.</div>}
        </div>
      </div>
    </div>
  );
}
