//! Outbox → relay → Kafka → consumer, against the local Kafka and PostgreSQL
//! (`make infra`). Each test uses its own topics and consumer group.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use event_bus::{run_consumer, Event, Handler, HandlerError, KafkaSettings, OutboxRelay};
use event_contracts::{outbox::OUTBOX_TABLE_SQL, Actor, EventEnvelope};
use rdkafka::{
    admin::{AdminClient, AdminOptions, NewTopic, TopicReplication},
    client::DefaultClientContext,
    consumer::{Consumer, StreamConsumer},
    ClientConfig, Message,
};
use serde_json::json;
use sqlx::{migrate::Migrator, Executor};
use test_utilities::TestDb;
use uuid::Uuid;

fn brokers() -> String {
    std::env::var("TEST_KAFKA_BROKERS").unwrap_or_else(|_| "127.0.0.1:59092".to_owned())
}

async fn create_topics(names: &[&str]) {
    let admin: AdminClient<DefaultClientContext> =
        ClientConfig::new().set("bootstrap.servers", brokers()).create().expect("admin client");
    let topics: Vec<NewTopic> = names.iter().map(|n| NewTopic::new(n, 3, TopicReplication::Fixed(1))).collect();
    for result in admin.create_topics(&topics, &AdminOptions::new()).await.expect("create topics") {
        result.expect("topic created");
    }
}

fn envelope(tenant: Uuid, aggregate: Uuid, version: i64, label: &str) -> serde_json::Value {
    let env = EventEnvelope {
        event_id: Uuid::now_v7(),
        event_type: "test.happened".into(),
        schema_version: 1,
        tenant_id: Some(tenant),
        aggregate_type: "test".into(),
        aggregate_id: aggregate,
        aggregate_version: version,
        occurred_at: time::OffsetDateTime::now_utc(),
        correlation_id: None,
        causation_id: None,
        actor: Actor::System("test".into()),
        payload: json!({ "label": label }),
    };
    serde_json::to_value(env).unwrap()
}

async fn outbox_db() -> TestDb {
    static EMPTY: Migrator = Migrator::DEFAULT;
    let db = TestDb::create(&EMPTY).await.expect("test database");
    db.owner.execute(OUTBOX_TABLE_SQL).await.unwrap();
    db.owner.execute("GRANT SELECT, INSERT, UPDATE, DELETE ON outbox, inbox TO platform_test_app").await.unwrap();
    db
}

async fn enqueue(db: &TestDb, topic: &str, key: &str, payload: &serde_json::Value) {
    sqlx::query("INSERT INTO outbox (id, tenant_id, topic, partition_key, event_type, payload) VALUES ($1, $2, $3, $4, 'test.happened', $5)")
        .bind(Uuid::parse_str(payload["event_id"].as_str().unwrap()).unwrap())
        .bind(Uuid::parse_str(payload["tenant_id"].as_str().unwrap()).unwrap())
        .bind(topic)
        .bind(key)
        .bind(payload)
        .execute(&db.app)
        .await
        .unwrap();
}

#[tokio::test]
async fn relay_publishes_committed_events_in_key_order_once() {
    let topic = format!("test.relay.{}", Uuid::new_v4().simple());
    create_topics(&[&topic]).await;
    let db = outbox_db().await;
    let tenant = Uuid::now_v7();
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    for (aggregate, version) in [(a, 1), (b, 1), (a, 2), (a, 3), (b, 2)] {
        enqueue(&db, &topic, &format!("{tenant}:{aggregate}"), &envelope(tenant, aggregate, version, "x")).await;
    }

    let relay = OutboxRelay::new("test", &KafkaSettings::new(brokers(), "test-relay")).unwrap();
    assert_eq!(relay.publish_batch(&db.app).await.unwrap(), 5);
    assert_eq!(relay.publish_batch(&db.app).await.unwrap(), 0, "published rows are not sent twice");
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE published_at IS NULL").fetch_one(&db.app).await.unwrap();
    assert_eq!(pending, 0);

    // Read the topic back: per aggregate, versions arrive in order.
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers())
        .set("group.id", format!("verify-{}", Uuid::new_v4()))
        .set("auto.offset.reset", "earliest")
        .create()
        .unwrap();
    consumer.subscribe(&[&topic]).unwrap();
    let mut seen: HashMap<Uuid, Vec<i64>> = HashMap::new();
    while seen.values().map(Vec::len).sum::<usize>() < 5 {
        let message = tokio::time::timeout(Duration::from_secs(20), consumer.recv()).await.expect("events arrive").unwrap();
        let event = Event::parse(&topic, message.payload().unwrap()).unwrap();
        assert_eq!(message.key().unwrap(), format!("{tenant}:{}", event.envelope.aggregate_id).as_bytes());
        seen.entry(event.envelope.aggregate_id).or_default().push(event.envelope.aggregate_version);
    }
    assert_eq!(seen[&a], vec![1, 2, 3]);
    assert_eq!(seen[&b], vec![1, 2]);
    db.cleanup().await;
}

/// Fails the first two attempts of "flaky" events, and always fails "poison" ones.
struct Recorder {
    handled: Mutex<Vec<String>>,
    attempts: Mutex<HashMap<String, u32>>,
}

#[async_trait]
impl Handler for Recorder {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let label = event.envelope.payload["label"].as_str().unwrap_or_default().to_owned();
        let attempt = {
            let mut attempts = self.attempts.lock().unwrap();
            let n = attempts.entry(label.clone()).or_default();
            *n += 1;
            *n
        };
        match label.as_str() {
            "flaky" if attempt < 3 => Err(HandlerError::Transient(anyhow::anyhow!("dependency down"))),
            "poison" => Err(HandlerError::Permanent(anyhow::anyhow!("cannot apply"))),
            _ => {
                self.handled.lock().unwrap().push(label);
                Ok(())
            }
        }
    }
}

#[tokio::test]
async fn consumers_retry_transient_failures_and_park_poison_events() {
    let topic = format!("test.consume.{}", Uuid::new_v4().simple());
    let dlq = event_contracts::topics::dead_letter(&topic);
    create_topics(&[&topic, &dlq]).await;
    let db = outbox_db().await;
    let tenant = Uuid::now_v7();
    let aggregate = Uuid::now_v7();
    for (version, label) in [(1, "first"), (2, "flaky"), (3, "poison"), (4, "last")] {
        enqueue(&db, &topic, &format!("{tenant}:{aggregate}"), &envelope(tenant, aggregate, version, label)).await;
    }
    let settings = KafkaSettings::new(brokers(), "test-consumer");
    let relay = OutboxRelay::new("test", &settings).unwrap();
    assert_eq!(relay.publish_batch(&db.app).await.unwrap(), 4);

    let recorder = Arc::new(Recorder { handled: Mutex::new(Vec::new()), attempts: Mutex::new(HashMap::new()) });
    let group = format!("test-{}", Uuid::new_v4());
    let task = {
        let (settings, recorder, topic, group) = (settings.clone(), Arc::clone(&recorder), topic.clone(), group.clone());
        tokio::spawn(async move { run_consumer(&settings, &group, &[topic.as_str()], recorder).await })
    };

    let dlq_reader: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers())
        .set("group.id", format!("verify-{}", Uuid::new_v4()))
        .set("auto.offset.reset", "earliest")
        .create()
        .unwrap();
    dlq_reader.subscribe(&[&dlq]).unwrap();
    let parked = tokio::time::timeout(Duration::from_secs(30), dlq_reader.recv()).await.expect("poison event parked").unwrap();
    let parked_event = Event::parse(&topic, parked.payload().unwrap()).unwrap();
    assert_eq!(parked_event.envelope.payload["label"], "poison");
    let header = |name: &str| {
        use rdkafka::message::Headers;
        parked
            .headers()
            .and_then(|h| h.iter().find(|x| x.key == name).and_then(|x| x.value.map(|v| String::from_utf8_lossy(v).into_owned())))
    };
    assert_eq!(header("x-consumer").as_deref(), Some(group.as_str()));
    assert_eq!(header("x-original-topic").as_deref(), Some(topic.as_str()));
    assert!(header("x-error").unwrap().contains("cannot apply"));

    for _ in 0..100 {
        if recorder.handled.lock().unwrap().len() == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(*recorder.handled.lock().unwrap(), vec!["first", "flaky", "last"], "in order, the poison event skipped");
    assert_eq!(recorder.attempts.lock().unwrap()["flaky"], 3);
    assert_eq!(recorder.attempts.lock().unwrap()["poison"], 1, "permanent failures are not retried");
    task.abort();
    db.cleanup().await;
}
