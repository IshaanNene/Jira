//! Issue vocabulary and rules that need no I/O.
//!
//! Statuses and transitions are not defined here: they come from workflow-service.

use error_handling::{ApiError, ApiResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueType {
    pub key: &'static str,
    pub name: &'static str,
    /// 1 = epic, 0 = standard, -1 = subtask.
    pub level: i8,
}

pub const ISSUE_TYPES: [IssueType; 7] = [
    IssueType { key: "epic", name: "Epic", level: 1 },
    IssueType { key: "story", name: "Story", level: 0 },
    IssueType { key: "task", name: "Task", level: 0 },
    IssueType { key: "bug", name: "Bug", level: 0 },
    IssueType { key: "feature", name: "Feature", level: 0 },
    IssueType { key: "improvement", name: "Improvement", level: 0 },
    IssueType { key: "subtask", name: "Subtask", level: -1 },
];

pub fn issue_type(key: &str) -> ApiResult<IssueType> {
    ISSUE_TYPES
        .iter()
        .copied()
        .find(|t| t.key == key)
        .ok_or_else(|| ApiError::invalid("type", "unknown_type", "Unknown issue type."))
}

pub const STATUS_CATEGORIES: [&str; 3] = ["todo", "in_progress", "done"];

pub const PRIORITIES: [&str; 5] = ["highest", "high", "medium", "low", "lowest"];
pub const LINK_TYPES: [(&str, &str, &str); 5] = [
    ("blocks", "blocks", "is blocked by"),
    ("relates", "relates to", "relates to"),
    ("duplicates", "duplicates", "is duplicated by"),
    ("depends_on", "depends on", "is depended on by"),
    ("clones", "clones", "is cloned by"),
];

pub fn validate_priority(value: &str) -> ApiResult<()> {
    if PRIORITIES.contains(&value) {
        Ok(())
    } else {
        Err(ApiError::invalid("priority", "unknown_priority", "Unknown priority."))
    }
}

pub fn validate_link_type(value: &str) -> ApiResult<()> {
    if LINK_TYPES.iter().any(|(k, ..)| *k == value) {
        Ok(())
    } else {
        Err(ApiError::invalid("link_type", "unknown_link_type", "Unknown link type."))
    }
}

/// Hierarchy: epic → standard issue → subtask.
///
/// * a subtask must have a standard-level parent,
/// * a standard issue may have an epic as parent,
/// * an epic has no parent.
///
/// Because a parent is always exactly one level up, cycles are impossible.
pub fn check_parent(child: IssueType, parent: Option<IssueType>) -> ApiResult<()> {
    let err = |msg: &str| Err(ApiError::invalid("parent_key", "invalid_parent", msg));
    match (child.level, parent.map(|p| p.level)) {
        (1, None) | (0, None | Some(1)) | (-1, Some(0)) => Ok(()),
        (1, Some(_)) => err("An epic cannot have a parent."),
        (0, Some(_)) => err("The parent of this issue type must be an epic."),
        (-1, None) => err("A subtask needs a parent issue."),
        _ => err("A subtask's parent must be a story, task, bug or similar issue."),
    }
}

/// Lower-cased, trimmed, de-duplicated labels: letters, digits, `-`, `_`, `.`; up to 20
/// labels of up to 50 characters.
pub fn normalize_labels(labels: Vec<String>) -> ApiResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for raw in labels {
        let label = raw.trim().to_lowercase();
        let ok = (1..=50).contains(&label.chars().count())
            && label.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !ok {
            return Err(ApiError::invalid(
                "labels",
                "invalid_label",
                "Labels use letters, numbers, hyphens, underscores and dots, without spaces.",
            ));
        }
        if !out.contains(&label) {
            out.push(label);
        }
    }
    if out.len() > 20 {
        return Err(ApiError::invalid("labels", "too_many_labels", "An issue can have at most 20 labels."));
    }
    Ok(out)
}

pub fn is_project_key(key: &str) -> bool {
    (2..=10).contains(&key.len())
        && key.as_bytes()[0].is_ascii_uppercase()
        && key.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// `PAY-123` → (`PAY`, 123).
pub fn parse_key(key: &str) -> Option<(&str, i64)> {
    let (project, number) = key.rsplit_once('-')?;
    let valid_project = is_project_key(project);
    let number: i64 = number.parse().ok().filter(|n| *n > 0 && number.len() <= 12 && !number.starts_with('+'))?;
    valid_project.then_some((project, number))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str) -> IssueType {
        issue_type(key).unwrap()
    }

    #[test]
    fn hierarchy_rules() {
        assert!(check_parent(t("epic"), None).is_ok());
        assert!(check_parent(t("story"), None).is_ok());
        assert!(check_parent(t("story"), Some(t("epic"))).is_ok());
        assert!(check_parent(t("subtask"), Some(t("task"))).is_ok());
        assert!(check_parent(t("epic"), Some(t("epic"))).is_err());
        assert!(check_parent(t("story"), Some(t("task"))).is_err());
        assert!(check_parent(t("subtask"), None).is_err());
        assert!(check_parent(t("subtask"), Some(t("epic"))).is_err());
        assert!(check_parent(t("subtask"), Some(t("subtask"))).is_err());
    }

    #[test]
    fn key_parsing() {
        assert_eq!(parse_key("PAY-123"), Some(("PAY", 123)));
        assert_eq!(parse_key("A1-7"), Some(("A1", 7)));
        for bad in ["PAY", "PAY-", "PAY-0", "PAY--1", "pay-1", "PAY-1x", "-1", "PAY-+1", "P-1", "PAY-99999999999999999999"] {
            assert_eq!(parse_key(bad), None, "{bad}");
        }
    }

    #[test]
    fn labels() {
        assert_eq!(normalize_labels(vec![" Backend ".into(), "backend".into(), "v1.2".into()]).unwrap(), ["backend", "v1.2"]);
        assert!(normalize_labels(vec!["two words".into()]).is_err());
        assert!(normalize_labels(vec!["<script>".into()]).is_err());
        assert!(normalize_labels((0..21).map(|i| format!("l{i}")).collect()).is_err());
    }

    #[test]
    fn vocabulary() {
        assert!(issue_type("saga").is_err());
        assert!(validate_priority("urgent").is_err());
        assert!(validate_link_type("blocks").is_ok());
    }
}
