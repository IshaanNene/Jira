use tenant_service::{migrate, run, Config, SERVICE_NAME};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = observability::init(SERVICE_NAME)?;
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        return migrate().await;
    }
    let telemetry = run(Config::from_env()?, telemetry).await?;
    telemetry.shutdown();
    Ok(())
}
