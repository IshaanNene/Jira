use analytics_service::{migrate, run, SERVICE_NAME};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = observability::init(SERVICE_NAME)?;
    match std::env::args().nth(1).as_deref() {
        Some("migrate") => migrate().await,
        _ => {
            run(telemetry).await?.shutdown();
            Ok(())
        }
    }
}
