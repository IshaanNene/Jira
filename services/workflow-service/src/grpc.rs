use std::sync::Arc;

use api_contracts::workflow::v1::{
    workflow_engine_server::WorkflowEngine, EvaluateRequest, Evaluation, Facts as WireFacts, PostFunction as WirePostFunction,
    Status as WireStatus, TransitionOption, Transitions, TransitionsRequest, Workflow, WorkflowRequest,
};
use error_handling::ApiError;
use tenant_context::DbRouter;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::{
    engine::{Facts, PostFunction, StatusDef},
    resolve::{resolve, Resolved},
};

pub struct EngineService {
    pub db: Arc<DbRouter>,
}

fn id(field: &'static str, value: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value).map_err(|_| Status::from(ApiError::invalid(field, "invalid_uuid", "Must be a UUID.")))
}

fn wire_status(s: &StatusDef) -> WireStatus {
    WireStatus { key: s.key.clone(), name: s.name.clone(), category: s.category.clone() }
}

fn facts(wire: Option<WireFacts>) -> Facts {
    let w = wire.unwrap_or_default();
    Facts {
        actor_role: w.actor_role,
        actor_is_assignee: w.actor_is_assignee,
        actor_is_reporter: w.actor_is_reporter,
        present_fields: w.present_fields.into_iter().collect(),
        children_done: w.children_done,
    }
}

fn wire_post_function(p: &PostFunction) -> WirePostFunction {
    let (kind, value) = match p {
        PostFunction::AssignToCurrentUser {} => ("assign_to_current_user", ""),
        PostFunction::AssignToReporter {} => ("assign_to_reporter", ""),
        PostFunction::ClearAssignee {} => ("clear_assignee", ""),
        PostFunction::SetResolution { value } => ("set_resolution", value.as_str()),
        PostFunction::ClearResolution {} => ("clear_resolution", ""),
    };
    WirePostFunction { r#type: kind.to_owned(), value: value.to_owned() }
}

impl EngineService {
    async fn load(&self, request: Option<WorkflowRequest>) -> Result<Resolved, Status> {
        let req = request.ok_or_else(|| Status::invalid_argument("workflow is required"))?;
        let tenant_id = id("tenant_id", &req.tenant_id)?;
        let project_id = id("project_id", &req.project_id)?;
        let mut tx = self.db.begin(tenant_id).await?;
        let resolved = resolve(tx.conn(), tenant_id, project_id, &req.issue_type).await?;
        tx.commit().await?;
        Ok(resolved)
    }
}

#[tonic::async_trait]
impl WorkflowEngine for EngineService {
    async fn get_workflow(&self, request: Request<WorkflowRequest>) -> Result<Response<Workflow>, Status> {
        let resolved = self.load(Some(request.into_inner())).await?;
        Ok(Response::new(Workflow {
            workflow_id: resolved.workflow_id.map(|w| w.to_string()).unwrap_or_default(),
            name: resolved.name,
            version: resolved.version,
            statuses: resolved.definition.statuses.iter().map(wire_status).collect(),
            initial_status: resolved.definition.initial,
        }))
    }

    async fn list_transitions(&self, request: Request<TransitionsRequest>) -> Result<Response<Transitions>, Status> {
        let req = request.into_inner();
        let resolved = self.load(req.workflow).await?;
        let transitions = resolved
            .definition
            .available(&req.from_status, &facts(req.facts))
            .into_iter()
            .map(|(t, s)| TransitionOption { id: t.id.clone(), name: t.name.clone(), to: Some(wire_status(s)) })
            .collect();
        Ok(Response::new(Transitions { transitions }))
    }

    async fn evaluate_transition(&self, request: Request<EvaluateRequest>) -> Result<Response<Evaluation>, Status> {
        let req = request.into_inner();
        let resolved = self.load(req.workflow).await?;
        let evaluation = match resolved.definition.evaluate(&req.from_status, &req.to_status, &facts(req.facts)) {
            Ok(decision) => Evaluation {
                allowed: true,
                to: Some(wire_status(decision.to)),
                transition_name: decision.transition_name,
                post_functions: decision.post_functions.iter().map(wire_post_function).collect(),
                ..Evaluation::default()
            },
            Err(denial) => Evaluation {
                allowed: false,
                denial_code: denial.code.to_owned(),
                denial_message: denial.message,
                ..Evaluation::default()
            },
        };
        Ok(Response::new(evaluation))
    }
}
