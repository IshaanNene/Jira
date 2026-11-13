//! `GET /api/v1/rt/stream?channel=…` — Server-Sent Events.
//!
//! Event names: `ready` (subscribed; its id is the cursor to resume from), `change` (a hint,
//! see [`crate::channels::Hint`]), `resync` (something was missed: refetch, then carry on),
//! `expired` (the access token ran out: reconnect with a fresh one) and `denied` (access
//! was withdrawn: stop). Resume with the `Last-Event-ID` header or `?cursor=`.

use std::{convert::Infallible, sync::Arc, time::Duration};

use auth_middleware::{AuthContext, OrgContext, ProjectPermission};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Router,
};
use error_handling::{ApiError, ApiResult};
use serde::Deserialize;
use tokio::sync::{broadcast::error::RecvError, mpsc, OwnedSemaphorePermit};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{
    channels::{Channel, StreamId},
    hub::{Live, Replay},
    AppState,
};

const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub channel: String,
    pub cursor: Option<String>,
}

/// Who may listen where. Missing access reads as "not found", like the REST APIs.
pub async fn authorize(state: &AppState, ctx: &OrgContext, channel: Channel) -> ApiResult<()> {
    match channel {
        Channel::Org => Ok(()),
        Channel::User(id) if id == ctx.user_id => Ok(()),
        Channel::User(_) => Err(ApiError::NotFound("channel")),
        Channel::Project(id) => {
            state.policy.snapshot(ctx).await?.require(ctx, id, ProjectPermission::Browse).map(|_| ()).map_err(|e| match e {
                ApiError::Forbidden | ApiError::NotFound(_) => ApiError::NotFound("channel"),
                other => other,
            })
        }
    }
}

pub async fn stream(
    State(state): State<AppState>,
    auth: AuthContext,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let ctx = OrgContext::try_from(auth.clone())?;
    let channel = Channel::parse(&query.channel)
        .ok_or_else(|| ApiError::invalid("channel", "invalid_channel", "Use project:<id>, user:<id> or org."))?;
    authorize(&state, &ctx, channel).await?;
    let cursor = match headers.get(&LAST_EVENT_ID).and_then(|v| v.to_str().ok()).or(query.cursor.as_deref()) {
        Some(raw) => {
            Some(StreamId::parse(raw).ok_or_else(|| ApiError::invalid("cursor", "invalid_cursor", "Not a stream cursor."))?)
        }
        None => None,
    };
    let permit = Arc::clone(&state.slots)
        .try_acquire_owned()
        .map_err(|_| ApiError::DependencyUnavailable("Too many live connections. Try again shortly."))?;

    let (tx, rx) = mpsc::channel::<SseEvent>(64);
    let live = state.hub.subscribe();
    let expires_in =
        Duration::from_secs(u64::try_from(auth.claims.exp - time::OffsetDateTime::now_utc().unix_timestamp()).unwrap_or(0));
    tokio::spawn(session(Session { state, ctx, channel, cursor, expires_in, tx, _permit: permit }, live));

    let events = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx)) });
    let mut response = Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))).into_response();
    // Proxies must not buffer the stream.
    response.headers_mut().insert(HeaderName::from_static("x-accel-buffering"), HeaderValue::from_static("no"));
    Ok(response)
}

struct Session {
    state: AppState,
    ctx: OrgContext,
    channel: Channel,
    cursor: Option<StreamId>,
    expires_in: Duration,
    tx: mpsc::Sender<SseEvent>,
    _permit: OwnedSemaphorePermit,
}

impl Session {
    async fn send(&self, event: SseEvent) -> bool {
        self.tx.send(event).await.is_ok()
    }

    async fn control(&self, name: &str, id: Option<StreamId>) -> bool {
        let mut event = SseEvent::default().event(name).data("{}");
        if let Some(id) = id {
            event = event.id(id.to_string());
        }
        self.send(event).await
    }

    async fn tip(&self) -> Option<StreamId> {
        self.state.hub.tip(self.ctx.tenant_id, &self.channel).await.ok()
    }

    async fn still_allowed(&self) -> bool {
        !self.state.verifier.is_revoked(self.ctx.session_id).await
            && authorize(&self.state, &self.ctx, self.channel).await.is_ok()
    }
}

async fn session(s: Session, mut live: tokio::sync::broadcast::Receiver<Arc<Live>>) {
    metrics::gauge!("realtime_connections").increment(1.0);
    run_session(&s, &mut live).await;
    metrics::gauge!("realtime_connections").decrement(1.0);
}

async fn run_session(s: &Session, live: &mut tokio::sync::broadcast::Receiver<Arc<Live>>) {
    let tenant = s.ctx.tenant_id;
    let channel_name = s.channel.to_string();

    // Catch up first. `live` was subscribed before this, so nothing falls in between;
    // duplicates are dropped by id below.
    let mut last = match s.cursor {
        Some(cursor) => match s.state.hub.replay(tenant, &s.channel, cursor).await {
            Ok(Replay::Entries(entries)) => {
                let mut last = cursor;
                for (id, data) in entries {
                    if !s.send(SseEvent::default().event("change").id(id.to_string()).data(data)).await {
                        return;
                    }
                    last = id;
                }
                if !s.control("ready", Some(last)).await {
                    return;
                }
                last
            }
            Ok(Replay::Resync) | Err(_) => {
                let tip = s.tip().await;
                if !s.control("resync", tip).await {
                    return;
                }
                tip.unwrap_or(cursor)
            }
        },
        None => {
            let tip = s.tip().await.unwrap_or(StreamId(0, 0));
            if !s.control("ready", Some(tip)).await {
                return;
            }
            tip
        }
    };

    let expiry = tokio::time::sleep(s.expires_in);
    tokio::pin!(expiry);
    let every = s.state.recheck;
    let mut recheck = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    loop {
        tokio::select! {
            message = live.recv() => match message {
                Ok(message) => match message.as_ref() {
                    Live::Entry(entry) if entry.tenant == tenant && entry.channel == channel_name => {
                        let Some(id) = StreamId::parse(&entry.id) else { continue };
                        if id > last {
                            if !s.send(SseEvent::default().event("change").id(entry.id.clone()).data(entry.data.clone())).await {
                                return;
                            }
                            last = id;
                        }
                    }
                    Live::Entry(_) => {}
                    Live::Resync => {
                        let tip = s.tip().await;
                        if !s.control("resync", tip).await {
                            return;
                        }
                        last = tip.unwrap_or(last);
                    }
                },
                // This connection fell behind the instance's fanout buffer.
                Err(RecvError::Lagged(_)) => {
                    let tip = s.tip().await;
                    if !s.control("resync", tip).await {
                        return;
                    }
                    last = tip.unwrap_or(last);
                }
                Err(RecvError::Closed) => return,
            },
            _ = recheck.tick() => {
                if !s.still_allowed().await {
                    let _ = s.control("denied", None).await;
                    return;
                }
            }
            () = &mut expiry => {
                let _ = s.control("expired", None).await;
                return;
            }
            () = s.tx.closed() => return,
        }
    }
}

pub fn router(state: AppState) -> Router {
    let origins: Vec<_> = state.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET])
        .allow_headers([header::AUTHORIZATION, LAST_EVENT_ID]);
    Router::new().route("/api/v1/rt/stream", get(stream)).layer(cors).with_state(state)
}
