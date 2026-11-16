/**
 * Editing help for MQL, the issue query language. The server parses and validates every
 * query; this only suggests what could come next at the caret and turns basic filters
 * into a starting query.
 */

export interface Suggestion {
  label: string;
  /** Text that replaces the word being typed. */
  insert: string;
  detail?: string;
}

export interface Suggestions {
  /** Character range in the query that a chosen suggestion replaces. */
  from: number;
  to: number;
  items: Suggestion[];
  /** The word being typed (without an opening quote). */
  partial: string;
  /** When values are being suggested: for which field (lower-case canonical name). */
  valuesFor?: string;
}

/** What the editor knows about the workspace, for value suggestions. */
export interface SuggestionData {
  projectKeys: string[];
  /** People matching the word being typed (email first, so it is unambiguous). */
  people: { name: string; email: string }[];
}

/** Which operators a field takes. `optional` fields can also be EMPTY. */
type Kind = "list" | "date" | "optional" | "text" | "priority" | "number";

const FIELDS: { name: string; kind: Kind; detail: string }[] = [
  { name: "project", kind: "list", detail: "Project key" },
  { name: "status", kind: "list", detail: "Workflow status" },
  { name: "statusCategory", kind: "list", detail: "To Do, In Progress or Done" },
  { name: "assignee", kind: "optional", detail: "Person, currentUser() or EMPTY" },
  { name: "reporter", kind: "optional", detail: "Person or currentUser()" },
  { name: "type", kind: "list", detail: "Issue type" },
  { name: "priority", kind: "priority", detail: "Lowest … Highest" },
  { name: "labels", kind: "optional", detail: "Label" },
  { name: "text", kind: "text", detail: "Summary, description and comments" },
  { name: "summary", kind: "text", detail: "Words in the summary" },
  { name: "description", kind: "text", detail: "Words in the description" },
  { name: "comment", kind: "text", detail: "Words in comments" },
  { name: "created", kind: "date", detail: "When it was created" },
  { name: "updated", kind: "date", detail: "When it last changed" },
  { name: "resolved", kind: "date", detail: "When it was resolved" },
  { name: "key", kind: "list", detail: "Issue key, e.g. PAY-12" },
  { name: "parent", kind: "optional", detail: "Parent issue key, e.g. PAY-3" },
  { name: "component", kind: "optional", detail: "Component name" },
  { name: "fixVersion", kind: "optional", detail: "Fix version name" },
  { name: "due", kind: "date", detail: "Due date" },
  { name: "storyPoints", kind: "number", detail: "Estimate in story points" },
];

const ALIASES: Record<string, string> = {
  issue: "key",
  issuekey: "key",
  issuetype: "type",
  category: "statuscategory",
  label: "labels",
  comments: "comment",
  components: "component",
  fixversions: "fixversion",
  duedate: "due",
  points: "storypoints",
};

const OPERATORS: Record<Kind, string[]> = {
  list: ["=", "!=", "IN", "NOT IN"],
  priority: ["=", "!=", ">=", "<=", ">", "<", "IN", "NOT IN"],
  optional: ["=", "!=", "IN", "NOT IN", "IS EMPTY", "IS NOT EMPTY"],
  text: ["~", "!~"],
  date: [">=", "<", ">", "<=", "=", "IS EMPTY", "IS NOT EMPTY"],
  number: ["=", "!=", ">=", "<=", ">", "<", "IS EMPTY", "IS NOT EMPTY"],
};

const SORT_FIELDS = ["created", "updated", "resolved", "due", "priority", "key"];
const DATE_VALUES: Suggestion[] = [
  { label: "-1d", insert: "-1d", detail: "A day ago" },
  { label: "-7d", insert: "-7d", detail: "A week ago" },
  { label: "-30d", insert: "-30d", detail: "30 days ago" },
  { label: "startOfDay()", insert: "startOfDay()", detail: "Today, 00:00 UTC" },
  { label: "startOfWeek()", insert: "startOfWeek()" },
  { label: "startOfMonth()", insert: "startOfMonth()" },
  { label: "now()", insert: "now()" },
];

const canonical = (name: string) => ALIASES[name.toLowerCase()] ?? name.toLowerCase();

function fieldKind(name: string): Kind | undefined {
  return FIELDS.find((f) => f.name.toLowerCase() === canonical(name))?.kind;
}

function quote(value: string): string {
  return /^[A-Za-z0-9_.@+-]+$/.test(value) && !/^(and|or|not|in|is|empty|null|order|by|asc|desc)$/i.test(value)
    ? value
    : `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

function values(field: string, data: SuggestionData): Suggestion[] {
  switch (canonical(field)) {
    case "project":
      return data.projectKeys.map((k) => ({ label: k, insert: k }));
    case "status":
      return ["To Do", "In Progress", "In Review", "Done"].map((s) => ({ label: s, insert: quote(s) }));
    case "statuscategory":
      return ["To Do", "In Progress", "Done"].map((s) => ({ label: s, insert: quote(s) }));
    case "type":
      return ["Story", "Bug", "Task", "Epic", "Subtask"].map((s) => ({ label: s, insert: s }));
    case "priority":
      return ["Highest", "High", "Medium", "Low", "Lowest"].map((s) => ({ label: s, insert: s }));
    case "assignee":
    case "reporter":
      return [
        { label: "currentUser()", insert: "currentUser()", detail: "You" },
        ...data.people.map((p) => ({ label: p.name, insert: quote(p.email), detail: p.email })),
      ];
    case "created":
    case "updated":
    case "resolved":
      return DATE_VALUES;
    case "due":
      return [{ label: "now()", insert: "now()", detail: "Overdue when before this" }, { label: "7d", insert: "7d", detail: "A week from now" }, ...DATE_VALUES];
    default:
      return [];
  }
}

interface Tok {
  text: string;
  start: number;
  end: number;
  quoted: boolean;
}

/** Same token boundaries as the server's lexer, tolerant of unfinished input. */
export function tokenize(text: string): Tok[] {
  const out: Tok[] = [];
  const re = /\s+|"(?:\\.|[^"\\])*"?|'(?:\\.|[^'\\])*'?|!=|!~|<=|>=|[()=,<>~!]|[^\s()=,!<>~"']+/g;
  for (const m of text.matchAll(re)) {
    if (/^\s/.test(m[0])) continue;
    const start = m.index ?? 0;
    out.push({ text: m[0], start, end: start + m[0].length, quoted: m[0].startsWith('"') || m[0].startsWith("'") });
  }
  return out;
}

const word = (t: Tok | undefined, w: string) => Boolean(t && !t.quoted && t.text.toLowerCase() === w);

/** What may come next at `caret`. */
export function suggest(text: string, caret: number, data: SuggestionData): Suggestions {
  const tokens = tokenize(text.slice(0, caret));
  // The word being typed, if the caret is at the end of one.
  const last = tokens.at(-1);
  const typing = last && last.end === caret && !/^[()=,<>~!]/.test(last.text) && !(last.quoted && last.text.length > 1 && last.text.at(-1) === last.text[0]);
  const before = typing ? tokens.slice(0, -1) : tokens;
  const partial = typing ? last.text.replace(/^["']/, "") : "";
  const from = typing ? last.start : caret;

  const finish = (items: Suggestion[], valuesFor?: string): Suggestions => {
    const p = partial.toLowerCase();
    const seen = new Set<string>();
    const matching = items.filter((i) => {
      if (seen.has(i.label) || (p && !i.label.toLowerCase().startsWith(p) && !i.insert.toLowerCase().startsWith(p) && !(i.detail ?? "").toLowerCase().startsWith(p))) {
        return false;
      }
      seen.add(i.label);
      return true;
    });
    // Nothing to offer for exactly what is already typed.
    const useful = matching.length === 1 && matching[0]?.insert.toLowerCase() === partial.toLowerCase() ? [] : matching;
    return { from, to: caret, items: useful.slice(0, 8), partial, valuesFor };
  };
  const fieldItems = (): Suggestion[] => FIELDS.map((f) => ({ label: f.name, insert: f.name, detail: f.detail }));

  // ORDER BY …
  const orderAt = before.findIndex((t, i) => word(t, "order") && word(before[i + 1], "by"));
  if (orderAt >= 0) {
    const tail = before.slice(orderAt + 2);
    const prev = tail.at(-1);
    if (!prev || prev.text === ",") return finish(SORT_FIELDS.map((f) => ({ label: f, insert: f })));
    if (SORT_FIELDS.includes(prev.text.toLowerCase())) return finish([{ label: "DESC", insert: "DESC" }, { label: "ASC", insert: "ASC" }]);
    return finish([]);
  }

  const prev = before.at(-1);
  const startOfClause = !prev || prev.text === "(" || word(prev, "and") || word(prev, "or") || word(prev, "not");
  if (startOfClause) {
    const fields = fieldItems();
    // At the very start, sorting is offered among the common fields rather than after all of them.
    const order: Suggestion[] = !prev ? [{ label: "ORDER BY", insert: "ORDER BY", detail: "Sort the results" }] : [];
    return finish([...fields.slice(0, 7), ...order, ...fields.slice(7), { label: "NOT", insert: "NOT" }]);
  }

  // Walk back to the clause's field: [field, op…, value…]
  let i = before.length - 1;
  for (; i > 0; i--) {
    const t = before[i];
    if (t && !t.quoted && fieldKind(t.text) && isClauseStart(before, i)) break;
  }
  const field = before[i];
  if (!field || field.quoted) return finish([]);
  const kind = fieldKind(field.text);
  if (!kind) return finish([]);
  const rest = before.slice(i + 1).map((t) => (t.quoted ? t.text : t.text.toUpperCase()));

  if (rest.length === 0) return finish(OPERATORS[kind].map((o) => ({ label: o, insert: o })));
  const op = rest[0] === "NOT" || rest[0] === "IS" ? `${rest[0]} ${rest[1] ?? ""}`.trim() : (rest[0] ?? "");
  if (rest[0] === "NOT" && rest.length === 1) return finish([{ label: "IN", insert: "IN" }]);
  if (rest[0] === "IS") {
    if (rest.length === 1) return finish([{ label: "EMPTY", insert: "EMPTY" }, { label: "NOT EMPTY", insert: "NOT EMPTY" }]);
    if (rest.length === 2 && rest[1] === "NOT") return finish([{ label: "EMPTY", insert: "EMPTY" }]);
    return finish(connectives());
  }
  const isList = op === "IN" || op === "NOT IN";
  const afterOp = rest.slice(op.split(" ").length);
  if (isList) {
    const open = afterOp.lastIndexOf("(");
    const closed = afterOp.lastIndexOf(")") > open;
    if (open < 0) return finish([{ label: "(", insert: "(" }]);
    if (closed) return finish(connectives());
    const lastInList = afterOp.at(-1);
    if (lastInList === "(" || lastInList === ",") return finish(values(field.text, data), canonical(field.text));
    return finish([]);
  }
  if (afterOp.length === 0) return finish(values(field.text, data), canonical(field.text));
  return finish(connectives());
}

function connectives(): Suggestion[] {
  return [
    { label: "AND", insert: "AND" },
    { label: "OR", insert: "OR" },
    { label: "ORDER BY", insert: "ORDER BY" },
  ];
}

/** Whether `tokens[i]` is where a clause starts (so a value that is also a field name is not mistaken for one). */
function isClauseStart(tokens: Tok[], i: number): boolean {
  const prev = tokens[i - 1];
  return !prev || prev.text === "(" || word(prev, "and") || word(prev, "or") || word(prev, "not");
}

/** Apply a suggestion: the new text and where the caret goes. */
export function applySuggestion(text: string, s: Suggestions, item: Suggestion): { text: string; caret: number } {
  // The rest of a word the caret is inside is replaced too.
  const rest = text.slice(s.to).replace(/^[^\s()=,!<>~]*/, "");
  const spaced = rest.startsWith(" ");
  const glue = item.insert === "(" || spaced ? "" : " ";
  const next = text.slice(0, s.from) + item.insert + glue + rest;
  return { text: next, caret: s.from + item.insert.length + (glue || spaced ? 1 : 0) };
}

/** The basic search's filters as a query to start from. */
export function basicToMql(basic: { q?: string; projectKey?: string; statusCategory?: string; mine?: boolean }): string {
  const parts: string[] = [];
  if (basic.projectKey) parts.push(`project = ${basic.projectKey}`);
  if (basic.statusCategory) {
    const label = { todo: "To Do", in_progress: "In Progress", done: "Done" }[basic.statusCategory] ?? basic.statusCategory;
    parts.push(`statusCategory = ${quote(label)}`);
  }
  if (basic.mine) parts.push("assignee = currentUser()");
  if (basic.q?.trim()) parts.push(`text ~ ${quote(basic.q.trim())}`);
  return parts.length ? `${parts.join(" AND ")} ORDER BY updated DESC` : "ORDER BY updated DESC";
}

export const MQL_EXAMPLES: { query: string; meaning: string }[] = [
  { query: "assignee = currentUser() AND statusCategory != Done", meaning: "My open issues" },
  { query: "project = PAY AND type = Bug AND priority >= High", meaning: "Important bugs in one project" },
  { query: "created >= -7d ORDER BY created DESC", meaning: "Created in the last week, newest first" },
  { query: 'text ~ "refund" AND labels IN (backend, api)', meaning: "Words anywhere, with either label" },
  { query: "resolved >= startOfMonth() ORDER BY resolved DESC", meaning: "Resolved this month" },
  { query: "assignee IS EMPTY AND statusCategory = \"To Do\"", meaning: "Unassigned work not started" },
];
