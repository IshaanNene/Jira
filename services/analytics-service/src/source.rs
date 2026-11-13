//! Current issue state comes from issue-service, the owner of the data. Events say which
//! issue changed; the snapshot written to ClickHouse is always read fresh.

use api_contracts::issue::v1::{issues_client::IssuesClient, Document, GetDocumentsRequest};
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
}
