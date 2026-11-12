use workflow_service::{migrate, run, SERVICE_NAME};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = observability::init(SERVICE_NAME)?;
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        return migrate().await;
    }
    run(telemetry).await?.shutdown();
    Ok(())
}
