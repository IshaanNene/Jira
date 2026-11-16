//! SCIM 2.0 (RFC 7643 / 7644), the protocol half: reading a User resource, applying a PATCH,
//! parsing the filters identity providers send, and rendering resources and errors. No I/O.
//!
//! Only what Okta, Microsoft Entra ID and OneLogin use for user provisioning is supported: the
//! core User schema's `userName`, `name.givenName`, `name.familyName`, `displayName`, `emails`,
//! `externalId` and `active`, and `eq` filters on `userName`, `externalId`, `emails.value` and
//! `id`. Attributes we do not keep (titles, phone numbers, the enterprise extension) are accepted
//! and ignored, as the RFC allows. Groups are not supported.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Map, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

pub const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
pub const LIST_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
pub const ERROR_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
pub const CONTENT_TYPE: &str = "application/scim+json";

/// Most operations one PATCH may carry.
const MAX_OPERATIONS: usize = 100;

/// A SCIM error response (RFC 7644 §3.12). `detail` is shown to the provider's admin, so it
/// never contains anything the token holder could not already see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScimError {
    pub status: StatusCode,
    pub scim_type: Option<&'static str>,
    pub detail: String,
}

impl ScimError {
    pub fn new(status: StatusCode, scim_type: Option<&'static str>, detail: impl Into<String>) -> Self {
        Self { status, scim_type, detail: detail.into() }
    }
    pub fn invalid_value(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, Some("invalidValue"), detail)
    }
    pub fn uniqueness(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, Some("uniqueness"), detail)
    }
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, None, "No such user.")
    }
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, None, "The provisioning token is missing, revoked or no longer valid.")
    }
}

impl IntoResponse for ScimError {
    fn into_response(self) -> Response {
        let mut body = json!({ "schemas": [ERROR_SCHEMA], "status": self.status.as_u16().to_string(), "detail": self.detail });
        if let Some(t) = self.scim_type {
            body["scimType"] = json!(t);
        }
        scim_json(self.status, &body)
    }
}

/// A JSON response with the SCIM media type.
pub fn scim_json(status: StatusCode, body: &Value) -> Response {
    let mut response = (status, body.to_string()).into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE));
    response
}

/// A person as the identity provider describes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub user_name: String,
    /// The primary email when the provider sends one that differs from `userName`.
    pub email: Option<String>,
    pub external_id: Option<String>,
    pub display_name: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub active: bool,
}

impl Person {
    /// The address invitations go to and members are matched by.
    pub fn address(&self) -> Result<String, ScimError> {
        let address = self.email.as_deref().unwrap_or(&self.user_name).trim().to_lowercase();
        if plausible_email(&address) {
            Ok(address)
        } else {
            Err(ScimError::invalid_value("userName, or the primary email, must be an email address."))
        }
    }

    /// Read a full User resource (POST and PUT).
    pub fn from_resource(body: &Value) -> Result<Self, ScimError> {
        let object = body.as_object().ok_or_else(|| ScimError::invalid_value("The body must be a User resource."))?;
        let mut person = Person {
            user_name: String::new(),
            email: None,
            external_id: None,
            display_name: None,
            given_name: None,
            family_name: None,
            active: true,
        };
        for (key, value) in object {
            person.set(key, value)?;
        }
        if person.user_name.is_empty() {
            return Err(ScimError::invalid_value("userName is required."));
        }
        person.address()?;
        Ok(person)
    }

    /// Apply a PatchOp request (RFC 7644 §3.5.2) and return the result.
    pub fn patched(&self, body: &Value) -> Result<Self, ScimError> {
        let operations = body
            .get("Operations")
            .or_else(|| body.get("operations"))
            .and_then(Value::as_array)
            .ok_or_else(|| ScimError::new(StatusCode::BAD_REQUEST, Some("invalidSyntax"), "Operations is required."))?;
        if operations.len() > MAX_OPERATIONS {
            return Err(ScimError::new(StatusCode::BAD_REQUEST, Some("tooMany"), "Too many operations in one request."));
        }
        let mut next = self.clone();
        for operation in operations {
            let op = operation.get("op").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
            let path = operation.get("path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty());
            let value = operation.get("value").unwrap_or(&Value::Null);
            match (op.as_str(), path) {
                ("add" | "replace", None) => {
                    let object = value
                        .as_object()
                        .ok_or_else(|| ScimError::invalid_value("An operation without a path needs an object value."))?;
                    for (key, value) in object {
                        next.set(key, value)?;
                    }
                }
                ("add" | "replace", Some(path)) => next.set(path, value)?,
                ("remove", Some(path)) => next.set(path, &Value::Null)?,
                ("remove", None) => {
                    return Err(ScimError::new(StatusCode::BAD_REQUEST, Some("noTarget"), "remove needs a path."));
                }
                _ => {
                    return Err(ScimError::new(
                        StatusCode::BAD_REQUEST,
                        Some("invalidSyntax"),
                        "op must be add, replace or remove.",
                    ))
                }
            }
        }
        if next.user_name.is_empty() {
            return Err(ScimError::new(StatusCode::BAD_REQUEST, Some("mutability"), "userName cannot be removed."));
        }
        next.address()?;
        Ok(next)
    }

    /// Set one attribute by name or path. `null` clears an optional attribute.
    fn set(&mut self, key: &str, value: &Value) -> Result<(), ScimError> {
        let key = key.trim();
        // Paths may be written in full: "urn:...:core:2.0:User:name.givenName".
        let key = key
            .get(..USER_SCHEMA.len() + 1)
            .filter(|p| p.eq_ignore_ascii_case(&format!("{USER_SCHEMA}:")))
            .map_or(key, |_| &key[USER_SCHEMA.len() + 1..]);
        let lower = key.to_ascii_lowercase();
        match lower.as_str() {
            "username" => {
                self.user_name = text(value, "userName", 320)?.unwrap_or_default();
            }
            "externalid" => self.external_id = text(value, "externalId", 200)?,
            "displayname" => self.display_name = text(value, "displayName", 200)?,
            "name.givenname" => self.given_name = text(value, "name.givenName", 100)?,
            "name.familyname" => self.family_name = text(value, "name.familyName", 100)?,
            "name" => match value {
                Value::Null => {
                    self.given_name = None;
                    self.family_name = None;
                }
                Value::Object(name) => {
                    for (part, v) in name {
                        match part.to_ascii_lowercase().as_str() {
                            "givenname" => self.given_name = text(v, "name.givenName", 100)?,
                            "familyname" => self.family_name = text(v, "name.familyName", 100)?,
                            _ => {}
                        }
                    }
                }
                _ => return Err(ScimError::invalid_value("name must be an object.")),
            },
            "active" => {
                self.active = match value {
                    Value::Bool(b) => *b,
                    // Microsoft Entra ID sends "True" and "False" as strings.
                    Value::String(s) if s.eq_ignore_ascii_case("true") => true,
                    Value::String(s) if s.eq_ignore_ascii_case("false") => false,
                    _ => return Err(ScimError::invalid_value("active must be true or false.")),
                }
            }
            "emails" => self.email = primary_email(value)?,
            _ if lower.starts_with("emails[") && lower.ends_with("].value") => {
                self.email = text(value, "emails.value", 320)?;
            }
            // id, meta and schemas are the service's; everything else is accepted and not kept.
            _ => {}
        }
        Ok(())
    }

    /// The User resource for this person.
    pub fn to_resource(&self, stored: &Stored, location: &str) -> Value {
        let mut resource = Map::new();
        resource.insert("schemas".into(), json!([USER_SCHEMA]));
        resource.insert("id".into(), json!(stored.id));
        if let Some(external_id) = &self.external_id {
            resource.insert("externalId".into(), json!(external_id));
        }
        resource.insert("userName".into(), json!(self.user_name));
        let mut name = Map::new();
        if let Some(g) = &self.given_name {
            name.insert("givenName".into(), json!(g));
        }
        if let Some(f) = &self.family_name {
            name.insert("familyName".into(), json!(f));
        }
        if !name.is_empty() {
            resource.insert("name".into(), Value::Object(name));
        }
        if let Some(d) = &self.display_name {
            resource.insert("displayName".into(), json!(d));
        }
        let address = self.email.clone().unwrap_or_else(|| self.user_name.clone());
        resource.insert("emails".into(), json!([{ "value": address, "type": "work", "primary": true }]));
        resource.insert("active".into(), json!(self.active));
        resource.insert(
            "meta".into(),
            json!({
                "resourceType": "User",
                "created": rfc3339(stored.created_at),
                "lastModified": rfc3339(stored.updated_at),
                "location": location,
                "version": format!("W/\"{}\"", stored.version),
            }),
        );
        Value::Object(resource)
    }
}

/// What the service keeps about a provisioned person besides the attributes.
#[derive(Debug, Clone)]
pub struct Stored {
    pub id: Uuid,
    pub version: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

/// A bounded single-line string, or `None` for null or blank.
fn text(value: &Value, attribute: &str, max: usize) -> Result<Option<String>, ScimError> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => {
            let s = s.trim();
            if s.chars().count() > max || s.chars().any(char::is_control) {
                Err(ScimError::invalid_value(format!("{attribute} is too long or contains control characters.")))
            } else {
                Ok((!s.is_empty()).then(|| s.to_owned()))
            }
        }
        _ => Err(ScimError::invalid_value(format!("{attribute} must be a string."))),
    }
}

/// The primary email of an `emails` value: the one marked primary, else the work one, else the first.
fn primary_email(value: &Value) -> Result<Option<String>, ScimError> {
    let emails = match value {
        Value::Null => return Ok(None),
        Value::Array(items) => items,
        _ => return Err(ScimError::invalid_value("emails must be a list.")),
    };
    let pick = emails
        .iter()
        .find(|e| e.get("primary").and_then(Value::as_bool) == Some(true))
        .or_else(|| emails.iter().find(|e| e.get("type").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("work"))))
        .or_else(|| emails.first());
    match pick {
        Some(email) => text(email.get("value").unwrap_or(&Value::Null), "emails.value", 320),
        None => Ok(None),
    }
}

/// A deliberately simple address check: one `@`, something on each side, a dot in the domain,
/// no spaces. Whether the address is real is proved when its owner accepts the invitation.
pub fn plausible_email(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else { return false };
    (3..=320).contains(&address.len())
        && !local.is_empty()
        && local.len() <= 64
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !address.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The filters supported on `GET /Users`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    UserName(String),
    ExternalId(String),
    Email(String),
    Id(Uuid),
    /// An `id` that is not one of ours: matches nothing.
    Nothing,
}

/// Parse `attribute eq "value"`; anything else is `invalidFilter`.
pub fn parse_filter(raw: &str) -> Result<Filter, ScimError> {
    let unsupported = || {
        ScimError::new(
            StatusCode::BAD_REQUEST,
            Some("invalidFilter"),
            "Only filters of the form `userName eq \"…\"` (also externalId, emails.value or id) are supported.",
        )
    };
    let raw = raw.trim();
    let (attribute, rest) = raw.split_once(char::is_whitespace).ok_or_else(unsupported)?;
    let rest = rest.trim_start();
    let (operator, value) = rest.split_once(char::is_whitespace).ok_or_else(unsupported)?;
    if !operator.eq_ignore_ascii_case("eq") {
        return Err(unsupported());
    }
    // The value is a JSON string literal.
    let value: String = serde_json::from_str(value.trim()).map_err(|_| unsupported())?;
    if value.chars().count() > 320 {
        return Err(unsupported());
    }
    let attribute = attribute.to_ascii_lowercase();
    let attribute = attribute.strip_prefix(&format!("{}:", USER_SCHEMA.to_ascii_lowercase())).unwrap_or(&attribute);
    Ok(match attribute {
        "username" => Filter::UserName(value),
        "externalid" => Filter::ExternalId(value),
        "emails.value" | "emails" => Filter::Email(value.to_lowercase()),
        "id" => value.parse().map_or(Filter::Nothing, Filter::Id),
        _ => return Err(unsupported()),
    })
}

/// `/ServiceProviderConfig`: what this service supports.
pub fn service_provider_config() -> Value {
    json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
        "patch": { "supported": true },
        "bulk": { "supported": false, "maxOperations": 0, "maxPayloadSize": 0 },
        "filter": { "supported": true, "maxResults": 200 },
        "changePassword": { "supported": false },
        "sort": { "supported": false },
        "etag": { "supported": false },
        "authenticationSchemes": [{
            "type": "oauthbearertoken",
            "name": "Provisioning token",
            "description": "A workspace provisioning token, sent as `Authorization: Bearer <token>`.",
            "primary": true,
        }],
        "meta": { "resourceType": "ServiceProviderConfig" },
    })
}

/// `/ResourceTypes`: only User.
pub fn resource_types(base: &str) -> Value {
    json!({
        "schemas": [LIST_SCHEMA],
        "totalResults": 1,
        "itemsPerPage": 1,
        "startIndex": 1,
        "Resources": [{
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],
            "id": "User",
            "name": "User",
            "endpoint": "/Users",
            "schema": USER_SCHEMA,
            "meta": { "resourceType": "ResourceType", "location": format!("{base}/ResourceTypes/User") },
        }],
    })
}

/// `/Schemas`: the attributes of User that are kept.
pub fn schemas() -> Value {
    let attribute = |name: &str, required: bool, uniqueness: &str| {
        json!({ "name": name, "type": "string", "multiValued": false, "required": required, "caseExact": false,
                "mutability": "readWrite", "returned": "default", "uniqueness": uniqueness })
    };
    let user = json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Schema"],
        "id": USER_SCHEMA,
        "name": "User",
        "description": "A person in the workspace",
        "attributes": [
            attribute("userName", true, "server"),
            attribute("externalId", false, "none"),
            attribute("displayName", false, "none"),
            { "name": "name", "type": "complex", "multiValued": false, "required": false, "mutability": "readWrite",
              "returned": "default", "subAttributes": [attribute("givenName", false, "none"), attribute("familyName", false, "none")] },
            { "name": "emails", "type": "complex", "multiValued": true, "required": false, "mutability": "readWrite",
              "returned": "default", "subAttributes": [attribute("value", false, "none")] },
            { "name": "active", "type": "boolean", "multiValued": false, "required": false, "mutability": "readWrite",
              "returned": "default" },
        ],
        "meta": { "resourceType": "Schema" },
    });
    json!({ "schemas": [LIST_SCHEMA], "totalResults": 1, "itemsPerPage": 1, "startIndex": 1, "Resources": [user] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ada() -> Person {
        Person::from_resource(&json!({
            "schemas": [USER_SCHEMA],
            "userName": "Ada@Example.com",
            "name": { "givenName": "Ada", "familyName": "Lovelace", "formatted": "ignored" },
            "emails": [{ "value": "ada.l@example.com", "type": "home" }, { "value": "ada@example.com", "primary": true }],
            "externalId": "00u1",
            "title": "Engineer",
            "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User": { "department": "R&D" },
        }))
        .unwrap()
    }

    #[test]
    fn reads_a_user_and_picks_the_primary_email() {
        let p = ada();
        assert_eq!(p.user_name, "Ada@Example.com");
        assert_eq!(p.email.as_deref(), Some("ada@example.com"));
        assert_eq!((p.given_name.as_deref(), p.family_name.as_deref()), (Some("Ada"), Some("Lovelace")));
        assert_eq!(p.external_id.as_deref(), Some("00u1"));
        assert!(p.active);
        assert_eq!(p.address().unwrap(), "ada@example.com");
    }

    #[test]
    fn rejects_users_without_an_email_address() {
        assert!(Person::from_resource(&json!({ "userName": "ada" })).is_err());
        assert!(Person::from_resource(&json!({ "displayName": "Ada" })).is_err());
        assert!(Person::from_resource(&json!({ "userName": "ada", "emails": [{ "value": "ada@example.com" }] })).is_ok());
        assert!(Person::from_resource(&json!({ "userName": 7 })).is_err());
        assert!(Person::from_resource(&json!({ "userName": "a@b.co", "active": "maybe" })).is_err());
        assert!(Person::from_resource(&json!({ "userName": format!("{}@example.com", "a".repeat(400)) })).is_err());
    }

    #[test]
    fn applies_okta_and_entra_patches() {
        // Okta: pathless replace.
        let off = ada().patched(&json!({ "Operations": [{ "op": "replace", "value": { "active": false } }] })).unwrap();
        assert!(!off.active);
        // Entra: capitalised ops, string booleans, filtered email paths, full-URN paths.
        let entra = ada()
            .patched(&json!({ "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"], "Operations": [
                { "op": "Replace", "path": "active", "value": "False" },
                { "op": "Replace", "path": "emails[type eq \"work\"].value", "value": "ada@new.example.com" },
                { "op": "Add", "path": "urn:ietf:params:scim:schemas:core:2.0:User:name.givenName", "value": "Augusta" },
                { "op": "Remove", "path": "externalId" },
                { "op": "Replace", "path": "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User:department", "value": "x" },
            ]}))
            .unwrap();
        assert!(!entra.active);
        assert_eq!(entra.address().unwrap(), "ada@new.example.com");
        assert_eq!(entra.given_name.as_deref(), Some("Augusta"));
        assert_eq!(entra.external_id, None);

        for bad in [
            json!({}),
            json!({ "Operations": [{ "op": "remove", "path": "userName" }] }),
            json!({ "Operations": [{ "op": "remove" }] }),
            json!({ "Operations": [{ "op": "move", "path": "active", "value": true }] }),
            json!({ "Operations": [{ "op": "replace", "path": "userName", "value": "not-an-email" }, { "op": "remove", "path": "emails" }] }),
            json!({ "Operations": vec![json!({ "op": "replace", "path": "active", "value": true }); 101] }),
        ] {
            assert!(ada().patched(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_the_filters_providers_send() {
        assert_eq!(parse_filter("userName eq \"ada@example.com\"").unwrap(), Filter::UserName("ada@example.com".into()));
        assert_eq!(parse_filter("externalId eq \"00u1\"").unwrap(), Filter::ExternalId("00u1".into()));
        assert_eq!(parse_filter("emails.value EQ \"Ada@Example.com\"").unwrap(), Filter::Email("ada@example.com".into()));
        assert_eq!(parse_filter("id eq \"not-a-uuid\"").unwrap(), Filter::Nothing);
        assert_eq!(
            parse_filter("urn:ietf:params:scim:schemas:core:2.0:User:userName eq \"a\\\"b\"").unwrap(),
            Filter::UserName("a\"b".into())
        );
        for bad in ["userName sw \"a\"", "title eq \"x\"", "userName eq a", "userName eq \"a\" and active eq true", ""] {
            assert!(parse_filter(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn renders_a_user_resource() {
        let p = ada();
        let now = OffsetDateTime::now_utc();
        let id = Uuid::now_v7();
        let r = p.to_resource(&Stored { id, version: 3, created_at: now, updated_at: now }, "https://x/scim/v2/Users/1");
        assert_eq!(r["id"], json!(id));
        assert_eq!(r["emails"][0]["value"], "ada@example.com");
        assert_eq!(r["meta"]["version"], "W/\"3\"");
        assert_eq!(r["name"]["familyName"], "Lovelace");
        assert!(r.get("title").is_none());
    }

    #[test]
    fn email_check() {
        for ok in ["a@b.co", "first.last+tag@sub.example.org"] {
            assert!(plausible_email(ok), "{ok}");
        }
        for bad in ["", "ab", "a@b", "@b.co", "a@.co", "a@b.co.", "a b@c.co", "a@b@c.co"] {
            assert!(!plausible_email(bad), "{bad}");
        }
    }
}
