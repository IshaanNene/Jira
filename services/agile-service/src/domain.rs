//! Board and sprint rules that need no I/O: column configuration and burndown maths.

use std::collections::{HashMap, HashSet};

use error_handling::{ApiError, ApiResult, FieldError};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime, Time};
use utoipa::ToSchema;
use uuid::Uuid;

pub const CATEGORIES: [&str; 3] = ["todo", "in_progress", "done"];
pub const MAX_COLUMNS: usize = 12;

/// One board column. An issue lands in the column that lists its status; a status no
/// column lists falls into the column that catches its category, if there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub id: String,
    pub name: String,
    /// Workflow status keys shown in this column.
    #[serde(default)]
    pub statuses: Vec<String>,
    /// todo | in_progress | done: also show every status of this category that no
    /// column lists explicitly.
    #[serde(default)]
    pub category: Option<String>,
    /// Soft limit on the number of issues; the board highlights a column over it.
    #[serde(default)]
    pub wip_limit: Option<u32>,
}

/// To Do / In Progress / Done by status category: correct for any workflow.
pub fn default_columns() -> Vec<Column> {
    [("todo", "To Do"), ("in_progress", "In Progress"), ("done", "Done")]
        .into_iter()
        .map(|(category, name)| Column {
            id: category.to_owned(),
            name: name.to_owned(),
            statuses: vec![],
            category: Some(category.to_owned()),
            wip_limit: None,
        })
        .collect()
}

fn slug_like(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Normalises names and rejects configurations in which an issue could belong to two columns.
pub fn validate_columns(columns: Vec<Column>) -> ApiResult<Vec<Column>> {
    let mut errors = Vec::new();
    let mut err = |code: &str, message: String| errors.push(FieldError::new("columns", code, message));
    if columns.is_empty() || columns.len() > MAX_COLUMNS {
        err("invalid_count", format!("A board needs between 1 and {MAX_COLUMNS} columns."));
    }
    let (mut ids, mut names, mut statuses, mut categories) = (HashSet::new(), HashSet::new(), HashSet::new(), HashSet::new());
    let mut out = Vec::with_capacity(columns.len());
    for mut column in columns {
        column.name = column.name.trim().to_owned();
        if column.name.is_empty() || column.name.chars().count() > 40 {
            err("invalid_name", "Column names must be between 1 and 40 characters.".into());
        }
        if !names.insert(column.name.to_lowercase()) {
            err("duplicate_name", format!("Two columns are named \"{}\".", column.name));
        }
        if !slug_like(&column.id, 40) {
            err("invalid_id", "Column ids may contain letters, digits, - and _ (at most 40).".into());
        }
        if !ids.insert(column.id.clone()) {
            err("duplicate_id", format!("Two columns share the id \"{}\".", column.id));
        }
        if column.statuses.len() > 50 {
            err("too_many_statuses", format!("\"{}\" lists too many statuses.", column.name));
        }
        for status in &column.statuses {
            if !slug_like(status, 64) {
                err("invalid_status", format!("\"{status}\" is not a valid status key."));
            } else if !statuses.insert(status.clone()) {
                err("duplicate_status", format!("The status \"{status}\" is mapped to more than one column."));
            }
        }
        if let Some(category) = &column.category {
            if !CATEGORIES.contains(&category.as_str()) {
                err("invalid_category", "The category must be todo, in_progress or done.".into());
            } else if !categories.insert(category.clone()) {
                err("duplicate_category", format!("More than one column catches the remaining \"{category}\" statuses."));
            }
        }
        if column.wip_limit.is_some_and(|limit| limit == 0 || limit > 999) {
            err("invalid_wip_limit", "A column limit must be between 1 and 999.".into());
        }
        out.push(column);
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(ApiError::Validation(errors))
    }
}

/// Index of the column showing an issue with this status, if any.
pub fn column_for(columns: &[Column], status: &str, category: &str) -> Option<usize> {
    columns
        .iter()
        .position(|c| c.statuses.iter().any(|s| s == status))
        .or_else(|| columns.iter().position(|c| c.category.as_deref() == Some(category)))
}

// --- burndown ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeChange {
    pub issue_id: Uuid,
    pub added: bool,
    pub at: OffsetDateTime,
}

/// What is known about an issue when the chart is computed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IssueFact {
    pub points: Option<f64>,
    pub done: bool,
    pub resolved_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BurndownSample {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// Issues in the sprint at this moment, and their story points.
    pub scope_issues: u32,
    pub scope_points: f64,
    /// Of those, the ones not finished yet.
    pub remaining_issues: u32,
    pub remaining_points: f64,
}

const MAX_SAMPLES: i64 = 120;

/// Remaining work over time: one sample at the start, one at each following midnight
/// (UTC) and one at `until`. Scope comes from the recorded changes; completion from each
/// issue's current resolution time and current estimate, so an issue that was finished
/// and later reopened counts as unfinished for the whole chart.
pub fn burndown(
    start: OffsetDateTime,
    until: OffsetDateTime,
    changes: &[ScopeChange],
    facts: &HashMap<Uuid, IssueFact>,
) -> Vec<BurndownSample> {
    let until = until.max(start);
    let mut instants = vec![start];
    let days = (until - start).whole_days() + 1;
    let step = (days / MAX_SAMPLES).max(0) + 1;
    let mut next = start.replace_time(Time::MIDNIGHT) + Duration::days(step);
    while next < until {
        instants.push(next);
        next += Duration::days(step);
    }
    if until > start {
        instants.push(until);
    }

    let mut ordered = changes.to_vec();
    ordered.sort_by_key(|c| c.at);
    instants
        .into_iter()
        .map(|at| {
            let mut in_scope: HashMap<Uuid, bool> = HashMap::new();
            for change in ordered.iter().take_while(|c| c.at <= at) {
                in_scope.insert(change.issue_id, change.added);
            }
            let mut sample =
                BurndownSample { at, scope_issues: 0, scope_points: 0.0, remaining_issues: 0, remaining_points: 0.0 };
            for (issue_id, _) in in_scope.iter().filter(|(_, present)| **present) {
                let Some(fact) = facts.get(issue_id) else { continue }; // deleted since
                let points = fact.points.unwrap_or(0.0);
                sample.scope_issues += 1;
                sample.scope_points += points;
                if !(fact.done && fact.resolved_at.is_none_or(|resolved| resolved <= at)) {
                    sample.remaining_issues += 1;
                    sample.remaining_points += points;
                }
            }
            sample
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn column(id: &str, statuses: &[&str], category: Option<&str>) -> Column {
        Column {
            id: id.into(),
            name: id.to_uppercase(),
            statuses: statuses.iter().map(|s| (*s).to_owned()).collect(),
            category: category.map(str::to_owned),
            wip_limit: None,
        }
    }

    #[test]
    fn explicit_status_mapping_wins_over_the_category_catch_all() {
        let columns = vec![
            column("todo", &[], Some("todo")),
            column("review", &["in_review"], None),
            column("doing", &[], Some("in_progress")),
        ];
        assert_eq!(column_for(&columns, "in_review", "in_progress"), Some(1));
        assert_eq!(column_for(&columns, "building", "in_progress"), Some(2));
        assert_eq!(column_for(&columns, "shipped", "done"), None);
        assert!(validate_columns(default_columns()).is_ok());
    }

    #[test]
    fn ambiguous_or_malformed_columns_are_rejected() {
        let codes = |columns: Vec<Column>| match validate_columns(columns) {
            Err(ApiError::Validation(errors)) => errors.into_iter().map(|e| e.code).collect::<Vec<_>>(),
            other => panic!("expected validation errors, got {other:?}"),
        };
        assert_eq!(codes(vec![]), ["invalid_count"]);
        assert_eq!(codes(vec![column("a", &["x"], None), column("b", &["x"], None)]), ["duplicate_status"]);
        assert_eq!(codes(vec![column("a", &[], Some("done")), column("b", &[], Some("done"))]), ["duplicate_category"]);
        assert_eq!(codes(vec![column("a", &[], Some("finished"))]), ["invalid_category"]);
        assert_eq!(codes(vec![column("a", &[], None), column("a", &[], None)]), ["duplicate_name", "duplicate_id"]);
        let mut limited = column("a", &[], None);
        limited.wip_limit = Some(0);
        assert_eq!(codes(vec![limited]), ["invalid_wip_limit"]);
    }

    #[test]
    fn burndown_follows_scope_changes_and_completions() {
        let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let start = datetime!(2026-03-02 09:00 UTC);
        let changes = [
            ScopeChange { issue_id: a, added: true, at: datetime!(2026-03-01 12:00 UTC) },
            ScopeChange { issue_id: b, added: true, at: datetime!(2026-03-01 12:00 UTC) },
            // c joins on day two, b leaves on day three.
            ScopeChange { issue_id: c, added: true, at: datetime!(2026-03-03 10:00 UTC) },
            ScopeChange { issue_id: b, added: false, at: datetime!(2026-03-04 10:00 UTC) },
        ];
        let fact = |points, resolved_at: Option<OffsetDateTime>| IssueFact { points, done: resolved_at.is_some(), resolved_at };
        let facts = HashMap::from([
            (a, fact(Some(5.0), Some(datetime!(2026-03-02 15:00 UTC)))),
            (b, fact(Some(3.0), None)),
            (c, fact(None, None)),
        ]);
        let samples = burndown(start, datetime!(2026-03-05 12:00 UTC), &changes, &facts);
        let view: Vec<_> =
            samples.iter().map(|s| (s.scope_issues, s.scope_points, s.remaining_issues, s.remaining_points)).collect();
        assert_eq!(
            view,
            [
                (2, 8.0, 2, 8.0), // start
                (2, 8.0, 1, 3.0), // midnight after day 1: a finished
                (3, 8.0, 2, 3.0), // c added (unestimated)
                (2, 5.0, 1, 0.0), // b removed
                (2, 5.0, 1, 0.0), // now
            ]
        );
        assert_eq!(samples[1].at, datetime!(2026-03-03 00:00 UTC));
    }

    #[test]
    fn burndown_of_a_long_or_instant_sprint_is_bounded() {
        let start = datetime!(2026-01-01 00:00 UTC);
        assert_eq!(burndown(start, start, &[], &HashMap::new()).len(), 1);
        let long = burndown(start, start + Duration::days(2000), &[], &HashMap::new());
        assert!(long.len() <= MAX_SAMPLES as usize + 2, "{}", long.len());
    }
}
