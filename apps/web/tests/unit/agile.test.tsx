import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  applyCardMove,
  applyMove,
  BACKLOG,
  burndownSeries,
  changesColumn,
  columnIndex,
  formatPoints,
  inEpic,
  moveRequest,
  sprintTimeLeft,
  swimlanes,
  totalPoints,
  transitionInto,
} from "@/modules/agile/model";
import BacklogPage from "@/modules/agile/pages/BacklogPage";
import BoardPage from "@/modules/agile/pages/BoardPage";
import type { BacklogView, Board, BoardCard, BoardColumn, BoardView, IssueTransition, Sprint } from "@/services/api";

import { fakeApi, project, renderAt, signIn, STATUSES, type Call } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const card = (n: number, overrides: Partial<BoardCard> = {}): BoardCard => ({
  id: `c${n}`, key: `PAY-${n}`, type: "story", status: "todo", status_category: "todo", priority: "medium",
  summary: `Story ${n}`, labels: [], version: 1, story_points: n, sprint_id: null, ...overrides,
});

const COLUMNS: BoardColumn[] = [
  { id: "todo", name: "To Do", category: "todo", statuses: [] },
  { id: "doing", name: "In Progress", category: "in_progress", statuses: ["in_review"] },
  { id: "done", name: "Done", category: "done", statuses: [] },
];

const board = (overrides: Partial<Board> = {}): Board => ({
  id: "b1", name: "PAY board", kind: "scrum", project_id: "00000000-0000-7000-8000-0000000000b1", project_key: "PAY", version: 1,
  columns: COLUMNS,
  permissions: { manage_board: false, manage_sprints: true, move_issues: true, transition_issues: true },
  ...overrides,
});

const sprint = (overrides: Partial<Sprint> = {}): Sprint => ({
  id: "s1", board_id: "b1", name: "PAY Sprint 1", goal: "", sequence: 1, state: "future", version: 1, ...overrides,
});

const backlogView = (): BacklogView => ({
  board: board(),
  sprints: [{ sprint: sprint(), issues: [card(1, { sprint_id: "s1" }), card(2, { sprint_id: "s1" })] }],
  backlog: [card(3), card(4), card(5)],
  truncated: false,
  epics: [],
});

describe("planning moves", () => {
  it("moves issues between lists in place, keeping the order of the rest", () => {
    const next = applyMove(backlogView(), { issueIds: ["c4"], list: "s1", beforeId: "c2" });
    expect(next.sprints[0]!.issues.map((c) => c.id)).toEqual(["c1", "c4", "c2"]);
    expect(next.sprints[0]!.issues[1]!.sprint_id).toBe("s1");
    expect(next.backlog.map((c) => c.id)).toEqual(["c3", "c5"]);

    const out = applyMove(backlogView(), { issueIds: ["c1"], list: BACKLOG });
    expect(out.backlog.map((c) => c.id)).toEqual(["c3", "c4", "c5", "c1"]);
    expect(out.backlog.at(-1)!.sprint_id).toBeNull();
    // An unknown target leaves the view untouched.
    expect(applyMove(backlogView(), { issueIds: ["c1"], list: "nope" })).toEqual(backlogView());
  });

  it("asks the server for positions relative to issues that are not moving", () => {
    const view = backlogView();
    expect(moveRequest(view, { issueIds: ["c4"], list: "s1", beforeId: "c2" })).toEqual({ issue_ids: ["c4"], sprint_id: "s1", before_id: "c2" });
    // Dropped at the end of a list: after its last other issue.
    expect(moveRequest(view, { issueIds: ["c3"], list: BACKLOG })).toEqual({ issue_ids: ["c3"], sprint_id: null, after_id: "c5" });
    // "Before itself" is not a position.
    expect(moveRequest(view, { issueIds: ["c1"], list: "s1", beforeId: "c1" })).toEqual({ issue_ids: ["c1"], sprint_id: "s1", after_id: "c2" });
    // Into an empty list there is nothing to be relative to.
    const empty = { ...view, sprints: [{ sprint: sprint({ id: "s2" }), issues: [] }] };
    expect(moveRequest(empty, { issueIds: ["c3"], list: "s2" })).toEqual({ issue_ids: ["c3"], sprint_id: "s2" });
  });
});

describe("epic filter", () => {
  it("keeps one epic's cards, cards without an epic, or every card", () => {
    const inCheckout = card(1, { parent_key: "PAY-9", parent_summary: "Checkout" });
    const loose = card(2);
    expect([inCheckout, loose].filter((c) => inEpic(c, "all"))).toHaveLength(2);
    expect([inCheckout, loose].filter((c) => inEpic(c, "PAY-9"))).toEqual([inCheckout]);
    expect([inCheckout, loose].filter((c) => inEpic(c, "none"))).toEqual([loose]);
    expect([inCheckout, loose].filter((c) => inEpic(c, "PAY-10"))).toEqual([]);
  });
});

describe("swimlanes", () => {
  const epics = [
    { id: "e9", key: "PAY-9", summary: "Checkout", status_category: "in_progress", issues: 3, done: 1, points: 8, done_points: 3 },
    { id: "e8", key: "PAY-8", summary: "Empty epic", status_category: "todo", issues: 0, done: 0, points: 0, done_points: 0 },
  ];
  const cards = [
    card(1, { assignee_id: "u2" }),
    card(2, { parent_key: "PAY-7", parent_summary: "Old epic", assignee_id: "u1" }),
    card(3, { parent_key: "PAY-9", parent_summary: "Checkout", assignee_id: "u2" }),
    card(4, { parent_key: "PAY-9", parent_summary: "Checkout" }),
  ];

  it("groups by epic in the board's epic order, unlisted parents next, no epic last, empty lanes left out", () => {
    const lanes = swimlanes(cards, "epic", epics);
    expect(lanes.map((l) => [l.id, l.title, l.cards.map((c) => c.key)])).toEqual([
      ["epic:PAY-9", "Checkout", ["PAY-3", "PAY-4"]],
      ["epic:PAY-7", "Old epic", ["PAY-2"]],
      ["no-epic", undefined, ["PAY-1"]],
    ]);
  });

  it("groups by assignee as people first appear, unassigned last", () => {
    const lanes = swimlanes(cards, "assignee", epics);
    expect(lanes.map((l) => [l.id, l.cards.map((c) => c.key)])).toEqual([
      ["user:u2", ["PAY-1", "PAY-3"]],
      ["user:u1", ["PAY-2"]],
      ["unassigned", ["PAY-4"]],
    ]);
  });
});

describe("board columns", () => {
  it("places statuses by explicit mapping first, then by category", () => {
    expect(columnIndex(COLUMNS, "in_review", "done")).toBe(1);
    expect(columnIndex(COLUMNS, "qa", "in_progress")).toBe(1);
    expect(columnIndex(COLUMNS, "closed", "done")).toBe(2);
    expect(columnIndex([{ id: "x", name: "X", statuses: ["todo"] }], "other", "todo")).toBe(-1);
  });

  it("decides whether a drop is a status change from where the card was picked up", () => {
    expect(changesColumn(COLUMNS, { status: "todo", status_category: "todo" }, 0)).toBe(false);
    expect(changesColumn(COLUMNS, { status: "todo", status_category: "todo" }, 1)).toBe(true);
    expect(changesColumn(COLUMNS, { status: "in_review", status_category: "in_progress" }, 1)).toBe(false);
  });

  it("finds the workflow transition into a column", () => {
    const t = (id: string, key: string, category: string): IssueTransition =>
      ({ id, name: id, to: { key, name: key, category } }) as IssueTransition;
    const transitions = [t("start", "in_progress", "in_progress"), t("review", "in_review", "in_progress"), t("finish", "done", "done")];
    expect(transitionInto(COLUMNS, 1, transitions)?.id).toBe("start");
    expect(transitionInto(COLUMNS, 2, transitions)?.id).toBe("finish");
    expect(transitionInto(COLUMNS, 0, transitions)).toBeUndefined();
  });

  it("moves a card between columns for an instant update", () => {
    const view: BoardView = {
      board: board(), truncated: false, unmapped: [], epics: [],
      columns: [
        { ...COLUMNS[0]!, issues: [card(1), card(2)] },
        { ...COLUMNS[1]!, issues: [card(3)] },
        { ...COLUMNS[2]!, issues: [] },
      ],
    } as unknown as BoardView;
    const next = applyCardMove(view, "c2", 1, "c3");
    expect(next.columns.map((c) => c.issues.map((i) => i.id))).toEqual([["c1"], ["c2", "c3"], []]);
    expect(applyCardMove(view, "missing", 1)).toBe(view);
  });
});

describe("numbers and charts", () => {
  it("sums and formats story points", () => {
    expect(totalPoints([{ story_points: 3 }, { story_points: null }, { story_points: 2.5 }])).toBe(5.5);
    expect([formatPoints(3), formatPoints(2.5)]).toEqual(["3", "2.5"]);
  });

  it("says how long a sprint has left", () => {
    const now = new Date("2026-11-12T10:00:00");
    expect(sprintTimeLeft({ end_date: "2026-11-15" }, now)).toBe("3 days left");
    expect(sprintTimeLeft({ end_date: "2026-11-13" }, now)).toBe("1 day left");
    expect(sprintTimeLeft({ end_date: "2026-11-12" }, now)).toBe("Ends today");
    expect(sprintTimeLeft({ end_date: "2026-11-10" }, now)).toBe("2 days over");
    expect(sprintTimeLeft({ end_date: null }, now)).toBeNull();
  });

  it("draws the guideline from the starting scope to zero on the end date", () => {
    const sample = (at: string, remaining: number) => ({
      at, remaining_points: remaining, remaining_issues: 2, scope_points: 8, scope_issues: 3,
    });
    const series = burndownSeries([sample("2026-11-02T09:00:00Z", 8), sample("2026-11-05T09:00:00Z", 5)], { end_date: "2026-11-13" }, "points");
    expect(series.remaining.map((p) => p[1])).toEqual([8, 5]);
    expect(series.guideline).toEqual([
      [Date.parse("2026-11-02T09:00:00Z"), 8],
      [Date.parse("2026-11-13T23:59:59Z"), 0],
    ]);
    expect(burndownSeries([sample("2026-11-02T09:00:00Z", 8)], { end_date: null }, "issues").guideline[0]![1]).toBe(3);
    expect(burndownSeries([], { end_date: null }, "points").guideline).toEqual([]);
  });
});

const agileRoutes = (extra: Record<string, unknown> = {}) => ({
  "GET /api/v1/projects/PAY": project(),
  "GET /api/v1/boards": { items: [board()] },
  "GET /api/v1/workflow-statuses": STATUSES,
  ...extra,
});

describe("backlog page", () => {
  it("shows sprints and the backlog, and moves an issue from the row menu", async () => {
    signIn();
    let view = backlogView();
    const api = fakeApi(
      agileRoutes({
        "GET /api/v1/boards/b1/backlog": () => ({ body: view }),
        "POST /api/v1/boards/b1/move": () => {
          view = applyMove(view, { issueIds: ["c3"], list: "s1" });
          return { status: 204 };
        },
      }),
    );
    renderAt(<BacklogPage />, "/projects/:projectKey/backlog", "/projects/PAY/backlog");

    const sprintSection = await screen.findByRole("region", { name: "PAY Sprint 1" });
    expect(within(sprintSection).getByText("PAY-1")).toBeInTheDocument();
    const backlog = screen.getByRole("region", { name: "Backlog" });
    expect(within(backlog).getAllByText(/^PAY-[345]$/)).toHaveLength(3);
    expect(within(sprintSection).getByRole("button", { name: "Start sprint" })).toBeEnabled();

    const user = userEvent.setup();
    await user.click(within(backlog).getByRole("button", { name: "Move PAY-3" }));
    await user.click(await screen.findByRole("menuitem", { name: "PAY Sprint 1" }));

    await waitFor(() => expect(api.sent("POST", "/api/v1/boards/b1/move")).toHaveLength(1));
    expect(api.sent("POST", "/api/v1/boards/b1/move")[0]!.body).toEqual({ issue_ids: ["c3"], sprint_id: "s1", after_id: "c2" });
    await waitFor(() => expect(within(sprintSection).getByText("PAY-3")).toBeInTheDocument());
    expect(within(backlog).queryByText("PAY-3")).not.toBeInTheDocument();
  });

  it("puts a refused move back and says why", async () => {
    signIn();
    const api = fakeApi(
      agileRoutes({
        "GET /api/v1/boards/b1/backlog": backlogView(),
        "POST /api/v1/boards/b1/move": () => ({ status: 409, body: { code: "conflict", detail: "The sprint is closed." } }),
      }),
    );
    renderAt(<BacklogPage />, "/projects/:projectKey/backlog", "/projects/PAY/backlog");
    const backlog = await screen.findByRole("region", { name: "Backlog" });

    const user = userEvent.setup();
    await user.click(within(backlog).getByRole("button", { name: "Move PAY-4" }));
    await user.click(await screen.findByRole("menuitem", { name: "PAY Sprint 1" }));

    expect(await screen.findByText("The sprint is closed.")).toBeInTheDocument();
    expect(api.sent("POST", "/api/v1/boards/b1/move")).toHaveLength(1);
    await waitFor(() => expect(within(backlog).getByText("PAY-4")).toBeInTheDocument());
  });

  it("hides planning controls from people who cannot plan", async () => {
    signIn();
    const readOnly = board({ permissions: { manage_board: false, manage_sprints: false, move_issues: false, transition_issues: false } });
    fakeApi(
      agileRoutes({
        "GET /api/v1/boards": { items: [readOnly] },
        "GET /api/v1/boards/b1/backlog": { ...backlogView(), board: readOnly },
      }),
    );
    renderAt(<BacklogPage />, "/projects/:projectKey/backlog", "/projects/PAY/backlog");
    await screen.findByRole("region", { name: "PAY Sprint 1" });
    expect(screen.queryByRole("button", { name: "Start sprint" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Move PAY-/ })).not.toBeInTheDocument();
  });

  it("narrows the backlog and sprints to one epic and pauses ranking by drag", async () => {
    signIn();
    const view: BacklogView = {
      ...backlogView(),
      backlog: [card(3, { parent_key: "PAY-9", parent_summary: "Checkout" }), card(4), card(5, { parent_key: "PAY-9", parent_summary: "Checkout" })],
      epics: [{ id: "e9", key: "PAY-9", summary: "Checkout", status_category: "in_progress", issues: 3, done: 1, points: 8, done_points: 3 }],
    };
    fakeApi(agileRoutes({ "GET /api/v1/boards/b1/backlog": view }));
    renderAt(<BacklogPage />, "/projects/:projectKey/backlog", "/projects/PAY/backlog");
    const backlog = await screen.findByRole("region", { name: "Backlog" });
    const sprintSection = screen.getByRole("region", { name: "PAY Sprint 1" });
    expect(within(backlog).getAllByText(/^PAY-[345]$/)).toHaveLength(3);

    const user = userEvent.setup();
    await user.click(screen.getByRole("combobox", { name: "Filter by epic" }));
    await user.click(await screen.findByRole("option", { name: "PAY-9 · Checkout" }));
    await waitFor(() => expect(within(backlog).queryByText("PAY-4")).not.toBeInTheDocument());
    expect(within(backlog).getByText("PAY-3")).toBeInTheDocument();
    expect(within(backlog).getByText(/^2 of 3 issues/)).toBeInTheDocument();
    expect(within(sprintSection).getByText("No issues here match the filter.")).toBeInTheDocument();
    expect(screen.getByText("Clear the filters to rank issues by dragging.")).toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "Filter by epic" }));
    await user.click(await screen.findByRole("option", { name: "Issues without epic" }));
    await waitFor(() => expect(within(backlog).queryByText("PAY-3")).not.toBeInTheDocument());
    expect(within(backlog).getByText("PAY-4")).toBeInTheDocument();
    expect(within(sprintSection).getByText("PAY-1")).toBeInTheDocument();
  });

  it("lists epics with their progress and filters by the one chosen", async () => {
    signIn();
    const view: BacklogView = {
      ...backlogView(),
      backlog: [card(3, { parent_key: "PAY-9", parent_summary: "Checkout" }), card(4)],
      epics: [
        { id: "e9", key: "PAY-9", summary: "Checkout", status_category: "in_progress", issues: 3, done: 1, points: 8, done_points: 3 },
        { id: "e8", key: "PAY-8", summary: "Empty epic", status_category: "todo", issues: 0, done: 0, points: 0, done_points: 0 },
      ],
    };
    fakeApi(agileRoutes({ "GET /api/v1/boards/b1/backlog": view }));
    renderAt(<BacklogPage />, "/projects/:projectKey/backlog", "/projects/PAY/backlog");
    const backlog = await screen.findByRole("region", { name: "Backlog" });

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Epics" }));
    const panel = screen.getByRole("complementary", { name: "Epics" });
    expect(within(panel).getByRole("progressbar", { name: "PAY-9 progress" })).toHaveAttribute("aria-valuenow", "33");
    expect(within(panel).getByText("1 of 3 issues done · 3 of 8 pts")).toBeInTheDocument();
    expect(within(panel).getByText("No issues yet")).toBeInTheDocument();
    // The panel replaces the dropdown while it is open.
    expect(screen.queryByRole("combobox", { name: "Filter by epic" })).not.toBeInTheDocument();

    const checkout = within(panel).getByRole("button", { name: /Checkout/ });
    await user.click(checkout);
    expect(checkout).toHaveAttribute("aria-pressed", "true");
    await waitFor(() => expect(within(backlog).queryByText("PAY-4")).not.toBeInTheDocument());
    await user.click(checkout);
    expect(await within(backlog).findByText("PAY-4")).toBeInTheDocument();

    await user.click(within(panel).getByRole("button", { name: "Close epics" }));
    expect(screen.queryByRole("complementary", { name: "Epics" })).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Filter by epic" })).toBeInTheDocument();
  });
});

describe("board page", () => {
  it("shows the running sprint's issues in their columns and flags a column over its limit", async () => {
    signIn();
    const columns = [{ ...COLUMNS[0]!, wip_limit: 1 }, COLUMNS[1]!, COLUMNS[2]!];
    const running = sprint({ state: "active", started_at: "2026-11-10T09:00:00Z", start_date: "2026-11-10", end_date: "2026-11-24" });
    fakeApi(
      agileRoutes({
        "GET /api/v1/boards": { items: [board({ columns })] },
        "GET /api/v1/boards/b1/view": {
          board: board({ columns }), sprint: running, truncated: false, unmapped: [], epics: [],
          columns: [
            { ...columns[0], issues: [card(1), card(2)] },
            { ...columns[1], issues: [card(3, { status: "in_progress", status_category: "in_progress" })] },
            { ...columns[2], issues: [] },
          ],
        },
      }),
    );
    renderAt(<BoardPage />, "/projects/:projectKey/board", "/projects/PAY/board");

    const todo = await screen.findByRole("region", { name: /^To Do/ });
    expect(within(todo).getByText("PAY-1")).toBeInTheDocument();
    expect(within(todo).getByText(/Max 1/)).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: /^In Progress/ })).getByText("PAY-3")).toBeInTheDocument();
    expect(screen.getByText("PAY Sprint 1")).toBeInTheDocument();
  });

  it("splits the board into swimlanes by epic, each with every column", async () => {
    signIn();
    const running = sprint({ state: "active", started_at: "2026-11-10T09:00:00Z", start_date: "2026-11-10", end_date: "2026-11-24" });
    const checkout = { parent_key: "PAY-9", parent_summary: "Checkout" };
    fakeApi(
      agileRoutes({
        "GET /api/v1/boards/b1/view": {
          board: board(), sprint: running, truncated: false, unmapped: [],
          epics: [{ id: "e9", key: "PAY-9", summary: "Checkout", status_category: "in_progress", issues: 3, done: 1, points: 8, done_points: 3 }],
          columns: [
            { ...COLUMNS[0], issues: [card(1, checkout), card(2)] },
            { ...COLUMNS[1], issues: [card(3, { ...checkout, status: "in_progress", status_category: "in_progress" })] },
            { ...COLUMNS[2], issues: [] },
          ],
        },
      }),
    );
    renderAt(<BoardPage />, "/projects/:projectKey/board", "/projects/PAY/board");
    await screen.findByRole("region", { name: /^To Do/ });

    const user = userEvent.setup();
    await user.click(screen.getByRole("combobox", { name: "Swimlanes" }));
    await user.click(await screen.findByRole("option", { name: "Swimlanes by epic" }));

    const lane = await screen.findByRole("region", { name: /PAY-9 Checkout/ });
    expect(within(lane).getByText("2 issues")).toBeInTheDocument();
    expect(within(lane).getByText("PAY-1")).toBeInTheDocument();
    expect(within(lane).getByText("PAY-3")).toBeInTheDocument();
    const loose = screen.getByRole("region", { name: /No epic/ });
    expect(within(loose).getByText("PAY-2")).toBeInTheDocument();
    expect(within(loose).queryByText("PAY-1")).not.toBeInTheDocument();
  });
});

describe("filtering the board and backlog with MQL", () => {
  const IDS = "/api/v1/search/issues/ids";

  it("keeps only matching cards on the board, with the query in the URL", async () => {
    signIn();
    const running = sprint({ state: "active" });
    const api = fakeApi(
      agileRoutes({
        "GET /api/v1/projects": { items: [project()], next_cursor: null },
        "GET /api/v1/boards/b1/view": {
          board: board(), sprint: running, truncated: false, unmapped: [], epics: [],
          columns: [
            { ...COLUMNS[0], issues: [card(1), card(2)] },
            { ...COLUMNS[1], issues: [card(3, { status: "in_progress", status_category: "in_progress" })] },
            { ...COLUMNS[2], issues: [] },
          ],
        },
        [`GET ${IDS}`]: { ids: ["c1", "c3", "elsewhere"], truncated: false },
      }),
    );
    renderAt(<BoardPage />, "/projects/:projectKey/board", "/projects/PAY/board?mql=type%20%3D%20story");

    const todo = await screen.findByRole("region", { name: /^To Do/ });
    await waitFor(() => expect(within(todo).queryByText("PAY-2")).not.toBeInTheDocument());
    expect(within(todo).getByText("PAY-1")).toBeInTheDocument();
    expect(within(todo).getByText("1 of 2")).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: /^In Progress/ })).getByText("PAY-3")).toBeInTheDocument();
    const sent = api.sent("GET", IDS)[0]!;
    expect(sent.query.get("mql")).toBe("type = story");
    expect(sent.query.get("project")).toBe(board().project_id);
    expect(screen.getByRole("button", { name: "MQL filter on" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Clear" }));
    await waitFor(() => expect(within(todo).getByText("PAY-2")).toBeInTheDocument());
  });

  it("filters the backlog, pauses ranking by drag, and shows a bad query's mistake", async () => {
    signIn();
    fakeApi(
      agileRoutes({
        "GET /api/v1/projects": { items: [project()], next_cursor: null },
        "GET /api/v1/boards/b1/backlog": backlogView(),
        [`GET ${IDS}`]: (call: Call) =>
          call.query.get("mql")?.startsWith("stauts")
            ? { status: 422, body: { code: "validation_failed", detail: "Invalid", errors: [{ field: "mql", code: "invalid_query", message: 'Unknown field "stauts".', span: [0, 6] }] } }
            : { body: { ids: ["c3"], truncated: false } },
      }),
    );
    renderAt(<BacklogPage />, "/projects/:projectKey/backlog", "/projects/PAY/backlog");
    const backlog = await screen.findByRole("region", { name: "Backlog" });

    await userEvent.click(screen.getByRole("button", { name: "Filter with MQL" }));
    const editor = await screen.findByRole("combobox", { name: "MQL query" });
    await userEvent.type(editor, "stauts = done{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent('Unknown field "stauts".');
    expect(within(backlog).getAllByText(/^PAY-[345]$/)).toHaveLength(3);

    await userEvent.clear(editor);
    await userEvent.type(editor, "assignee is EMPTY{Enter}");
    await waitFor(() => expect(within(backlog).queryByText("PAY-4")).not.toBeInTheDocument());
    expect(within(backlog).getByText("PAY-3")).toBeInTheDocument();
    expect(within(backlog).getByText(/1 of 3 issues/)).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "PAY Sprint 1" })).getByText("No issues here match the filter.")).toBeInTheDocument();
    expect(screen.getByText("Clear the filters to rank issues by dragging.")).toBeInTheDocument();
    expect(within(backlog).getByRole("button", { name: "Move PAY-3" })).toBeInTheDocument();
  });
});

