//! The charts a dashboard can hold. Each kind is a fixed query with a few validated
//! options; nothing a person types becomes SQL. Every query is limited to the caller's
//! tenant (in the SQL and by the reader's row policy) and to projects they can browse.

use std::collections::BTreeMap;

use error_handling::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::clickhouse::{unavailable, ClickHouse, Params};

/// What to chart. Stored as a widget's `query`, and sent to `POST /analytics/query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WidgetQuery {
    /// Issues created and resolved per day.
    CreatedVsResolved {
        /// Empty: every project the viewer can browse.
        #[serde(default)]
        projects: Vec<Uuid>,
        /// 7 to 365.
        days: u16,
    },
    /// Days from first moving to an in-progress status until resolved, per week.
    CycleTime {
        #[serde(default)]
        projects: Vec<Uuid>,
        /// 4 to 52.
        weeks: u16,
    },
    /// Issues counted by one field, unresolved only unless `include_done`.
    Breakdown {
        #[serde(default)]
        projects: Vec<Uuid>,
        group_by: GroupBy,
        #[serde(default)]
        include_done: bool,
    },
    /// Days issues spent in each status, for stays that ended (the issue moved on) in the
    /// last weeks.
    TimeInStatus {
        #[serde(default)]
        projects: Vec<Uuid>,
        /// 4 to 52.
        weeks: u16,
    },
    /// Each issue resolved in the last weeks, with its cycle time, and a rolling average.
    ControlChart {
        #[serde(default)]
        projects: Vec<Uuid>,
        /// 4 to 52.
        weeks: u16,
    },
    /// Issues in each status category at the end of each day: a cumulative flow diagram.
    CumulativeFlow {
        #[serde(default)]
        projects: Vec<Uuid>,
        /// 7 to 365.
        days: u16,
    },
    /// Issues completed and left unfinished in a project's last sprints.
    Velocity {
        project: Uuid,
        /// 3 to 20.
        sprints: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    StatusCategory,
    Status,
    Type,
    Priority,
    Assignee,
    Project,
}

impl GroupBy {
    /// The column, from this fixed list only.
    fn column(self) -> &'static str {
        match self {
            GroupBy::StatusCategory => "status_category",
            GroupBy::Status => "status",
            GroupBy::Type => "type",
            GroupBy::Priority => "priority",
            GroupBy::Assignee => "assignee_id",
            GroupBy::Project => "toString(project_id)",
        }
    }
}

const MAX_PROJECTS: usize = 50;

impl WidgetQuery {
    pub fn validate(&self) -> ApiResult<()> {
        let range = |field: &str, value: u16, min: u16, max: u16| {
            if (min..=max).contains(&value) {
                Ok(())
            } else {
                Err(ApiError::invalid(field, "out_of_range", &format!("Choose between {min} and {max}.")))
            }
        };
        let projects = |p: &Vec<Uuid>| {
            if p.len() > MAX_PROJECTS {
                Err(ApiError::invalid("projects", "too_many", "Choose at most 50 projects."))
            } else {
                Ok(())
            }
        };
        match self {
            WidgetQuery::CreatedVsResolved { projects: p, days } => projects(p).and(range("days", *days, 7, 365)),
            WidgetQuery::CycleTime { projects: p, weeks } => projects(p).and(range("weeks", *weeks, 4, 52)),
            WidgetQuery::Breakdown { projects: p, .. } => projects(p),
            WidgetQuery::TimeInStatus { projects: p, weeks } => projects(p).and(range("weeks", *weeks, 4, 52)),
            WidgetQuery::ControlChart { projects: p, weeks } => projects(p).and(range("weeks", *weeks, 4, 52)),
            WidgetQuery::CumulativeFlow { projects: p, days } => projects(p).and(range("days", *days, 7, 365)),
            WidgetQuery::Velocity { sprints, .. } => range("sprints", *sprints, 3, 20),
        }
    }

    fn projects(&self) -> Vec<Uuid> {
        match self {
            WidgetQuery::CreatedVsResolved { projects, .. }
            | WidgetQuery::CycleTime { projects, .. }
            | WidgetQuery::Breakdown { projects, .. }
            | WidgetQuery::CumulativeFlow { projects, .. }
            | WidgetQuery::TimeInStatus { projects, .. }
            | WidgetQuery::ControlChart { projects, .. } => projects.clone(),
            WidgetQuery::Velocity { project, .. } => vec![*project],
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct DayCounts {
    /// `YYYY-MM-DD`, UTC.
    pub date: String,
    pub created: u64,
    pub resolved: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct WeekCycleTime {
    /// Monday of the week, `YYYY-MM-DD`.
    pub week: String,
    pub median_days: f64,
    pub p85_days: f64,
    pub issues: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct Group {
    /// Status key, type, priority, user id or project id; empty for "none" (e.g. unassigned).
    pub key: String,
    pub count: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct DayFlow {
    /// `YYYY-MM-DD`, UTC; counts are at the end of that day.
    pub date: String,
    pub todo: u64,
    pub in_progress: u64,
    pub done: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct StatusTime {
    /// Status key.
    pub status: String,
    pub median_days: f64,
    pub mean_days: f64,
    /// Stays that ended in the period.
    pub stays: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct ControlPoint {
    pub issue_id: Uuid,
    pub key: String,
    /// RFC 3339, UTC.
    pub resolved_at: String,
    /// From the first move into an in-progress status until resolved.
    pub days: f64,
    /// Mean of this issue's cycle time and those of the issues resolved just before it
    /// (ten in all).
    pub rolling_days: f64,
}

/// Issues in the control chart's rolling average.
pub const ROLLING: usize = 10;
/// Most recently resolved issues shown on a control chart.
const MAX_POINTS: usize = 2000;

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct SprintResult {
    pub sprint_id: Uuid,
    pub name: String,
    pub completed: u64,
    pub incomplete: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub completed_at: OffsetDateTime,
}

/// The numbers behind a chart.
#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WidgetData {
    CreatedVsResolved {
        days: Vec<DayCounts>,
    },
    CycleTime {
        weeks: Vec<WeekCycleTime>,
    },
    Breakdown {
        group_by: GroupBy,
        groups: Vec<Group>,
        total: u64,
    },
    CumulativeFlow {
        days: Vec<DayFlow>,
    },
    TimeInStatus {
        statuses: Vec<StatusTime>,
    },
    ControlChart {
        /// Oldest resolution first.
        issues: Vec<ControlPoint>,
        mean_days: f64,
        /// More issues were resolved than are shown; these are the latest.
        truncated: bool,
    },
    Velocity {
        sprints: Vec<SprintResult>,
    },
}

impl WidgetData {
    /// What a chart shows when the viewer can see none of its projects.
    fn empty(query: &WidgetQuery, today: Date) -> Self {
        match query {
            WidgetQuery::CreatedVsResolved { days, .. } => {
                WidgetData::CreatedVsResolved { days: fill_days(BTreeMap::new(), today, *days) }
            }
            WidgetQuery::CycleTime { .. } => WidgetData::CycleTime { weeks: vec![] },
            WidgetQuery::Breakdown { group_by, .. } => WidgetData::Breakdown { group_by: *group_by, groups: vec![], total: 0 },
            WidgetQuery::CumulativeFlow { days, .. } => {
                WidgetData::CumulativeFlow { days: flow(&BTreeMap::new(), &BTreeMap::new(), today, *days) }
            }
            WidgetQuery::TimeInStatus { .. } => WidgetData::TimeInStatus { statuses: vec![] },
            WidgetQuery::ControlChart { .. } => WidgetData::ControlChart { issues: vec![], mean_days: 0.0, truncated: false },
            WidgetQuery::Velocity { .. } => WidgetData::Velocity { sprints: vec![] },
        }
    }
}

/// Which projects a query covers, already narrowed to what the viewer may browse:
/// `None` = all of the tenant's, `Some(ids)` = only these (never empty).
pub type Scope = Option<Vec<Uuid>>;

/// `requested` narrowed by `visible` (`None` = everything visible). `None` overall when
/// nothing is left, so the caller can answer without querying.
pub fn scope(requested: &[Uuid], visible: Option<&[Uuid]>) -> Option<Scope> {
    let narrowed: Scope = match (requested.is_empty(), visible) {
        (true, None) => None,
        (true, Some(v)) => Some(v.to_vec()),
        (false, None) => Some(requested.to_vec()),
        (false, Some(v)) => Some(requested.iter().copied().filter(|p| v.contains(p)).collect()),
    };
    match narrowed {
        Some(ids) if ids.is_empty() => None,
        other => Some(other),
    }
}

fn day_string(date: Date) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), u8::from(date.month()), date.day())
}

/// One entry per day, oldest first, ending today; days without rows are zero.
fn fill_days(found: BTreeMap<String, (u64, u64)>, today: Date, days: u16) -> Vec<DayCounts> {
    (0..i64::from(days))
        .rev()
        .map(|back| {
            let date = day_string(today - Duration::days(back));
            let (created, resolved) = found.get(&date).copied().unwrap_or_default();
            DayCounts { date, created, resolved }
        })
        .collect()
}

/// Fills each point's rolling average and returns the overall mean; points are oldest first.
fn roll(points: &mut [ControlPoint]) -> f64 {
    for i in 0..points.len() {
        let window = &points[i.saturating_sub(ROLLING - 1)..=i];
        let mean = window.iter().map(|p| p.days).sum::<f64>() / window.len() as f64;
        points[i].rolling_days = (mean * 100.0).round() / 100.0;
    }
    if points.is_empty() {
        return 0.0;
    }
    let mean = points.iter().map(|p| p.days).sum::<f64>() / points.len() as f64;
    (mean * 100.0).round() / 100.0
}

/// Counts per category, in the order todo, in progress, done.
type Flow = [i64; 3];

fn category_index(category: &str) -> Option<usize> {
    ["todo", "in_progress", "done"].iter().position(|c| *c == category)
}

/// Works back from today's counts: the end of each earlier day is the end of the next day
/// minus what changed on that next day. Anchoring on today means today always matches the
/// issues' current state, even where older history was never recorded.
fn flow(current: &BTreeMap<String, i64>, changes: &BTreeMap<String, Flow>, today: Date, days: u16) -> Vec<DayFlow> {
    let mut counts: Flow = [0; 3];
    for (category, n) in current {
        if let Some(i) = category_index(category) {
            counts[i] = *n;
        }
    }
    let mut out = Vec::with_capacity(usize::from(days));
    for back in 0..i64::from(days) {
        let date = today - Duration::days(back);
        if back > 0 {
            if let Some(change) = changes.get(&day_string(date + Duration::days(1))) {
                for (count, delta) in counts.iter_mut().zip(change) {
                    *count -= delta;
                }
            }
        }
        let n = |i: usize| u64::try_from(counts[i].max(0)).unwrap_or_default();
        out.push(DayFlow { date: day_string(date), todo: n(0), in_progress: n(1), done: n(2) });
    }
    out.reverse();
    out
}

#[derive(Deserialize)]
struct CategoryCount {
    category: String,
    n: i64,
}

#[derive(Deserialize)]
struct CategoryChange {
    day: String,
    category: String,
    n: i64,
}

const PROJECT_FILTER: &str = " AND project_id IN {projects:Array(UUID)}";

#[derive(Deserialize)]
struct DayRow {
    day: String,
    created: u64,
    resolved: u64,
}

#[derive(Deserialize)]
struct TotalRow {
    total: u64,
}

/// Run a chart's query for `tenant`, over `scope`.
pub async fn run(store: &ClickHouse, tenant: Uuid, query: &WidgetQuery, visible: Option<&[Uuid]>) -> ApiResult<WidgetData> {
    query.validate()?;
    let today = OffsetDateTime::now_utc().date();
    let Some(scope) = scope(&query.projects(), visible) else {
        return Ok(WidgetData::empty(query, today));
    };
    let filter = if scope.is_some() { PROJECT_FILTER } else { "" };
    let mut params = Params::new();
    if let Some(ids) = &scope {
        params = params.uuids("projects", ids);
    }

    match query {
        WidgetQuery::CreatedVsResolved { days, .. } => {
            let from = today - Duration::days(i64::from(*days) - 1);
            let sql = format!(
                "SELECT toString(day) AS day, sum(c) AS created, sum(r) AS resolved FROM ( \
                   SELECT toDate(created_at) AS day, count() AS c, 0 AS r FROM issue_state FINAL \
                   WHERE tenant_id = {{tenant:UUID}}{filter} AND toDate(created_at) >= {{from:Date}} GROUP BY day \
                   UNION ALL \
                   SELECT toDate(assumeNotNull(resolved_at)) AS day, 0 AS c, count() AS r FROM issue_state FINAL \
                   WHERE tenant_id = {{tenant:UUID}}{filter} AND resolved_at IS NOT NULL \
                     AND toDate(assumeNotNull(resolved_at)) >= {{from:Date}} GROUP BY day \
                 ) GROUP BY day ORDER BY day"
            );
            let rows: Vec<DayRow> =
                store.query(tenant, &sql, &params.add("from", day_string(from))).await.map_err(unavailable)?;
            let found = rows.into_iter().map(|r| (r.day, (r.created, r.resolved))).collect();
            Ok(WidgetData::CreatedVsResolved { days: fill_days(found, today, *days) })
        }
        WidgetQuery::CycleTime { weeks, .. } => {
            let from = today - Duration::weeks(i64::from(*weeks)) + Duration::days(1);
            let sql = format!(
                "SELECT toString(toMonday(resolved)) AS week, \
                        round(quantileExact(0.5)(days), 1) AS median_days, \
                        round(quantileExact(0.85)(days), 1) AS p85_days, count() AS issues \
                 FROM ( \
                   SELECT s.resolved AS resolved, dateDiff('second', e.started, s.resolved) / 86400 AS days \
                   FROM ( \
                     SELECT issue_id, assumeNotNull(resolved_at) AS resolved FROM issue_state FINAL \
                     WHERE tenant_id = {{tenant:UUID}}{filter} AND resolved_at IS NOT NULL \
                       AND toDate(assumeNotNull(resolved_at)) >= {{from:Date}} \
                   ) AS s \
                   INNER JOIN ( \
                     SELECT issue_id, min(occurred_at) AS started FROM issue_events FINAL \
                     WHERE tenant_id = {{tenant:UUID}} AND event_type = 'issue.transitioned' AND to_category = 'in_progress'{filter} \
                     GROUP BY issue_id \
                   ) AS e ON s.issue_id = e.issue_id \
                   WHERE e.started <= s.resolved \
                 ) GROUP BY week ORDER BY week"
            );
            let weeks = store.query(tenant, &sql, &params.add("from", day_string(from))).await.map_err(unavailable)?;
            Ok(WidgetData::CycleTime { weeks })
        }
        WidgetQuery::Breakdown { group_by, include_done, .. } => {
            let open = if *include_done { "" } else { " AND status_category != 'done'" };
            let column = group_by.column();
            let base = format!("FROM issue_state FINAL WHERE tenant_id = {{tenant:UUID}}{filter}{open}");
            let groups: Vec<Group> = store
                .query(
                    tenant,
                    &format!("SELECT {column} AS key, count() AS count {base} GROUP BY key ORDER BY count DESC, key LIMIT 50"),
                    &params,
                )
                .await
                .map_err(unavailable)?;
            let total: Vec<TotalRow> =
                store.query(tenant, &format!("SELECT count() AS total {base}"), &params).await.map_err(unavailable)?;
            Ok(WidgetData::Breakdown { group_by: *group_by, groups, total: total.first().map_or(0, |t| t.total) })
        }
        WidgetQuery::CumulativeFlow { days, .. } => {
            let from = today - Duration::days(i64::from(*days) - 1);
            let live = format!("SELECT issue_id FROM issue_state FINAL WHERE tenant_id = {{tenant:UUID}}{filter}");
            let current: Vec<CategoryCount> = store
                .query(
                    tenant,
                    &format!(
                        "SELECT status_category AS category, toInt64(count()) AS n FROM issue_state FINAL \
                         WHERE tenant_id = {{tenant:UUID}}{filter} GROUP BY category"
                    ),
                    &params,
                )
                .await
                .map_err(unavailable)?;
            // What moved on each day after the first: an issue appears in the category it
            // started in (the one its first move left, or where it still is), and every move
            // takes one from a category and gives one to another. Moves recorded before events
            // carried both categories are left out, so each issue's moves still add up to where
            // it is now.
            let known = " AND from_category != '' AND to_category != ''";
            let sql = format!(
                "SELECT toString(day) AS day, category, sum(n) AS n FROM ( \
                   SELECT toDate(l.created_at) AS day, if(f.first_from != '', f.first_from, l.status_category) AS category, \
                          toInt64(1) AS n \
                   FROM ( \
                     SELECT issue_id, status_category, created_at FROM issue_state FINAL \
                     WHERE tenant_id = {{tenant:UUID}}{filter} AND toDate(created_at) > {{from:Date}} \
                   ) AS l \
                   LEFT JOIN ( \
                     SELECT issue_id, argMin(from_category, occurred_at) AS first_from FROM issue_events FINAL \
                     WHERE tenant_id = {{tenant:UUID}} AND event_type = 'issue.transitioned'{known} GROUP BY issue_id \
                   ) AS f ON l.issue_id = f.issue_id \
                   UNION ALL \
                   SELECT toDate(occurred_at) AS day, arrayJoin([from_category, to_category]) AS category, \
                          if(category = from_category, toInt64(-1), toInt64(1)) AS n \
                   FROM issue_events FINAL \
                   WHERE tenant_id = {{tenant:UUID}} AND event_type = 'issue.transitioned' \
                     AND from_category != to_category{known} AND toDate(occurred_at) > {{from:Date}} AND issue_id IN ({live}) \
                 ) WHERE category IN ('todo', 'in_progress', 'done') GROUP BY day, category"
            );
            let rows: Vec<CategoryChange> =
                store.query(tenant, &sql, &params.add("from", day_string(from))).await.map_err(unavailable)?;
            let current = current.into_iter().map(|c| (c.category, c.n)).collect();
            let mut changes: BTreeMap<String, Flow> = BTreeMap::new();
            for row in rows {
                if let Some(i) = category_index(&row.category) {
                    changes.entry(row.day).or_default()[i] += row.n;
                }
            }
            Ok(WidgetData::CumulativeFlow { days: flow(&current, &changes, today, *days) })
        }
        WidgetQuery::ControlChart { weeks, .. } => {
            let from = today - Duration::weeks(i64::from(*weeks)) + Duration::days(1);
            let sql = format!(
                "SELECT s.issue_id AS issue_id, s.key AS key, \
                        formatDateTime(s.resolved, '%Y-%m-%dT%H:%i:%SZ') AS resolved_at, \
                        round(dateDiff('second', e.started, s.resolved) / 86400, 2) AS days, toFloat64(0) AS rolling_days \
                 FROM ( \
                   SELECT issue_id, key, assumeNotNull(resolved_at) AS resolved FROM issue_state FINAL \
                   WHERE tenant_id = {{tenant:UUID}}{filter} AND resolved_at IS NOT NULL \
                     AND toDate(assumeNotNull(resolved_at)) >= {{from:Date}} \
                 ) AS s \
                 INNER JOIN ( \
                   SELECT issue_id, min(occurred_at) AS started FROM issue_events FINAL \
                   WHERE tenant_id = {{tenant:UUID}} AND event_type = 'issue.transitioned' AND to_category = 'in_progress'{filter} \
                   GROUP BY issue_id \
                 ) AS e ON s.issue_id = e.issue_id \
                 WHERE e.started <= s.resolved \
                 ORDER BY s.resolved DESC, s.issue_id LIMIT {{limit:UInt32}}"
            );
            let limit = u32::try_from(MAX_POINTS + 1).unwrap_or(u32::MAX);
            let mut issues: Vec<ControlPoint> = store
                .query(tenant, &sql, &params.add("from", day_string(from)).add("limit", limit))
                .await
                .map_err(unavailable)?;
            let truncated = issues.len() > MAX_POINTS;
            issues.truncate(MAX_POINTS);
            issues.reverse();
            let mean_days = roll(&mut issues);
            Ok(WidgetData::ControlChart { issues, mean_days, truncated })
        }
        WidgetQuery::TimeInStatus { weeks, .. } => {
            let from = today - Duration::weeks(i64::from(*weeks)) + Duration::days(1);
            // A stay in a status ends when the issue leaves it, and began at the move before
            // (or when the issue was created). Earlier moves are read too, so a stay that began
            // before the period still has its start.
            let sql = format!(
                "SELECT status, round(quantileExact(0.5)(days), 1) AS median_days, round(avg(days), 1) AS mean_days, \
                        count() AS stays \
                 FROM ( \
                   SELECT t.from_status AS status, \
                          dateDiff('second', if(t.n = 1, l.created_at, t.prev), t.occurred_at) / 86400 AS days \
                   FROM ( \
                     SELECT issue_id, from_status, occurred_at, \
                            row_number() OVER w AS n, lagInFrame(occurred_at) OVER w AS prev \
                     FROM issue_events FINAL \
                     WHERE tenant_id = {{tenant:UUID}} AND event_type = 'issue.transitioned' \
                     WINDOW w AS (PARTITION BY issue_id ORDER BY occurred_at, event_id \
                                  ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) \
                   ) AS t \
                   INNER JOIN ( \
                     SELECT issue_id, created_at FROM issue_state FINAL WHERE tenant_id = {{tenant:UUID}}{filter} \
                   ) AS l ON t.issue_id = l.issue_id \
                   WHERE t.from_status != '' AND toDate(t.occurred_at) >= {{from:Date}} \
                 ) WHERE days >= 0 \
                 GROUP BY status ORDER BY median_days DESC, status LIMIT 50"
            );
            let statuses = store.query(tenant, &sql, &params.add("from", day_string(from))).await.map_err(unavailable)?;
            Ok(WidgetData::TimeInStatus { statuses })
        }
        WidgetQuery::Velocity { project, sprints } => {
            let sql = "SELECT sprint_id, any(name) AS name, any(completed) AS completed, any(incomplete) AS incomplete, \
                         formatDateTime(max(occurred_at), '%Y-%m-%dT%H:%i:%SZ') AS completed_at \
                       FROM issue_events FINAL \
                       WHERE tenant_id = {tenant:UUID} AND event_type = 'sprint.completed' AND project_id = {project:UUID} \
                       GROUP BY sprint_id ORDER BY completed_at DESC LIMIT {sprints:UInt16}";
            let mut found: Vec<SprintResult> =
                store.query(tenant, sql, &params.add("project", project).add("sprints", sprints)).await.map_err(unavailable)?;
            found.reverse();
            Ok(WidgetData::Velocity { sprints: found })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queries_are_parsed_strictly_and_validated() {
        let q: WidgetQuery = serde_json::from_value(json!({ "kind": "created_vs_resolved", "days": 30 })).unwrap();
        assert_eq!(q, WidgetQuery::CreatedVsResolved { projects: vec![], days: 30 });
        assert!(q.validate().is_ok());
        let too_long: WidgetQuery = serde_json::from_value(json!({ "kind": "cycle_time", "weeks": 400 })).unwrap();
        assert!(too_long.validate().is_err());
        assert!(serde_json::from_value::<WidgetQuery>(json!({ "kind": "raw_sql", "sql": "SELECT 1" })).is_err());
        assert!(serde_json::from_value::<WidgetQuery>(json!({ "kind": "breakdown", "group_by": "password" })).is_err());
        let many: Vec<Uuid> = (0..51).map(|_| Uuid::now_v7()).collect();
        assert!(WidgetQuery::Breakdown { projects: many, group_by: GroupBy::Type, include_done: false }.validate().is_err());
    }

    #[test]
    fn scope_never_widens_what_the_viewer_can_see() {
        let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        assert_eq!(scope(&[], None), Some(None));
        assert_eq!(scope(&[], Some(&[a, b])), Some(Some(vec![a, b])));
        assert_eq!(scope(&[a, c], Some(&[a, b])), Some(Some(vec![a])));
        assert_eq!(scope(&[c], Some(&[a, b])), None);
        assert_eq!(scope(&[], Some(&[])), None);
        assert_eq!(scope(&[c], None), Some(Some(vec![c])));
    }

    #[test]
    fn days_are_filled_oldest_first() {
        let today = Date::from_calendar_date(2026, time::Month::March, 2).unwrap();
        let found = BTreeMap::from([("2026-02-28".to_owned(), (2, 1))]);
        let days = fill_days(found, today, 3);
        assert_eq!(days.iter().map(|d| d.date.as_str()).collect::<Vec<_>>(), ["2026-02-28", "2026-03-01", "2026-03-02"]);
        assert_eq!((days[0].created, days[0].resolved, days[2].created), (2, 1, 0));
    }

    #[test]
    fn the_rolling_average_covers_the_last_issues() {
        let point = |days: f64| ControlPoint {
            issue_id: Uuid::nil(),
            key: String::new(),
            resolved_at: String::new(),
            days,
            rolling_days: 0.0,
        };
        let mut points: Vec<ControlPoint> = (1..=12).map(|d| point(f64::from(d))).collect();
        assert_eq!(roll(&mut points), 6.5);
        assert_eq!(points[0].rolling_days, 1.0);
        assert_eq!(points[1].rolling_days, 1.5);
        assert_eq!(points[9].rolling_days, 5.5);
        assert_eq!(points[11].rolling_days, 7.5, "issues 3 to 12");
        assert_eq!(roll(&mut []), 0.0);
    }

    #[test]
    fn flow_works_back_from_today() {
        let today = Date::from_calendar_date(2026, time::Month::March, 3).unwrap();
        let current = BTreeMap::from([("todo".to_owned(), 1), ("in_progress".to_owned(), 1), ("done".to_owned(), 2)]);
        // 03-02: one issue created in todo. 03-03: one moved from in progress to done.
        let changes = BTreeMap::from([("2026-03-02".to_owned(), [1, 0, 0]), ("2026-03-03".to_owned(), [0, -1, 1])]);
        let days = flow(&current, &changes, today, 3);
        let counts: Vec<_> = days.iter().map(|d| (d.date.as_str(), d.todo, d.in_progress, d.done)).collect();
        assert_eq!(counts, [("2026-03-01", 0, 2, 1), ("2026-03-02", 1, 2, 1), ("2026-03-03", 1, 1, 2)]);
    }
}
