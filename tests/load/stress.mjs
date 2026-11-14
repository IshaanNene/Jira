// Stress test with autocannon: finds where reads and writes saturate (tests/load/platform.js, k6,
// checks thresholds at a steady rate instead).
//   cd tests/load && npm install --no-save autocannon@8.0.0
//   node stress.mjs http://localhost:8088 reads 250,500,1000,2000,max 30
//   FILTER=/search/ node stress.mjs http://localhost:8088 reads max 15    # one endpoint only
// Each stage signs in afresh, holds the target overall rate (or 200 connections for "max") and prints
// one JSON line. The gateway allows 200 requests/s per pod; raise gateway.rateLimit for a stress run.
// READ_PROJECT (default WEB) needs issues and a board; WRITE_PROJECT (default LOAD) receives the
// created issues and comments and needs at least one issue already.
import autocannon from "autocannon";

const [base, kind, rates, seconds = "30"] = process.argv.slice(2);
const API = `${base}/api/v1`;
const email = process.env.LOAD_EMAIL ?? "olivia@demo.example";
const password = process.env.LOAD_PASSWORD ?? "demo-password-123"; // seed script default
const READ = process.env.READ_PROJECT ?? "WEB";
const WRITE = process.env.WRITE_PROJECT ?? "LOAD";

async function call(method, path, token, body) {
  const r = await fetch(API + path, {
    method,
    headers: { "content-type": "application/json", ...(token && { authorization: `Bearer ${token}` }) },
    body: body && JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${await r.text()}`);
  return r.status === 204 ? null : r.json();
}

async function session() {
  const { access_token: account } = await call("POST", "/auth/login", null, { email, password });
  const org = (await call("GET", "/organizations", account)).items[0];
  const { access_token } = await call("POST", "/auth/token/exchange", account, { organization_id: org.id });
  return access_token;
}

async function requests(token) {
  const json = { "content-type": "application/json", authorization: `Bearer ${token}` };
  if (kind === "writes") {
    const loadKeys = (await call("GET", `/issues?project=${WRITE}&limit=50`, token)).items.map((i) => i.key);
    const body = (t) => ({ type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: t }] }] });
    const list = [];
    for (let i = 0; i < 10; i++) {
      list.push({ method: "POST", path: "/api/v1/issues", headers: json, body: JSON.stringify({ project_key: WRITE, type: "task", summary: `autocannon issue ${i}` }) });
      list.push({ method: "POST", path: `/api/v1/issues/${loadKeys[i % loadKeys.length]}/comments`, headers: json, body: JSON.stringify({ body: body("autocannon comment") }) });
    }
    return list;
  }
  // Reads: the same mix as tests/load/platform.js, laid out as a 20-request cycle.
  const issues = (await call("GET", `/issues?project=${READ}&limit=50`, token)).items.map((i) => i.key);
  const board = (await call("GET", `/boards?project_key=${READ}`, token)).items[0].id;
  const project = (await call("GET", `/projects/${READ}`, token)).id;
  const get = (path) => ({ method: "GET", path: `/api/v1${path}`, headers: json });
  const mix = [
    ...Array(7).fill(0).map(() => get(`/issues?project=${READ}&limit=50`)),
    ...Array(5).fill(0).map((_, i) => get(`/issues/${issues[i % issues.length]}`)),
    ...Array(3).fill(0).map(() => get(`/boards/${board}/view`)),
    ...Array(2).fill(0).map(() => get("/notifications/unread-count")),
    ...["checkout", "search"].map((q) => get(`/search/issues?q=${q}`)),
    { method: "POST", path: "/api/v1/analytics/query", headers: json, body: JSON.stringify({ kind: "breakdown", projects: [project], group_by: "status_category" }) },
  ];
  // Interleave so each connection sees the whole mix.
  return mix.sort((a, b) => (a.path + a.method).localeCompare(b.path + b.method)).map((r, i, all) => all[(i * 7) % all.length]);
}

for (const rate of rates.split(",")) {
  const token = await session();
  const max = rate === "max";
  const result = await autocannon({
    url: base,
    connections: max ? 200 : Math.min(400, Math.max(20, Math.ceil(Number(rate) / 20))),
    ...(max ? {} : { overallRate: Number(rate) }),
    duration: Number(seconds),
    requests: (await requests(token)).filter((r) => !process.env.FILTER || r.path.includes(process.env.FILTER)),
    timeout: 10,
  });
  const by = result.statusCodeStats ?? {};
  console.log(JSON.stringify({
    kind, target: rate, seconds: Number(seconds), connections: result.connections,
    achieved: Math.round(result.requests.average), total: result.requests.total,
    p50: result.latency.p50, p90: result.latency.p90, p97_5: result.latency.p97_5, p99: result.latency.p99, max: result.latency.max,
    non2xx: result.non2xx, errors: result.errors, timeouts: result.timeouts,
    status: Object.fromEntries(Object.entries(by).map(([k, v]) => [k, v.count])),
  }));
}
