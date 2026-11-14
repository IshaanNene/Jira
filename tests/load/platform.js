// Load test through the edge gateway, as the web app uses the API.
//   k6 run tests/load/platform.js            (or: make load)
// Env: BASE_URL (default http://localhost:8080), SEED_EMAIL / SEED_PASSWORD (the seed
// script's demo owner), READ_RPS, WRITE_RPS, DURATION.
//
// Reads and writes run as open models (constant arrival rate), so a slow server shows up as
// latency and dropped iterations instead of quietly lowering the load.
import http from "k6/http";
import { check, fail } from "k6";
import { Trend } from "k6/metrics";

const BASE = __ENV.BASE_URL || "http://localhost:8080";
const API = `${BASE}/api/v1`;
const DURATION = __ENV.DURATION || "2m";
const READ_RPS = Number(__ENV.READ_RPS || 100);
const WRITE_RPS = Number(__ENV.WRITE_RPS || 10);

const readLatency = new Trend("read_latency", true);
const writeLatency = new Trend("write_latency", true);

export const options = {
  scenarios: {
    reads: { executor: "constant-arrival-rate", exec: "reads", rate: READ_RPS, timeUnit: "1s", duration: DURATION, preAllocatedVUs: 50, maxVUs: 200 },
    writes: { executor: "constant-arrival-rate", exec: "writes", rate: WRITE_RPS, timeUnit: "1s", duration: DURATION, preAllocatedVUs: 10, maxVUs: 50 },
  },
  // The design targets: p95 200 ms for reads, 300 ms for writes, under 1 % errors.
  thresholds: {
    read_latency: ["p(95)<200"],
    write_latency: ["p(95)<300"],
    http_req_failed: ["rate<0.01"],
    dropped_iterations: ["count<10"],
  },
  summaryTrendStats: ["avg", "med", "p(90)", "p(95)", "p(99)", "max"],
};

const json = (token) => ({ headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" } });

export function setup() {
  const email = __ENV.SEED_EMAIL || "olivia@demo.example";
  const password = __ENV.SEED_PASSWORD || "demo-password-123";
  const login = http.post(`${API}/auth/login`, JSON.stringify({ email, password }), { headers: { "Content-Type": "application/json" } });
  if (login.status !== 200) fail(`sign-in failed (${login.status}): run make seed first`);
  const account = login.json("access_token");
  const orgs = http.get(`${API}/organizations`, json(account)).json("items");
  const org = orgs.find((o) => o.slug === "demo-workspace") || orgs[0];
  const token = http.post(`${API}/auth/token/exchange`, JSON.stringify({ organization_id: org.id }), json(account)).json("access_token");

  // Writes go to their own project so the demo project stays readable.
  if (http.get(`${API}/projects/LOAD`, json(token)).status === 404) {
    const created = http.post(`${API}/projects`, JSON.stringify({ name: "Load test", key: "LOAD", project_type: "kanban", access: "open", description: "Issues created by tests/load." }), json(token));
    if (created.status !== 201) fail(`could not create the LOAD project (${created.status})`);
  }
  const issues = http.get(`${API}/issues?project=DEMO&limit=50`, json(token)).json("items").map((i) => i.key);
  const boards = http.get(`${API}/boards?project_key=DEMO`, json(token)).json("items");
  const projects = http.get(`${API}/projects`, json(token)).json("items");
  return { token, issues, board: boards.length ? boards[0].id : null, project: projects.find((p) => p.key === "DEMO").id };
}

const pick = (list) => list[Math.floor(Math.random() * list.length)];

// The screens people open most, weighted roughly by how often.
export function reads(data) {
  const p = json(data.token);
  const roll = Math.random();
  let res;
  if (roll < 0.35) res = http.get(`${API}/issues?project=DEMO&limit=50`, { ...p, tags: { name: "list issues" } });
  else if (roll < 0.6) res = http.get(`${API}/issues/${pick(data.issues)}`, { ...p, tags: { name: "view issue" } });
  else if (roll < 0.75 && data.board) res = http.get(`${API}/boards/${data.board}/view`, { ...p, tags: { name: "board" } });
  else if (roll < 0.85) res = http.get(`${API}/notifications/unread-count`, { ...p, tags: { name: "unread count" } });
  else if (roll < 0.95) res = http.get(`${API}/search/issues?q=${pick(["payment", "checkout", "card", "upgrade"])}`, { ...p, tags: { name: "search" } });
  else res = http.post(`${API}/analytics/query`, JSON.stringify({ kind: "breakdown", projects: [data.project], group_by: "status_category" }), { ...p, tags: { name: "chart" } });
  check(res, { "read ok": (r) => r.status === 200 });
  readLatency.add(res.timings.duration);
}

export function writes(data) {
  const p = json(data.token);
  const created = http.post(`${API}/issues`, JSON.stringify({ project_key: "LOAD", type: "task", summary: `Load test issue ${__VU}-${__ITER}` }), { ...p, tags: { name: "create issue" } });
  check(created, { "created": (r) => r.status === 201 });
  writeLatency.add(created.timings.duration);
  if (created.status !== 201) return;
  const body = { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "Comment from the load test." }] }] };
  const comment = http.post(`${API}/issues/${created.json("key")}/comments`, JSON.stringify({ body }), { ...p, tags: { name: "comment" } });
  check(comment, { "commented": (r) => r.status === 201 });
  writeLatency.add(comment.timings.duration);
}
