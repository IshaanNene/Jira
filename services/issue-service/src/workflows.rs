//! Client side of the workflow engine. The rules live in workflow-service; this service
//! reports facts about the actor and the issue and applies the decision.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use api_contracts::workflow::v1::{
    workflow_engine_client::WorkflowEngineClient, EvaluateRequest, Facts as WireFacts, TransitionsRequest, WorkflowRequest,
};
use async_trait::async_trait;
use auth_middleware::service_auth::{is_transient, lazy_channel, ServiceClientAuth};
use error_handling::{from_status, ApiError, ApiResult};
use platform_common::resilience::{retry, CircuitBreaker, RetryPolicy};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusInfo {
    pub key: String,
    pub name: String,
    /// todo | in_progress | done
    pub category: String,
}

/// The workflow governing one (project, issue type): what to display, where issues start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowInfo {
    pub statuses: Vec<StatusInfo>,
    pub initial: String,
}

impl WorkflowInfo {
    pub fn status(&self, key: &str) -> Option<&StatusInfo> {
        self.statuses.iter().find(|s| s.key == key)
    }
}

/// What this service knows that workflow rules may depend on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueFacts {
    pub actor_role: String,
    pub actor_is_assignee: bool,
    pub actor_is_reporter: bool,
    /// Names of fields that have a value: built-ins and `custom:<field id>`.
    pub present_fields: Vec<String>,
    pub children_done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionOption {
    pub id: String,
    pub name: String,
    pub to: StatusInfo,
}

/// The closed set of effects a transition can have on an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostFunction {
    AssignToCurrentUser,
    AssignToReporter,
    ClearAssignee,
    SetResolution(String),
    ClearResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evaluation {
    Allowed {
        to: StatusInfo,
        post_functions: Vec<PostFunction>,
    },
    /// `message` is written for the person attempting the change.
    Denied {
        message: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub tenant_id: Uuid,
    pub project_id: Uuid,
    pub issue_type: &'a str,
}

#[async_trait]
pub trait Workflows: Send + Sync {
    async fn info(&self, scope: Scope<'_>) -> ApiResult<WorkflowInfo>;
    async fn transitions(&self, scope: Scope<'_>, from: &str, facts: &IssueFacts) -> ApiResult<Vec<TransitionOption>>;
    async fn evaluate(&self, scope: Scope<'_>, from: &str, to: &str, facts: &IssueFacts) -> ApiResult<Evaluation>;
}

type Client = WorkflowEngineClient<InterceptedService<Channel, ServiceClientAuth>>;
const SERVICE: &str = "workflow-service";
/// Display data (status names) may be this stale. Decisions are never cached.
const INFO_TTL: Duration = Duration::from_secs(15);

/// (tenant, project, issue type) → (fetched at, workflow)
type InfoCache = HashMap<(Uuid, Uuid, String), (Instant, WorkflowInfo)>;

pub struct GrpcWorkflows {
    client: Client,
    breaker: CircuitBreaker,
    info_cache: Mutex<InfoCache>,
}

impl GrpcWorkflows {
    pub fn new(url: &str, secret: &str) -> anyhow::Result<Self> {
        let auth = ServiceClientAuth::new(secret, crate::SERVICE_NAME, SERVICE)?;
        Ok(Self {
            client: WorkflowEngineClient::with_interceptor(lazy_channel(url)?, auth),
            breaker: CircuitBreaker::default(),
            info_cache: Mutex::new(HashMap::new()),
        })
    }

    fn guard(&self) -> ApiResult<()> {
        self.breaker.try_acquire().map_err(|_| ApiError::DependencyUnavailable(SERVICE))
    }

    fn settle<T>(&self, result: Result<tonic::Response<T>, tonic::Status>) -> ApiResult<T> {
        self.breaker.record(result.as_ref().map_or_else(|s| !is_transient(s), |_| true));
        result.map(tonic::Response::into_inner).map_err(|s| from_status(&s, SERVICE))
    }
}

fn request(scope: Scope<'_>) -> WorkflowRequest {
    WorkflowRequest {
        tenant_id: scope.tenant_id.to_string(),
        project_id: scope.project_id.to_string(),
        issue_type: scope.issue_type.to_owned(),
    }
}

fn wire_facts(f: &IssueFacts) -> WireFacts {
    WireFacts {
        actor_role: f.actor_role.clone(),
        actor_is_assignee: f.actor_is_assignee,
        actor_is_reporter: f.actor_is_reporter,
        present_fields: f.present_fields.clone(),
        children_done: f.children_done,
    }
}

fn status(wire: Option<api_contracts::workflow::v1::Status>) -> ApiResult<StatusInfo> {
    let s = wire.ok_or_else(|| ApiError::internal(anyhow::anyhow!("workflow-service omitted a status")))?;
    Ok(StatusInfo { key: s.key, name: s.name, category: s.category })
}

#[async_trait]
impl Workflows for GrpcWorkflows {
    async fn info(&self, scope: Scope<'_>) -> ApiResult<WorkflowInfo> {
        let key = (scope.tenant_id, scope.project_id, scope.issue_type.to_owned());
        if let Some((at, info)) = self.info_cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
            if at.elapsed() < INFO_TTL {
                return Ok(info);
            }
        }
        self.guard()?;
        // Read-only: safe to retry.
        let result = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.client.clone();
            let req = request(scope);
            async move { client.get_workflow(req).await }
        })
        .await;
        let reply = self.settle(result)?;
        let info = WorkflowInfo {
            statuses: reply.statuses.into_iter().map(|s| StatusInfo { key: s.key, name: s.name, category: s.category }).collect(),
            initial: reply.initial_status,
        };
        if let Ok(mut cache) = self.info_cache.lock() {
            if cache.len() > 10_000 {
                cache.clear();
            }
            cache.insert(key, (Instant::now(), info.clone()));
        }
        Ok(info)
    }

    async fn transitions(&self, scope: Scope<'_>, from: &str, facts: &IssueFacts) -> ApiResult<Vec<TransitionOption>> {
        self.guard()?;
        let req =
            TransitionsRequest { workflow: Some(request(scope)), from_status: from.to_owned(), facts: Some(wire_facts(facts)) };
        let reply = self.settle(self.client.clone().list_transitions(req).await)?;
        reply.transitions.into_iter().map(|t| Ok(TransitionOption { id: t.id, name: t.name, to: status(t.to)? })).collect()
    }

    async fn evaluate(&self, scope: Scope<'_>, from: &str, to: &str, facts: &IssueFacts) -> ApiResult<Evaluation> {
        self.guard()?;
        let req = EvaluateRequest {
            workflow: Some(request(scope)),
            from_status: from.to_owned(),
            to_status: to.to_owned(),
            facts: Some(wire_facts(facts)),
        };
        let reply = self.settle(self.client.clone().evaluate_transition(req).await)?;
        if !reply.allowed {
            return Ok(Evaluation::Denied { message: reply.denial_message });
        }
        let mut post_functions = Vec::new();
        for p in reply.post_functions {
            post_functions.push(match p.r#type.as_str() {
                "assign_to_current_user" => PostFunction::AssignToCurrentUser,
                "assign_to_reporter" => PostFunction::AssignToReporter,
                "clear_assignee" => PostFunction::ClearAssignee,
                "set_resolution" => PostFunction::SetResolution(p.value),
                "clear_resolution" => PostFunction::ClearResolution,
                // An effect this build does not implement must stop the transition rather
                // than be skipped: half-applied workflows are worse than a clear failure.
                other => return Err(ApiError::internal(anyhow::anyhow!("workflow requested unknown post-function {other:?}"))),
            });
        }
        Ok(Evaluation::Allowed { to: status(reply.to)?, post_functions })
    }
}

/// "in_review" → "In review": what to show for a status the workflow no longer defines,
/// or when the workflow service cannot be reached.
pub fn humanize(key: &str) -> String {
    let mut out = key.replace('_', " ");
    if let Some(first) = out.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    out
}
