//! Custom field definitions and value validation. Pure logic: no I/O.
//!
//! Definitions are rows; values live in `issues.custom_fields` as `{ field_id: value }`.
//! Nothing a client sends is stored without passing [`validate_value`] for its field.

use std::collections::BTreeMap;

use error_handling::{ApiError, ApiResult, FieldError};
use serde_json::{Map, Value};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

pub const FIELD_TYPES: [&str; 8] = ["text", "number", "date", "select", "multi_select", "checkbox", "url", "user"];

#[derive(Debug, Clone, FromRow)]
pub struct Field {
    pub id: Uuid,
    pub name: String,
    pub field_type: String,
    pub options: Vec<String>,
    pub required: bool,
    pub project_keys: Vec<String>,
    pub issue_types: Vec<String>,
    pub created_at: OffsetDateTime,
}

impl Field {
    /// Whether this field is part of issues of `issue_type` in `project_key`.
    pub fn applies_to(&self, project_key: &str, issue_type: &str) -> bool {
        (self.project_keys.is_empty() || self.project_keys.iter().any(|k| k == project_key))
            && (self.issue_types.is_empty() || self.issue_types.iter().any(|t| t == issue_type))
    }
}

/// Validate the definition itself (used when an administrator creates or edits a field).
pub fn validate_definition(
    name: &str,
    field_type: &str,
    options: &[String],
    project_keys: &[String],
    issue_types: &[String],
) -> ApiResult<()> {
    if name.trim().is_empty() || name.chars().count() > 60 {
        return Err(ApiError::invalid("name", "invalid_name", "The name must be between 1 and 60 characters."));
    }
    if !FIELD_TYPES.contains(&field_type) {
        return Err(ApiError::invalid("field_type", "unknown_type", "Unknown field type."));
    }
    let has_options = matches!(field_type, "select" | "multi_select");
    if has_options {
        let mut seen = std::collections::HashSet::new();
        let ok = (1..=100).contains(&options.len())
            && options.iter().all(|o| !o.trim().is_empty() && o.chars().count() <= 80 && seen.insert(o.to_lowercase()));
        if !ok {
            return Err(ApiError::invalid(
                "options",
                "invalid_options",
                "Provide 1–100 distinct options of up to 80 characters.",
            ));
        }
    } else if !options.is_empty() {
        return Err(ApiError::invalid("options", "unexpected_options", "Only dropdown fields have options."));
    }
    if project_keys.len() > 200 || project_keys.iter().any(|k| !crate::domain::is_project_key(k)) {
        return Err(ApiError::invalid("project_keys", "invalid_project", "A project key is not valid."));
    }
    for t in issue_types {
        crate::domain::issue_type(t).map_err(|_| ApiError::invalid("issue_types", "unknown_type", "Unknown issue type."))?;
    }
    Ok(())
}

fn bad(field: &Field, message: &str) -> ApiError {
    ApiError::Validation(vec![FieldError::new(
        format!("custom_fields.{}", field.id),
        "invalid_value",
        format!("{}: {message}", field.name),
    )])
}

/// Check `value` against the field's type and return it in canonical form.
pub fn validate_value(field: &Field, value: &Value) -> ApiResult<Value> {
    match field.field_type.as_str() {
        "text" => match value.as_str().map(str::trim) {
            Some(s) if !s.is_empty() && s.chars().count() <= 2000 => Ok(Value::String(s.to_owned())),
            _ => Err(bad(field, "enter up to 2000 characters of text.")),
        },
        "number" => match value.as_f64() {
            Some(n) if n.is_finite() && n.abs() <= 1e15 => Ok(value.clone()),
            _ => Err(bad(field, "enter a number.")),
        },
        "date" => {
            let format = time::macros::format_description!("[year]-[month]-[day]");
            match value.as_str().and_then(|s| time::Date::parse(s, &format).ok()) {
                Some(_) => Ok(value.clone()),
                None => Err(bad(field, "enter a date as YYYY-MM-DD.")),
            }
        }
        "select" => match value.as_str() {
            Some(s) if field.options.iter().any(|o| o == s) => Ok(value.clone()),
            _ => Err(bad(field, "choose one of the available options.")),
        },
        "multi_select" => {
            let items = value.as_array().ok_or_else(|| bad(field, "choose from the available options."))?;
            let mut chosen: Vec<&str> = Vec::new();
            for item in items {
                match item.as_str() {
                    Some(s) if field.options.iter().any(|o| o == s) => {
                        if !chosen.contains(&s) {
                            chosen.push(s);
                        }
                    }
                    _ => return Err(bad(field, "choose from the available options.")),
                }
            }
            if chosen.is_empty() {
                return Err(bad(field, "choose at least one option, or clear the field."));
            }
            // Stored in the definition's option order so equal selections compare equal.
            Ok(Value::Array(
                field.options.iter().filter(|o| chosen.contains(&o.as_str())).map(|o| Value::String(o.clone())).collect(),
            ))
        }
        "checkbox" => value.as_bool().map(Value::Bool).ok_or_else(|| bad(field, "must be true or false.")),
        "url" => match value.as_str().map(str::trim) {
            Some(s)
                if s.len() <= 2000
                    && !s.chars().any(|c| c.is_control() || c.is_whitespace())
                    && (s.to_ascii_lowercase().starts_with("https://") || s.to_ascii_lowercase().starts_with("http://")) =>
            {
                Ok(Value::String(s.to_owned()))
            }
            _ => Err(bad(field, "enter a link starting with http:// or https://.")),
        },
        "user" => match value.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
            Some(id) => Ok(Value::String(id.to_string())),
            None => Err(bad(field, "choose a person.")),
        },
        _ => Err(bad(field, "this field cannot be set.")),
    }
}

pub struct Applied {
    /// The full value map to store.
    pub values: Map<String, Value>,
    /// `(field name, old, new)` for every value that changed.
    pub changes: Vec<(String, Value, Value)>,
    /// People referenced by `user` fields that were set; the caller verifies membership.
    pub users: Vec<Uuid>,
}

/// Merge `incoming` (`null` clears) into `current`, validating each value against its
/// definition and the issue's context.
pub fn apply(
    fields: &[Field],
    project_key: &str,
    issue_type: &str,
    current: &Map<String, Value>,
    incoming: &Map<String, Value>,
    creating: bool,
) -> ApiResult<Applied> {
    let by_id: BTreeMap<String, &Field> = fields.iter().map(|f| (f.id.to_string(), f)).collect();
    let mut values = current.clone();
    let mut changes = Vec::new();
    let mut users = Vec::new();

    for (id, value) in incoming {
        let field = by_id.get(id).copied().filter(|f| f.applies_to(project_key, issue_type)).ok_or_else(|| {
            ApiError::invalid(
                &format!("custom_fields.{id}"),
                "unknown_field",
                "This field does not exist for this kind of issue.",
            )
        })?;
        let old = current.get(id).cloned().unwrap_or(Value::Null);
        let new = if value.is_null() {
            if field.required {
                return Err(bad(field, "this field is required."));
            }
            values.remove(id);
            Value::Null
        } else {
            let normalized = validate_value(field, value)?;
            if field.field_type == "user" {
                if let Some(user) = normalized.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                    users.push(user);
                }
            }
            values.insert(id.clone(), normalized.clone());
            normalized
        };
        if old != new {
            changes.push((field.name.clone(), old, new));
        }
    }

    if creating {
        for field in fields.iter().filter(|f| f.required && f.applies_to(project_key, issue_type)) {
            if !values.contains_key(&field.id.to_string()) {
                return Err(bad(field, "this field is required."));
            }
        }
    }
    Ok(Applied { values, changes, users })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn field(field_type: &str, options: &[&str]) -> Field {
        Field {
            id: Uuid::now_v7(),
            name: format!("My {field_type}"),
            field_type: field_type.into(),
            options: options.iter().map(|s| (*s).to_owned()).collect(),
            required: false,
            project_keys: vec![],
            issue_types: vec![],
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn values_are_checked_against_their_type() {
        let cases: Vec<(Field, Vec<Value>, Vec<Value>)> = vec![
            (field("text", &[]), vec![json!("hello")], vec![json!(""), json!(5), json!("x".repeat(2001))]),
            (field("number", &[]), vec![json!(3), json!(-2.5)], vec![json!("3"), json!(null), json!(1e300)]),
            (field("date", &[]), vec![json!("2026-12-24")], vec![json!("24/12/2026"), json!("2026-13-01"), json!(20261224)]),
            (field("select", &["Low", "High"]), vec![json!("High")], vec![json!("high"), json!("Other"), json!(["High"])]),
            (field("multi_select", &["A", "B", "C"]), vec![json!(["C", "A"])], vec![json!([]), json!(["Z"]), json!("A")]),
            (field("checkbox", &[]), vec![json!(true), json!(false)], vec![json!("true"), json!(1)]),
            (
                field("url", &[]),
                vec![json!("https://example.com/x")],
                vec![json!("javascript:alert(1)"), json!("example.com"), json!("https://a b")],
            ),
            (field("user", &[]), vec![json!(Uuid::now_v7().to_string())], vec![json!("ada"), json!(7)]),
        ];
        for (f, good, bad_values) in cases {
            for v in good {
                assert!(validate_value(&f, &v).is_ok(), "{} should accept {v}", f.field_type);
            }
            for v in bad_values {
                assert!(validate_value(&f, &v).is_err(), "{} should reject {v}", f.field_type);
            }
        }
        let multi = field("multi_select", &["A", "B", "C"]);
        assert_eq!(validate_value(&multi, &json!(["C", "A", "C"])).unwrap(), json!(["A", "C"]), "canonical order, no duplicates");
    }

    #[test]
    fn context_limits_where_a_field_applies() {
        let mut f = field("text", &[]);
        assert!(f.applies_to("PAY", "bug"));
        f.project_keys = vec!["PAY".into()];
        f.issue_types = vec!["bug".into()];
        assert!(f.applies_to("PAY", "bug"));
        assert!(!f.applies_to("OPS", "bug"));
        assert!(!f.applies_to("PAY", "task"));

        let values = json!({ f.id.to_string(): "x" });
        let incoming = values.as_object().unwrap();
        assert!(apply(std::slice::from_ref(&f), "PAY", "bug", &Map::new(), incoming, true).is_ok());
        assert!(apply(std::slice::from_ref(&f), "OPS", "bug", &Map::new(), incoming, true).is_err(), "not applicable here");
        let unknown = json!({ Uuid::now_v7().to_string(): "x" });
        assert!(apply(&[f], "PAY", "bug", &Map::new(), unknown.as_object().unwrap(), true).is_err());
    }

    #[test]
    fn required_fields_and_merging() {
        let mut severity = field("select", &["S1", "S2"]);
        severity.required = true;
        let notes = field("text", &[]);
        let fields = [severity.clone(), notes.clone()];
        let (sid, nid) = (severity.id.to_string(), notes.id.to_string());

        assert!(apply(&fields, "PAY", "bug", &Map::new(), &Map::new(), true).is_err(), "required on create");
        let created =
            apply(&fields, "PAY", "bug", &Map::new(), json!({ &sid: "S1", &nid: "n" }).as_object().unwrap(), true).unwrap();
        assert_eq!(created.changes.len(), 2);

        // Update: untouched values stay, null clears optional ones, required ones cannot be cleared.
        let updated = apply(&fields, "PAY", "bug", &created.values, json!({ &nid: null }).as_object().unwrap(), false).unwrap();
        assert_eq!(updated.values.get(&sid), Some(&json!("S1")));
        assert!(!updated.values.contains_key(&nid));
        assert_eq!(updated.changes, vec![(notes.name.clone(), json!("n"), Value::Null)]);
        assert!(apply(&fields, "PAY", "bug", &created.values, json!({ &sid: null }).as_object().unwrap(), false).is_err());
        let same = apply(&fields, "PAY", "bug", &created.values, json!({ &sid: "S1" }).as_object().unwrap(), false).unwrap();
        assert!(same.changes.is_empty(), "unchanged values are not changes");
    }

    #[test]
    fn definitions_are_validated() {
        let opts = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(validate_definition("Severity", "select", &opts(&["S1", "S2"]), &[], &[]).is_ok());
        assert!(validate_definition("", "text", &[], &[], &[]).is_err());
        assert!(validate_definition("X", "blob", &[], &[], &[]).is_err());
        assert!(validate_definition("X", "select", &[], &[], &[]).is_err(), "dropdowns need options");
        assert!(validate_definition("X", "select", &opts(&["a", "A"]), &[], &[]).is_err(), "duplicate options");
        assert!(validate_definition("X", "text", &opts(&["a"]), &[], &[]).is_err());
        assert!(validate_definition("X", "text", &[], &opts(&["not a key"]), &[]).is_err());
        assert!(validate_definition("X", "text", &[], &opts(&["PAY"]), &opts(&["saga"])).is_err());
    }
}
