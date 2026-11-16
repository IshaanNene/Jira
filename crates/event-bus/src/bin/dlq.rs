//! Dead-letter topics, from the command line (see `event_bus::replay`).
//!
//!   dlq list   <topic> --consumer <group> [--limit N]   parked events not yet replayed
//!   dlq replay <topic> --consumer <group> [--limit N]   send them back, for that group only
//!
//! `<topic>` is the topic or its `.dlq`. Kafka settings come from the environment
//! (`KAFKA_BROKERS` and the `KAFKA_SECURITY_*` / `KAFKA_SASL_*` variables services use).
//! Payloads are never printed.

use event_bus::{replay, KafkaSettings};

const USAGE: &str = "usage: dlq list|replay <topic> --consumer <group> [--limit N]";

struct Args {
    command: String,
    topic: String,
    group: String,
    limit: usize,
}

fn args() -> anyhow::Result<Args> {
    let mut raw = std::env::args().skip(1);
    let command = raw.next().ok_or_else(|| anyhow::anyhow!(USAGE))?;
    if command != "list" && command != "replay" {
        anyhow::bail!(USAGE);
    }
    let topic = raw.next().ok_or_else(|| anyhow::anyhow!(USAGE))?;
    let (mut group, mut limit) = (None, None);
    while let Some(flag) = raw.next() {
        let value = raw.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))?;
        match flag.as_str() {
            "--consumer" => group = Some(value),
            "--limit" => limit = Some(value.parse::<usize>().map_err(|_| anyhow::anyhow!("--limit takes a number"))?),
            _ => anyhow::bail!(USAGE),
        }
    }
    let group = group.ok_or_else(|| anyhow::anyhow!("--consumer is required: which group's events?"))?;
    let default = if command == "list" { 50 } else { usize::MAX };
    Ok(Args { command, topic, group, limit: limit.unwrap_or(default) })
}

fn line(p: &replay::Parked) -> String {
    let or_unknown = |v: Option<String>| v.unwrap_or_else(|| "?".into());
    format!(
        "{}:{}  {}  {}  tenant {}  attempts {}  {}",
        p.partition,
        p.offset,
        or_unknown(p.event_id.map(|i| i.to_string())),
        or_unknown(p.event_type.clone()),
        or_unknown(p.tenant_id.map(|i| i.to_string())),
        or_unknown(p.attempts.map(|a| a.to_string())),
        p.error
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = args()?;
    let settings = KafkaSettings::from_env("meridian-dlq").ok_or_else(|| anyhow::anyhow!("KAFKA_BROKERS is not set"))?;
    if args.command == "list" {
        let (listed, total) = replay::pending(&settings, &args.topic, &args.group, args.limit)?;
        for p in &listed {
            println!("{}", line(p));
        }
        println!("{total} parked event(s) of {} not yet replayed", args.group);
    } else {
        let done = replay::replay(&settings, &args.topic, &args.group, args.limit).await?;
        for p in &done.sent {
            println!("replayed {}", line(p));
        }
        println!("{} replayed for {}, {} still parked", done.sent.len(), args.group, done.remaining);
    }
    Ok(())
}
