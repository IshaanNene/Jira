//! Who a query means by `assignee = maya@example.com` or `reporter = "Maya Chen"`:
//! exact email or display name, ignoring case, among the workspace's members
//! (user-service).

use std::collections::HashMap;

use api_contracts::user::v1::{membership_client::MembershipClient, FindMembersRequest, GetMembersRequest};
use async_trait::async_trait;
use auth_middleware::{
    service_auth::{is_transient, lazy_channel, ServiceClientAuth},
    OrgRole,
};
use error_handling::{from_status, ApiResult};
use platform_common::resilience::{retry, RetryPolicy};
use tonic::{service::interceptor::InterceptedService, transport::Channel};
use uuid::Uuid;

use crate::mql::person_key;

#[async_trait]
pub trait People: Send + Sync {
    /// For each term (as given by [`crate::mql::person_terms`]), the members it names.
    async fn find(&self, tenant_id: Uuid, terms: &[String]) -> ApiResult<HashMap<String, Vec<Uuid>>>;
    /// The person's workspace role while they are an active member; `None` otherwise.
    async fn active_role(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<Option<OrgRole>>;
}

pub struct GrpcPeople {
    members: MembershipClient<InterceptedService<Channel, ServiceClientAuth>>,
}

impl GrpcPeople {
    pub fn new(user_url: &str, secret: &str) -> anyhow::Result<Self> {
        Ok(Self {
            members: MembershipClient::with_interceptor(
                lazy_channel(user_url)?,
                ServiceClientAuth::new(secret, crate::SERVICE_NAME, "user-service")?,
            ),
        })
    }
}

#[async_trait]
impl People for GrpcPeople {
    async fn find(&self, tenant_id: Uuid, terms: &[String]) -> ApiResult<HashMap<String, Vec<Uuid>>> {
        let reply = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.members.clone();
            let req = FindMembersRequest { tenant_id: tenant_id.to_string(), terms: terms.to_vec() };
            async move { client.find_members(req).await }
        })
        .await
        .map_err(|s| from_status(&s, "user-service"))?
        .into_inner();
        let mut found: HashMap<String, Vec<Uuid>> = HashMap::new();
        for m in reply.members {
            let Ok(id) = Uuid::parse_str(&m.user_id) else { continue };
            // One person can match a term by email and by name; count them once.
            let mut keys = vec![person_key(&m.email), person_key(&m.display_name)];
            keys.dedup();
            for key in keys {
                if terms.contains(&key) {
                    let ids = found.entry(key).or_default();
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
        Ok(found)
    }

    async fn active_role(&self, tenant_id: Uuid, user_id: Uuid) -> ApiResult<Option<OrgRole>> {
        let reply = retry(RetryPolicy::default(), is_transient, || {
            let mut client = self.members.clone();
            let req = GetMembersRequest { tenant_id: tenant_id.to_string(), user_ids: vec![user_id.to_string()] };
            async move { client.get_members(req).await }
        })
        .await
        .map_err(|s| from_status(&s, "user-service"))?
        .into_inner();
        Ok(reply
            .members
            .into_iter()
            .find(|m| m.user_id == user_id.to_string() && m.status == "active")
            .and_then(|m| OrgRole::parse(&m.org_role)))
    }
}
