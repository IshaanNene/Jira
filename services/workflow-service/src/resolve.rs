//! Which workflow governs an issue. Shared by the gRPC engine and the REST API.

use error_handling::{ApiError, ApiResult};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    engine::{default_definition, union_statuses, Definition, StatusDef},
    store,
};

pub struct Resolved {
    /// `None` for the built-in default workflow.
    pub workflow_id: Option<Uuid>,
    pub name: String,
    pub version: i32,
    pub definition: Definition,
}

fn parse(value: serde_json::Value) -> ApiResult<Definition> {
    // Published definitions were validated on the way in; failing here means stored data
    // no longer matches the model, which must be loud rather than silently permissive.
    serde_json::from_value(value).map_err(|e| ApiError::internal(anyhow::anyhow!("stored workflow definition is invalid: {e}")))
}

pub async fn resolve(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid, issue_type: &str) -> ApiResult<Resolved> {
    match store::resolve(conn, tenant_id, project_id, issue_type).await? {
        Some((id, name, version, definition)) => {
            Ok(Resolved { workflow_id: Some(id), name, version, definition: parse(definition)? })
        }
        None => Ok(Resolved { workflow_id: None, name: "Default workflow".into(), version: 0, definition: default_definition() }),
    }
}

/// All statuses an issue of this project can be in, for filters.
pub async fn project_statuses(conn: &mut PgConnection, tenant_id: Uuid, project_id: Uuid) -> ApiResult<Vec<StatusDef>> {
    let rows = store::definitions_for_project(conn, tenant_id, project_id).await?;
    // The default applies to any issue type without an "all types" assignment.
    let covers_all_types = rows.iter().any(|(issue_type, _, _)| issue_type.is_none());
    let mut definitions = Vec::new();
    for (_, _, value) in rows {
        definitions.push(parse(value)?);
    }
    if !covers_all_types {
        definitions.push(default_definition());
    }
    Ok(union_statuses(&definitions))
}
