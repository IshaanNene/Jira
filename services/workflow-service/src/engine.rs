//! The workflow model and its evaluation. Pure and declarative.
//!
//! A workflow is data: statuses, transitions, and for each transition a list of
//! conditions, validators and post-functions chosen from the closed sets below. There is
//! no expression language and nothing user-supplied is ever executed — an unknown rule
//! type simply fails to deserialize.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const CATEGORIES: [&str; 3] = ["todo", "in_progress", "done"];
const ROLES: [&str; 3] = ["viewer", "member", "administrator"];
/// Built-in issue fields a validator may require.
pub const REQUIRABLE_FIELDS: [&str; 9] =
    ["assignee", "description", "due_date", "start_date", "story_points", "labels", "components", "fix_versions", "parent"];
const MAX_STATUSES: usize = 50;
const MAX_TRANSITIONS: usize = 200;
const MAX_RULES: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusDef {
    /// Stable identifier stored on issues, e.g. `in_review`.
    pub key: String,
    pub name: String,
    /// todo | in_progress | done
    pub category: String,
    /// Position on the designer canvas.
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
}

// Parameterless rules are written as empty struct variants (`IsAssignee {}`) on purpose:
// serde only enforces `deny_unknown_fields` on struct variants, and a rule carrying
// properties this model does not know must be rejected, not silently accepted.

/// Who may use a transition. All conditions must hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    /// The actor's project role is at least `role` (viewer < member < administrator).
    RoleAtLeast {
        role: String,
    },
    IsAssignee {},
    IsReporter {},
}

/// What must be true of the issue. All validators must pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Validator {
    /// `field` is a built-in field name or `custom:<field id>`. `label` is what to call
    /// it in the message shown to the person.
    FieldRequired {
        field: String,
        #[serde(default)]
        label: Option<String>,
    },
    /// Every child issue is in a done status.
    ChildrenDone {},
}

/// What happens to the issue when the transition is taken. Executed by issue-service
/// from this closed list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PostFunction {
    AssignToCurrentUser {},
    AssignToReporter {},
    ClearAssignee {},
    SetResolution { value: String },
    ClearResolution {},
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    pub name: String,
    /// Statuses this transition starts from. Empty = from any status.
    #[serde(default)]
    pub from: Vec<String>,
    pub to: String,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub validators: Vec<Validator>,
    #[serde(default)]
    pub post_functions: Vec<PostFunction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub statuses: Vec<StatusDef>,
    /// Key of the status new issues start in.
    pub initial: String,
    pub transitions: Vec<Transition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Problem {
    /// Where the problem is, e.g. `statuses[2].key` or `transitions[0].to`.
    pub path: String,
    pub message: String,
}

fn problem(path: impl Into<String>, message: impl Into<String>) -> Problem {
    Problem { path: path.into(), message: message.into() }
}

fn is_slug(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_name(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= 40 && !s.chars().any(char::is_control)
}

impl Definition {
    pub fn status(&self, key: &str) -> Option<&StatusDef> {
        self.statuses.iter().find(|s| s.key == key)
    }

    /// Everything that makes a definition unusable. A definition with any problem can be
    /// saved as a draft but never published.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        if self.statuses.is_empty() {
            out.push(problem("statuses", "A workflow needs at least one status."));
        }
        if self.statuses.len() > MAX_STATUSES {
            out.push(problem("statuses", format!("A workflow can have at most {MAX_STATUSES} statuses.")));
        }
        if self.transitions.len() > MAX_TRANSITIONS {
            out.push(problem("transitions", format!("A workflow can have at most {MAX_TRANSITIONS} transitions.")));
        }

        let mut keys = HashSet::new();
        let mut names = HashSet::new();
        for (i, s) in self.statuses.iter().enumerate() {
            if !is_slug(&s.key) {
                out.push(problem(
                    format!("statuses[{i}].key"),
                    "Status keys use lowercase letters, digits and underscores, starting with a letter.",
                ));
            } else if !keys.insert(s.key.as_str()) {
                out.push(problem(format!("statuses[{i}].key"), format!("The status key “{}” is used twice.", s.key)));
            }
            if !valid_name(&s.name) {
                out.push(problem(format!("statuses[{i}].name"), "Status names are 1–40 characters."));
            } else if !names.insert(s.name.trim().to_lowercase()) {
                out.push(problem(format!("statuses[{i}].name"), format!("Two statuses are named “{}”.", s.name.trim())));
            }
            if !CATEGORIES.contains(&s.category.as_str()) {
                out.push(problem(format!("statuses[{i}].category"), "Category must be todo, in_progress or done."));
            }
            if !(s.x.is_finite() && s.y.is_finite()) {
                out.push(problem(format!("statuses[{i}]"), "The status position is not valid."));
            }
        }
        if !self.statuses.is_empty() && !keys.contains(self.initial.as_str()) {
            out.push(problem("initial", "Choose which status new issues start in."));
        }

        let mut ids = HashSet::new();
        for (i, t) in self.transitions.iter().enumerate() {
            let at = |field: &str| format!("transitions[{i}].{field}");
            if !(1..=64).contains(&t.id.len()) || !ids.insert(t.id.as_str()) {
                out.push(problem(at("id"), "Each transition needs a unique id."));
            }
            if !valid_name(&t.name) {
                out.push(problem(at("name"), "Transition names are 1–40 characters."));
            }
            if !keys.contains(t.to.as_str()) {
                out.push(problem(at("to"), format!("“{}” leads to a status that does not exist.", t.name)));
            }
            for from in &t.from {
                if !keys.contains(from.as_str()) {
                    out.push(problem(at("from"), format!("“{}” starts from a status that does not exist.", t.name)));
                }
            }
            if t.from.len() == 1 && t.from[0] == t.to {
                out.push(problem(at("to"), format!("“{}” starts and ends in the same status.", t.name)));
            }
            if t.conditions.len() > MAX_RULES || t.validators.len() > MAX_RULES || t.post_functions.len() > MAX_RULES {
                out.push(problem(at("rules"), format!("A transition can have at most {MAX_RULES} rules of each kind.")));
            }
            for c in &t.conditions {
                if let Condition::RoleAtLeast { role } = c {
                    if !ROLES.contains(&role.as_str()) {
                        out.push(problem(at("conditions"), "Role must be viewer, member or administrator."));
                    }
                }
            }
            for v in &t.validators {
                if let Validator::FieldRequired { field, label } = v {
                    let custom = field.strip_prefix("custom:").is_some_and(|id| uuid::Uuid::parse_str(id).is_ok());
                    if !custom && !REQUIRABLE_FIELDS.contains(&field.as_str()) {
                        out.push(problem(at("validators"), format!("“{field}” is not a field that can be required.")));
                    }
                    if label.as_ref().is_some_and(|l| !valid_name(l)) {
                        out.push(problem(at("validators"), "A field label is 1–40 characters."));
                    }
                }
            }
            for p in &t.post_functions {
                if let PostFunction::SetResolution { value } = p {
                    if !is_slug(value) {
                        out.push(problem(at("post_functions"), "A resolution uses lowercase letters, digits and underscores."));
                    }
                }
            }
        }

        // Every status must be reachable from the initial one, or issues could never get there.
        if out.is_empty() {
            let mut reached: HashSet<&str> = HashSet::from([self.initial.as_str()]);
            let mut queue = VecDeque::from([self.initial.as_str()]);
            while let Some(current) = queue.pop_front() {
                for t in &self.transitions {
                    if (t.from.is_empty() || t.from.iter().any(|f| f == current)) && reached.insert(t.to.as_str()) {
                        queue.push_back(t.to.as_str());
                    }
                }
            }
            for (i, s) in self.statuses.iter().enumerate() {
                if !reached.contains(s.key.as_str()) {
                    out.push(problem(
                        format!("statuses[{i}]"),
                        format!("No transition leads to “{}”, so issues can never reach it.", s.name),
                    ));
                }
            }
        }
        out
    }
}

/// The workflow used when nothing else is assigned: four statuses, free movement.
pub fn default_definition() -> Definition {
    let status = |key: &str, name: &str, category: &str, x: f64| StatusDef {
        key: key.into(),
        name: name.into(),
        category: category.into(),
        x,
        y: 120.0,
    };
    let statuses = vec![
        status("todo", "To Do", "todo", 60.0),
        status("in_progress", "In Progress", "in_progress", 360.0),
        status("in_review", "In Review", "in_progress", 660.0),
        status("done", "Done", "done", 960.0),
    ];
    let transitions = statuses
        .iter()
        .map(|s| Transition {
            id: format!("to_{}", s.key),
            name: s.name.clone(),
            from: vec![],
            to: s.key.clone(),
            conditions: vec![],
            validators: vec![],
            post_functions: vec![],
        })
        .collect();
    Definition { statuses, initial: "todo".into(), transitions }
}

/// What the caller reports about the actor and the issue.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub actor_role: String,
    pub actor_is_assignee: bool,
    pub actor_is_reporter: bool,
    pub present_fields: HashSet<String>,
    pub children_done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision<'a> {
    pub to: &'a StatusDef,
    pub transition_name: String,
    pub post_functions: Vec<PostFunction>,
}

fn role_rank(role: &str) -> Option<usize> {
    ROLES.iter().position(|r| *r == role)
}

fn condition_holds(condition: &Condition, facts: &Facts) -> bool {
    match condition {
        // An unknown role on either side never satisfies the condition.
        Condition::RoleAtLeast { role } => {
            matches!((role_rank(&facts.actor_role), role_rank(role)), (Some(have), Some(need)) if have >= need)
        }
        Condition::IsAssignee {} => facts.actor_is_assignee,
        Condition::IsReporter {} => facts.actor_is_reporter,
    }
}

fn field_label(field: &str, label: Option<&String>) -> String {
    if let Some(label) = label {
        return label.clone();
    }
    let mut words = field.replace('_', " ");
    if let Some(first) = words.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    words
}

fn validator_failure(validator: &Validator, facts: &Facts, target: &str) -> Option<String> {
    match validator {
        Validator::FieldRequired { field, label } if !facts.present_fields.contains(field) => {
            Some(format!("{} is required to move this issue to {target}.", field_label(field, label.as_ref())))
        }
        Validator::ChildrenDone {} if !facts.children_done => {
            Some(format!("All child issues must be done before moving this issue to {target}."))
        }
        _ => None,
    }
}

impl Definition {
    fn candidates<'a, 'b: 'a>(&'b self, from: &'a str) -> impl Iterator<Item = &'b Transition> + 'a {
        self.transitions.iter().filter(move |t| t.to != from && (t.from.is_empty() || t.from.iter().any(|f| f == from)))
    }

    /// Transitions out of `from` that the actor is allowed to use (validators are not
    /// checked here: a person should see the step and be told what is missing).
    pub fn available(&self, from: &str, facts: &Facts) -> Vec<(&Transition, &StatusDef)> {
        if self.status(from).is_none() {
            // The issue sits in a status this workflow no longer has: the only way out is
            // back to the start.
            return self.status(&self.initial).map(|s| vec![(recovery(), s)]).unwrap_or_default();
        }
        let mut seen = HashSet::new();
        self.candidates(from)
            .filter(|t| t.conditions.iter().all(|c| condition_holds(c, facts)))
            .filter(|t| seen.insert(t.to.as_str()))
            .filter_map(|t| self.status(&t.to).map(|s| (t, s)))
            .collect()
    }

    /// Decide one status change. When several transitions connect the two statuses, the
    /// first whose conditions hold is used.
    pub fn evaluate(&self, from: &str, to: &str, facts: &Facts) -> Result<Decision<'_>, Denial> {
        let target = self.status(to).ok_or_else(|| Denial {
            code: "no_transition",
            message: "That status is not part of this issue's workflow.".into(),
        })?;
        if self.status(from).is_none() {
            return if to == self.initial {
                Ok(Decision { to: target, transition_name: recovery().name.clone(), post_functions: vec![] })
            } else {
                Err(Denial {
                    code: "no_transition",
                    message: format!(
                        "This issue's status was removed from its workflow. Move it to {} first.",
                        target_name(self)
                    ),
                })
            };
        }
        let candidates: Vec<&Transition> = self.candidates(from).filter(|t| t.to == to).collect();
        if candidates.is_empty() {
            let from_name = self.status(from).map_or(from, |s| s.name.as_str());
            return Err(Denial {
                code: "no_transition",
                message: format!("An issue cannot move from {from_name} to {}.", target.name),
            });
        }
        let Some(transition) = candidates.iter().find(|t| t.conditions.iter().all(|c| condition_holds(c, facts))) else {
            return Err(Denial {
                code: "condition_failed",
                message: format!("You are not allowed to move this issue to {}.", target.name),
            });
        };
        if let Some(message) = transition.validators.iter().find_map(|v| validator_failure(v, facts, &target.name)) {
            return Err(Denial { code: "validator_failed", message });
        }
        Ok(Decision { to: target, transition_name: transition.name.clone(), post_functions: transition.post_functions.clone() })
    }
}

fn target_name(def: &Definition) -> &str {
    def.status(&def.initial).map_or("the first status", |s| s.name.as_str())
}

fn recovery() -> &'static Transition {
    static RECOVERY: std::sync::OnceLock<Transition> = std::sync::OnceLock::new();
    RECOVERY.get_or_init(|| Transition {
        id: "recover".into(),
        name: "Move to start".into(),
        from: vec![],
        to: String::new(),
        conditions: vec![],
        validators: vec![],
        post_functions: vec![],
    })
}

/// Statuses of several definitions merged by key (first definition wins on conflicts).
pub fn union_statuses<'a>(definitions: impl IntoIterator<Item = &'a Definition>) -> Vec<StatusDef> {
    let mut seen: HashMap<&str, ()> = HashMap::new();
    let mut out = Vec::new();
    for def in definitions {
        for s in &def.statuses {
            if seen.insert(s.key.as_str(), ()).is_none() {
                out.push(s.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn review_flow() -> Definition {
        serde_json::from_value(json!({
            "statuses": [
                { "key": "backlog", "name": "Backlog", "category": "todo" },
                { "key": "in_progress", "name": "In Progress", "category": "in_progress" },
                { "key": "code_review", "name": "Code Review", "category": "in_progress" },
                { "key": "done", "name": "Done", "category": "done" }
            ],
            "initial": "backlog",
            "transitions": [
                { "id": "start", "name": "Start work", "from": ["backlog"], "to": "in_progress", "post_functions": [{ "type": "assign_to_current_user" }] },
                { "id": "review", "name": "Send for review", "from": ["in_progress"], "to": "code_review",
                  "validators": [{ "type": "field_required", "field": "assignee" }, { "type": "field_required", "field": "custom:0195c1e2-0000-7000-8000-000000000001", "label": "Pull request" }] },
                { "id": "approve", "name": "Approve", "from": ["code_review"], "to": "done",
                  "conditions": [{ "type": "role_at_least", "role": "administrator" }],
                  "validators": [{ "type": "children_done" }],
                  "post_functions": [{ "type": "set_resolution", "value": "done" }] },
                { "id": "self_approve", "name": "Reporter accepts", "from": ["code_review"], "to": "done", "conditions": [{ "type": "is_reporter" }] },
                { "id": "reopen", "name": "Reopen", "from": [], "to": "backlog", "post_functions": [{ "type": "clear_resolution" }, { "type": "clear_assignee" }] }
            ]
        }))
        .unwrap()
    }

    fn facts(role: &str, fields: &[&str]) -> Facts {
        Facts {
            actor_role: role.into(),
            present_fields: fields.iter().map(|f| (*f).to_owned()).collect(),
            children_done: true,
            ..Facts::default()
        }
    }

    #[test]
    fn default_and_example_workflows_are_valid() {
        assert_eq!(default_definition().problems(), vec![]);
        assert_eq!(review_flow().problems(), vec![]);
    }

    #[test]
    fn only_defined_transitions_are_possible() {
        let wf = review_flow();
        let member = facts("member", &["assignee"]);
        assert_eq!(wf.evaluate("backlog", "in_progress", &member).unwrap().transition_name, "Start work");
        let skip = wf.evaluate("backlog", "done", &member).unwrap_err();
        assert_eq!((skip.code, skip.message.as_str()), ("no_transition", "An issue cannot move from Backlog to Done."));
        assert_eq!(wf.evaluate("backlog", "nowhere", &member).unwrap_err().code, "no_transition");
        // A global transition applies from every status except its own target.
        assert!(wf.evaluate("done", "backlog", &member).is_ok());
        assert_eq!(wf.evaluate("backlog", "backlog", &member).unwrap_err().code, "no_transition");
    }

    #[test]
    fn conditions_decide_who_may_move() {
        let wf = review_flow();
        let denied = wf.evaluate("code_review", "done", &facts("member", &[])).unwrap_err();
        assert_eq!(
            (denied.code, denied.message.as_str()),
            ("condition_failed", "You are not allowed to move this issue to Done.")
        );
        assert_eq!(wf.evaluate("code_review", "done", &facts("administrator", &[])).unwrap().transition_name, "Approve");
        // A second transition to the same status with a different condition.
        let mut reporter = facts("member", &[]);
        reporter.actor_is_reporter = true;
        assert_eq!(wf.evaluate("code_review", "done", &reporter).unwrap().transition_name, "Reporter accepts");
        // Unknown or missing roles satisfy nothing.
        assert!(wf.evaluate("code_review", "done", &facts("superuser", &[])).is_err());
        assert!(wf.evaluate("code_review", "done", &facts("", &[])).is_err());
    }

    #[test]
    fn validators_explain_what_is_missing() {
        let wf = review_flow();
        let missing = wf.evaluate("in_progress", "code_review", &facts("member", &[])).unwrap_err();
        assert_eq!(
            (missing.code, missing.message.as_str()),
            ("validator_failed", "Assignee is required to move this issue to Code Review.")
        );
        let custom = wf.evaluate("in_progress", "code_review", &facts("member", &["assignee"])).unwrap_err();
        assert_eq!(custom.message, "Pull request is required to move this issue to Code Review.");
        assert!(wf
            .evaluate(
                "in_progress",
                "code_review",
                &facts("member", &["assignee", "custom:0195c1e2-0000-7000-8000-000000000001"])
            )
            .is_ok());

        let mut open_children = facts("administrator", &[]);
        open_children.children_done = false;
        assert_eq!(wf.evaluate("code_review", "done", &open_children).unwrap_err().code, "validator_failed");
    }

    #[test]
    fn post_functions_are_returned_for_the_caller_to_apply() {
        let wf = review_flow();
        let start = wf.evaluate("backlog", "in_progress", &facts("member", &[])).unwrap();
        assert_eq!(start.post_functions, vec![PostFunction::AssignToCurrentUser {}]);
        assert_eq!(start.to.category, "in_progress");
        let done = wf.evaluate("code_review", "done", &facts("administrator", &[])).unwrap();
        assert_eq!(done.post_functions, vec![PostFunction::SetResolution { value: "done".into() }]);
    }

    #[test]
    fn available_lists_what_the_actor_may_use() {
        let wf = review_flow();
        let names = |from: &str, f: &Facts| {
            wf.available(from, f).iter().map(|(t, s)| format!("{}→{}", t.name, s.key)).collect::<Vec<_>>()
        };
        assert_eq!(names("backlog", &facts("member", &[])), ["Start work→in_progress"]);
        assert_eq!(names("code_review", &facts("member", &[])), ["Reopen→backlog"]);
        // One entry per target status even when two transitions lead there.
        let mut both = facts("administrator", &[]);
        both.actor_is_reporter = true;
        assert_eq!(names("code_review", &both), ["Approve→done", "Reopen→backlog"]);
    }

    #[test]
    fn an_issue_in_a_removed_status_can_only_go_back_to_the_start() {
        let wf = review_flow();
        let f = facts("viewer", &[]);
        assert_eq!(wf.available("qa", &f).iter().map(|(_, s)| s.key.as_str()).collect::<Vec<_>>(), ["backlog"]);
        assert!(wf.evaluate("qa", "backlog", &f).is_ok());
        assert_eq!(wf.evaluate("qa", "done", &f).unwrap_err().code, "no_transition");
    }

    #[test]
    fn structural_problems_are_reported_with_their_location() {
        let mut wf = review_flow();
        wf.statuses.push(StatusDef { key: "Bad Key".into(), name: "Done".into(), category: "finished".into(), x: 0.0, y: 0.0 });
        wf.initial = "missing".into();
        wf.transitions[0].to = "nowhere".into();
        wf.transitions[1].from = vec!["ghost".into()];
        wf.transitions[2].id = wf.transitions[1].id.clone();
        wf.transitions[3].conditions = vec![Condition::RoleAtLeast { role: "god".into() }];
        wf.transitions[4].validators = vec![Validator::FieldRequired { field: "salary".into(), label: None }];
        let paths: Vec<String> = wf.problems().into_iter().map(|p| p.path).collect();
        for expected in [
            "statuses[4].key",
            "statuses[4].name",
            "statuses[4].category",
            "initial",
            "transitions[0].to",
            "transitions[1].from",
            "transitions[2].id",
            "transitions[3].conditions",
            "transitions[4].validators",
        ] {
            assert!(paths.contains(&expected.to_owned()), "missing {expected} in {paths:?}");
        }
    }

    #[test]
    fn unreachable_statuses_block_publishing() {
        let mut wf = review_flow();
        wf.statuses.push(StatusDef { key: "island".into(), name: "Island".into(), category: "todo".into(), x: 0.0, y: 0.0 });
        let problems = wf.problems();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].message, "No transition leads to “Island”, so issues can never reach it.");
        assert!(Definition { statuses: vec![], initial: String::new(), transitions: vec![] }
            .problems()
            .iter()
            .any(|p| p.path == "statuses"));
    }

    #[test]
    fn unknown_rule_types_and_fields_cannot_be_loaded() {
        for rule in [
            json!({ "conditions": [{ "type": "run_script", "code": "rm -rf /" }] }),
            json!({ "validators": [{ "type": "eval", "expr": "1+1" }] }),
            json!({ "post_functions": [{ "type": "http_request", "url": "http://169.254.169.254/" }] }),
            json!({ "post_functions": [{ "type": "clear_assignee", "extra": true }] }),
            json!({ "script": "x" }),
        ] {
            let mut t = json!({ "id": "t", "name": "T", "from": [], "to": "a" });
            t.as_object_mut().unwrap().extend(rule.as_object().unwrap().clone());
            let doc =
                json!({ "statuses": [{ "key": "a", "name": "A", "category": "todo" }], "initial": "a", "transitions": [t] });
            assert!(serde_json::from_value::<Definition>(doc.clone()).is_err(), "{doc}");
        }
    }
}
