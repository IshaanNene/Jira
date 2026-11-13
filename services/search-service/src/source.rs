//! Where documents come from: issue-service, the owner of the data. Events only say
//! *which* issue changed; the indexed content is always read fresh from the source.

use api_contracts::issue::v1::{issues_client::IssuesClient, Document, GetDocumentsRequest, ListIssueIdsRequest};
use async_trait::async_trait;
use auth_middleware::service_auth::{is_transient, lazy_channel, ServiceClientAuth};
use error_handling::{from_status, ApiResult};
use platform_common::resilience::{retry, RetryPolicy};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

/// Most documents asked for in one call (issue-service allows 500).
pub const DOCUMENT_BATCH: usize = 200;
/// Ids per page when listing a whole tenant.
pub const ID_PAGE: u32 = 1000;

#[derive(Debug, Default)]
pub struct Fetched {
    pub documents: Vec<Document>,
    /// Asked for but gone (deleted).
    pub missing: Vec<Uuid>,
}

#[async_trait]
pub trait Source: Send + Sync {
    async fn documents(&self, tenant_id: Uuid, ids: &[Uuid]) -> ApiResult<Fetched>;
    /// Issue ids of the tenant after `after`, in id order.
    async fn issue_ids(&self, tenant_id: Uuid, after: Option<Uuid>, limit: u32) -> ApiResult<Vec<Uuid>>;
}

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

fn ids(raw: Vec<String>) -> Vec<Uuid> {
    raw.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect()
}

#[async_trait]
impl Source for GrpcSource {
    async fn documents(&self, tenant_id: Uuid, wanted: &[Uuid]) -> ApiResult<Fetched> {
        let reply = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.issues.clone();
            let req =
                GetDocumentsRequest { tenant_id: tenant_id.to_string(), issue_ids: wanted.iter().map(Uuid::to_string).collect() };
            async move { client.get_documents(req).await }
        })
        .await
        .map_err(|s| from_status(&s, "issue-service"))?
        .into_inner();
        Ok(Fetched { documents: reply.documents, missing: ids(reply.missing_ids) })
    }

    async fn issue_ids(&self, tenant_id: Uuid, after: Option<Uuid>, limit: u32) -> ApiResult<Vec<Uuid>> {
        let reply = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.issues.clone();
            let req = ListIssueIdsRequest {
                tenant_id: tenant_id.to_string(),
                after_id: after.map(|a| a.to_string()).unwrap_or_default(),
                limit,
            };
            async move { client.list_issue_ids(req).await }
        })
        .await
        .map_err(|s| from_status(&s, "issue-service"))?
        .into_inner();
        Ok(ids(reply.ids))
    }
}
