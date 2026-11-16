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

/// Refuses "poison" events for good until `fixed` is set; records what it handled.
struct Fixable {
    fixed: std::sync::atomic::AtomicBool,
    handled: Mutex<Vec<String>>,
}

#[async_trait]
impl Handler for Fixable {
    async fn handle(&self, event: &Event) -> Result<(), HandlerError> {
        let label = event.envelope.payload["label"].as_str().unwrap_or_default().to_owned();
        if label.starts_with("poison") && !self.fixed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(HandlerError::Permanent(anyhow::anyhow!("cannot apply {label}")));
        }
        self.handled.lock().unwrap().push(label);
        Ok(())
    }
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..300 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn dead_lettered_events_are_replayed_for_the_failing_consumer_only() {
    use event_bus::replay;
    use rdkafka::producer::{FutureProducer, FutureRecord};

    let topic = format!("test.replay.{}", Uuid::new_v4().simple());
    let dlq = event_contracts::topics::dead_letter(&topic);
    create_topics(&[&topic, &dlq]).await;
    let (tenant, aggregate) = (Uuid::now_v7(), Uuid::now_v7());
    let producer: FutureProducer = ClientConfig::new().set("bootstrap.servers", brokers()).create().unwrap();
    for (version, label) in [(1, "ok1"), (2, "poison1"), (3, "poison2"), (4, "ok2")] {
        let payload = envelope(tenant, aggregate, version, label).to_string();
        let key = format!("{tenant}:{aggregate}");
        producer.send(FutureRecord::to(&topic).key(&key).payload(&payload), Duration::from_secs(10)).await.unwrap();
    }

    let settings = KafkaSettings::new(brokers(), "test-consumer");
    let failing = Arc::new(Fixable { fixed: false.into(), handled: Mutex::new(Vec::new()) });
    let other = Arc::new(Fixable { fixed: true.into(), handled: Mutex::new(Vec::new()) });
    let (group, other_group) = (format!("test-{}", Uuid::new_v4()), format!("test-{}", Uuid::new_v4()));
    let mut tasks = Vec::new();
    for (g, handler) in [(group.clone(), Arc::clone(&failing)), (other_group.clone(), Arc::clone(&other))] {
        let (settings, topic) = (settings.clone(), topic.clone());
        tasks.push(tokio::spawn(async move { run_consumer(&settings, &g, &[topic.as_str()], handler).await }));
    }
    eventually("both groups to finish", || {
        failing.handled.lock().unwrap().len() == 2 && other.handled.lock().unwrap().len() == 4
    })
    .await;

    // Both poison events are parked for the failing group; the dead-letter topic name or the
    // topic itself names them, and only metadata is reported.
    let mut listed = (Vec::new(), 0);
    for _ in 0..100 {
        listed = replay::pending(&settings, &dlq, &group, 10).unwrap();
        if listed.1 == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(listed.1, 2);
    assert_eq!(listed.0.iter().map(|p| p.error.as_str()).collect::<Vec<_>>(), ["cannot apply poison1", "cannot apply poison2"]);
    assert!(listed.0.iter().all(|p| p.event_type.as_deref() == Some("test.happened") && p.tenant_id == Some(tenant)));
    assert_eq!(listed.0[0].attempts, Some(1));
    assert_eq!(replay::pending(&settings, &topic, &other_group, 10).unwrap().1, 0, "nothing parked for the other group");

    // Fixed: replay one, then the rest. Only the failing group handles them again.
    failing.fixed.store(true, std::sync::atomic::Ordering::SeqCst);
    let first = replay::replay(&settings, &topic, &group, 1).await.unwrap();
    assert_eq!((first.sent.len(), first.remaining), (1, 1));
    eventually("the first replayed event", || failing.handled.lock().unwrap().len() == 3).await;
    assert_eq!(replay::pending(&settings, &topic, &group, 10).unwrap().1, 1, "the replayed one is no longer pending");
    let rest = replay::replay(&settings, &dlq, &group, usize::MAX).await.unwrap();
    assert_eq!((rest.sent.len(), rest.remaining), (1, 0));
    eventually("the second replayed event", || failing.handled.lock().unwrap().len() == 4).await;
    assert_eq!(*failing.handled.lock().unwrap(), ["ok1", "ok2", "poison1", "poison2"]);
    assert_eq!(
        replay::replay(&settings, &topic, &group, usize::MAX).await.unwrap(),
        replay::Replayed::default(),
        "never sent twice"
    );

    // The other group skipped the replayed events (it had handled them the first time).
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(other.handled.lock().unwrap().len(), 4);
    assert!(replay::replay(&settings, "bad topic", &group, 1).await.is_err());
    for task in tasks {
        task.abort();
    }
}
