use analytics_service::{migrate, rebuild, run, SERVICE_NAME};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = observability::init(SERVICE_NAME)?;
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("migrate") => migrate().await,
        Some("rebuild") => {
            let tenant = args.next().ok_or_else(|| anyhow::anyhow!("usage: analytics-service rebuild <tenant-id>"))?;
            rebuild(tenant.parse()?).await
        }
        _ => {
            run(telemetry).await?.shutdown();
            Ok(())
        }
    }
}
