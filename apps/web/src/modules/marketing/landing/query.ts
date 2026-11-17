// A small, self-contained version of the query language for the landing page's playground.
// It runs in the browser on example issues only; the real language lives in search-service.

export interface SampleIssue {
  key: string;
  summary: string;
  type: string;
  status: string;
  statusCategory: "todo" | "in_progress" | "done";
  assignee: string | null;
  priority: "lowest" | "low" | "medium" | "high" | "highest";
  points: number | null;
  labels: string[];
  epic: string | null;
}

/** Who `currentUser()` stands for in the playground. */
export const ME = "you";

export const QUERY_FIELDS = ["key", "summary", "type", "status", "statusCategory", "assignee", "priority", "points", "labels", "epic"] as const;
type Field = (typeof QUERY_FIELDS)[number];
const BY_LOWER = new Map<string, Field>(QUERY_FIELDS.map((f) => [f.toLowerCase(), f]));
const PRIORITY: Record<string, number> = { lowest: 1, low: 2, medium: 3, high: 4, highest: 5 };

export class QueryError extends Error {
  constructor(
    message: string,
    /** Character offset of the mistake, when there is one. */
    readonly at?: number,
  ) {
    super(message);
  }
}

type Token =
  | { t: "(" | ")" | "," | "AND" | "OR" | "NOT" | "IN" | "IS" | "EMPTY"; v: string; at: number }
  | { t: "op"; v: string; at: number }
  | { t: "val"; v: string; at: number };

type Node =
  | { k: "and" | "or"; a: Node; b: Node }
  | { k: "not"; a: Node }
  | { k: "empty"; field: Field; neg: boolean }
  | { k: "in"; field: Field; list: string[]; neg: boolean }
  | { k: "cmp"; field: Field; op: string; val: string };

const KEYWORDS = new Set(["AND", "OR", "NOT", "IN", "IS", "EMPTY"]);

function lex(src: string): Token[] {
  const out: Token[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i]!;
    if (/\s/.test(c)) {
      i++;
      continue;
    }
    if (c === "(" || c === ")" || c === ",") {
      out.push({ t: c, v: c, at: i });
      i++;
      continue;
    }
    const op = /^(!=|>=|<=|=|>|<|~)/.exec(src.slice(i));
    if (op) {
      out.push({ t: "op", v: op[1]!, at: i });
      i += op[1]!.length;
      continue;
    }
    if (c === '"' || c === "'") {
      const end = src.indexOf(c, i + 1);
      if (end < 0) throw new QueryError("This quote is never closed.", i);
      out.push({ t: "val", v: src.slice(i + 1, end), at: i });
      i = end + 1;
      continue;
    }
    const word = /^[A-Za-z0-9_.\-$]+(\(\))?/.exec(src.slice(i))?.[0];
    if (!word) throw new QueryError(`"${c}" isn't something a query can contain.`, i);
    const upper = word.toUpperCase();
    out.push(KEYWORDS.has(upper) ? ({ t: upper, v: word, at: i } as Token) : { t: "val", v: word, at: i });
    i += word.length;
  }
  return out;
}

/** Parse a query; `null` for an empty one (every issue). Throws `QueryError` with a readable reason. */
export function parseQuery(src: string): Node | null {
  const toks = lex(src);
  let p = 0;
  const peek = () => toks[p];
  const want = <T extends Token["t"]>(t: T, what: string): Extract<Token, { t: T }> => {
    const tok = toks[p];
    if (!tok || tok.t !== t) throw new QueryError(`Expected ${what}${tok ? ` before "${tok.v}"` : " at the end"}.`, tok ? tok.at : src.length);
    p++;
    return tok as Extract<Token, { t: T }>;
  };
  const value = () => {
    const tok = want("val", "a value");
    return /^currentuser\(\)$/i.test(tok.v) ? ME : tok.v;
  };
  const clause = (): Node => {
    const f = want("val", "a field name");
    const field = BY_LOWER.get(f.v.toLowerCase());
    if (!field) throw new QueryError(`There's no field called "${f.v}". Try one of: ${QUERY_FIELDS.join(", ")}.`, f.at);
    const t = peek();
    if (t?.t === "IS") {
      p++;
      const neg = peek()?.t === "NOT";
      if (neg) p++;
      want("EMPTY", "EMPTY");
      return { k: "empty", field, neg };
    }
    if (t?.t === "IN" || (t?.t === "NOT" && toks[p + 1]?.t === "IN")) {
      const neg = t.t === "NOT";
      p += neg ? 2 : 1;
      want("(", 'a "(" after IN');
      const list = [value()];
      while (peek()?.t === ",") {
        p++;
        list.push(value());
      }
      want(")", 'a closing ")"');
      return { k: "in", field, list, neg };
    }
    const op = want("op", "an operator like = or !=");
    return { k: "cmp", field, op: op.v, val: value() };
  };
  const not = (): Node => {
    if (peek()?.t === "NOT") {
      p++;
      return { k: "not", a: not() };
    }
    if (peek()?.t === "(") {
      p++;
      const e = or();
      want(")", 'a closing ")"');
      return e;
    }
    return clause();
  };
  const and = (): Node => {
    let left = not();
    while (peek()?.t === "AND") {
      p++;
      left = { k: "and", a: left, b: not() };
    }
    return left;
  };
  const or = (): Node => {
    let left = and();
    while (peek()?.t === "OR") {
      p++;
      left = { k: "or", a: left, b: and() };
    }
    return left;
  };
  if (toks.length === 0) return null;
  const tree = or();
  const extra = toks[p];
  if (extra) throw new QueryError(`Didn't expect "${extra.v}" here. Join conditions with AND or OR.`, extra.at);
  return tree;
}

const lower = (v: unknown) => (v === null || v === undefined ? v : String(v).toLowerCase());

function compare(issue: SampleIssue, field: Field, op: string, val: string): boolean {
  const raw = issue[field];
  if (op === "~") {
    const hay = Array.isArray(raw) ? raw.join(" ") : (raw ?? "");
    return String(hay).toLowerCase().includes(val.toLowerCase());
  }
  if (field === "labels") {
    const has = issue.labels.some((l) => l.toLowerCase() === val.toLowerCase());
    if (op === "=") return has;
    if (op === "!=") return !has;
    throw new QueryError("Labels can be compared with =, !=, ~ or IN.");
  }
  if ([">", "<", ">=", "<="].includes(op)) {
    let a: number | null | undefined;
    let b: number | undefined;
    if (field === "points") {
      a = issue.points;
      b = Number(val);
      if (Number.isNaN(b)) throw new QueryError(`Points are numbers, so "${val}" can't be compared.`);
    } else if (field === "priority") {
      a = PRIORITY[issue.priority];
      b = PRIORITY[val.toLowerCase()];
      if (!b) throw new QueryError(`"${val}" isn't a priority. Use lowest, low, medium, high or highest.`);
    } else {
      throw new QueryError(`Only points and priority can use ${op}.`);
    }
    if (a === null || a === undefined) return false;
    return op === ">" ? a > b : op === "<" ? a < b : op === ">=" ? a >= b : a <= b;
  }
  const want = field === "statusCategory" ? val.toLowerCase().replace(/\s+/g, "_").replace("inprogress", "in_progress") : val.toLowerCase();
  const eq = field === "points" ? raw === Number(val) : lower(raw) === want;
  return op === "=" ? eq : !eq;
}

function matches(node: Node, issue: SampleIssue): boolean {
  switch (node.k) {
    case "or":
      return matches(node.a, issue) || matches(node.b, issue);
    case "and":
      return matches(node.a, issue) && matches(node.b, issue);
    case "not":
      return !matches(node.a, issue);
    case "empty": {
      const v = issue[node.field];
      const empty = v === null || v === undefined || (Array.isArray(v) && v.length === 0);
      return node.neg ? !empty : empty;
    }
    case "in": {
      const hit = node.list.some((v) => compare(issue, node.field, "=", v));
      return node.neg ? !hit : hit;
    }
    case "cmp":
      return compare(issue, node.field, node.op, node.val);
  }
}

/** The issues a query keeps. Throws `QueryError` for a query that can't run. */
export function runQuery(src: string, issues: SampleIssue[]): SampleIssue[] {
  const tree = parseQuery(src);
  return tree ? issues.filter((i) => matches(tree, i)) : issues;
}
