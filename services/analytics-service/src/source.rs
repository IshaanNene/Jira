//! Current issue state comes from issue-service, the owner of the data. Events say which
//! issue changed; the snapshot written to ClickHouse is always read fresh.

use api_contracts::issue::v1::{issues_client::IssuesClient, Document, GetDocumentsRequest, ListIssueIdsRequest};
use async_trait::async_trait;
use auth_middleware::service_auth::{is_transient, lazy_channel, ServiceClientAuth};
use error_handling::{from_status, ApiResult};
use platform_common::resilience::{retry, RetryPolicy};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

#[async_trait]
pub trait Source: Send + Sync {
    /// The issue as it is now, or `None` if it no longer exists.
    async fn document(&self, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Option<Document>>;

    /// The issues that still exist among `issue_ids`, as they are now.
    async fn documents(&self, tenant_id: Uuid, issue_ids: &[Uuid]) -> ApiResult<Vec<Document>> {
        let mut found = Vec::with_capacity(issue_ids.len());
        for id in issue_ids {
            found.extend(self.document(tenant_id, *id).await?);
        }
        Ok(found)
    }

    /// One page of the tenant's issue ids in id order, after `after` (rebuilds only).
    async fn issue_ids(&self, tenant_id: Uuid, after: Option<Uuid>, limit: u32) -> ApiResult<Vec<Uuid>>;
}

/// `GetDocuments` serves at most this many issues per call.
const PER_CALL: usize = 500;

pub struct GrpcSource {
    issues: IssuesClient<InterceptedService<Channel, ServiceClientAuth>>,
}

impl GrpcSource {
    pub fn new(issue_url: &str, secret: &str) -> anyhow::Result<Self> {
        Ok(Self {
            issues: IssuesClient::with_interceptor(
                lazy_channel(issue_url)?,
                ServiceClientAuth::new(secret, crate::SERVICE_NAME, "issue-service")?,
            ),
        })
    }
}

#[async_trait]
impl Source for GrpcSource {
    async fn document(&self, tenant_id: Uuid, issue_id: Uuid) -> ApiResult<Option<Document>> {
        let reply = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.issues.clone();
            let req = GetDocumentsRequest { tenant_id: tenant_id.to_string(), issue_ids: vec![issue_id.to_string()] };
            async move { client.get_documents(req).await }
        })
        .await
        .map_err(|s| from_status(&s, "issue-service"))?
        .into_inner();
        Ok(reply.documents.into_iter().next())
    }

    async fn documents(&self, tenant_id: Uuid, issue_ids: &[Uuid]) -> ApiResult<Vec<Document>> {
        let mut found = Vec::with_capacity(issue_ids.len());
        for chunk in issue_ids.chunks(PER_CALL) {
            let ids: Vec<String> = chunk.iter().map(Uuid::to_string).collect();
            let reply = retry(RetryPolicy::default(), is_transient, || {
                let mut client = self.issues.clone();
                let req = GetDocumentsRequest { tenant_id: tenant_id.to_string(), issue_ids: ids.clone() };
                async move { client.get_documents(req).await }
            })
            .await
            .map_err(|s| from_status(&s, "issue-service"))?
            .into_inner();
            found.extend(reply.documents);
        }
        Ok(found)
    }

    async fn issue_ids(&self, tenant_id: Uuid, after: Option<Uuid>, limit: u32) -> ApiResult<Vec<Uuid>> {
        let reply = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.issues.clone();
            let req = ListIssueIdsRequest {
                tenant_id: tenant_id.to_string(),
                after_id: after.map(|a| a.to_string()).unwrap_or_default(),
                limit,
                project_id: String::new(),
            };
            async move { client.list_issue_ids(req).await }
        })
        .await
        .map_err(|s| from_status(&s, "issue-service"))?
        .into_inner();
        Ok(reply.ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect())
    }
}
