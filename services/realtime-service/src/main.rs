use realtime_service::{run, SERVICE_NAME};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = observability::init(SERVICE_NAME)?;
    // No database, so nothing to migrate; accepted for symmetry with the other services.
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        return Ok(());
    }
    run(telemetry).await?.shutdown();
    Ok(())
}
